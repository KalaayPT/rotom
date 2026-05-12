use crate::{
    BatchCompileResult, BatchDecompileResult, CompileError, CompileFailure, CompiledFile,
    ConstantDb, DatabaseV2, DecompileFailure, GameFamily, decompile_file_internal,
};
use rayon::prelude::{IntoParallelRefIterator, ParallelIterator};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use uxie::{GameLanguage, RomHeader, Workspace};
use xxhash_rust::xxh3::xxh3_64;

use super::config::{ProjectTypeConfig, RotomConfig};
use super::error::{ProjectError, Result};
use crate::compile_state::{COMPILER_VERSION, CompileState, FileState, FileStatus};

enum WorkerResult {
    Skip {
        relative_path: String,
        source_hash: u64,
        dependency_hashes: HashMap<String, u64>,
    },
    Failure {
        relative_path: String,
        failure: CompileFailure,
    },
    Success {
        relative_path: String,
        source_hash: u64,
        dependency_hashes: HashMap<String, u64>,
        result: CompiledFile,
    },
}

struct CompileSession {
    db: DatabaseV2,
    constants: ConstantDb,
    db_hash: u64,
    force_compile: bool,
    state: CompileState,
    status_path: PathBuf,
}

fn relative_project_path(root: &Path, path: &Path) -> Result<String> {
    if let Ok(relative) = path.strip_prefix(root) {
        return Ok(relative.to_string_lossy().replace('\\', "/"));
    }

    let canonical_root = root.canonicalize().map_err(|source| ProjectError::Io {
        action: "Failed to canonicalize project root",
        path: root.to_path_buf(),
        source,
    })?;
    let canonical_path = path.canonicalize().map_err(|source| ProjectError::Io {
        action: "Failed to canonicalize tracked path",
        path: path.to_path_buf(),
        source,
    })?;
    let relative = canonical_path.strip_prefix(&canonical_root).map_err(|_| {
        ProjectError::PathOutsideProject {
            root: canonical_root.clone(),
            path: canonical_path.clone(),
        }
    })?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

pub fn project_output_path(
    source_path: &Path,
    source_root: &Path,
    binary_root: &Path,
    project_type: ProjectTypeConfig,
    game_family: Option<GameFamily>,
) -> PathBuf {
    let relative = source_path.strip_prefix(source_root).unwrap_or(source_path);
    let extensionless = matches!(
        project_type,
        ProjectTypeConfig::Dspre | ProjectTypeConfig::HgEngine
    ) || matches!(
        (project_type, game_family),
        (ProjectTypeConfig::Decomp, Some(GameFamily::Platinum))
    );
    if extensionless {
        let stem = relative.file_stem().unwrap_or_default();
        match relative.parent() {
            Some(parent) => binary_root.join(parent).join(stem),
            None => binary_root.join(stem),
        }
    } else {
        binary_root.join(relative).with_extension("bin")
    }
}

#[allow(clippy::too_many_lines)]
pub fn compile_project(
    root: &Path,
    config: &RotomConfig,
    force: bool,
) -> Result<BatchCompileResult> {
    let work = collect_project_compile_work(root, config)?;
    let mut session = load_compile_session(root, config, force)?;

    // Borrow fields immutably so the parallel closure can use them.
    let constants = &session.constants;
    let state = &session.state;
    let force_compile = session.force_compile;
    let db = &session.db;

    let total_files = work.len();
    let progress = indicatif::ProgressBar::new(total_files as u64);
    progress.set_style(
        indicatif::ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );
    progress.set_message("");

    // Single parallel pass: check staleness and compile stale files in one go.
    let results: Vec<_> = work
        .par_iter()
        .map(|(input, output)| {
            let relative_path = match relative_project_path(root, input) {
                Ok(rp) => rp,
                Err(error) => {
                    progress.inc(1);
                    return Ok(WorkerResult::Failure {
                        relative_path: input.to_string_lossy().to_string(),
                        failure: CompileFailure {
                            path: input.clone(),
                            error: CompileError::Io {
                                message: error.to_string(),
                            },
                            source: String::new(),
                        },
                    });
                }
            };

            let source = match fs::read_to_string(input) {
                Ok(s) => s,
                Err(source) => {
                    progress.inc(1);
                    return Ok(WorkerResult::Failure {
                        relative_path,
                        failure: CompileFailure {
                            path: input.clone(),
                            error: CompileError::Io {
                                message: format!(
                                    "Failed to read input file '{}': {}",
                                    input.display(),
                                    source
                                ),
                            },
                            source: String::new(),
                        },
                    });
                }
            };
            let source_hash = xxh3_64(source.as_bytes());
            let output_hash = fs::read(output).ok().map(|bytes| xxh3_64(&bytes));

            // Fast path: if source_hash + output_hash match stored values, the
            // file isn't dirty, and all stored dependency files are still
            // unchanged, skip include-parsing entirely.
            if !force_compile
                && let Some(entry) = state.entries.get(&relative_path)
                && entry.status != FileStatus::Dirty
                && entry.source_hash == source_hash
                && entry.output_hash == output_hash.unwrap_or(0)
                && entry
                    .dependency_hashes
                    .iter()
                    .all(|(dep_path, &stored_hash)| {
                        fs::read(root.join(dep_path))
                            .map(|bytes| xxh3_64(&bytes) == stored_hash)
                            .unwrap_or(false)
                    })
            {
                progress.inc(1);
                return Ok(WorkerResult::Skip {
                    relative_path,
                    source_hash,
                    dependency_hashes: entry.dependency_hashes.clone(),
                });
            }

            let file_name = input
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            progress.set_message(format!("Resolving {file_name}"));
            let (dependency_hashes, loaded_constants) =
                match dependency_hashes_for_script(root, input, constants) {
                    Ok(result) => result,
                    Err(failure) => {
                        progress.inc(1);
                        return Ok(WorkerResult::Failure {
                            relative_path,
                            failure,
                        });
                    }
                };

            if !force_compile
                && !state.file_is_stale(
                    &relative_path,
                    source_hash,
                    output_hash,
                    &dependency_hashes,
                )
            {
                progress.inc(1);
                return Ok(WorkerResult::Skip {
                    relative_path,
                    source_hash,
                    dependency_hashes,
                });
            }

            progress.set_message(format!("Compiling {file_name}"));
            // Use the already-loaded per-file constants to avoid a second clone_for_script.
            let constants_for_compile = loaded_constants.as_ref().unwrap_or(constants);
            let binary_quirks = state
                .entries
                .get(&relative_path)
                .map(|e| e.quirks.clone())
                .unwrap_or_default();
            let compile_result = crate::compile_file_internal(
                input,
                output,
                db,
                constants_for_compile,
                false,
                binary_quirks,
            );
            progress.inc(1);

            match compile_result {
                Ok(result) => Ok(WorkerResult::Success {
                    relative_path,
                    source_hash,
                    dependency_hashes,
                    result,
                }),
                Err(e) => Ok(WorkerResult::Failure {
                    relative_path,
                    failure: e,
                }),
            }
        })
        .collect::<Result<Vec<_>>>()?;

    progress.finish_with_message("Done");

    let mut successes = Vec::new();
    let mut failures = Vec::new();
    let mut current_paths = Vec::with_capacity(work.len());

    for worker_result in results {
        match worker_result {
            WorkerResult::Skip {
                relative_path,
                source_hash,
                dependency_hashes,
            } => {
                current_paths.push(relative_path.clone());
                // Preserve the existing output_hash so the file isn't
                // spuriously marked stale on the next run.
                let existing = session
                    .state
                    .entries
                    .get(&relative_path)
                    .map_or(0, |e| e.output_hash);
                let quirks = session
                    .state
                    .entries
                    .get(&relative_path)
                    .map(|e| e.quirks.clone())
                    .unwrap_or_default();
                let file_state = FileState::compiled(source_hash, existing, dependency_hashes)
                    .with_quirks(quirks);
                session.state.entries.insert(relative_path, file_state);
            }
            WorkerResult::Success {
                relative_path,
                source_hash,
                dependency_hashes,
                result,
            } => {
                current_paths.push(relative_path.clone());
                let output_hash = fs::read(&result.output).ok().map(|bytes| xxh3_64(&bytes));
                let quirks = session
                    .state
                    .entries
                    .get(&relative_path)
                    .map(|e| e.quirks.clone())
                    .unwrap_or_default();
                let file_state =
                    FileState::compiled(source_hash, output_hash.unwrap_or(0), dependency_hashes)
                        .with_quirks(quirks);
                session.state.entries.insert(relative_path, file_state);
                successes.push(result);
            }
            WorkerResult::Failure {
                relative_path,
                failure,
            } => {
                current_paths.push(relative_path.clone());
                let output_hash = fs::read(&failure.path).ok().map(|bytes| xxh3_64(&bytes));
                session.state.entries.insert(
                    relative_path,
                    FileState::dirty(0, output_hash.unwrap_or(0), HashMap::new()),
                );
                failures.push(failure);
            }
        }
    }

    finish_compile_session(&mut session, current_paths)?;

    Ok(BatchCompileResult {
        successes,
        failures,
    })
}

/// Clone the constant database for a script file, discover its include-file
/// dependencies, and return both the loaded `ConstantDb` and the dependency
/// hashes so the caller can reuse the clone for compilation without a second
/// include-parse.  Returns `None` for the `ConstantDb` on non-script files.
fn dependency_hashes_for_script(
    root: &Path,
    input: &Path,
    constants: &ConstantDb,
) -> std::result::Result<(HashMap<String, u64>, Option<ConstantDb>), CompileFailure> {
    let extension = input
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if extension != "s" && extension != "rotom" {
        return Ok((HashMap::new(), None));
    }

    let loaded = constants
        .clone_for_script(input)
        .map_err(|error| CompileFailure {
            path: input.to_path_buf(),
            error,
            source: String::new(),
        })?;

    let canonical_input = input.canonicalize().unwrap_or_else(|_| input.to_path_buf());
    let mut dependency_hashes = HashMap::new();

    for path in loaded.loaded_script_file_paths() {
        if path == canonical_input {
            continue;
        }

        match fs::read(&path).map(|bytes| xxh3_64(&bytes)) {
            Ok(hash) => {
                let relative =
                    relative_project_path(root, &path).map_err(|error| CompileFailure {
                        path: input.to_path_buf(),
                        error: CompileError::Io {
                            message: error.to_string(),
                        },
                        source: String::new(),
                    })?;
                dependency_hashes.insert(relative, hash);
            }
            Err(source) => {
                return Err(CompileFailure {
                    path: input.to_path_buf(),
                    error: CompileError::Io {
                        message: format!(
                            "Failed to hash dependency '{}' for '{}': {}",
                            path.display(),
                            input.display(),
                            source
                        ),
                    },
                    source: String::new(),
                });
            }
        }
    }

    Ok((dependency_hashes, Some(loaded)))
}

/// Decompile every project binary into its configured source tree and record the
/// generated sources in compile state so a follow-up project compile can skip
/// unchanged outputs.
pub fn decompile_project(root: &Path, config: &RotomConfig) -> Result<BatchDecompileResult> {
    let (db, constants, db_hash, _) = load_project_database_and_constants(root, config)?;

    let work = collect_project_decompile_work(root, config)?;

    let total_files = work.len();
    let progress = indicatif::ProgressBar::new(total_files as u64);
    progress.set_style(
        indicatif::ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );
    progress.set_message("");

    let results: Vec<std::result::Result<crate::DecompileFileResult, DecompileFailure>> = work
        .par_iter()
        .map(|(input, output_dir)| {
            let result =
                decompile_file_internal(input, None, Some(output_dir), &db, Some(&constants));
            progress.inc(1);
            result
        })
        .collect();

    let mut successes = Vec::new();
    let mut failures = Vec::new();
    for result in results {
        match result {
            Ok(success) => successes.push(success),
            Err(failure) => failures.push(failure),
        }
    }

    progress.finish_with_message("Done");

    update_decompile_state(root, config, db_hash, &successes)?;

    Ok(BatchDecompileResult {
        successes,
        failures,
    })
}

/// Load the shared inputs for a project compile run: command database, constant
/// sets, uxie cache state, and the persisted compile-state snapshot. This stage
/// also decides whether the run must rebuild from scratch.
fn load_compile_session(root: &Path, config: &RotomConfig, force: bool) -> Result<CompileSession> {
    let status_path = config.status_dir(root).join("compile-state.json");
    let (db, constants, db_hash, constant_cache_rebuilt) =
        load_project_database_and_constants(root, config)?;
    let mut state =
        CompileState::load_or_default(&status_path).map_err(|source| ProjectError::Io {
            action: "Failed to read compile state",
            path: status_path.clone(),
            source,
        })?;
    let force_compile =
        force || state.needs_rebuild(db_hash, COMPILER_VERSION, constant_cache_rebuilt);
    if force_compile {
        state.entries.clear();
    }

    Ok(CompileSession {
        db,
        constants,
        db_hash,
        force_compile,
        state,
        status_path,
    })
}

/// Finalize a compile session by dropping stale entries for files that no longer
/// exist in the project, updating global metadata, and atomically writing the
/// refreshed compile-state file to disk.
fn finish_compile_session(session: &mut CompileSession, current_paths: Vec<String>) -> Result<()> {
    session.state.retain_only(current_paths);
    session
        .state
        .mark_metadata(session.db_hash, COMPILER_VERSION);
    session
        .state
        .save(&session.status_path)
        .map_err(|source| ProjectError::Io {
            action: "Failed to write compile state",
            path: session.status_path.clone(),
            source,
        })
}

/// Merge successful decompile outputs into compile state so regenerated sources are not stale on
/// the next [`compile_project`] pass (matches [`decompile_project`] bookkeeping).
pub(crate) fn update_decompile_state(
    root: &Path,
    config: &RotomConfig,
    db_hash: u64,
    successes: &[crate::DecompileFileResult],
) -> Result<()> {
    if successes.is_empty() {
        return Ok(());
    }

    let status_path = config.status_dir(root).join("compile-state.json");
    let mut state =
        CompileState::load_or_default(&status_path).map_err(|source| ProjectError::Io {
            action: "Failed to read compile state",
            path: status_path.clone(),
            source,
        })?;

    for success in successes {
        let relative_path = relative_project_path(root, &success.output)?;
        let source_hash = fs::read(&success.output)
            .map(|bytes| xxh3_64(&bytes))
            .map_err(|source| ProjectError::Io {
                action: "Failed to hash decompiled source",
                path: success.output.clone(),
                source,
            })?;
        let output_hash = fs::read(&success.input)
            .map(|bytes| xxh3_64(&bytes))
            .map_err(|source| ProjectError::Io {
                action: "Failed to hash decompiled input",
                path: success.input.clone(),
                source,
            })?;
        let mut file_state = FileState::decompiled(source_hash, output_hash);
        file_state.quirks.clone_from(&success.quirks);
        state.entries.insert(relative_path, file_state);
    }

    state.mark_metadata(db_hash, COMPILER_VERSION);
    state.save(&status_path).map_err(|source| ProjectError::Io {
        action: "Failed to write compile state",
        path: status_path,
        source,
    })
}

/// Load the project command database and all shared constants that apply to the
/// whole run, including local database overrides and the uxie-managed cached
/// decomp symbol set when the project type needs it.
fn load_project_database_and_constants(
    root: &Path,
    config: &RotomConfig,
) -> Result<(DatabaseV2, ConstantDb, u64, bool)> {
    let db_path = config
        .database_file(root)
        .ok_or(ProjectError::MissingDefaultDatabase)?;
    let db_hash = fs::read(&db_path)
        .map(|bytes| xxh3_64(&bytes))
        .map_err(|source| ProjectError::Io {
            action: "Failed to hash database file",
            path: db_path.clone(),
            source,
        })?;
    let db = DatabaseV2::load(&db_path).map_err(ProjectError::from)?;

    let mut constants = ConstantDb::new();
    let _ = constants.load_from_db(&db);
    let mut constant_cache_rebuilt = false;

    let database_dir = config.database_dir(root);
    if database_dir.exists() {
        let _ = constants
            .load_directory(&database_dir)
            .map_err(ProjectError::from)?;
    }

    match config.workspace.project_type {
        ProjectTypeConfig::Decomp => {
            let game_family = config
                .game_family()
                .ok_or(ProjectError::MissingGameFamily)?;
            let (symbols, rebuilt) = Workspace::load_cached_symbols(
                &config.cache_dir(root),
                root,
                &config.include_roots(root),
                game_family,
            )
            .map_err(|source| ProjectError::Io {
                action: "Failed to load constant cache",
                path: config.cache_dir(root),
                source,
            })?;
            let _ = constants.load_decomp_symbols(root, (*symbols).clone());
            constant_cache_rebuilt = rebuilt;
        }
        ProjectTypeConfig::Dspre => {
            let language = RomHeader::open(root)
                .map(|h| h.detect_language())
                .unwrap_or(GameLanguage::English);
            let _ = constants
                .load_dspre_text_archives(root, language)
                .map_err(ProjectError::from)?;
        }
        ProjectTypeConfig::HgEngine => {
            let mut ws = Workspace::open(root).map_err(|source| ProjectError::Io {
                action: "Failed to open HgEngine workspace",
                path: root.to_path_buf(),
                source,
            })?;
            ws.load_hg_engine_constants()
                .map_err(|source| ProjectError::Io {
                    action: "Failed to load HgEngine constants",
                    path: root.to_path_buf(),
                    source,
                })?;
            let _ = constants.load_decomp_symbols(root, (*ws.symbols).clone());
        }
        ProjectTypeConfig::Generic => {}
    }

    Ok((db, constants, db_hash, constant_cache_rebuilt))
}

/// Discover every project source file, map it to its target binary path, and
/// reject ambiguous configurations where multiple inputs would write to the same
/// output.
fn collect_project_compile_work(
    root: &Path,
    config: &RotomConfig,
) -> Result<Vec<(PathBuf, PathBuf)>> {
    let root_pairs = project_root_pairs(root, config)?;
    let mut work = Vec::new();

    for (source_root, binary_root) in root_pairs {
        let mut files = Vec::new();
        collect_compile_source_files(&source_root, &mut files).map_err(|source| {
            ProjectError::Io {
                action: "Failed to read source root",
                path: source_root.clone(),
                source,
            }
        })?;

        for input in files {
            let output = project_output_path(
                &input,
                &source_root,
                &binary_root,
                config.workspace.project_type,
                config.game_family(),
            );
            work.push((input, output));
        }
    }

    if work.is_empty() {
        return Err(ProjectError::NoProjectSourceFiles);
    }

    let collisions = detect_project_output_collisions(&work);
    if !collisions.is_empty() {
        return Err(ProjectError::OutputCollision {
            details: collisions.join("; "),
        });
    }

    work.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(work)
}

fn collect_project_decompile_work(
    root: &Path,
    config: &RotomConfig,
) -> Result<Vec<(PathBuf, PathBuf)>> {
    let root_pairs = project_root_pairs(root, config)?;
    let mut work = Vec::new();

    for (source_root, binary_root) in root_pairs {
        let mut files = Vec::new();
        collect_binary_files(&binary_root, &mut files).map_err(|source| ProjectError::Io {
            action: "Failed to read binary root",
            path: binary_root.clone(),
            source,
        })?;

        for input in files {
            let relative = input.strip_prefix(&binary_root).unwrap_or(&input);
            let output_dir = match relative.parent() {
                Some(parent) => source_root.join(parent),
                None => source_root.clone(),
            };
            work.push((input, output_dir));
        }
    }

    if work.is_empty() {
        return Err(ProjectError::NoProjectBinaryFiles);
    }

    work.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(work)
}

/// DSPRE extensionless binary path paired with `script_path` (`[paths]` source/binary roots).
pub(crate) fn dspre_binary_path_for_script(
    root: &Path,
    config: &RotomConfig,
    script_path: &Path,
) -> Result<PathBuf> {
    debug_assert!(matches!(
        config.workspace.project_type,
        ProjectTypeConfig::Dspre
    ));

    let pairs = project_root_pairs(root, config)?;
    for (source_root, binary_root) in pairs {
        if script_path.starts_with(&source_root) {
            return Ok(project_output_path(
                script_path,
                &source_root,
                &binary_root,
                ProjectTypeConfig::Dspre,
                config.game_family(),
            ));
        }
    }
    Err(ProjectError::DspreScriptOutsideSourceRoots {
        script: script_path.to_path_buf(),
    })
}

fn project_root_pairs(root: &Path, config: &RotomConfig) -> Result<Vec<(PathBuf, PathBuf)>> {
    let source_roots = config.source_roots(root);
    if source_roots.is_empty() {
        return Err(ProjectError::MissingSourceRoots);
    }

    let binary_roots = config.binary_roots(root);
    if binary_roots.is_empty() {
        return Err(ProjectError::MissingBinaryRoots);
    }

    if binary_roots.len() == source_roots.len() {
        return Ok(source_roots.into_iter().zip(binary_roots).collect());
    }

    let primary_binary_root = binary_roots[0].clone();
    Ok(source_roots
        .into_iter()
        .map(|source_root| (source_root, primary_binary_root.clone()))
        .collect())
}

fn collect_compile_source_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if !dir.exists() {
        return Ok(());
    }

    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_compile_source_files(&path, out)?;
            continue;
        }

        let is_supported = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("rotom")
                    || extension.eq_ignore_ascii_case("script")
                    || extension.eq_ignore_ascii_case("s")
                    || extension.eq_ignore_ascii_case("json")
            });

        if is_supported {
            out.push(path);
        }
    }

    Ok(())
}

