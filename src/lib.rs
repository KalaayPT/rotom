//! Rotom - A Pokemon Gen 4 script compiler/decompiler
//!
//! This library provides functionality to compile Rotoscript source files
//! to binary format and decompile binary scripts back to Rotoscript.

#![allow(
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap
)]

mod autovar;
pub mod compile_state;
pub mod compiler;
pub mod database;
pub mod decompiler;
pub mod project;
pub mod transpiler;

pub use project::{
    compile as project_compile, config as project_config, convert as project_convert,
    error as project_error, init as project_init,
};

pub use compiler::codegen::Emitter;
pub use compiler::{
    Analyzer, Lexer, Lowerer, Parser,
    parse_error::{CompileError, print_error},
};
pub use database::{ConstantDb, DatabaseV2, GameFamily, GameFamilyExt, game_family_from_hint};
pub use decompiler::{
    DecompileError, DecompileResult, Disassembler, LevelScript, LevelScriptHeaderEntry,
    LevelScriptVarConditionEntry, ScriptOutput, ScriptType, disassemble_bytes, ir_to_source,
};

use rayon::prelude::{IntoParallelRefIterator, ParallelIterator};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
pub struct CompileResult {
    pub input: PathBuf,
    pub output: PathBuf,
    pub size: usize,
}

/// A compilation failure with enough context for rich error display
#[derive(Debug, Serialize)]
pub struct CompileFailure {
    pub path: PathBuf,
    pub error: CompileError,
    /// The source code (transpiled if applicable) for codespan-reporting
    /// Skipped in JSON output to avoid bloating machine-readable responses
    #[serde(skip)]
    pub source: String,
}

/// Result of compiling a path (file or directory)
#[derive(Debug, Serialize)]
pub struct BatchCompileResult {
    pub successes: Vec<CompileResult>,
    pub failures: Vec<CompileFailure>,
}

impl BatchCompileResult {
    /// Returns true if all files compiled successfully
    pub fn is_success(&self) -> bool {
        self.failures.is_empty()
    }

    /// Total number of files attempted
    pub fn total(&self) -> usize {
        self.successes.len() + self.failures.len()
    }
}

#[derive(Debug, Serialize)]
pub struct DecompileFileResult {
    pub input: PathBuf,
    pub output: PathBuf,
    pub size: usize,
    pub quirks: Vec<crate::compile_state::BinaryQuirk>,
}

#[derive(Debug, Serialize)]
pub struct DecompileFailure {
    pub path: PathBuf,
    pub error: DecompileError,
}

#[derive(Debug, Serialize)]
pub struct BatchDecompileResult {
    pub successes: Vec<DecompileFileResult>,
    pub failures: Vec<DecompileFailure>,
}

impl BatchDecompileResult {
    pub fn is_success(&self) -> bool {
        self.failures.is_empty()
    }

    pub fn total(&self) -> usize {
        self.successes.len() + self.failures.len()
    }
}

pub fn compile_to_bytes(
    source: &str,
    db: &DatabaseV2,
    constants: &ConstantDb,
) -> Result<Vec<u8>, CompileError> {
    compile_to_bytes_with_options(source, db, constants, 1)
}

pub fn compile_to_bytes_with_options(
    source: &str,
    db: &DatabaseV2,
    constants: &ConstantDb,
    jump_table_end_marker_count: u8,
) -> Result<Vec<u8>, CompileError> {
    let cleaned_source = compiler::preprocessor::preprocess(source).cleaned_source;
    let lexer = Lexer::new(&cleaned_source);
    let mut parser = Parser::new(lexer);
    let file = parser.parse_script_file()?;

    let mut analyzer = Analyzer::with_database(constants, db);
    analyzer.analyze(&file)?;

    let mut lowerer = Lowerer::with_constants(&analyzer.symbols, db, constants);
    let items = lowerer.lower_script_file(&file)?;

    let mut emitter = Emitter::new(db);
    emitter.emit_script_file(&items, jump_table_end_marker_count)
}

pub fn compile_levelscript_to_bytes(
    source: &str,
    constants: &ConstantDb,
) -> Result<Vec<u8>, CompileError> {
    let result = transpiler::transpile_levelscript(source, Some(constants)).map_err(|e| {
        CompileError::Transpile {
            message: format!("Levelscript transpile error: {}", e),
        }
    })?;

    let mut bytes = result.levelscript.to_bytes();

    for _ in 0..result.extra_padding {
        bytes.push(0);
    }

    Ok(bytes)
}

