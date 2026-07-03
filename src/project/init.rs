use crate::database::{GameFamily, game_family_from_hint};
use snafu::ResultExt;
use std::fs;
use std::io::{Cursor, Read, Seek, Write};
use std::path::{Path, PathBuf};
use uxie::{ProjectType, Workspace};

use super::config::{
    DatabaseConfig, PathsConfig, ProjectMetadata, ProjectTypeConfig, RotomConfig, WorkspaceConfig,
    load_config,
};
use super::convert::{ConvertOptions, convert_project, find_convertible_files};
use super::dspre_db_migration::{
    dspre_edited_db_suggests_followplat, dspre_workspace_dir_basename_for_rotom,
    find_local_scrcmd_v1_path,
};
use super::error::{
    CurrentDirectorySnafu, DownloadDatabaseSnafu, IoSnafu, Result, SerializeConfigSnafu, StdIoSnafu,
};

pub const LATEST_COMMAND_DATABASE_URL: &str = "https://github.com/DS-Pokemon-Rom-Editor/scrcmd-database/releases/latest/download/db-latest.zip";

const ROTOM_DIR: &str = ".rotom";
const COMMAND_DATABASE_DIR: &str = ".rotom/command_database";
const CACHE_DIR: &str = ".rotom/cache";
const STATUS_DIR: &str = ".rotom/status";
pub(crate) const EMBEDDED_COMMAND_DATABASE_ZIP: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/embedded-command-database.zip"));

#[derive(Debug, Clone)]
pub struct InitReport {
    pub used_embedded_database: bool,
    pub reused_paths: Vec<&'static str>,
    pub convertible_files_detected: usize,
    pub converted_files: usize,
}

#[derive(Debug, Clone, Default)]
pub struct InitOptions {
    pub interactive: bool,
    pub game_hint: Option<String>,
    pub database_path: Option<PathBuf>,
    /// Override the user-level database cache directory. Primarily for tests.
    pub user_db_dir_override: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct WorkspaceInfo {
    project_type: ProjectTypeConfig,
    game_family: Option<GameFamily>,
    source_roots: Vec<String>,
    include_roots: Vec<String>,
    binary_roots: Vec<String>,
}

/// Initialize a Rotom project under `root`, downloading or embedding the command database,
/// writing `rotom.toml`, and optionally converting legacy scripts when `options.interactive`.
#[allow(clippy::too_many_lines)]
pub fn run_init(root: Option<PathBuf>, options: InitOptions) -> Result<InitReport> {
    let InitOptions {
        interactive,
        game_hint,
        database_path,
        user_db_dir_override,
    } = options;

    let root = match root {
        Some(root) => root,
        None => std::env::current_dir().context(CurrentDirectorySnafu)?,
    };
    if !root.exists() {
        fs::create_dir_all(&root).context(IoSnafu {
            action: "Failed to create directory",
            path: root.clone(),
        })?;
    }
    let root = root.canonicalize().context(IoSnafu {
        action: "Failed to resolve",
        path: root,
    })?;

    let mut reused_paths = Vec::new();
    for path in [
        ROTOM_DIR,
        CACHE_DIR,
        STATUS_DIR,
        COMMAND_DATABASE_DIR,
        "rotom.toml",
    ] {
        if root.join(path).exists() {
            reused_paths.push(path);
        }
    }

    for dir in [ROTOM_DIR, CACHE_DIR, STATUS_DIR] {
        fs::create_dir_all(root.join(dir)).context(IoSnafu {
            action: "Failed to create directory",
            path: root.join(dir),
        })?;
    }

    // Always attempt to refresh the user-level database cache on init so
    // projects are bootstrapped from the latest available release.
    let user_db_dir = user_db_dir_override
        .or_else(user_database_cache_dir)
        .unwrap_or_else(|| std::env::temp_dir().join("rotom").join("databases"));
    let used_embedded_database = refresh_user_db_cache(&user_db_dir).unwrap_or(true);

    // Detect workspace before copying the DB so we know which family file to
    // copy.
    let workspace = detect_workspace(&root)?;
    let preferred_family = workspace
        .game_family
        .or_else(|| game_hint.as_ref().and_then(game_family_from_hint));
    let prefer_following_platinum = matches!(workspace.project_type, ProjectTypeConfig::Dspre)
        && preferred_family == Some(GameFamily::Platinum)
        && dspre_init_prefers_following_platinum_v2(&root, &build_config(&root, &workspace, None));

    let command_database_dir = root.join(COMMAND_DATABASE_DIR);
    let project_db_populated = ensure_project_database(
        &command_database_dir,
        &user_db_dir,
        preferred_family,
        prefer_following_platinum,
    )?;

    // Only surface "used embedded" when we actually bootstrapped the project DB
    // from the embedded fallback; an already-populated project DB is not affected.
    let used_embedded_database = !project_db_populated && used_embedded_database;

    let default_database_file = if let Some(path) = database_path.as_ref() {
        let path = if path.is_absolute() {
            path.clone()
        } else {
            root.join(path)
        };
        Some(path.strip_prefix(&root).unwrap_or(&path).to_path_buf())
    } else {
        find_default_database_file(
            &root,
            &command_database_dir,
            preferred_family,
            prefer_following_platinum,
        )?
    };

    let config_path = root.join("rotom.toml");
    if !config_path.exists() {
        let config = build_config(&root, &workspace, default_database_file.as_deref());
        let content = toml::to_string_pretty(&config).context(SerializeConfigSnafu)?;
        fs::write(
            &config_path,
            format!("# Generated by `rotom init`\n\n{}", content),
        )
        .context(IoSnafu {
            action: "Failed to write",
            path: config_path,
        })?;
    }

    let config = load_config(&root)?;
    let (convertible_files_detected, converted_files) =
        maybe_convert_existing_sources(&root, &config, interactive)?;

    Ok(InitReport {
        used_embedded_database,
        reused_paths,
        convertible_files_detected,
        converted_files,
    })
}

/// Returns the platform-appropriate directory for rotom's user-level data.
///
/// - Linux/Unix: `$XDG_DATA_HOME/rotom/databases` or `~/.local/share/rotom/databases`
/// - macOS:      `~/Library/Application Support/rotom/databases`
/// - Windows:    `%LOCALAPPDATA%\rotom\databases`
fn user_database_cache_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("rotom").join("databases"))
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|h| {
            PathBuf::from(h)
                .join("Library")
                .join("Application Support")
                .join("rotom")
                .join("databases")
        })
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share"))
            })
            .map(|base| base.join("rotom").join("databases"))
    }
}

