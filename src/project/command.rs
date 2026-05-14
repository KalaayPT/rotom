use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use crate::{BatchCompileResult, BatchDecompileResult};

use super::compile::{compile_project, decompile_project};
use super::config::{find_project_root, load_config};
use super::convert::{ConvertReport, convert_project, ConvertOptions};
use super::error::{ProjectError, Result};
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

pub fn convert_mode(
    root: Option<&Path>,
    options: ConvertOptions,
) -> Result<ConvertReport> {
    let root = resolve_project_root(root)?;
    let config = load_config(&root)?;
    convert_project(&root, &config, options)
}

fn resolve_project_root(root: Option<&Path>) -> Result<PathBuf> {
    let start = match root {
        Some(path) => path.canonicalize().map_err(|source| ProjectError::Io {
            action: "Failed to resolve",
            path: path.to_path_buf(),
            source,
        })?,
        None => {
            std::env::current_dir().map_err(|source| ProjectError::CurrentDirectory { source })?
        }
    };

    find_project_root(&start)
        .or(Some(start))
        .filter(|path| path.join("rotom.toml").exists())
        .ok_or(ProjectError::ProjectRootNotFound)
}