pub fn compile_levelscript_json_to_bytes(source: &str) -> Result<Vec<u8>, CompileError> {
    let levelscript = LevelScript::from_json(source).map_err(|e| CompileError::Transpile {
        message: format!("Failed to parse levelscript JSON: {}", e),
    })?;

    Ok(levelscript.to_bytes())
}

/// Returns true when the path follows a known levelscript naming convention.
pub fn is_levelscript_path(path: &Path) -> bool {
    let stem = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("");

    (stem.contains("_init_") && !stem.contains("_init_new_game")) || stem.ends_with("_hdr")
}

pub fn decompile_to_ir(bytes: Vec<u8>, db: &DatabaseV2) -> DecompileResult<ScriptOutput> {
    disassemble_bytes(db, bytes)
}

enum CompileFileError {
    IoError(CompileError),
    CompileError { error: CompileError, source: String },
}

fn compile_file_internal(
    input: &Path,
    output: &Path,
    db: &DatabaseV2,
    constants: &ConstantDb,
    load_file_constants: bool,
) -> Result<CompileResult, CompileFileError> {
    let source = std::fs::read_to_string(input).map_err(|e| {
        CompileFileError::IoError(CompileError::Io {
            message: format!("Failed to read input file '{}': {}", input.display(), e),
        })
    })?;

    let extension = input
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let file_constants = if load_file_constants && (extension == "s" || extension == "rotom") {
        Some(
            constants
                .clone_for_script(input)
                .map_err(CompileFileError::IoError)?,
        )
    } else {
        None
    };
    let constants = file_constants.as_ref().unwrap_or(constants);

    let is_levelscript = is_levelscript_path(input)
        || (extension == "s" && transpiler::is_levelscript_source(&source));

    let bytes = if extension == "json" {
        compile_levelscript_json_to_bytes(&source).map_err(|e| CompileFileError::CompileError {
            error: e,
            source: source.clone(),
        })?
    } else if is_levelscript && extension == "s" {
        compile_levelscript_to_bytes(&source, constants).map_err(|e| {
            CompileFileError::CompileError {
                error: e,
                source: source.clone(),
            }
        })?
    } else {
        let (rotom_source, jump_table_end_marker_count) = match extension.as_str() {
            "rotom" => (source, 1),
            "script" => (transpiler::transpile_dspre(&source, Some(db)), 1),
            "s" => {
                let result = transpiler::transpile_decomp(&source, Some(db)).map_err(|e| {
                    CompileFileError::CompileError {
                        error: CompileError::Transpile {
                            message: format!("Decomp transpile error at line {}: {}", e.line, e),
                        },
                        source: source.clone(),
                    }
                })?;
                (result.source, result.jump_table_end_marker_count)
            }
            _ => {
                return Err(CompileFileError::IoError(CompileError::Io {
                    message: format!("Unsupported file extension: .{}", extension),
                }));
            }
        };

        compile_to_bytes_with_options(&rotom_source, db, constants, jump_table_end_marker_count).map_err(
            |e| CompileFileError::CompileError {
                error: e,
                source: rotom_source.clone(),
            },
        )?
    };
    let size = bytes.len();

    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            CompileFileError::IoError(CompileError::Io {
                message: format!(
                    "Failed to create output directory '{}': {}",
                    parent.display(),
                    e
                ),
            })
        })?;
    }

    std::fs::write(output, &bytes).map_err(|e| {
        CompileFileError::IoError(CompileError::Io {
            message: format!("Failed to write output file '{}': {}", output.display(), e),
        })
    })?;

    Ok(CompileResult {
        input: input.to_path_buf(),
        output: output.to_path_buf(),
        size,
    })
}

pub(crate) fn compile_file_for_batch(
    input: &Path,
    output: &Path,
    db: &DatabaseV2,
    constants: &ConstantDb,
) -> Result<CompileResult, CompileFailure> {
    compile_file_internal(input, output, db, constants, true).map_err(|error| match error {
        CompileFileError::IoError(error) => CompileFailure {
            path: input.to_path_buf(),
            error,
            source: String::new(),
        },
        CompileFileError::CompileError { error, source } => CompileFailure {
            path: input.to_path_buf(),
            error,
            source,
        },
    })
}

