use crate::{CompileError, DecompileError};
use snafu::Snafu;
use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, ProjectError>;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum ProjectError {
    #[snafu(display("{action} '{}': {source}", path.display()))]
    Io {
        action: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("{action}: {source}"))]
    StdIo {
        action: &'static str,
        source: std::io::Error,
    },

    #[snafu(display("Failed to determine current directory: {source}"))]
    CurrentDirectory { source: std::io::Error },

    #[snafu(display("Failed to parse '{}': {source}", path.display()))]
    ParseConfig {
        path: PathBuf,
        source: toml::de::Error,
    },

    #[snafu(display("Failed to serialize rotom.toml: {source}"))]
    SerializeConfig { source: toml::ser::Error },

    #[snafu(display("Failed to serialize JSON: {source}"))]
    SerializeJson { source: serde_json::Error },

    #[snafu(display("rotom.toml is missing [database].default_file"))]
    MissingDefaultDatabase,

    #[snafu(display("rotom.toml does not define any source_roots"))]
    MissingSourceRoots,

    #[snafu(display("rotom.toml does not define any binary_roots"))]
    MissingBinaryRoots,

    #[snafu(display("rotom.toml is missing [workspace].game_family for decomp project mode"))]
    MissingGameFamily,

    #[snafu(display("No supported project source files were found"))]
    NoProjectSourceFiles,

    #[snafu(display("No project binary files were found"))]
    NoProjectBinaryFiles,

    #[snafu(display("Project compile output collision detected: {details}"))]
    OutputCollision { details: String },

    #[snafu(display(
        "Tracked project path '{}' is outside project root '{}'",
        path.display(),
        root.display()
    ))]
    PathOutsideProject { root: PathBuf, path: PathBuf },

    #[snafu(display("Failed to convert '{}': line {line}: {message}", path.display()))]
    ConvertDecomp {
        path: PathBuf,
        line: usize,
        message: String,
    },

    #[snafu(display("Could not find rotom.toml in the current directory or any parent directory"))]
    ProjectRootNotFound,

    #[snafu(display("--output and --decomp-root are not supported in project compile mode"))]
    UnsupportedProjectCompileArgs,

    #[snafu(display("--output is not supported in project decompile mode"))]
    UnsupportedProjectDecompileArgs,

    #[snafu(display("--database and --input must both be provided outside project mode"))]
    MissingCompileArgs,

    #[snafu(display("--database and --input must both be provided outside project mode"))]
    MissingDecompileArgs,

    #[snafu(display("{count} file(s) failed to compile"))]
    CompileFailures { count: usize },

    #[snafu(display("{count} file(s) failed to decompile"))]
    DecompileFailures { count: usize },

    #[snafu(display("Failed to download latest command database archive: {source}"))]
    DownloadDatabase { source: minreq::Error },

    #[snafu(display("scrcmd-database baseline: {message}"))]
    ScrcmdBaseline { message: String },

    #[snafu(display("Failed to unpack command database archive: {source}"))]
    Zip { source: zip::result::ZipError },

    #[snafu(display("Project compile failed: {source}"))]
    Compile { source: CompileError },

    #[snafu(display("Project decompile failed: {source}"))]
    Decompile { source: DecompileError },

    #[snafu(display(
        "DSPRE convert expected paired script binary '{}' for '{}', but the file is missing",
        binary.display(),
        script.display(),
    ))]
    DspreConvertMissingBinary { script: PathBuf, binary: PathBuf },

    /// Decompilation failed after resolving the DSPRE script ↔ binary pairing.
    #[snafu(display(
        "{source}\nDSPRE paths: '{}' (plaintext) ← '{}' (paired binary)",
        script.display(),
        binary.display(),
    ))]
    DspreDecompile {
        script: PathBuf,
        binary: PathBuf,
        source: DecompileError,
    },

    #[snafu(display(
        "DSPRE script '{}' is not under any configured paths.source_roots entry",
        script.display()
    ))]
    DspreScriptOutsideSourceRoots { script: PathBuf },
}

impl From<zip::result::ZipError> for ProjectError {
    fn from(source: zip::result::ZipError) -> Self {
        Self::Zip { source }
    }
}

impl From<CompileError> for ProjectError {
    fn from(source: CompileError) -> Self {
        Self::Compile { source }
    }
}

impl From<DecompileError> for ProjectError {
    fn from(source: DecompileError) -> Self {
        Self::Decompile { source }
    }
}