fn collect_binary_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if !dir.exists() {
        return Ok(());
    }

    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_binary_files(&path, out)?;
            continue;
        }

        // Skip hidden files and known non-binary metadata files
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if file_name.starts_with('.') {
            continue;
        }

        if path.extension().is_none_or(|extension| {
            extension
                .to_str()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("bin"))
        }) {
            out.push(path);
        }
    }

    Ok(())
}

fn detect_project_output_collisions(work: &[(PathBuf, PathBuf)]) -> Vec<String> {
    let mut output_to_inputs: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    for (input, output) in work {
        output_to_inputs
            .entry(output.clone())
            .or_default()
            .push(input.clone());
    }

    let mut collisions = output_to_inputs
        .into_iter()
        .filter(|(_, inputs)| inputs.len() > 1)
        .map(|(output, inputs)| {
            let mut names: Vec<String> = inputs
                .iter()
                .map(|input| input.display().to_string())
                .collect();
            names.sort();
            format!("{} <= [{}]", output.display(), names.join(", "))
        })
        .collect::<Vec<_>>();
    collisions.sort();
    collisions
}

#[cfg(test)]
mod tests {
    use super::{
        collect_compile_source_files, compile_project, decompile_project,
        detect_project_output_collisions, project_output_path, project_root_pairs,
        relative_project_path,
    };
    use crate::compile_state::{COMPILER_VERSION, CompileState, FileStatus};
    use crate::project::config::{
        DatabaseConfig, PathsConfig, ProjectMetadata, ProjectTypeConfig, RotomConfig,
        WorkspaceConfig,
    };
    use crate::{DatabaseV2, GameFamily};
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    fn project_config(project_type: ProjectTypeConfig) -> RotomConfig {
        RotomConfig {
            format_version: 1,
            project: ProjectMetadata {
                name: "example".to_string(),
            },
            workspace: WorkspaceConfig {
                project_type,
                game_family: None,
            },
            paths: PathsConfig {
                database_dir: ".rotom/command_database".to_string(),
                cache_dir: ".rotom/cache".to_string(),
                status_dir: ".rotom/status".to_string(),
                source_roots: vec!["scripts".to_string()],
                include_roots: Vec::new(),
                binary_roots: vec!["build/scripts".to_string()],
            },
            database: Some(DatabaseConfig {
                default_file: DatabaseV2::test_platinum_path().display().to_string(),
            }),
        }
    }