pub(crate) fn compile_file_for_batch_preloaded_constants(
    input: &Path,
    output: &Path,
    db: &DatabaseV2,
    constants: &ConstantDb,
) -> Result<CompileResult, CompileFailure> {
    compile_file_internal(input, output, db, constants, false).map_err(|error| match error {
        CompileFileError::IoError(error) => CompileFailure {
            path: input.to_path_buf(),
            error,
            source: String::new(),
        },
        CompileFileError::CompileError { error, source } => CompileFailure {
            path: input.to_path_buf(),
            error,
            source,
        },
    })
}

/// Compile a single file to binary bytes, handling transpilation if needed.
///
/// Supports:
/// - .rotom (Native Rotoscript)
/// - .script (legacy DSPRE script)
/// - .s (Decomp assembly)
///
/// # Arguments
/// * `input` - Path to the input file
/// * `output` - Path to the output .bin file
/// * `db` - The command database
/// * `constants` - The constant database (with decomp constants loaded if needed)
pub fn compile_file(
    input: &Path,
    output: &Path,
    db: &DatabaseV2,
    constants: &ConstantDb,
) -> Result<CompileResult, CompileError> {
    compile_file_internal(input, output, db, constants, true).map_err(|e| match e {
        CompileFileError::IoError(err) => err,
        CompileFileError::CompileError { error, .. } => error,
    })
}

fn generate_output_path_compile(input: &Path, output_dir: &Path) -> PathBuf {
    let stem = input.file_stem().unwrap_or_default();
    output_dir.join(format!("{}.bin", stem.to_string_lossy()))
}

fn generate_output_path_decompile(
    input: &Path,
    output_dir: &Path,
    is_levelscript: bool,
) -> PathBuf {
    let stem = input.file_stem().unwrap_or_default();
    let extension = if is_levelscript { "json" } else { "rotom" };
    output_dir.join(format!("{}.{}", stem.to_string_lossy(), extension))
}

fn resolve_decompile_output_path(
    input: &Path,
    output_file: Option<&Path>,
    output_dir: Option<&Path>,
    is_levelscript: bool,
) -> PathBuf {
    if let Some(path) = output_file {
        path.to_path_buf()
    } else if let Some(dir) = output_dir {
        generate_output_path_decompile(input, dir, is_levelscript)
    } else {
        let extension = if is_levelscript { "json" } else { "rotom" };
        input.with_extension(extension)
    }
}

fn detect_compile_output_collisions(
    files: &[PathBuf],
    output_dir: &Path,
) -> Vec<(PathBuf, Vec<PathBuf>)> {
    let mut output_to_inputs: std::collections::HashMap<PathBuf, Vec<PathBuf>> =
        std::collections::HashMap::new();

    for input_file in files {
        let output_path = generate_output_path_compile(input_file, output_dir);
        output_to_inputs
            .entry(output_path)
            .or_default()
            .push(input_file.clone());
    }

    let mut collisions: Vec<(PathBuf, Vec<PathBuf>)> = output_to_inputs
        .into_iter()
        .filter(|(_, inputs)| inputs.len() > 1)
        .collect();
    collisions.sort_by(|a, b| a.0.cmp(&b.0));
    collisions
}

