use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use crate::{BatchCompileResult, BatchDecompileResult};
use snafu::ResultExt;

use super::compile::{compile_project, decompile_project};
use super::config::{find_project_root, load_config};
use super::convert::{ConvertOptions, ConvertReport, convert_project};
use super::error::{CurrentDirectorySnafu, IoSnafu, ProjectError, Result};
use super::init::{InitOptions, InitReport, run_init};

pub fn compile_mode(force: bool) -> Result<BatchCompileResult> {
    let root = resolve_project_root(None)?;
    let config = load_config(&root)?;
    compile_project(&root, &config, force)
}

pub fn decompile_mode() -> Result<BatchDecompileResult> {
    let root = resolve_project_root(None)?;
    let config = load_config(&root)?;
    decompile_project(&root, &config)
}

pub fn init_mode(root: Option<&Path>, non_interactive: bool) -> Result<InitReport> {
    run_init(
        root.map(Path::to_path_buf),
        InitOptions {
            interactive: !non_interactive && std::io::stdin().is_terminal(),
            ..InitOptions::default()
        },
    )
}

pub fn convert_mode(root: Option<&Path>, options: ConvertOptions) -> Result<ConvertReport> {
    let root = resolve_project_root(root)?;
    let config = load_config(&root)?;
    convert_project(&root, &config, options)
}

fn resolve_project_root(root: Option<&Path>) -> Result<PathBuf> {
    let start = match root {
        Some(path) => path.canonicalize().context(IoSnafu {
            action: "Failed to resolve",
            path: path.to_path_buf(),
        })?,
        None => std::env::current_dir().context(CurrentDirectorySnafu)?,
    };

    find_project_root(&start)
        .or(Some(start))
        .filter(|path| path.join("rotom.toml").exists())
        .ok_or(ProjectError::ProjectRootNotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_project_root_finds_root_from_walking_upwards() {
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        std::fs::write(temp.path().join("rotom.toml"), "[workspace]\n")
            .expect("failed to write config");
        let child = temp.path().join("src/scripts");
        std::fs::create_dir_all(&child).expect("failed to create child dir");

        let resolved = resolve_project_root(Some(&child)).expect("project root should resolve");

        assert_eq!(resolved, temp.path().canonicalize().unwrap());
    }

    #[test]
    fn resolve_project_root_rejects_directory_without_config() {
        let temp = tempfile::tempdir().expect("failed to create temp dir");

        let err = resolve_project_root(Some(temp.path())).unwrap_err();

        assert!(matches!(err, ProjectError::ProjectRootNotFound));
    }
}
