//! Rotom - A Pokemon Gen 4 script compiler/decompiler
//!
//! This library provides functionality to compile Rotoscript source files
//! to binary format and decompile binary scripts back to Rotoscript.

pub mod compiler;
pub mod database;
pub mod decompiler;
pub mod transpiler;

pub use compiler::codegen::Emitter;
pub use compiler::{
    Analyzer, Lexer, Lowerer, Parser,
    parse_error::{CompileError, print_error},
};
pub use database::{ConstantDb, DatabaseV2, GameFamily};
pub use decompiler::{
    DecompileError, DecompileResult, Disassembler, LevelScript, LevelScriptEntry, ScriptOutput,
    ScriptType, disassemble_bytes, ir_to_source,
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
    compile_to_bytes_with_options(source, db, constants, true)
}

pub fn compile_to_bytes_with_options(
    source: &str,
    db: &DatabaseV2,
    constants: &ConstantDb,
    emit_end_marker: bool,
) -> Result<Vec<u8>, CompileError> {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let file = parser.parse_script_file()?;

    let mut analyzer = Analyzer::with_database(constants, db);
    analyzer.analyze(&file)?;

    let mut lowerer = Lowerer::with_constants(&analyzer.symbols, db, constants);
    let items = lowerer.lower_script_file(&file)?;

    let mut emitter = Emitter::new(db);
    emitter.emit_script_file(&items, emit_end_marker)
}

pub fn compile_levelscript_to_bytes(
    source: &str,
    constants: &ConstantDb,
) -> Result<Vec<u8>, CompileError> {
    let result = transpiler::transpile_levelscript(source, Some(constants)).map_err(|e| {
        CompileError::Io {
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
    let levelscript = LevelScript::from_json(source).map_err(|e| CompileError::Io {
        message: format!("Failed to parse levelscript JSON: {}", e),
    })?;

    Ok(levelscript.to_bytes())
}

fn is_levelscript_file(path: &Path) -> bool {
    let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    filename.contains("_init_") && !filename.contains("_init_new_game")
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

    let is_levelscript = is_levelscript_file(input)
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
        let (rotom_source, emit_end_marker) = match extension.as_str() {
            "rotom" => (source, true),
            "script" => (transpiler::transpile_dspre(&source, Some(db)), true),
            "s" => {
                let result = transpiler::transpile_decomp(&source, Some(db));
                (result.source, result.emit_end_marker)
            }
            _ => {
                return Err(CompileFileError::IoError(CompileError::Io {
                    message: format!("Unsupported file extension: .{}", extension),
                }));
            }
        };

        compile_to_bytes_with_options(&rotom_source, db, constants, emit_end_marker).map_err(
            |e| CompileFileError::CompileError {
                error: e,
                source: rotom_source.clone(),
            },
        )?
    };
    let size = bytes.len();

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
    compile_file_internal(input, output, db, constants).map_err(|e| match e {
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

        match compile_file_internal(input, &output_path, db, constants) {
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
                compile_file_internal(input_file, &output_path, db, constants).map_err(
                    |e| match e {
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
                    },
                )
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

    let is_levelscript = matches!(script_output, ScriptOutput::Levelscript(_));

    let output_path = resolve_decompile_output_path(input, output_file, output_dir, is_levelscript);

    let source_text = ir_to_source(&script_output, db);
    let size = source_text.len();

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
    })
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
    use super::{detect_compile_output_collisions, resolve_decompile_output_path};
    use std::path::{Path, PathBuf};

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
}