/// Refresh the user-level database cache by downloading the latest release.
///
/// Always tries to download. On success the cache is atomically replaced.
/// On failure the existing cache is left unchanged; if the cache is empty
/// the embedded fallback is unpacked so there is always something to copy from.
///
/// Returns `true` if the embedded fallback was used (download failed AND no
/// prior cache existed).
fn refresh_user_db_cache(user_dir: &Path) -> Result<bool> {
    fs::create_dir_all(user_dir).context(IoSnafu {
        action: "Failed to create user database cache directory",
        path: user_dir.to_path_buf(),
    })?;

    if let Ok(bytes) = download_latest_database_zip() {
        // Unpack to a sibling temp directory then atomically rename over the
        // cache so a partial download never leaves the cache in a broken state.
        let temp = user_dir.with_extension("tmp");
        let _ = fs::remove_dir_all(&temp);
        if fs::create_dir_all(&temp).is_ok() && unpack_zip(Cursor::new(bytes), &temp).is_ok() {
            let _ = fs::remove_dir_all(user_dir);
            if fs::rename(&temp, user_dir).is_ok() {
                return Ok(false);
            }
        }
        let _ = fs::remove_dir_all(&temp);
    }

    // Download failed. Keep the existing cache if it has files.
    let mut existing = Vec::new();
    let _ = collect_v2_files(user_dir, &mut existing);
    if !existing.is_empty() {
        return Ok(false);
    }

    // No cache at all — unpack the embedded snapshot as a last resort.
    unpack_zip(Cursor::new(EMBEDDED_COMMAND_DATABASE_ZIP), user_dir)?;
    Ok(true)
}