pub fn compile_path(
    input: &Path,
    output: &Path,
    db: &DatabaseV2,
    constants: &ConstantDb,
) -> Result<BatchCompileResult, CompileError> {
    if input.is_file() {
        let output_path = if output.is_dir() {
            generate_output_path_compile(input, output)
        } else {
            output.to_path_buf()
        };

        match compile_file_internal(input, &output_path, db, constants, true) {
            Ok(result) => Ok(BatchCompileResult {
                successes: vec![result],
                failures: vec![],
            }),
            Err(CompileFileError::IoError(e)) => Ok(BatchCompileResult {
                successes: vec![],
                failures: vec![CompileFailure {
                    path: input.to_path_buf(),
                    error: e,
                    source: String::new(),
                }],
            }),
            Err(CompileFileError::CompileError { error, source }) => Ok(BatchCompileResult {
                successes: vec![],
                failures: vec![CompileFailure {
                    path: input.to_path_buf(),
                    error,
                    source,
                }],
            }),
        }
    } else if input.is_dir() {
        if output.exists() && !output.is_dir() {
            return Err(CompileError::Io {
                message: format!(
                    "Output must be a directory when input is a directory, got: {}",
                    output.display()
                ),
            });
        }

        if !output.exists() {
            std::fs::create_dir_all(output).map_err(|e| CompileError::Io {
                message: format!(
                    "Failed to create output directory '{}': {}",
                    output.display(),
                    e
                ),
            })?;
        }

        let files: Vec<PathBuf> = std::fs::read_dir(input)
            .map_err(|e| CompileError::Io {
                message: format!("Failed to read directory '{}': {}", input.display(), e),
            })?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path
                        .extension()
                        .and_then(|s| s.to_str())
                        .is_some_and(|ext| {
                            ext.eq_ignore_ascii_case("rotom")
                                || ext.eq_ignore_ascii_case("script")
                                || ext.eq_ignore_ascii_case("s")
                                || ext.eq_ignore_ascii_case("json")
                        })
            })
            .collect();

        if files.is_empty() {
            return Err(CompileError::Io {
                message: format!(
                    "No supported script files (.rotom, .script, .s, .json) found in directory: {}",
                    input.display()
                ),
            });
        }

        let collisions = detect_compile_output_collisions(&files, output);
        if !collisions.is_empty() {
            let details = collisions
                .iter()
                .map(|(out, inputs)| {
                    let mut names: Vec<String> = inputs
                        .iter()
                        .map(|p| {
                            p.file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned()
                        })
                        .collect();
                    names.sort();
                    format!("{} <= [{}]", out.display(), names.join(", "))
                })
                .collect::<Vec<_>>()
                .join("; ");

            return Err(CompileError::Io {
                message: format!(
                    "Output path collision detected for directory compile. Multiple inputs map to the same output: {}",
                    details
                ),
            });
        }

        // Compile all files in parallel
        let results: Vec<Result<CompileResult, CompileFailure>> = files
            .par_iter()
            .map(|input_file| {
                let output_path = generate_output_path_compile(input_file, output);
                compile_file_internal(input_file, &output_path, db, constants, true).map_err(|e| {
                    match e {
                        CompileFileError::IoError(error) => CompileFailure {
                            path: input_file.clone(),
                            error,
                            source: String::new(),
                        },
                        CompileFileError::CompileError { error, source } => CompileFailure {
                            path: input_file.clone(),
                            error,
                            source,
                        },
                    }
                })
            })
            .collect();

        let mut successes = Vec::new();
        let mut failures = Vec::new();

        for result in results {
            match result {
                Ok(r) => successes.push(r),
                Err(failure) => failures.push(failure),
            }
        }

        Ok(BatchCompileResult {
            successes,
            failures,
        })
    } else {
        Err(CompileError::Io {
            message: format!("Input path does not exist: {}", input.display()),
        })
    }
}

fn decompile_file_internal(
    input: &Path,
    output_file: Option<&Path>,
    output_dir: Option<&Path>,
    db: &DatabaseV2,
) -> Result<DecompileFileResult, DecompileFailure> {
    let bytes = std::fs::read(input).map_err(|e| DecompileFailure {
        path: input.to_path_buf(),
        error: DecompileError::Io {
            message: format!("Failed to read input file '{}': {}", input.display(), e),
        },
    })?;

    let script_output = decompile_to_ir(bytes, db).map_err(|e| DecompileFailure {
        path: input.to_path_buf(),
        error: e,
    })?;

    let quirks = match &script_output {
        ScriptOutput::Normal { jump_table_end_marker_count, .. } if *jump_table_end_marker_count != 1 => {
            vec![crate::compile_state::BinaryQuirk::JumpTableEndMarkerCount(*jump_table_end_marker_count)]
        }
        _ => Vec::new(),
    };

    let is_levelscript = matches!(script_output, ScriptOutput::Levelscript(_));

    let output_path = resolve_decompile_output_path(input, output_file, output_dir, is_levelscript);

    let source_text = ir_to_source(&script_output, db);
    let size = source_text.len();

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| DecompileFailure {
            path: input.to_path_buf(),
            error: DecompileError::Io {
                message: format!(
                    "Failed to create output directory '{}': {}",
                    parent.display(),
                    e
                ),
            },
        })?;
    }

    std::fs::write(&output_path, &source_text).map_err(|e| DecompileFailure {
        path: input.to_path_buf(),
        error: DecompileError::Io {
            message: format!(
                "Failed to write output file '{}': {}",
                output_path.display(),
                e
            ),
        },
    })?;

    Ok(DecompileFileResult {
        input: input.to_path_buf(),
        output: output_path,
        size,
        quirks,
    })
}

