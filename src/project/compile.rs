use crate::{
    BatchCompileResult, BatchDecompileResult, CompileFailure, ConstantDb, DatabaseV2,
    DecompileFailure, compile_file_for_batch, decompile_file_for_batch,
};
use rayon::prelude::{IntoParallelRefIterator, ParallelIterator};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::config::{ProjectTypeConfig, RotomConfig};
use super::error::{ProjectError, Result};

pub fn project_output_path(
    source_path: &Path,
    source_root: &Path,
    binary_root: &Path,
    project_type: ProjectTypeConfig,
) -> PathBuf {
    let relative = source_path.strip_prefix(source_root).unwrap_or(source_path);
    match project_type {
        ProjectTypeConfig::Dspre => {
            let stem = relative.file_stem().unwrap_or_default();
            match relative.parent() {
                Some(parent) => binary_root.join(parent).join(stem),
                None => binary_root.join(stem),
            }
        }
        ProjectTypeConfig::Decomp | ProjectTypeConfig::Generic => {
            binary_root.join(relative).with_extension("bin")
        }
    }
}

pub fn compile_project(
    root: &Path,
    config: &RotomConfig,
    _force: bool,
) -> Result<BatchCompileResult> {
    let (db, constants) = load_project_database_and_constants(root, config)?;
    let work = collect_project_compile_work(root, config)?;

    let results: Vec<std::result::Result<crate::CompileResult, CompileFailure>> = work
        .par_iter()
        .map(|(input, output)| compile_file_for_batch(input, output, &db, &constants))
        .collect();

    let mut successes = Vec::new();
    let mut failures = Vec::new();
    for result in results {
        match result {
            Ok(success) => successes.push(success),
            Err(failure) => failures.push(failure),
        }
    }

    Ok(BatchCompileResult {
        successes,
        failures,
    })
}

pub fn decompile_project(
    root: &Path,
    config: &RotomConfig,
) -> Result<BatchDecompileResult> {
    let db_path = config
        .database_file(root)
        .ok_or(ProjectError::MissingDefaultDatabase)?;
    let db = DatabaseV2::load(&db_path).map_err(ProjectError::from)?;

    let work = collect_project_decompile_work(root, config)?;

    let results: Vec<std::result::Result<crate::DecompileFileResult, DecompileFailure>> = work
        .par_iter()
        .map(|(input, output_dir)| decompile_file_for_batch(input, None, Some(output_dir), &db))
        .collect();

    let mut successes = Vec::new();
    let mut failures = Vec::new();
    for result in results {
        match result {
            Ok(success) => successes.push(success),
            Err(failure) => failures.push(failure),
        }
    }

    Ok(BatchDecompileResult {
        successes,
        failures,
    })
}

fn load_project_database_and_constants(
    root: &Path,
    config: &RotomConfig,
) -> Result<(DatabaseV2, ConstantDb)> {
    let db_path = config
        .database_file(root)
        .ok_or(ProjectError::MissingDefaultDatabase)?;
    let db = DatabaseV2::load(&db_path).map_err(ProjectError::from)?;

    let mut constants = ConstantDb::new();
    let _ = constants.load_from_db(&db);

    let database_dir = config.database_dir(root);
    if database_dir.exists() {
        let _ = constants
            .load_directory(&database_dir)
            .map_err(ProjectError::from)?;
    }

    if matches!(config.workspace.project_type, ProjectTypeConfig::Decomp) {
        let _ = constants.load_decomp_project(root).map_err(ProjectError::from)?;
    }

    Ok((db, constants))
}

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
        collect_binary_files(&binary_root, &mut files).map_err(|source| {
            ProjectError::Io {
                action: "Failed to read binary root",
                path: binary_root.clone(),
                source,
            }
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

fn project_root_pairs(
    root: &Path,
    config: &RotomConfig,
) -> Result<Vec<(PathBuf, PathBuf)>> {
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
    };
    use crate::project::config::{
        DatabaseConfig, PathsConfig, ProjectMetadata, ProjectTypeConfig, RotomConfig,
        WorkspaceConfig,
    };
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
                default_file: std::env::current_dir()
                    .unwrap()
                    .join("src/db/platinum_v2.json")
                    .display()
                    .to_string(),
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
                ProjectTypeConfig::Dspre
            ),
            Path::new("/tmp/build/scripts/0001")
        );
        assert_eq!(
            project_output_path(
                Path::new("/tmp/scripts/sub/0001.rotom"),
                source_root,
                binary_root,
                ProjectTypeConfig::Decomp
            ),
            Path::new("/tmp/build/scripts/sub/0001.bin")
        );
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
            (PathBuf::from("scripts/a.rotom"), PathBuf::from("build/0001")),
            (PathBuf::from("scripts/b.rotom"), PathBuf::from("build/0001")),
        ]);

        assert_eq!(
            collisions,
            vec!["build/0001 <= [scripts/a.rotom, scripts/b.rotom]".to_string()]
        );
        assert!(detect_project_output_collisions(&[
            (PathBuf::from("scripts/a.rotom"), PathBuf::from("build/0001")),
            (PathBuf::from("scripts/b.rotom"), PathBuf::from("build/0002")),
        ])
        .is_empty());
    }

    #[test]
    fn compile_project_writes_to_project_binary_root() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("scripts")).unwrap();
        fs::write(
            root.join("scripts/0001.rotom"),
            "function Main #1:\n    End\n",
        )
        .unwrap();

        let result = compile_project(root, &project_config(ProjectTypeConfig::Dspre), false)
            .expect("project compile should succeed");

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
            "function Main #1:\n    End\n",
        )
        .unwrap();

        compile_project(root, &project_config(ProjectTypeConfig::Dspre), false).unwrap();
        fs::remove_file(root.join("scripts/0001.rotom")).unwrap();

        let result = decompile_project(root, &project_config(ProjectTypeConfig::Dspre))
            .expect("project decompile should succeed");

        assert!(result.is_success());
        assert!(root.join("scripts/0001.rotom").exists());
    }
}
