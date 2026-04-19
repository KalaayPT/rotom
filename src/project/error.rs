use crate::{CompileError, DecompileError};
use std::path::PathBuf;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, ProjectError>;

#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("{action} '{}': {source}", path.display())]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{action}: {source}")]
    StdIo {
        action: &'static str,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to determine current directory: {source}")]
    CurrentDirectory {
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to parse '{}': {source}", path.display())]
    ParseConfig {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("Failed to serialize rotom.toml: {source}")]
    SerializeConfig {
        #[source]
        source: toml::ser::Error,
    },

    #[error("rotom.toml is missing [database].default_file")]
    MissingDefaultDatabase,

    #[error("rotom.toml does not define any source_roots")]
    MissingSourceRoots,

    #[error("rotom.toml does not define any binary_roots")]
    MissingBinaryRoots,

    #[error("rotom.toml is missing [workspace].game_family for decomp project mode")]
    MissingGameFamily,

    #[error("No supported project source files were found")]
    NoProjectSourceFiles,

    #[error("No project binary files were found")]
    NoProjectBinaryFiles,

    #[error("Project compile output collision detected: {details}")]
    OutputCollision { details: String },

    #[error(
        "Tracked project path '{}' is outside project root '{}'",
        path.display(),
        root.display()
    )]
    PathOutsideProject { root: PathBuf, path: PathBuf },

    #[error("Failed to convert '{path}': line {line}: {message}")]
    ConvertDecomp {
        path: PathBuf,
        line: usize,
        message: String,
    },

    #[error("Could not find rotom.toml in the current directory or any parent directory")]
    ProjectRootNotFound,

    #[error("--output and --decomp-root are not supported in project compile mode")]
    UnsupportedProjectCompileArgs,

    #[error("--output is not supported in project decompile mode")]
    UnsupportedProjectDecompileArgs,

    #[error("--database and --input must both be provided outside project mode")]
    MissingCompileArgs,

    #[error("--database and --input must both be provided outside project mode")]
    MissingDecompileArgs,

    #[error("{0} file(s) failed to compile")]
    CompileFailures(usize),

    #[error("{0} file(s) failed to decompile")]
    DecompileFailures(usize),

    #[error("Failed to download latest command database archive: {source}")]
    DownloadDatabase {
        #[source]
        source: minreq::Error,
    },

    #[error("Failed to unpack command database archive: {source}")]
    Zip {
        #[from]
        source: zip::result::ZipError,
    },

    #[error("Project compile failed: {0}")]
    Compile(#[from] CompileError),

    #[error("Project decompile failed: {0}")]
    Decompile(#[from] DecompileError),
}