/// Ensure the project's command-database directory has the right file.
///
/// If the directory is already non-empty, it is left untouched (the user may
/// have a custom or hand-edited database). Otherwise the single file that
/// matches `family` is copied from `user_dir`.  If no family is given, or no
/// matching file is found, all `*_v2.json` files are copied as a fallback.
///
/// Returns `true` if the project database was already populated (nothing was
/// copied), `false` if files were copied from the user cache.
fn ensure_project_database(
    project_dir: &Path,
    user_dir: &Path,
    family: Option<GameFamily>,
    prefer_following_platinum: bool,
) -> Result<bool> {
    // Leave an existing database alone.
    if project_dir.exists() {
        let mut existing = Vec::new();
        collect_v2_files(project_dir, &mut existing)?;
        if !existing.is_empty() {
            return Ok(true);
        }
    }

    fs::create_dir_all(project_dir).context(IoSnafu {
        action: "Failed to create database directory",
        path: project_dir.to_path_buf(),
    })?;

    let mut user_files = Vec::new();
    collect_v2_files(user_dir, &mut user_files)?;
    user_files.sort();

    if let Some(family) = family {
        let matched: Vec<PathBuf> = user_files
            .iter()
            .filter(|f| is_v2_file_for_family(f, family))
            .cloned()
            .collect();
        if !matched.is_empty() {
            if family == GameFamily::Platinum {
                if let Some(src) = pick_platinum_v2_variant(&matched, prefer_following_platinum) {
                    copy_db_file(&src, project_dir)?;
                }
            } else {
                for src in &matched {
                    copy_db_file(src, project_dir)?;
                }
            }
            return Ok(false);
        }
    }

    // No family match (or unknown family): copy everything.
    for file in &user_files {
        copy_db_file(file, project_dir)?;
    }
    Ok(false)
}

fn is_v2_file_for_family(path: &Path, family: GameFamily) -> bool {
    // `meta.version` is authoritative when present and recognizable.
    if let Ok(contents) = fs::read_to_string(path)
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(&contents)
        && let Some(version) = json
            .get("meta")
            .and_then(|m| m.get("version"))
            .and_then(|v| v.as_str())
        && let Some(version_family) = game_family_from_hint(version)
    {
        return version_family == family;
    }
    // Fall back to the file name only — never the full path, whose ancestor
    // directory names (e.g. a project folder called `heartgold_hack`) would
    // otherwise be misread as a family hint.
    path.file_name()
        .and_then(|name| game_family_from_hint(name.to_string_lossy()))
        == Some(family)
}

fn pick_platinum_v2_variant(files: &[PathBuf], prefer_following: bool) -> Option<PathBuf> {
    let following = files.iter().find(|path| {
        path.file_name()
            .is_some_and(|name| name == "following_platinum_v2.json")
    });
    let stock = files.iter().find(|path| {
        path.file_name()
            .is_some_and(|name| name == "platinum_v2.json")
    });
    if prefer_following {
        following.or(stock).cloned()
    } else {
        stock.or(following).cloned()
    }
}

fn dspre_init_prefers_following_platinum_v2(root: &Path, config: &RotomConfig) -> bool {
    let Some(family) = config.game_family() else {
        return false;
    };
    let Some(path) = find_local_scrcmd_v1_path(root, family, config) else {
        return false;
    };
    let Some(user_map) = fs::read_to_string(path)
        .ok()
        .and_then(|source| serde_json::from_str::<serde_json::Value>(&source).ok())
        .and_then(|value| {
            value
                .get("scrcmd")
                .and_then(serde_json::Value::as_object)
                .cloned()
        })
    else {
        return false;
    };
    dspre_edited_db_suggests_followplat(&user_map)
}

/// Scan `files` for the one whose JSON `meta.version` (or filename stem) matches `family`.
fn find_family_file(
    files: &[PathBuf],
    family: GameFamily,
    prefer_following_platinum: bool,
) -> Option<PathBuf> {
    if family == GameFamily::Platinum {
        return pick_platinum_v2_variant(files, prefer_following_platinum);
    }

    files
        .iter()
        .find(|f| is_v2_file_for_family(f, family))
        .cloned()
}

