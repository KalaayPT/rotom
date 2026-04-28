use crate::{ConstantDb, DatabaseV2, is_levelscript_path, transpiler};
use chrono::Utc;
use std::fs;
use std::path::{Path, PathBuf};
use uxie::Workspace;

use super::config::{ProjectTypeConfig, RotomConfig};
use super::error::{ProjectError, Result};

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

pub fn convert_project(root: &Path, config: &RotomConfig, dry_run: bool) -> Result<ConvertReport> {
    let files = find_convertible_files(root, config)?;
    if files.is_empty() {
        return Ok(ConvertReport {
            converted: 0,
            plans: Vec::new(),
            backup_dir: None,
            dry_run,
        });
    }

    let timestamp = Utc::now().format("%Y%m%d%H%M%S").to_string();
    let backup_dir = root.join(".rotom/backups").join(timestamp);
    let db = config
        .database_file(root)
        .map(|path| DatabaseV2::load(&path))
        .transpose()
        .map_err(ProjectError::from)?;

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
            let _ = constants
                .load_dspre_text_archives(root)
                .map_err(ProjectError::from)?;
        }
        ProjectTypeConfig::Generic => {}
    }

    let mut plans = Vec::with_capacity(files.len());
    for input in files {
        let relative = input.strip_prefix(root).unwrap_or(&input);
        let backup = backup_dir.join(relative);

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
                ProjectTypeConfig::Dspre => transpiler::transpile_dspre(&source, db.as_ref()),
                ProjectTypeConfig::Decomp => transpiler::transpile_decomp(&source, db.as_ref())
                    .map(|result| result.source)
                    .map_err(|error| ProjectError::ConvertDecomp {
                        path: input.clone(),
                        line: error.line,
                        message: error.to_string(),
                    })?,
                ProjectTypeConfig::Generic => continue,
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

        if dry_run {
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

    Ok(ConvertReport {
        converted: if dry_run { 0 } else { plans.len() },
        plans,
        backup_dir: Some(backup_dir),
        dry_run,
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
        ProjectTypeConfig::Generic => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{convert_project, find_convertible_files};
    use crate::project::config::{
        GameFamilyConfig, PathsConfig, ProjectMetadata, ProjectTypeConfig, RotomConfig,
        WorkspaceConfig,
    };
    use std::fs;
    use tempfile::tempdir;

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
            database: None,
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
                game_family: Some(GameFamilyConfig::Platinum),
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
        let source_dir = root.join("scripts");
        fs::create_dir_all(&source_dir).unwrap();
        fs::write(
            source_dir.join("test.script"),
            "=== script 1\nMessage 0\nEnd\n",
        )
        .unwrap();

        let config = dspre_config();
        let report = convert_project(root, &config, true).unwrap();

        assert_eq!(report.converted, 0);
        assert_eq!(report.plans.len(), 1);
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
        let source_dir = root.join("scripts/sub");
        fs::create_dir_all(&source_dir).unwrap();
        fs::write(
            source_dir.join("test.script"),
            "=== script 1\nMessage 0\nEnd\n",
        )
        .unwrap();

        let config = dspre_config();
        let report = convert_project(root, &config, false).unwrap();

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
        fs::write(
            source_dir.join("keep.rotom"),
            "script Main #1:\n    End\n",
        )
        .unwrap();
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

        let report = convert_project(root, &decomp_config(), false).unwrap();
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
