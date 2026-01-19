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
    DecompileError, DecompileResult, Disassembler, disassemble_bytes, ir_to_source,
};

use rayon::prelude::*;
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::compiler::ir::TopLevelItem;

#[derive(Debug, Serialize)]
pub struct CompileResult {
    /// The input file path
    pub input: PathBuf,
    /// The output file path (where the binary was written)
    pub output: PathBuf,
    /// Size of the compiled binary in bytes
    pub size: usize,
}

/// A compilation failure with enough context for rich error display
#[derive(Debug, Serialize)]
pub struct CompileFailure {
    /// The input file path that failed
    pub path: PathBuf,
    /// The compilation error
    pub error: CompileError,
    /// The source code (transpiled if applicable) for codespan-reporting
    /// Skipped in JSON output to avoid bloating machine-readable responses
    #[serde(skip)]
    pub source: String,
}

/// Result of compiling a path (file or directory)
#[derive(Debug, Serialize)]
pub struct BatchCompileResult {
    /// Successfully compiled files
    pub successes: Vec<CompileResult>,
    /// Failed compilations with source context for rich error display
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
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let file = parser.parse_script_file()?;

    let mut analyzer = Analyzer::with_constants(constants);
    analyzer.analyze(&file)?;

    let mut lowerer = Lowerer::with_constants(&analyzer.symbols, db, constants);
    let items = lowerer.lower_script_file(&file)?;

    let mut emitter = Emitter::new(db);
    emitter.emit_script_file(&items)
}

pub fn decompile_to_ir(bytes: Vec<u8>, db: &DatabaseV2) -> DecompileResult<Vec<TopLevelItem>> {
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

    let rotom_source = match extension.as_str() {
        "rotom" => source,
        "script" => transpiler::transpile_dspre(&source, Some(db)),
        "s" => transpiler::transpile_decomp(&source, Some(db)),
        _ => {
            return Err(CompileFileError::IoError(CompileError::Io {
                message: format!("Unsupported file extension: .{}", extension),
            }));
        }
    };

    let bytes = compile_to_bytes(&rotom_source, db, constants).map_err(|e| {
        CompileFileError::CompileError {
            error: e,
            source: rotom_source.clone(),
        }
    })?;
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

fn generate_output_path_decompile(input: &Path, output_dir: &Path) -> PathBuf {
    let stem = input.file_stem().unwrap_or_default();
    output_dir.join(format!("{}.rotom", stem.to_string_lossy()))
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
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                if !path.is_file() {
                    return false;
                }
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| {
                        let ext = ext.to_lowercase();
                        ext == "rotom" || ext == "script" || ext == "s"
                    })
                    .unwrap_or(false)
            })
            .collect();

        if files.is_empty() {
            return Err(CompileError::Io {
                message: format!(
                    "No supported script files (.rotom, .script, .s) found in directory: {}",
                    input.display()
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
    output: &Path,
    db: &DatabaseV2,
) -> Result<DecompileFileResult, DecompileFailure> {
    let bytes = std::fs::read(input).map_err(|e| DecompileFailure {
        path: input.to_path_buf(),
        error: DecompileError::Io {
            message: format!("Failed to read input file '{}': {}", input.display(), e),
        },
    })?;

    let items = decompile_to_ir(bytes, db).map_err(|e| DecompileFailure {
        path: input.to_path_buf(),
        error: e,
    })?;

    let source_text = ir_to_source(&items);
    let size = source_text.len();

    std::fs::write(output, &source_text).map_err(|e| DecompileFailure {
        path: input.to_path_buf(),
        error: DecompileError::Io {
            message: format!("Failed to write output file '{}': {}", output.display(), e),
        },
    })?;

    Ok(DecompileFileResult {
        input: input.to_path_buf(),
        output: output.to_path_buf(),
        size,
    })
}

pub fn decompile_file(
    input: &Path,
    output: &Path,
    db: &DatabaseV2,
) -> DecompileResult<DecompileFileResult> {
    decompile_file_internal(input, output, db).map_err(|f| f.error)
}

pub fn decompile_path(
    input: &Path,
    output: &Path,
    db: &DatabaseV2,
) -> Result<BatchDecompileResult, DecompileError> {
    if input.is_file() {
        let output_path = if output.is_dir() {
            generate_output_path_decompile(input, output)
        } else {
            output.to_path_buf()
        };

        match decompile_file_internal(input, &output_path, db) {
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
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                if !path.is_file() {
                    return false;
                }
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| ext.to_lowercase() == "bin")
                    .unwrap_or(false)
            })
            .collect();

        if files.is_empty() {
            return Err(DecompileError::Io {
                message: format!("No .bin files found in directory: {}", input.display()),
            });
        }

        let results: Vec<Result<DecompileFileResult, DecompileFailure>> = files
            .par_iter()
            .map(|input_file| {
                let output_path = generate_output_path_decompile(input_file, output);
                decompile_file_internal(input_file, &output_path, db)
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