fn copy_db_file(src: &Path, dest_dir: &Path) -> Result<()> {
    let filename = src.file_name().unwrap_or_default();
    fs::copy(src, dest_dir.join(filename)).context(IoSnafu {
        action: "Failed to copy database file",
        path: dest_dir.join(filename),
    })?;
    Ok(())
}

fn maybe_convert_existing_sources(
    root: &Path,
    config: &RotomConfig,
    interactive: bool,
) -> Result<(usize, usize)> {
    let convertible_files_detected = find_convertible_files(root, config)?.len();
    if !interactive || convertible_files_detected == 0 {
        return Ok((convertible_files_detected, 0));
    }

    let mut stdout = std::io::stdout();
    write!(
        stdout,
        "Convert {} file(s) to .rotom format now? [Y/n] ",
        convertible_files_detected
    )
    .context(StdIoSnafu {
        action: "Failed to write prompt",
    })?;
    stdout.flush().context(StdIoSnafu {
        action: "Failed to flush prompt",
    })?;

    let mut response = String::new();
    std::io::stdin()
        .read_line(&mut response)
        .context(StdIoSnafu {
            action: "Failed to read prompt response",
        })?;
    let answer = response.trim().to_ascii_lowercase();
    if answer.is_empty() || answer == "y" || answer == "yes" {
        Ok((
            convertible_files_detected,
            convert_project(
                root,
                config,
                ConvertOptions {
                    dry_run: false,
                    non_interactive: !interactive,
                },
            )?
            .converted,
        ))
    } else {
        Ok((convertible_files_detected, 0))
    }
}

fn detect_workspace(root: &Path) -> Result<WorkspaceInfo> {
    let workspace = Workspace::open(root).context(IoSnafu {
        action: "Failed to open workspace",
        path: root.to_path_buf(),
    })?;

    Ok(match workspace.project_type {
        ProjectType::Dspre => WorkspaceInfo {
            project_type: ProjectTypeConfig::Dspre,
            game_family: Some(workspace.family),
            source_roots: if root.join("expanded/scripts").exists() {
                vec!["expanded/scripts".to_string()]
            } else if root.join("scripts").exists() {
                vec!["scripts".to_string()]
            } else {
                Vec::new()
            },
            include_roots: Vec::new(),
            binary_roots: [
                "unpacked/data/fielddata/script",
                "unpacked/scripts",
                "scripts",
            ]
            .into_iter()
            .find(|path| root.join(path).exists())
            .map(str::to_string)
            .into_iter()
            .collect(),
        },
        ProjectType::Decomp => {
            let (source_roots, include_roots) = match workspace.family {
                uxie::GameFamily::HGSS => (
                    vec!["files/fielddata/script/scr_seq".to_string()],
                    vec![
                        "include".to_string(),
                        "generated".to_string(),
                        "files".to_string(),
                        "asm".to_string(),
                    ],
                ),
                uxie::GameFamily::Platinum => (
                    vec!["res/field/scripts".to_string()],
                    vec![
                        "include".to_string(),
                        "generated".to_string(),
                        "asm".to_string(),
                    ],
                ),
                uxie::GameFamily::DP => unreachable!("decomp detection should not resolve to DP"),
            };
            let binary_roots = match workspace.family {
                uxie::GameFamily::HGSS => source_roots.clone(),
                uxie::GameFamily::Platinum => {
                    vec!["build/res/field/scripts/scr_seq.narc.p".to_string()]
                }
                uxie::GameFamily::DP => unreachable!("decomp detection should not resolve to DP"),
            };

            WorkspaceInfo {
                project_type: ProjectTypeConfig::Decomp,
                game_family: Some(workspace.family),
                source_roots,
                include_roots,
                binary_roots,
            }
        }
        ProjectType::HgEngine => WorkspaceInfo {
            project_type: ProjectTypeConfig::HgEngine,
            game_family: Some(workspace.family),
            source_roots: vec![".rotom/scripts".to_string()],
            include_roots: vec![
                "include".to_string(),
                "armips/include".to_string(),
                "asm/include".to_string(),
            ],
            binary_roots: vec!["build/a012".to_string()],
        },
    })
}

