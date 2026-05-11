use crate::database::GameFamily;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::error::{ProjectError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RotomConfig {
    pub format_version: u32,
    pub project: ProjectMetadata,
    pub workspace: WorkspaceConfig,
    pub paths: PathsConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<DatabaseConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMetadata {
    pub name: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProjectTypeConfig {
    Generic,
    Dspre,
    Decomp,
    #[serde(rename = "hge")]
    HgEngine,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceConfig {
    pub project_type: ProjectTypeConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_family: Option<GameFamily>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PathsConfig {
    pub database_dir: String,
    pub cache_dir: String,
    pub status_dir: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_roots: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include_roots: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub binary_roots: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DatabaseConfig {
    pub default_file: String,
}

impl RotomConfig {
    pub fn database_file(&self, root: &Path) -> Option<PathBuf> {
        self.database
            .as_ref()
            .map(|database| resolve_config_path(root, &database.default_file))
    }

    pub fn database_dir(&self, root: &Path) -> PathBuf {
        resolve_config_path(root, &self.paths.database_dir)
    }

    pub fn cache_dir(&self, root: &Path) -> PathBuf {
        resolve_config_path(root, &self.paths.cache_dir)
    }

    pub fn status_dir(&self, root: &Path) -> PathBuf {
        resolve_config_path(root, &self.paths.status_dir)
    }

    pub fn source_roots(&self, root: &Path) -> Vec<PathBuf> {
        self.paths
            .source_roots
            .iter()
            .map(|path| resolve_config_path(root, path))
            .collect()
    }

    pub fn include_roots(&self, root: &Path) -> Vec<PathBuf> {
        self.paths
            .include_roots
            .iter()
            .map(|path| resolve_config_path(root, path))
            .collect()
    }

    pub fn binary_roots(&self, root: &Path) -> Vec<PathBuf> {
        self.paths
            .binary_roots
            .iter()
            .map(|path| resolve_config_path(root, path))
            .collect()
    }

    pub fn game_family(&self) -> Option<GameFamily> {
        self.workspace.game_family
    }

    pub fn global_include_path(&self) -> Option<&'static str> {
        self.game_family()
            .and_then(|family| global_include_path(family, self.workspace.project_type))
    }
}

pub fn load_config(root: &Path) -> Result<RotomConfig> {
    let config_path = root.join("rotom.toml");
    let content = std::fs::read_to_string(&config_path).map_err(|source| ProjectError::Io {
        action: "Failed to read",
        path: config_path.clone(),
        source,
    })?;
    toml::from_str(&content).map_err(|source| ProjectError::ParseConfig {
        path: config_path,
        source,
    })
}

pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let start_dir = if start.is_file() {
        start.parent()?
    } else {
        start
    };

    for ancestor in start_dir.ancestors() {
        if ancestor.join("rotom.toml").exists() {
            return Some(ancestor.to_path_buf());
        }
    }

    None
}

pub fn global_include_path(
    family: GameFamily,
    project_type: ProjectTypeConfig,
) -> Option<&'static str> {
    match (family, project_type) {
        (GameFamily::Platinum, ProjectTypeConfig::Decomp) => Some("macros/scrcmd.inc"),
        (GameFamily::HGSS, ProjectTypeConfig::Decomp) => Some("macros/script.inc"),
        _ => None,
    }
}

fn resolve_config_path(root: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DatabaseConfig, PathsConfig, ProjectMetadata, ProjectTypeConfig, RotomConfig,
        WorkspaceConfig, find_project_root, global_include_path,
    };
    use crate::database::GameFamily;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn find_project_root_walks_upward() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("project");
        let nested = root.join("res/field/scripts");
        fs::create_dir_all(&nested).unwrap();
        fs::write(root.join("rotom.toml"), "format_version = 1").unwrap();

        assert_eq!(find_project_root(&nested), Some(root));
    }

    #[test]
    fn global_include_path_matches_decomp_family_contract() {
        assert_eq!(
            global_include_path(GameFamily::Platinum, ProjectTypeConfig::Decomp),
            Some("macros/scrcmd.inc")
        );
        assert_eq!(
            global_include_path(GameFamily::HGSS, ProjectTypeConfig::Decomp),
            Some("macros/script.inc")
        );
        assert_eq!(
            global_include_path(GameFamily::Platinum, ProjectTypeConfig::Dspre),
            None
        );
    }

    #[test]
    fn load_config_round_trips_lowercase_project_metadata() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("rotom.toml");
        fs::write(
            &config_path,
            r#"
format_version = 1

[project]
name = "example"

[workspace]
project_type = "decomp"
game_family = "Platinum"

[paths]
database_dir = ".rotom/command_database"
cache_dir = ".rotom/cache"
status_dir = ".rotom/status"
source_roots = ["res/field/scripts"]
binary_roots = ["res/field/scripts"]

[database]
default_file = ".rotom/command_database/platinum_v2.json"
"#,
        )
        .unwrap();

        let config: RotomConfig = super::load_config(dir.path()).unwrap();
        assert_eq!(config.workspace.project_type, ProjectTypeConfig::Decomp);
        assert_eq!(config.workspace.game_family, Some(GameFamily::Platinum));
        assert_eq!(config.project.name, "example");
    }

    #[test]
    fn config_accessors_resolve_paths_and_game_family() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let config = RotomConfig {
            format_version: 1,
            project: ProjectMetadata {
                name: "example".to_string(),
            },
            workspace: WorkspaceConfig {
                project_type: ProjectTypeConfig::Decomp,
                game_family: Some(GameFamily::HGSS),
            },
            paths: PathsConfig {
                database_dir: ".rotom/command_database".to_string(),
                cache_dir: ".rotom/cache".to_string(),
                status_dir: ".rotom/status".to_string(),
                source_roots: vec!["res/field/scripts".to_string()],
                include_roots: vec!["include".to_string(), "generated".to_string()],
                binary_roots: vec!["build/scripts".to_string()],
            },
            database: Some(DatabaseConfig {
                default_file: ".rotom/command_database/hgss_v2.json".to_string(),
            }),
        };

        assert_eq!(
            config.database_file(root),
            Some(root.join(".rotom/command_database/hgss_v2.json"))
        );
        assert_eq!(
            config.database_dir(root),
            root.join(".rotom/command_database")
        );
        assert_eq!(config.cache_dir(root), root.join(".rotom/cache"));
        assert_eq!(config.status_dir(root), root.join(".rotom/status"));
        assert_eq!(
            config.source_roots(root),
            vec![root.join("res/field/scripts")]
        );
        assert_eq!(
            config.include_roots(root),
            vec![root.join("include"), root.join("generated")]
        );
        assert_eq!(config.binary_roots(root), vec![root.join("build/scripts")]);
        assert_eq!(config.game_family(), Some(GameFamily::HGSS));
        assert_eq!(config.global_include_path(), Some("macros/script.inc"));
    }
}