    fn decomp_project_config() -> RotomConfig {
        RotomConfig {
            format_version: 1,
            project: ProjectMetadata {
                name: "example".to_string(),
            },
            workspace: WorkspaceConfig {
                project_type: ProjectTypeConfig::Decomp,
                game_family: Some(GameFamily::Platinum),
            },
            paths: PathsConfig {
                database_dir: ".rotom/command_database".to_string(),
                cache_dir: ".rotom/cache".to_string(),
                status_dir: ".rotom/status".to_string(),
                source_roots: vec!["res/field/scripts".to_string()],
                include_roots: vec![
                    "include".to_string(),
                    "generated".to_string(),
                    "res/field/scripts".to_string(),
                ],
                binary_roots: vec!["res/field/scripts".to_string()],
            },
            database: Some(DatabaseConfig {
                default_file: DatabaseV2::test_platinum_path().display().to_string(),
            }),
        }
    }

    #[test]
    fn project_output_path_maps_dspre_and_decomp_layouts() {
        let source_root = Path::new("/tmp/scripts");
        let binary_root = Path::new("/tmp/build/scripts");

        assert_eq!(
            project_output_path(
                Path::new("/tmp/scripts/0001.rotom"),
                source_root,
                binary_root,
                ProjectTypeConfig::Dspre,
                None,
            ),
            Path::new("/tmp/build/scripts/0001")
        );
        assert_eq!(
            project_output_path(
                Path::new("/tmp/scripts/sub/0001.rotom"),
                source_root,
                binary_root,
                ProjectTypeConfig::Decomp,
                Some(GameFamily::Platinum),
            ),
            Path::new("/tmp/build/scripts/sub/0001")
        );
        assert_eq!(
            project_output_path(
                Path::new("/tmp/scripts/sub/0001.rotom"),
                source_root,
                binary_root,
                ProjectTypeConfig::Decomp,
                Some(GameFamily::HGSS),
            ),
            Path::new("/tmp/build/scripts/sub/0001.bin")
        );
    }