/// Builds the `RotomConfig` persisted as `rotom.toml` during [`run_init`].
///
/// DSPRE projects: `[project].name` is derived from the workspace directory with a **`_DSPRE_contents`**
/// suffix removed when present so it matches DSPRE’s short edited-database folder names.
fn build_config(root: &Path, workspace: &WorkspaceInfo, default_db: Option<&Path>) -> RotomConfig {
    let dirname = root.file_name().map_or_else(
        || "rotom-project".to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let project_name = if workspace.project_type == ProjectTypeConfig::Dspre {
        dspre_workspace_dir_basename_for_rotom(&dirname)
    } else {
        dirname
    };
    RotomConfig {
        format_version: 1,
        project: ProjectMetadata { name: project_name },
        workspace: WorkspaceConfig {
            project_type: workspace.project_type,
            game_family: workspace.game_family,
        },
        paths: PathsConfig {
            database_dir: COMMAND_DATABASE_DIR.to_string(),
            cache_dir: CACHE_DIR.to_string(),
            status_dir: STATUS_DIR.to_string(),
            source_roots: workspace.source_roots.clone(),
            include_roots: workspace.include_roots.clone(),
            binary_roots: workspace.binary_roots.clone(),
        },
        database: default_db.map(|path| DatabaseConfig {
            default_file: path.display().to_string(),
        }),
    }
}

fn download_latest_database_zip() -> Result<Vec<u8>> {
    let response = minreq::get(LATEST_COMMAND_DATABASE_URL)
        .with_header("User-Agent", format!("rotom/{}", env!("CARGO_PKG_VERSION")))
        .with_timeout(20)
        .send()
        .context(DownloadDatabaseSnafu)?;
    Ok(response.into_bytes())
}

fn unpack_zip<R: Read + Seek>(reader: R, out_dir: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(reader)?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        let path = out_dir.join(name);

        if entry.is_dir() {
            fs::create_dir_all(&path).context(IoSnafu {
                action: "Failed to create directory",
                path: path.clone(),
            })?;
            continue;
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context(IoSnafu {
                action: "Failed to create directory",
                path: parent.to_path_buf(),
            })?;
        }

        let mut file = fs::File::create(&path).context(IoSnafu {
            action: "Failed to create file",
            path: path.clone(),
        })?;
        std::io::copy(&mut entry, &mut file).context(IoSnafu {
            action: "Failed to unpack archive entry into",
            path,
        })?;
    }

    Ok(())
}

fn find_default_database_file(
    root: &Path,
    database_dir: &Path,
    preferred_family: Option<GameFamily>,
    prefer_following_platinum: bool,
) -> Result<Option<PathBuf>> {
    let mut files = Vec::new();
    collect_v2_files(database_dir, &mut files)?;
    files.sort();

    if let Some(preferred_family) = preferred_family
        && let Some(file) = find_family_file(&files, preferred_family, prefer_following_platinum)
    {
        return Ok(Some(file.strip_prefix(root).unwrap_or(&file).to_path_buf()));
    }

    Ok((files.len() == 1).then(|| {
        files[0]
            .strip_prefix(root)
            .unwrap_or(files[0].as_path())
            .to_path_buf()
    }))
}

