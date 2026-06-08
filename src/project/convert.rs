use crate::{
    ConstantDb, DatabaseV2, DecompileFileResult, ScriptOutput, decompile_to_ir,
    is_levelscript_path, transpiler,
};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::Value;
use uxie::{GameFamily, Workspace};
use xxhash_rust::xxh3::xxh3_64;

use super::compile::{dspre_binary_path_for_script, update_decompile_state};
use super::config::{ProjectTypeConfig, RotomConfig};
use super::dspre_db_migration::{
    dspre_edited_db_suggests_followplat, dspre_merge_shape_diff_score, find_local_scrcmd_v1_path,
    maybe_reconcile_scrcmd_v1_into_v2,
};
use super::dspre_script_header::dspre_export_baseline_from_script_paths;
use super::error::{ProjectError, Result};
use super::scrcmd_baseline::{
    VanillaScrcmdV1, fetch_following_platinum_scrcmd_v1_at_commit,
    fetch_vanilla_scrcmd_v1_at_baseline,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConvertOptions {
    pub dry_run: bool,
    pub non_interactive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionPlan {
    pub input: PathBuf,
    pub output: PathBuf,
    pub backup: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ConvertReport {
    pub converted: usize,
    pub plans: Vec<ConversionPlan>,
    pub backup_dir: Option<PathBuf>,
    pub dry_run: bool,
}

pub fn find_convertible_files(root: &Path, config: &RotomConfig) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();

    for source_root in config.source_roots(root) {
        collect_convertible_files(&source_root, config.workspace.project_type, &mut files)?;
    }

    files.sort();
    Ok(files)
}

/// One DSPRE `*.script` placeholder: paired binary drives output (`.rotom` vs levelscript JSON).
///
/// Reads and disassembles the binary once. With `dry_run`, returns a plan only; otherwise backs up
/// the `.script`, writes decompiled output beside it, removes the placeholder, and returns compile
/// state metadata.
#[allow(clippy::too_many_lines)]
fn convert_one_dspre_script_placeholder(
    root: &Path,
    config: &RotomConfig,
    script_placeholder: &Path,
    backup: &Path,
    dry_run: bool,
    db: &DatabaseV2,
    constants: &ConstantDb,
) -> Result<(ConversionPlan, Option<DecompileFileResult>)> {
    let binary_path = dspre_binary_path_for_script(root, config, script_placeholder)?;
    if !binary_path.exists() {
        return Err(ProjectError::DspreConvertMissingBinary {
            script: script_placeholder.to_path_buf(),
            binary: binary_path,
        });
    }

    let bytes = fs::read(&binary_path).map_err(|source| ProjectError::Io {
        action: "Failed to read paired script binary",
        path: binary_path.clone(),
        source,
    })?;
    if bytes.is_empty() {
        // DSPRE sometimes emits 0-byte binaries for unreferenced script slots. There is
        // nothing to disassemble, but we still emit a stub .rotom so the slot
        // is accounted for and can be repurposed if needed.
        let output_path = script_placeholder.with_extension("rotom");
        let plan = ConversionPlan {
            input: script_placeholder.to_path_buf(),
            output: output_path.clone(),
            backup: backup.to_path_buf(),
        };
        if dry_run {
            return Ok((plan, None));
        }
        if let Some(parent) = backup.parent() {
            fs::create_dir_all(parent).map_err(|source| ProjectError::Io {
                action: "Failed to create directory",
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::copy(script_placeholder, backup).map_err(|source| ProjectError::Io {
            action: "Failed to back up",
            path: backup.to_path_buf(),
            source,
        })?;
        let stub = "// This script file had an empty (0-byte) binary in the original ROM.\n\
                    // It is not referenced by any map header and can be repurposed.\n";
        fs::write(&output_path, stub).map_err(|source| ProjectError::Io {
            action: "Failed to write stub",
            path: output_path.clone(),
            source,
        })?;
        fs::remove_file(script_placeholder).map_err(|source| ProjectError::Io {
            action: "Failed to remove",
            path: script_placeholder.to_path_buf(),
            source,
        })?;
        return Ok((
            plan,
            Some(crate::DecompileFileResult {
                input: binary_path,
                output: output_path,
                size: 0,
                quirks: crate::BinaryQuirk::default(),
            }),
        ));
    }
    let script_output = decompile_to_ir(bytes, db).map_err(|e| ProjectError::DspreDecompile {
        script: script_placeholder.to_path_buf(),
        binary: binary_path.clone(),
        source: e,
    })?;
    let output_path = match &script_output {
        ScriptOutput::Levelscript(_) => script_placeholder.with_extension("json"),
        ScriptOutput::Normal { .. } => script_placeholder.with_extension("rotom"),
    };

    let plan = ConversionPlan {
        input: script_placeholder.to_path_buf(),
        output: output_path,
        backup: backup.to_path_buf(),
    };

    if dry_run {
        return Ok((plan, None));
    }

    if let Some(parent) = backup.parent() {
        fs::create_dir_all(parent).map_err(|source| ProjectError::Io {
            action: "Failed to create directory",
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::copy(script_placeholder, backup).map_err(|source| ProjectError::Io {
        action: "Failed to back up",
        path: backup.to_path_buf(),
        source,
    })?;

    let out_dir = script_placeholder
        .parent()
        .map_or_else(|| root.to_path_buf(), Path::to_path_buf);

    let decompiled = crate::decompile_file_from_ir(
        &binary_path,
        &script_output,
        None,
        Some(&out_dir),
        db,
        Some(constants),
    )
    .map_err(|failure| ProjectError::DspreDecompile {
        script: script_placeholder.to_path_buf(),
        binary: binary_path,
        source: failure.error,
    })?;

    fs::remove_file(script_placeholder).map_err(|source| ProjectError::Io {
        action: "Failed to remove",
        path: script_placeholder.to_path_buf(),
        source,
    })?;

    Ok((plan, Some(decompiled)))
}

/// Legacy DSPRE `.script` / decomp `.s` → `.rotom` / levelscript JSON. DSPRE: may merge edited scrcmd v1 descriptions into v2 DB before loading it.
#[allow(clippy::too_many_lines)] // Complex migration hooks will grow; split when DSPRE migrate path lands.
pub fn convert_project(
    root: &Path,
    config: &RotomConfig,
    options: ConvertOptions,
) -> Result<ConvertReport> {
    let files = find_convertible_files(root, config)?;
    if files.is_empty() {
        return Ok(ConvertReport {
            converted: 0,
            plans: Vec::new(),
            backup_dir: None,
            dry_run: options.dry_run,
        });
    }

    let dspre_migration = matches!(config.workspace.project_type, ProjectTypeConfig::Dspre);
    let mut db_path_opt = config.database_file(root);
    if dspre_migration && db_path_opt.is_none() {
        return Err(ProjectError::MissingDefaultDatabase);
    }

    // Resolve user scrcmd path once — used for Platinum baseline selection and reconciliation.
    let user_path_opt: Option<PathBuf> = if dspre_migration {
        config
            .game_family()
            .and_then(|family| find_local_scrcmd_v1_path(root, family, config))
    } else {
        None
    };

    let vanilla_opt: Option<VanillaScrcmdV1> = if dspre_migration
        && user_path_opt.is_some()
        && let Some(baseline) = dspre_export_baseline_from_script_paths(&files)?
    {
        eprintln!(
            "DSPRE script export baseline: {} (from {})",
            baseline.oldest.format("%Y-%m-%d %H:%M:%S"),
            baseline.sample_path.display()
        );
        if let Some(family) = config.game_family() {
            let vanilla = match family {
                GameFamily::Platinum => {
                    match fetch_vanilla_scrcmd_v1_at_baseline(GameFamily::Platinum, baseline.oldest)
                    {
                        Ok(stock) => {
                            // Try Following Platinum when the user's edited scrcmd fits it better.
                            let user_scrcmd = user_path_opt
                                .as_ref()
                                .and_then(|p| fs::read_to_string(p).ok())
                                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                                .and_then(|v| v.get("scrcmd").and_then(Value::as_object).cloned());
                            if let Some(ref user_map) = user_scrcmd {
                                match fetch_following_platinum_scrcmd_v1_at_commit(
                                    &stock.commit_sha,
                                ) {
                                    Ok(Some(follow)) => {
                                        let stock_root = serde_json::from_str::<Value>(&stock.json);
                                        let follow_root =
                                            serde_json::from_str::<Value>(&follow.json);
                                        if let (Ok(sr), Ok(fr)) = (stock_root, follow_root)
                                            && let (Some(sm), Some(fm)) = (
                                                sr.get("scrcmd").and_then(Value::as_object),
                                                fr.get("scrcmd").and_then(Value::as_object),
                                            )
                                        {
                                            let w_std = dspre_merge_shape_diff_score(user_map, sm);
                                            let w_follow =
                                                dspre_merge_shape_diff_score(user_map, fm);
                                            let suggests =
                                                dspre_edited_db_suggests_followplat(user_map);
                                            if w_follow < w_std || (w_follow == w_std && suggests) {
                                                eprintln!(
                                                    "Following Platinum scrcmd baseline chosen (merge-shape diff vs edited DB: follow={w_follow}, stock={w_std}; tie-break followplat_hint={suggests})."
                                                );
                                                Some(follow)
                                            } else {
                                                Some(stock)
                                            }
                                        } else {
                                            Some(stock)
                                        }
                                    }
                                    Ok(None) => Some(stock),
                                    Err(e) => {
                                        eprintln!(
                                            "Warning: could not fetch Following Platinum scrcmd baseline ({e}); using stock Platinum baseline."
                                        );
                                        Some(stock)
                                    }
                                }
                            } else {
                                Some(stock)
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "Warning: could not resolve vanilla scrcmd v1 baseline ({e}); skipping edited scrcmd v1 merge."
                            );
                            None
                        }
                    }
                }
                GameFamily::HGSS | GameFamily::DP => {
                    match fetch_vanilla_scrcmd_v1_at_baseline(family, baseline.oldest) {
                        Ok(vanilla) => Some(vanilla),
                        Err(e) => {
                            eprintln!(
                                "Warning: could not resolve vanilla scrcmd v1 baseline ({e}); skipping edited scrcmd v1 merge."
                            );
                            None
                        }
                    }
                }
            };
            if let Some(vanilla) = vanilla {
                let short = vanilla
                    .commit_sha
                    .get(..7)
                    .unwrap_or(vanilla.commit_sha.as_str());
                eprintln!(
                    "Resolved vanilla scrcmd v1 at commit {short} ({}) — {} bytes",
                    vanilla.repo_path,
                    vanilla.json.len()
                );
                Some(vanilla)
            } else {
                None
            }
        } else {
            eprintln!(
                "Skipping online scrcmd v1 baseline: add [workspace].game_family to rotom.toml (Platinum, HGSS, or DP)."
            );
            None
        }
    } else {
        None
    };

    // When the v1 baseline resolved to Following Platinum, use its v2 DB.
    if let (Some(vanilla), Some(family)) = (&vanilla_opt, config.game_family())
        && family == GameFamily::Platinum
        && vanilla.repo_path.to_ascii_lowercase().contains("following")
        && let Some(current) = &db_path_opt
    {
        let follow = config.database_dir(root).join("following_platinum_v2.json");
        if follow.is_file() && follow != *current {
            eprintln!(
                "Switching to Following Platinum v2 database ({}).",
                follow.display()
            );
            db_path_opt = Some(follow);
        }
    }

    let timestamp = Utc::now().format("%Y%m%d%H%M%S").to_string();
    let backup_dir = root.join(".rotom/backups").join(timestamp);

    if dspre_migration {
        let v2_path = db_path_opt
            .as_ref()
            .expect("[database].default_file required for DSPRE convert");
        if let Some(family) = config.game_family() {
            let _ = maybe_reconcile_scrcmd_v1_into_v2(
                root,
                config,
                vanilla_opt.as_ref(),
                family,
                v2_path,
                options,
                user_path_opt.as_deref(),
            )?;
        }
    }

    let (db, dspre_db_hash): (_, Option<u64>) = if dspre_migration {
        let path = db_path_opt.expect("[database].default_file required for DSPRE convert");
        let db_hash = fs::read(&path)
            .map(|bytes| xxh3_64(&bytes))
            .map_err(|source| ProjectError::Io {
                action: "Failed to hash database file",
                path: path.clone(),
                source,
            })?;
        let loaded_db = DatabaseV2::load(&path).map_err(ProjectError::from)?;
        (Some(loaded_db), Some(db_hash))
    } else {
        let loaded = db_path_opt
            .map(|path| DatabaseV2::load(&path))
            .transpose()
            .map_err(ProjectError::from)?;
        (loaded, None)
    };

    let mut constants = ConstantDb::new();
    if let Some(ref db) = db {
        let _ = constants.load_from_db(db);
    }
    let database_dir = config.database_dir(root);
    if database_dir.exists() {
        let _ = constants
            .load_directory(&database_dir)
            .map_err(ProjectError::from)?;
    }
    match config.workspace.project_type {
        ProjectTypeConfig::Decomp => {
            if let Some(game_family) = config.game_family() {
                let include_roots = config.include_roots(root);
                if let Ok((symbols, _)) = Workspace::load_cached_symbols(
                    &config.cache_dir(root),
                    root,
                    &include_roots,
                    game_family,
                ) {
                    let _ = constants.load_decomp_symbols(root, (*symbols).clone());
                }
            }
        }
        ProjectTypeConfig::Dspre => {
            let language = if let Ok(ws) = Workspace::open(root) {
                let language = ws.language;
                let _ = constants.load_dspre_symbols((*ws.symbols).clone());
                language
            } else {
                uxie::GameLanguage::English
            };
            let _ = constants
                .load_dspre_text_archives(root, language)
                .map_err(ProjectError::from)?;
        }
        ProjectTypeConfig::Generic | ProjectTypeConfig::HgEngine => {}
    }

    let mut dspre_from_binary_successes = Vec::new();
    let mut plans = Vec::with_capacity(files.len());
    for input in files {
        let relative = input.strip_prefix(root).unwrap_or(&input);
        let backup = backup_dir.join(relative);

        if dspre_migration {
            let db_loaded = db
                .as_ref()
                .expect("DSPRE convert requires rotom.toml [database].default_file");
            let (plan, decompiled) = convert_one_dspre_script_placeholder(
                root,
                config,
                &input,
                &backup,
                options.dry_run,
                db_loaded,
                &constants,
            )?;
            plans.push(plan);
            if let Some(success) = decompiled {
                dspre_from_binary_successes.push(success);
            }
            continue;
        }

        let source = fs::read_to_string(&input).map_err(|source| ProjectError::Io {
            action: "Failed to read",
            path: input.clone(),
            source,
        })?;

        // Detect levelscripts and route to the appropriate transpiler/output format.
        let is_levelscript =
            is_levelscript_path(&input) || transpiler::is_levelscript_source(&source);

        let (output, converted) = if is_levelscript {
            let output = input.with_extension("json");
            let result =
                transpiler::transpile_levelscript(&source, Some(&constants)).map_err(|error| {
                    ProjectError::ConvertDecomp {
                        path: input.clone(),
                        line: error.line,
                        message: error.to_string(),
                    }
                })?;
            let json = serde_json::to_string_pretty(&result.levelscript)
                .map_err(|source| ProjectError::SerializeJson { source })?;
            (output, json)
        } else {
            let output = input.with_extension("rotom");
            let mut converted = match config.workspace.project_type {
                ProjectTypeConfig::Decomp => {
                    transpiler::transpile_decomp(&source, db.as_ref(), Some(root))
                        .map(|result| result.source)
                        .map_err(|error| ProjectError::ConvertDecomp {
                            path: input.clone(),
                            line: error.line,
                            message: error.to_string(),
                        })?
                }
                _ => continue,
            };
            if let Some(include_path) = config.global_include_path() {
                let include_line = format!("#include \"{include_path}\"");
                if !converted.lines().any(|line| line.trim() == include_line) {
                    converted = format!("{include_line}\n\n{converted}");
                }
            }
            (output, converted)
        };

        plans.push(ConversionPlan {
            input: input.clone(),
            output: output.clone(),
            backup: backup.clone(),
        });

        if options.dry_run {
            continue;
        }

        if let Some(parent) = backup.parent() {
            fs::create_dir_all(parent).map_err(|source| ProjectError::Io {
                action: "Failed to create directory",
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::copy(&input, &backup).map_err(|source| ProjectError::Io {
            action: "Failed to back up",
            path: backup.clone(),
            source,
        })?;

        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent).map_err(|source| ProjectError::Io {
                action: "Failed to create directory",
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::write(&output, converted).map_err(|source| ProjectError::Io {
            action: "Failed to write",
            path: output.clone(),
            source,
        })?;

        if output != input {
            fs::remove_file(&input).map_err(|source| ProjectError::Io {
                action: "Failed to remove",
                path: input.clone(),
                source,
            })?;
        }
    }

    if !options.dry_run && !dspre_from_binary_successes.is_empty() {
        let db_hash = dspre_db_hash.expect("DSPRE convert records DB hash above");
        update_decompile_state(root, config, db_hash, &dspre_from_binary_successes)?;
    }

    Ok(ConvertReport {
        converted: if options.dry_run { 0 } else { plans.len() },
        plans,
        backup_dir: Some(backup_dir),
        dry_run: options.dry_run,
    })
}

fn collect_convertible_files(
    dir: &Path,
    project_type: ProjectTypeConfig,
    out: &mut Vec<PathBuf>,
) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(dir).map_err(|source| ProjectError::Io {
        action: "Failed to read source root",
        path: dir.to_path_buf(),
        source,
    })? {
        let path = entry
            .map_err(|source| ProjectError::Io {
                action: "Failed to read source root entry",
                path: dir.to_path_buf(),
                source,
            })?
            .path();
        if path.is_dir() {
            collect_convertible_files(&path, project_type, out)?;
            continue;
        }

        if is_convertible_file(&path, project_type) {
            out.push(path);
        }
    }

    Ok(())
}

fn is_convertible_file(path: &Path, project_type: ProjectTypeConfig) -> bool {
    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");

    match project_type {
        ProjectTypeConfig::Dspre => extension.eq_ignore_ascii_case("script"),
        ProjectTypeConfig::Decomp => {
            if !extension.eq_ignore_ascii_case("s") {
                return false;
            }

            // Both regular scripts and levelscripts are convertible,
            // just to different output formats.
            true
        }
        ProjectTypeConfig::Generic | ProjectTypeConfig::HgEngine => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{ConvertOptions, convert_project, find_convertible_files};
    use crate::compile_file_internal;
    use crate::database::GameFamily;
    use crate::project::config::{
        DatabaseConfig, PathsConfig, ProjectMetadata, ProjectTypeConfig, RotomConfig,
        WorkspaceConfig,
    };
    use crate::{BinaryQuirk, ConstantDb, DatabaseV2};
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn dspre_fixture_script_and_matching_binary(
        root: &Path,
        script_under_scripts: &[&str],
        stem: &str,
    ) {
        let db = DatabaseV2::test_platinum();
        let mut constants = ConstantDb::new();
        let _ = constants.load_from_db(db);

        let mut script_dir = root.join("scripts");
        for part in script_under_scripts {
            script_dir = script_dir.join(part);
        }
        fs::create_dir_all(&script_dir).unwrap();
        let script_path = script_dir.join(format!("{stem}.script"));
        fs::write(&script_path, "=== script 1\nMessage 0\nEnd\n").unwrap();

        let rotom_staging = script_dir.join(format!("{stem}.__fixture_build.rotom"));
        fs::write(&rotom_staging, "script Main #1:\n    End\n").unwrap();

        let mut bin_dir = root.join("unpacked/scripts");
        for part in script_under_scripts {
            bin_dir = bin_dir.join(part);
        }
        fs::create_dir_all(&bin_dir).unwrap();
        let binary_path = bin_dir.join(stem);
        let workspace = Arc::new(uxie::Workspace::new(
            std::path::PathBuf::new(),
            uxie::game::Game::Platinum,
        ));
        compile_file_internal(
            &rotom_staging,
            &binary_path,
            db,
            &constants,
            false,
            BinaryQuirk::default(),
            &workspace,
        )
        .unwrap();
        let _ = fs::remove_file(&rotom_staging);
    }

    fn dspre_config() -> RotomConfig {
        RotomConfig {
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
                source_roots: vec!["scripts".to_string()],
                include_roots: Vec::new(),
                binary_roots: vec!["unpacked/scripts".to_string()],
            },
            database: Some(DatabaseConfig {
                default_file: DatabaseV2::test_platinum_path().display().to_string(),
            }),
        }
    }

    fn decomp_config() -> RotomConfig {
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
                include_roots: vec!["include".to_string()],
                binary_roots: vec!["res/field/scripts".to_string()],
            },
            database: None,
        }
    }

    #[test]
    fn convert_project_dry_run_has_no_side_effects() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        dspre_fixture_script_and_matching_binary(root, &[], "test");

        let config = dspre_config();
        let report = convert_project(
            root,
            &config,
            ConvertOptions {
                dry_run: true,
                non_interactive: true,
            },
        )
        .unwrap();

        assert_eq!(report.converted, 0);
        assert_eq!(report.plans.len(), 1);
        let source_dir = root.join("scripts");
        assert!(source_dir.join("test.script").exists());
        assert!(!source_dir.join("test.rotom").exists());
        assert!(
            !report
                .backup_dir
                .as_ref()
                .expect("backup dir should still be reported")
                .exists()
        );
    }

    #[test]
    fn convert_project_creates_backups_preserving_directory_structure() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        dspre_fixture_script_and_matching_binary(root, &["sub"], "test");

        let config = dspre_config();
        let report = convert_project(
            root,
            &config,
            ConvertOptions {
                dry_run: false,
                non_interactive: true,
            },
        )
        .unwrap();

        let source_dir = root.join("scripts/sub");
        assert_eq!(report.converted, 1);
        assert!(!source_dir.join("test.script").exists());
        assert!(source_dir.join("test.rotom").exists());

        let backup_dir = report.backup_dir.expect("backup dir should be present");
        assert!(backup_dir.join("scripts/sub/test.script").exists());
    }

    #[test]
    fn find_convertible_files_only_returns_supported_legacy_sources() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let source_dir = root.join("scripts");
        fs::create_dir_all(&source_dir).unwrap();
        fs::write(source_dir.join("keep.rotom"), "script Main #1:\n    End\n").unwrap();
        fs::write(source_dir.join("convert.script"), "=== script 1\nEnd\n").unwrap();

        let files = find_convertible_files(root, &dspre_config()).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("convert.script"));
    }

    #[test]
    fn convert_project_prepends_global_include_when_missing() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let source_dir = root.join("res/field/scripts");
        fs::create_dir_all(&source_dir).unwrap();
        fs::write(
            source_dir.join("test.s"),
            "ScriptEntry Test\nScriptEntryEnd\n\nTest:\n\tEnd\n",
        )
        .unwrap();

        let report = convert_project(
            root,
            &decomp_config(),
            ConvertOptions {
                dry_run: false,
                non_interactive: true,
            },
        )
        .unwrap();
        let converted = fs::read_to_string(source_dir.join("test.rotom")).unwrap();

        assert_eq!(report.converted, 1);
        assert!(converted.starts_with("#include \"macros/scrcmd.inc\""));
    }

    #[test]
    fn find_convertible_files_for_decomp_includes_all_s_files_including_levelscripts() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let source_dir = root.join("res/field/scripts");
        fs::create_dir_all(&source_dir).unwrap();
        fs::write(source_dir.join("notes.txt"), "ScriptEntry Test\n").unwrap();
        fs::write(source_dir.join("town_init_main.s"), "Label:\n    End\n").unwrap();
        fs::write(source_dir.join("town_init_new_game.s"), "Label:\n    End\n").unwrap();
        fs::write(source_dir.join("map_hdr.s"), "Label:\n    End\n").unwrap();
        fs::write(source_dir.join("normal.s"), "Label:\n    End\n").unwrap();

        let config = RotomConfig {
            format_version: 1,
            project: ProjectMetadata {
                name: "example".to_string(),
            },
            workspace: WorkspaceConfig {
                project_type: ProjectTypeConfig::Decomp,
                game_family: None,
            },
            paths: PathsConfig {
                database_dir: ".rotom/command_database".to_string(),
                cache_dir: ".rotom/cache".to_string(),
                status_dir: ".rotom/status".to_string(),
                source_roots: vec!["res/field/scripts".to_string()],
                include_roots: Vec::new(),
                binary_roots: vec!["res/field/scripts".to_string()],
            },
            database: None,
        };

        let files = find_convertible_files(root, &config).unwrap();
        assert_eq!(
            files,
            vec![
                source_dir.join("map_hdr.s"),
                source_dir.join("normal.s"),
                source_dir.join("town_init_main.s"),
                source_dir.join("town_init_new_game.s")
            ]
        );
    }
}