    #[test]
    fn relative_project_path_normalizes_project_paths_and_rejects_external_paths() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let nested = root.join("scripts/test.rotom");
        fs::create_dir_all(nested.parent().unwrap()).unwrap();
        fs::write(&nested, "script Main #1:\n    End\n").unwrap();

        assert_eq!(
            relative_project_path(root, &nested).unwrap(),
            "scripts/test.rotom"
        );
        assert_eq!(
            relative_project_path(root, &nested.canonicalize().unwrap()).unwrap(),
            "scripts/test.rotom"
        );
        assert!(relative_project_path(root, Path::new("/tmp/not-in-project.rotom")).is_err());
    }

    #[test]
    fn project_root_pairs_preserve_positional_mapping_when_lengths_match() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let config = RotomConfig {
            format_version: 1,
            project: ProjectMetadata {
                name: "example".to_string(),
            },
            workspace: WorkspaceConfig {
                project_type: ProjectTypeConfig::Dspre,
                game_family: None,
            },
            paths: PathsConfig {
                database_dir: ".rotom/command_database".to_string(),
                cache_dir: ".rotom/cache".to_string(),
                status_dir: ".rotom/status".to_string(),
                source_roots: vec!["scripts/a".to_string(), "scripts/b".to_string()],
                include_roots: Vec::new(),
                binary_roots: vec!["build/a".to_string(), "build/b".to_string()],
            },
            database: None,
        };

        assert_eq!(
            project_root_pairs(root, &config).unwrap(),
            vec![
                (root.join("scripts/a"), root.join("build/a")),
                (root.join("scripts/b"), root.join("build/b"))
            ]
        );
    }

    #[test]
    fn project_root_pairs_fall_back_to_primary_binary_root_when_lengths_differ() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let config = RotomConfig {
            format_version: 1,
            project: ProjectMetadata {
                name: "example".to_string(),
            },
            workspace: WorkspaceConfig {
                project_type: ProjectTypeConfig::Dspre,
                game_family: None,
            },
            paths: PathsConfig {
                database_dir: ".rotom/command_database".to_string(),
                cache_dir: ".rotom/cache".to_string(),
                status_dir: ".rotom/status".to_string(),
                source_roots: vec!["scripts/a".to_string(), "scripts/b".to_string()],
                include_roots: Vec::new(),
                binary_roots: vec!["build/shared".to_string()],
            },
            database: None,
        };

        assert_eq!(
            project_root_pairs(root, &config).unwrap(),
            vec![
                (root.join("scripts/a"), root.join("build/shared")),
                (root.join("scripts/b"), root.join("build/shared"))
            ]
        );
    }

    #[test]
    fn collect_compile_source_files_only_keeps_supported_extensions() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("keep.rotom"), "").unwrap();
        fs::write(root.join("legacy.script"), "").unwrap();
        fs::write(root.join("asm.s"), "").unwrap();
        fs::write(root.join("level.json"), "").unwrap();
        fs::write(root.join("nested/ignore.txt"), "").unwrap();

        let mut files = Vec::new();
        collect_compile_source_files(root, &mut files).unwrap();
        files.sort();

        assert_eq!(
            files,
            vec![
                root.join("asm.s"),
                root.join("keep.rotom"),
                root.join("legacy.script"),
                root.join("level.json")
            ]
        );
    }

    #[test]
    fn detect_project_output_collisions_reports_duplicate_outputs() {
        let collisions = detect_project_output_collisions(&[
            (
                PathBuf::from("scripts/a.rotom"),
                PathBuf::from("build/0001"),
            ),
            (
                PathBuf::from("scripts/b.rotom"),
                PathBuf::from("build/0001"),
            ),
        ]);

        assert_eq!(
            collisions,
            vec!["build/0001 <= [scripts/a.rotom, scripts/b.rotom]".to_string()]
        );
        assert!(
            detect_project_output_collisions(&[
                (
                    PathBuf::from("scripts/a.rotom"),
                    PathBuf::from("build/0001")
                ),
                (
                    PathBuf::from("scripts/b.rotom"),
                    PathBuf::from("build/0002")
                ),
            ])
            .is_empty()
        );
    }

    #[test]
    fn compile_project_writes_to_project_binary_root() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("scripts")).unwrap();
        fs::write(
            root.join("scripts/0001.rotom"),
            "script Main #1:\n    End\n",
        )
        .unwrap();

        let result = compile_project(root, &project_config(ProjectTypeConfig::Dspre), false)
            .expect("project compile should succeed");

        assert!(result.is_success());
        assert!(root.join("build/scripts/0001").exists());
    }

    #[test]
    fn compile_project_loads_dspre_text_archive_constants() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("scripts")).unwrap();
        fs::create_dir_all(root.join("expanded/textArchives")).unwrap();
        fs::write(
            root.join("scripts/0001.rotom"),
            "alias SPECIES_NIDORANF as MON\nscript Main #1:\n    End\n",
        )
        .unwrap();
        fs::write(
            root.join("expanded/textArchives/0412.json"),
            concat!(
                "{\n",
                "  \"key\": 30764,\n",
                "  \"messages\": [\n",
                "    { \"id\": \"msg_0412_00000\", \"en_US\": \"-----\" },\n",
                "    { \"id\": \"msg_0412_00001\", \"en_US\": \"Bulbasaur\" },\n",
                "    { \"id\": \"msg_0412_00002\", \"en_US\": \"Nidoran♀\" }\n",
                "  ]\n",
                "}\n"
            ),
        )
        .unwrap();

        let result = compile_project(root, &project_config(ProjectTypeConfig::Dspre), false)
            .expect("project compile should succeed with DSPRE text archive constants");

        assert!(result.is_success());
        assert!(root.join("build/scripts/0001").exists());
    }

    #[test]
    fn decompile_project_writes_into_source_root() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("scripts")).unwrap();
        fs::write(
            root.join("scripts/0001.rotom"),
            "script Main #1:\n    End\n",
        )
        .unwrap();

        compile_project(root, &project_config(ProjectTypeConfig::Dspre), false).unwrap();
        fs::remove_file(root.join("scripts/0001.rotom")).unwrap();

        let result = decompile_project(root, &project_config(ProjectTypeConfig::Dspre))
            .expect("project decompile should succeed");

        assert!(result.is_success());
        assert!(root.join("scripts/0001.rotom").exists());
    }

    #[test]
    fn decompile_project_updates_compile_state_for_generated_sources() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("scripts")).unwrap();
        fs::write(
            root.join("scripts/0001.rotom"),
            "script Main #1:\n    End\n",
        )
        .unwrap();

        let config = project_config(ProjectTypeConfig::Dspre);

        compile_project(root, &config, false).unwrap();
        fs::remove_file(root.join("scripts/0001.rotom")).unwrap();
        decompile_project(root, &config).unwrap();

        let state: CompileState = serde_json::from_str(
            &fs::read_to_string(root.join(".rotom/status/compile-state.json")).unwrap(),
        )
        .unwrap();
        let entry = state.entries.get("scripts/0001.rotom").unwrap();
        let recompile = compile_project(root, &config, false).unwrap();

        assert_eq!(entry.status, FileStatus::Decompiled);
        assert_eq!(recompile.successes.len(), 0);
        assert!(recompile.failures.is_empty());
    }

    #[test]
    fn compile_project_skips_unchanged_files_after_first_build() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("res/field/scripts")).unwrap();
        fs::create_dir_all(root.join("include/constants")).unwrap();
        fs::write(
            root.join("include/constants/test.h"),
            "#define TEST_CONST 1\n",
        )
        .unwrap();
        fs::write(
            root.join("res/field/scripts/test.rotom"),
            "#include \"include/constants/test.h\"\nalias TEST_CONST as LOCAL\nscript Main #1:\n    End\n",
        )
        .unwrap();

        let config = decomp_project_config();

        let first = compile_project(root, &config, false).unwrap();
        let second = compile_project(root, &config, false).unwrap();
        fs::write(
            root.join("res/field/scripts/test.rotom"),
            "#include \"include/constants/test.h\"\nalias TEST_CONST as LOCAL\n\nscript Main #1:\n    End\n",
        )
        .unwrap();
        let third = compile_project(root, &config, false).unwrap();

        assert_eq!(first.successes.len(), 1);
        assert!(first.failures.is_empty());
        assert!(root.join(".rotom/status/compile-state.json").exists());
        assert_eq!(second.successes.len(), 0);
        assert!(second.failures.is_empty());
        assert_eq!(third.successes.len(), 1);
        assert!(third.failures.is_empty());
    }

    #[test]
    fn compile_project_rebuilds_when_dependency_header_changes() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let source_root = root.join("res/field/scripts");
        fs::create_dir_all(&source_root).unwrap();
        let header = source_root.join("local_dep.h");
        fs::write(&header, "#define TEST_CONST 1\n").unwrap();
        fs::write(
            source_root.join("test.rotom"),
            "#include \"local_dep.h\"\nalias TEST_CONST as LOCAL\nscript Main #1:\n    End\n",
        )
        .unwrap();

        let config = decomp_project_config();

        let first = compile_project(root, &config, false).unwrap();
        let second = compile_project(root, &config, false).unwrap();
        fs::write(&header, "#define TEST_CONST 2\n").unwrap();
        let third = compile_project(root, &config, false).unwrap();

        assert_eq!(first.successes.len(), 1);
        assert_eq!(second.successes.len(), 0);
        assert_eq!(third.successes.len(), 1);
        assert!(third.failures.is_empty());
    }

    #[test]
    fn compile_project_tracks_non_cached_local_include_dependencies() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let source_root = root.join("res/field/scripts");
        fs::create_dir_all(&source_root).unwrap();
        let include_path = source_root.join("local_dep.inc");
        fs::write(&include_path, "#define TEST_CONST 1\n").unwrap();
        fs::write(
            source_root.join("test.rotom"),
            "#include \"local_dep.inc\"\nalias TEST_CONST as LOCAL\nscript Main #1:\n    End\n",
        )
        .unwrap();

        let config = decomp_project_config();

        let first = compile_project(root, &config, false).unwrap();
        let second = compile_project(root, &config, false).unwrap();
        fs::write(&include_path, "#define TEST_CONST 2\n").unwrap();
        let third = compile_project(root, &config, false).unwrap();

        let state: CompileState = serde_json::from_str(
            &fs::read_to_string(root.join(".rotom/status/compile-state.json")).unwrap(),
        )
        .unwrap();
        let entry = state.entries.get("res/field/scripts/test.rotom").unwrap();

        assert_eq!(first.successes.len(), 1);
        assert_eq!(second.successes.len(), 0);
        assert_eq!(third.successes.len(), 1);
        assert!(
            entry
                .dependency_hashes
                .contains_key("res/field/scripts/local_dep.inc")
        );
        assert!(
            !entry
                .dependency_hashes
                .contains_key("res/field/scripts/test.rotom")
        );
    }

    #[test]
    fn compile_project_force_recompiles_unchanged_files() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("res/field/scripts")).unwrap();
        fs::write(
            root.join("res/field/scripts/test.rotom"),
            "script Main #1:\n    End\n",
        )
        .unwrap();

        let config = decomp_project_config();

        let first = compile_project(root, &config, false).unwrap();
        let forced = compile_project(root, &config, true).unwrap();

        assert_eq!(first.successes.len(), 1);
        assert_eq!(forced.successes.len(), 1);
        assert!(forced.failures.is_empty());
    }

    #[test]
    fn compile_project_rebuilds_when_database_hash_changes() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("res/field/scripts")).unwrap();
        fs::create_dir_all(root.join(".rotom/command_database")).unwrap();
        fs::copy(
            DatabaseV2::test_platinum_path(),
            root.join(".rotom/command_database/platinum_v2.json"),
        )
        .unwrap();
        fs::write(
            root.join("res/field/scripts/test.rotom"),
            "script Main #1:\n    End\n",
        )
        .unwrap();

        let mut config = decomp_project_config();
        config.database = Some(DatabaseConfig {
            default_file: root
                .join(".rotom/command_database/platinum_v2.json")
                .display()
                .to_string(),
        });

        let first = compile_project(root, &config, false).unwrap();
        let second = compile_project(root, &config, false).unwrap();
        let mut db =
            fs::read_to_string(root.join(".rotom/command_database/platinum_v2.json")).unwrap();
        db.push('\n');
        fs::write(root.join(".rotom/command_database/platinum_v2.json"), db).unwrap();
        let third = compile_project(root, &config, false).unwrap();

        assert_eq!(first.successes.len(), 1);
        assert_eq!(second.successes.len(), 0);
        assert_eq!(third.successes.len(), 1);
        assert!(third.failures.is_empty());
    }

    #[test]
    fn compile_project_rebuilds_when_compile_state_compiler_version_changes() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("res/field/scripts")).unwrap();
        fs::write(
            root.join("res/field/scripts/test.rotom"),
            "script Main #1:\n    End\n",
        )
        .unwrap();

        let config = decomp_project_config();

        let first = compile_project(root, &config, false).unwrap();
        let second = compile_project(root, &config, false).unwrap();
        assert_eq!(first.successes.len(), 1);
        assert_eq!(second.successes.len(), 0);

        let state_path = root.join(".rotom/status/compile-state.json");
        let state = fs::read_to_string(&state_path).unwrap();
        fs::write(
            &state_path,
            state.replace(COMPILER_VERSION, "phase-c-test-version"),
        )
        .unwrap();

        let third = compile_project(root, &config, false).unwrap();
        assert_eq!(third.successes.len(), 1);
        assert!(third.failures.is_empty());
    }

    #[test]
    fn compile_project_rebuilds_when_global_constant_cache_changes() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("res/field/scripts")).unwrap();
        fs::create_dir_all(root.join("include/constants")).unwrap();
        fs::write(
            root.join("include/constants/test.h"),
            "#define TEST_CONST 1\n",
        )
        .unwrap();
        fs::write(
            root.join("res/field/scripts/test.rotom"),
            "alias TEST_CONST as LOCAL\nscript Main #1:\n    End\n",
        )
        .unwrap();

        let config = decomp_project_config();

        let first = compile_project(root, &config, false).unwrap();
        let second = compile_project(root, &config, false).unwrap();
        fs::write(
            root.join("include/constants/test.h"),
            "#define TEST_CONST 2\n",
        )
        .unwrap();
        let third = compile_project(root, &config, false).unwrap();

        assert_eq!(first.successes.len(), 1);
        assert_eq!(second.successes.len(), 0);
        assert_eq!(third.successes.len(), 1);
        assert!(third.failures.is_empty());
    }

    #[test]
    fn compile_project_compiles_s_files_with_required_local_includes() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let source_root = root.join("res/field/scripts");
        fs::create_dir_all(&source_root).unwrap();
        fs::write(
            source_root.join("test.s"),
            "#define LOCAL_CONST 1\n#define LOCAL_ALIAS LOCAL_CONST\n\n    ScriptEntry Test\n    ScriptEntryEnd\n\nTest:\n    End\n",
        )
        .unwrap();

        let config = decomp_project_config();

        let result = compile_project(root, &config, false).unwrap();

        assert_eq!(result.successes.len(), 1);
        assert!(result.failures.is_empty());
    }

    #[test]
    fn compile_project_does_not_collect_file_local_constants_for_legacy_dspre_scripts() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("scripts")).unwrap();
        fs::write(
            root.join("scripts/test.script"),
            "/*\n#include \"missing.h\"\n*/\nScript 1:\nEnd\n",
        )
        .unwrap();

        let result =
            compile_project(root, &project_config(ProjectTypeConfig::Dspre), false).unwrap();

        assert_eq!(result.successes.len(), 1);
        assert!(result.failures.is_empty());
    }
}