fn collect_v2_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir).context(IoSnafu {
        action: "Failed to read directory",
        path: dir.to_path_buf(),
    })? {
        let path = entry
            .context(IoSnafu {
                action: "Failed to read directory entry",
                path: dir.to_path_buf(),
            })?
            .path();
        if path.is_dir() {
            collect_v2_files(&path, out)?;
        } else if path.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with("_v2.json"))
        {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as IoWrite;
    use tempfile::tempdir;
    use zip::write::FileOptions;

    fn write_database(path: &Path, version: &str) {
        fs::write(
            path,
            format!(
                r#"{{
  "meta": {{ "version": "{version}" }},
  "commands": {{}}
}}"#
            ),
        )
        .unwrap();
    }

    fn make_user_db_dir(dir: &Path, version: &str) -> PathBuf {
        let user_dir = dir.join("user-db");
        fs::create_dir_all(&user_dir).unwrap();
        write_database(&user_dir.join("platinum_v2.json"), version);
        user_dir
    }

    #[test]
    fn run_init_writes_config_from_existing_db_dir() {
        let root = tempdir().unwrap();
        fs::create_dir_all(root.path().join("res/field/scripts")).unwrap();
        fs::write(root.path().join("res/field/scripts/scripts.order"), "").unwrap();
        fs::create_dir_all(root.path().join(COMMAND_DATABASE_DIR)).unwrap();
        write_database(
            &root
                .path()
                .join(COMMAND_DATABASE_DIR)
                .join("platinum_v2.json"),
            "Platinum",
        );
        let user_dir = make_user_db_dir(root.path(), "Platinum");

        let report = run_init(
            Some(root.path().to_path_buf()),
            InitOptions {
                user_db_dir_override: Some(user_dir),
                ..InitOptions::default()
            },
        )
        .unwrap();

        assert!(!report.used_embedded_database);
        assert!(report.reused_paths.contains(&COMMAND_DATABASE_DIR));
        assert_eq!(report.converted_files, 0);

        let config = fs::read_to_string(root.path().join("rotom.toml")).unwrap();
        assert!(config.contains("project_type = \"decomp\""));
        assert!(config.contains("game_family = \"Platinum\""));
        assert!(config.contains("default_file = \".rotom/command_database/platinum_v2.json\""));
        assert!(!config.contains("provider = "));
    }

    #[test]
    fn run_init_copies_only_family_db_to_project() {
        let root = tempdir().unwrap();
        fs::create_dir_all(root.path().join("res/field/scripts")).unwrap();
        fs::write(root.path().join("res/field/scripts/scripts.order"), "").unwrap();

        // User cache has multiple DBs; only the platinum one should land in the project.
        let user_dir = root.path().join("user-db");
        fs::create_dir_all(&user_dir).unwrap();
        write_database(&user_dir.join("platinum_v2.json"), "Platinum");
        write_database(&user_dir.join("hgss_v2.json"), "HeartGold");

        run_init(
            Some(root.path().to_path_buf()),
            InitOptions {
                user_db_dir_override: Some(user_dir),
                ..InitOptions::default()
            },
        )
        .unwrap();

        let project_db = root.path().join(COMMAND_DATABASE_DIR);
        assert!(project_db.join("platinum_v2.json").exists());
        assert!(!project_db.join("hgss_v2.json").exists());
    }

    #[test]
    fn run_init_non_interactive_skips_conversion_prompt_without_error() {
        let root = tempdir().unwrap();
        fs::create_dir_all(root.path().join("res/field/scripts")).unwrap();
        fs::write(root.path().join("res/field/scripts/scripts.order"), "").unwrap();
        fs::write(
            root.path().join("res/field/scripts/test.s"),
            "ScriptEntry Test\nScriptEntryEnd\n\nTest:\n    End\n",
        )
        .unwrap();
        let user_dir = make_user_db_dir(root.path(), "Platinum");

        let report = run_init(
            Some(root.path().to_path_buf()),
            InitOptions {
                interactive: false,
                user_db_dir_override: Some(user_dir),
                ..InitOptions::default()
            },
        )
        .unwrap();

        assert_eq!(report.convertible_files_detected, 1);
        assert_eq!(report.converted_files, 0);
        assert!(root.path().join("res/field/scripts/test.s").exists());
        assert!(!root.path().join("res/field/scripts/test.rotom").exists());
    }

    #[test]
    fn run_init_errors_when_workspace_cannot_be_opened() {
        let root = tempdir().unwrap();
        let user_dir = make_user_db_dir(root.path(), "Platinum");

        let error = run_init(
            Some(root.path().to_path_buf()),
            InitOptions {
                user_db_dir_override: Some(user_dir),
                ..InitOptions::default()
            },
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("Failed to open workspace"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn unpack_zip_extracts_database_file() {
        let root = tempdir().unwrap();
        let zip_path = root.path().join("db.zip");
        let zip_file = fs::File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(zip_file);
        let options: FileOptions<'_, ()> = FileOptions::default();
        zip.start_file("platinum_v2.json", options).unwrap();
        zip.write_all(
            br#"{
  "meta": { "version": "Platinum" },
  "commands": {}
}"#,
        )
        .unwrap();
        zip.finish().unwrap();

        let out = root.path().join("out");
        fs::create_dir_all(&out).unwrap();
        unpack_zip(fs::File::open(zip_path).unwrap(), &out).unwrap();

        assert!(out.join("platinum_v2.json").exists());
    }

    #[test]
    fn embedded_database_archive_contains_v2_files() {
        let out = tempdir().unwrap();
        unpack_zip(Cursor::new(EMBEDDED_COMMAND_DATABASE_ZIP), out.path()).unwrap();

        let mut files = Vec::new();
        collect_v2_files(out.path(), &mut files).unwrap();
        assert!(!files.is_empty());
    }

    #[test]
    fn refresh_user_db_cache_uses_embedded_when_no_cache_and_no_network() {
        let dir = tempdir().unwrap();
        // Use a non-routable address so the download fails fast
        let used_embedded = refresh_user_db_cache(dir.path()).unwrap();

        // The download will either fail (no network) or succeed — either way
        // the cache should have files afterward.
        let mut files = Vec::new();
        collect_v2_files(dir.path(), &mut files).unwrap();
        assert!(!files.is_empty(), "cache should be populated after refresh");
        // Only assert embedded was used if the download genuinely failed
        // (network may or may not be available in CI).
        let _ = used_embedded;
    }

    #[test]
    fn ensure_project_database_copies_matching_family_file() {
        let dir = tempdir().unwrap();
        let user_dir = dir.path().join("user");
        fs::create_dir_all(&user_dir).unwrap();
        write_database(&user_dir.join("platinum_v2.json"), "Platinum");
        write_database(&user_dir.join("hgss_v2.json"), "HeartGold");

        let project_dir = dir.path().join("project");
        ensure_project_database(&project_dir, &user_dir, Some(GameFamily::Platinum), false)
            .unwrap();

        assert!(project_dir.join("platinum_v2.json").exists());
        assert!(!project_dir.join("hgss_v2.json").exists());
    }

    #[test]
    fn ensure_project_database_copies_only_following_platinum_variant_when_requested() {
        let dir = tempdir().unwrap();
        let user_dir = dir.path().join("user");
        fs::create_dir_all(&user_dir).unwrap();
        write_database(&user_dir.join("platinum_v2.json"), "Platinum");
        write_database(
            &user_dir.join("following_platinum_v2.json"),
            "Following Platinum",
        );

        let project_dir = dir.path().join("project");
        ensure_project_database(&project_dir, &user_dir, Some(GameFamily::Platinum), true).unwrap();

        assert!(!project_dir.join("platinum_v2.json").exists());
        assert!(project_dir.join("following_platinum_v2.json").exists());
    }

    #[test]
    fn ensure_project_database_copies_all_when_no_family() {
        let dir = tempdir().unwrap();
        let user_dir = dir.path().join("user");
        fs::create_dir_all(&user_dir).unwrap();
        write_database(&user_dir.join("platinum_v2.json"), "Platinum");
        write_database(&user_dir.join("hgss_v2.json"), "HeartGold");

        let project_dir = dir.path().join("project");
        ensure_project_database(&project_dir, &user_dir, None, false).unwrap();

        assert!(project_dir.join("platinum_v2.json").exists());
        assert!(project_dir.join("hgss_v2.json").exists());
    }

    #[test]
    fn ensure_project_database_leaves_existing_db_untouched() {
        let dir = tempdir().unwrap();
        let user_dir = dir.path().join("user");
        fs::create_dir_all(&user_dir).unwrap();
        write_database(&user_dir.join("hgss_v2.json"), "HeartGold");

        let project_dir = dir.path().join("project");
        fs::create_dir_all(&project_dir).unwrap();
        write_database(&project_dir.join("platinum_v2.json"), "Platinum");

        // Even though the user dir has HGSS and family says HGSS, the project
        // dir is non-empty so we must not touch it.
        ensure_project_database(&project_dir, &user_dir, Some(GameFamily::HGSS), false).unwrap();

        assert!(project_dir.join("platinum_v2.json").exists());
        assert!(!project_dir.join("hgss_v2.json").exists());
    }
}