pub(crate) fn decompile_file_for_batch(
    input: &Path,
    output_file: Option<&Path>,
    output_dir: Option<&Path>,
    db: &DatabaseV2,
) -> Result<DecompileFileResult, DecompileFailure> {
    decompile_file_internal(input, output_file, output_dir, db)
}

pub fn decompile_file(
    input: &Path,
    output_dir: Option<&Path>,
    db: &DatabaseV2,
) -> DecompileResult<DecompileFileResult> {
    decompile_file_internal(input, None, output_dir, db).map_err(|f| f.error)
}

pub fn decompile_path(
    input: &Path,
    output: &Path,
    db: &DatabaseV2,
) -> Result<BatchDecompileResult, DecompileError> {
    if input.is_file() {
        let (output_file, output_dir) = if output.is_dir() {
            (None, Some(output))
        } else {
            (Some(output), None)
        };

        match decompile_file_internal(input, output_file, output_dir, db) {
            Ok(result) => Ok(BatchDecompileResult {
                successes: vec![result],
                failures: vec![],
            }),
            Err(failure) => Ok(BatchDecompileResult {
                successes: vec![],
                failures: vec![failure],
            }),
        }
    } else if input.is_dir() {
        if output.exists() && !output.is_dir() {
            return Err(DecompileError::Io {
                message: format!(
                    "Output must be a directory when input is a directory, got: {}",
                    output.display()
                ),
            });
        }

        if !output.exists() {
            std::fs::create_dir_all(output).map_err(|e| DecompileError::Io {
                message: format!(
                    "Failed to create output directory '{}': {}",
                    output.display(),
                    e
                ),
            })?;
        }

        let files: Vec<PathBuf> = std::fs::read_dir(input)
            .map_err(|e| DecompileError::Io {
                message: format!("Failed to read directory '{}': {}", input.display(), e),
            })?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path.extension().is_none_or(|ext| {
                        ext.to_str().is_some_and(|s| s.eq_ignore_ascii_case("bin"))
                    })
            })
            .collect();

        if files.is_empty() {
            return Err(DecompileError::Io {
                message: format!("No .bin files found in directory: {}", input.display()),
            });
        }

        let results: Vec<Result<DecompileFileResult, DecompileFailure>> = files
            .par_iter()
            .map(|input_file| decompile_file_internal(input_file, None, Some(output), db))
            .collect();

        let mut successes = Vec::new();
        let mut failures = Vec::new();

        for result in results {
            match result {
                Ok(r) => successes.push(r),
                Err(failure) => failures.push(failure),
            }
        }

        Ok(BatchDecompileResult {
            successes,
            failures,
        })
    } else {
        Err(DecompileError::Io {
            message: format!("Input path does not exist: {}", input.display()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ConstantDb, DatabaseV2, compile_path, compile_to_bytes, decompile_path,
        detect_compile_output_collisions, resolve_decompile_output_path,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(name: &str) -> PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before UNIX_EPOCH")
            .as_nanos();
        std::env::temp_dir().join(format!("rotom_{}_{}_{}", name, std::process::id(), now))
    }

    fn load_test_db() -> DatabaseV2 {
        DatabaseV2::load(Path::new("src/db/platinum_v2.json"))
            .expect("Test database not found at src/db/platinum_v2.json")
    }

    fn minimal_script_source() -> &'static str {
        "function Main #1:\nEnd\n"
    }

    #[test]
    fn is_levelscript_path_matches_known_naming_conventions() {
        assert!(super::is_levelscript_path(Path::new("map_init_main.s")));
        assert!(super::is_levelscript_path(Path::new("event_hdr.s")));
        assert!(!super::is_levelscript_path(Path::new(
            "map_init_new_game.s"
        )));
        assert!(!super::is_levelscript_path(Path::new("normal.s")));
    }

    fn write_test_decomp_project(root: &Path) {
        fs::create_dir_all(root.join("include/constants"))
            .expect("failed to create include/constants");
        fs::create_dir_all(root.join("res/field/scripts"))
            .expect("failed to create res/field/scripts");
        fs::write(root.join("res/field/scripts/scripts.order"), "")
            .expect("failed to write scripts.order");
    }

    #[test]
    fn detect_compile_output_collisions_flags_same_stem() {
        let files = vec![
            PathBuf::from("in/same.rotom"),
            PathBuf::from("in/same.script"),
            PathBuf::from("in/unique.s"),
        ];
        let output_dir = Path::new("out");

        let collisions = detect_compile_output_collisions(&files, output_dir);

        assert_eq!(collisions.len(), 1);
        let (out_path, inputs) = &collisions[0];
        assert_eq!(*out_path, PathBuf::from("out/same.bin"));
        assert_eq!(inputs.len(), 2);
    }

    #[test]
    fn detect_compile_output_collisions_allows_unique_outputs() {
        let files = vec![
            PathBuf::from("in/a.rotom"),
            PathBuf::from("in/b.script"),
            PathBuf::from("in/c.s"),
        ];
        let output_dir = Path::new("out");

        let collisions = detect_compile_output_collisions(&files, output_dir);

        assert!(collisions.is_empty());
    }

    #[test]
    fn resolve_decompile_output_path_prefers_explicit_file() {
        let input = Path::new("scripts/0001.bin");
        let output_file = Path::new("custom/output_name.rotom");
        let output_dir = Path::new("ignored");

        let resolved =
            resolve_decompile_output_path(input, Some(output_file), Some(output_dir), false);

        assert_eq!(resolved, output_file);
    }

    #[test]
    fn resolve_decompile_output_path_uses_dir_and_type_extension() {
        let input = Path::new("scripts/0010.bin");
        let output_dir = Path::new("out");

        let levelscript_out = resolve_decompile_output_path(input, None, Some(output_dir), true);
        let script_out = resolve_decompile_output_path(input, None, Some(output_dir), false);

        assert_eq!(levelscript_out, PathBuf::from("out/0010.json"));
        assert_eq!(script_out, PathBuf::from("out/0010.rotom"));
    }

    #[test]
    fn resolve_decompile_output_path_defaults_to_input_extension_swap() {
        let input = Path::new("scripts/raw.bin");

        let resolved = resolve_decompile_output_path(input, None, None, false);

        assert_eq!(resolved, PathBuf::from("scripts/raw.rotom"));
    }

    #[test]
    fn compile_path_file_mode_respects_explicit_output_file() {
        let temp_dir = unique_temp_dir("compile_path_file_mode_explicit_output");
        fs::create_dir_all(&temp_dir).expect("failed to create temp dir");

        let input_path = temp_dir.join("input.rotom");
        let output_path = temp_dir.join("custom_name.bin");
        fs::write(&input_path, minimal_script_source()).expect("failed to write input script");

        let db = load_test_db();
        let constants = ConstantDb::new();

        let result = compile_path(&input_path, &output_path, &db, &constants)
            .expect("compile_path should return a batch result");
        fs::remove_dir_all(&temp_dir).ok();

        assert!(result.is_success(), "compile_path should succeed");
        assert_eq!(result.successes.len(), 1);
        assert_eq!(result.successes[0].output, output_path);
    }

    #[test]
    fn compile_path_directory_mode_rejects_file_output_target() {
        let temp_dir = unique_temp_dir("compile_path_dir_mode_reject_file_output");
        let input_dir = temp_dir.join("in");
        fs::create_dir_all(&input_dir).expect("failed to create input dir");
        fs::write(input_dir.join("a.rotom"), minimal_script_source())
            .expect("failed to write input script");

        let output_file = temp_dir.join("out.bin");
        fs::write(&output_file, b"already a file").expect("failed to create output file");

        let db = load_test_db();
        let constants = ConstantDb::new();

        let result = compile_path(&input_dir, &output_file, &db, &constants);
        fs::remove_dir_all(&temp_dir).ok();

        assert!(
            result.is_err(),
            "directory compile should reject file output"
        );
        let err_text = format!("{}", result.err().unwrap());
        assert!(
            err_text.contains("Output must be a directory"),
            "unexpected error: {}",
            err_text
        );
    }

    #[test]
    fn decompile_path_file_mode_respects_explicit_output_file() {
        let temp_dir = unique_temp_dir("decompile_path_file_mode_explicit_output");
        fs::create_dir_all(&temp_dir).expect("failed to create temp dir");

        let db = load_test_db();
        let constants = ConstantDb::new();
        let bytes = compile_to_bytes(minimal_script_source(), &db, &constants)
            .expect("failed to compile seed script");

        let input_path = temp_dir.join("input.bin");
        let output_path = temp_dir.join("custom_out.rotom");
        fs::write(&input_path, bytes).expect("failed to write input binary");

        let result = decompile_path(&input_path, &output_path, &db)
            .expect("decompile_path should return a batch result");
        fs::remove_dir_all(&temp_dir).ok();

        assert!(result.is_success(), "decompile_path should succeed");
        assert_eq!(result.successes.len(), 1);
        assert_eq!(result.successes[0].output, output_path);
    }

    #[test]
    fn decompile_path_directory_mode_accepts_extensionless_binary_inputs() {
        let temp_dir = unique_temp_dir("decompile_path_dir_mode_extensionless");
        let input_dir = temp_dir.join("in");
        let output_dir = temp_dir.join("out");
        fs::create_dir_all(&input_dir).expect("failed to create input dir");

        let db = load_test_db();
        let constants = ConstantDb::new();
        let bytes = compile_to_bytes(minimal_script_source(), &db, &constants)
            .expect("failed to compile seed script");

        fs::write(input_dir.join("0000"), bytes).expect("failed to write extensionless binary");

        let result = decompile_path(&input_dir, &output_dir, &db)
            .expect("decompile_path should return a batch result");
        fs::remove_dir_all(&temp_dir).ok();

        assert!(result.is_success(), "decompile_path should succeed");
        assert_eq!(result.successes.len(), 1);
        assert_eq!(result.successes[0].output, output_dir.join("0000.rotom"));
    }

    #[test]
    fn compile_path_rotom_supports_preprocessor_includes_and_defines() {
        let temp_dir = unique_temp_dir("compile_path_rotom_supports_includes_and_defines");
        write_test_decomp_project(&temp_dir);
        fs::write(
            temp_dir.join("include/constants/test.h"),
            "#define TEST_MESSAGE 7\n",
        )
        .expect("failed to write test header");

        let input_path = temp_dir.join("res/field/scripts/test.rotom");
        fs::write(
            &input_path,
            r#"#include "constants/test.h"
#define LOCAL_MESSAGE 7
function Main #1:
    Message LOCAL_MESSAGE
    Message TEST_MESSAGE
    End
"#,
        )
        .expect("failed to write .rotom source");

        let output_path = temp_dir.join("test.bin");
        let db = load_test_db();
        let mut constants = ConstantDb::new();
        constants
            .load_decomp_project(&temp_dir)
            .expect("failed to load test decomp project");

        let result = compile_path(&input_path, &output_path, &db, &constants)
            .expect("compile_path should return a batch result");
        assert!(result.is_success(), "compile_path should succeed");

        let compiled = fs::read(&output_path).expect("failed to read compiled output");
        let expected = compile_to_bytes(
            "function Main #1:\n    Message 7\n    Message 7\n    End\n",
            &db,
            &ConstantDb::new(),
        )
        .expect("expected source should compile");
        fs::remove_dir_all(&temp_dir).ok();

        assert_eq!(compiled, expected);
    }

    #[test]
    fn compile_to_bytes_supports_symbolic_alias_chains() {
        let db = load_test_db();
        let constants = ConstantDb::new();

        let compiled = compile_to_bytes(
            "alias 7 as FOO
alias FOO as BAR

function Main #1:
    Message BAR
    End
",
            &db,
            &constants,
        )
        .expect("symbolic alias chain should compile");

        let expected = compile_to_bytes(
            "function Main #1:\n    Message 7\n    End\n",
            &db,
            &ConstantDb::new(),
        )
        .expect("expected source should compile");

        assert_eq!(compiled, expected);
    }

    #[test]
    fn compile_to_bytes_supports_aliases_from_earlier_function_bodies() {
        let db = load_test_db();
        let constants = ConstantDb::new();

        let compiled = compile_to_bytes(
            "function DefineAlias #1:
    alias 7 as SHARED
    End

function Main #2:
    Message SHARED
    End
",
            &db,
            &constants,
        )
        .expect("alias from earlier function body should compile");

        let expected = compile_to_bytes(
            "function DefineAlias #1:\n    End\n\nfunction Main #2:\n    Message 7\n    End\n",
            &db,
            &ConstantDb::new(),
        )
        .expect("expected source should compile");

        assert_eq!(compiled, expected);
    }

    #[test]
    fn compile_to_bytes_supports_top_level_alias_redefinition_in_source_order() {
        let db = load_test_db();
        let constants = ConstantDb::new();

        let compiled = compile_to_bytes(
            "alias 7 as VALUE
function First #1:
    Message VALUE
    End

alias 9 as VALUE
function Second #2:
    Message VALUE
    End
",
            &db,
            &constants,
        )
        .expect("top-level alias redefinition should compile");

        let expected = compile_to_bytes(
            "function First #1:\n    Message 7\n    End\n\nfunction Second #2:\n    Message 9\n    End\n",
            &db,
            &ConstantDb::new(),
        )
        .expect("expected source should compile");

        assert_eq!(compiled, expected);
    }

    #[test]
    fn compile_to_bytes_rejects_forward_alias_reference_in_source_order() {
        let db = load_test_db();
        let constants = ConstantDb::new();

        let result = compile_to_bytes(
            "function Main #1:
    Message SHARED
    End

alias 7 as SHARED
",
            &db,
            &constants,
        );

        assert!(result.is_err(), "forward alias reference should fail");
    }

    #[test]
    fn compile_path_rotom_supports_symbolic_aliases_from_includes() {
        let temp_dir =
            unique_temp_dir("compile_path_rotom_supports_symbolic_aliases_from_includes");
        write_test_decomp_project(&temp_dir);
        fs::write(
            temp_dir.join("include/constants/test.h"),
            "#define TEST_MESSAGE 7\n",
        )
        .expect("failed to write test header");

        let input_path = temp_dir.join("res/field/scripts/test.rotom");
        fs::write(
            &input_path,
            r#"#include "constants/test.h"
alias TEST_MESSAGE as LOCAL_MESSAGE
function Main #1:
    Message LOCAL_MESSAGE
    End
"#,
        )
        .expect("failed to write .rotom source");

        let output_path = temp_dir.join("test.bin");
        let db = load_test_db();
        let mut constants = ConstantDb::new();
        constants
            .load_decomp_project(&temp_dir)
            .expect("failed to load test decomp project");

        let result = compile_path(&input_path, &output_path, &db, &constants)
            .expect("compile_path should return a batch result");
        assert!(result.is_success(), "compile_path should succeed");

        let compiled = fs::read(&output_path).expect("failed to read compiled output");
        let expected = compile_to_bytes(
            "function Main #1:\n    Message 7\n    End\n",
            &db,
            &ConstantDb::new(),
        )
        .expect("expected source should compile");
        fs::remove_dir_all(&temp_dir).ok();

        assert_eq!(compiled, expected);
    }

    #[test]
    fn compile_path_rotom_reports_unresolved_includes() {
        let temp_dir = unique_temp_dir("compile_path_rotom_reports_unresolved_includes");
        write_test_decomp_project(&temp_dir);

        let input_path = temp_dir.join("res/field/scripts/test.rotom");
        fs::write(
            &input_path,
            r#"#include "constants/missing.h"
function Main #1:
    End
"#,
        )
        .expect("failed to write .rotom source");

        let output_path = temp_dir.join("test.bin");
        let db = load_test_db();
        let mut constants = ConstantDb::new();
        constants
            .load_decomp_project(&temp_dir)
            .expect("failed to load test decomp project");

        let result = compile_path(&input_path, &output_path, &db, &constants)
            .expect("compile_path should return a batch result");
        fs::remove_dir_all(&temp_dir).ok();

        assert_eq!(result.failures.len(), 1);
        match &result.failures[0].error {
            crate::CompileError::Database { message } => {
                assert!(message.contains("Unresolved include 'constants/missing.h'"));
            }
            other => panic!("expected database error, got {other:?}"),
        }
    }
}
