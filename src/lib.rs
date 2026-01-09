//! Rotom - A Pokemon Gen 4 script compiler/decompiler
//!
//! This library provides functionality to compile Rotoscript source files
//! to binary format and decompile binary scripts back to Rotoscript.

pub mod compiler;
pub mod database;
pub mod transpiler;

// Re-export commonly used types for convenience
pub use compiler::{
    Lexer, Parser, Analyzer, Lowerer,
    parse_error::{CompileError, print_error},
};
pub use compiler::codegen::Emitter;
pub use database::{DatabaseV2, ConstantDb, GameFamily};

use rayon::prelude::*;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Result of compiling a single file
#[derive(Debug, Serialize)]
pub struct CompileResult {
    /// The input file path
    pub input: PathBuf,
    /// The output file path (where the binary was written)
    pub output: PathBuf,
    /// Size of the compiled binary in bytes
    pub size: usize,
}

/// Result of compiling a path (file or directory)
#[derive(Debug, Serialize)]
pub struct BatchCompileResult {
    /// Successfully compiled files
    pub successes: Vec<CompileResult>,
    /// Failed compilations with their errors
    pub failures: Vec<(PathBuf, CompileError)>,
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

/// Compile a single .rotom file to binary bytes.
///
/// This is the core compilation function that returns the raw bytes.
/// Use `compile_file` if you want to write directly to disk.
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

    let mut lowerer = Lowerer::new(&analyzer.symbols, db);
    let items = lowerer.lower_script_file(&file)?;

    let mut emitter = Emitter::new(db);
    emitter.emit_script_file(&items)
}

/// Compile a single .rotom file and write the output to disk.
///
/// # Arguments
/// * `input` - Path to the input .rotom file
/// * `output` - Path to the output .bin file
/// * `db` - The command database
/// * `constants` - The constant database (with decomp constants loaded if needed)
pub fn compile_file(
    input: &Path,
    output: &Path,
    db: &DatabaseV2,
    constants: &ConstantDb,
) -> Result<CompileResult, CompileError> {
    let source = std::fs::read_to_string(input).map_err(|e| CompileError::Io {
        message: format!("Failed to read input file '{}': {}", input.display(), e),
    })?;

    let bytes = compile_to_bytes(&source, db, constants)?;
    let size = bytes.len();

    std::fs::write(output, &bytes).map_err(|e| CompileError::Io {
        message: format!("Failed to write output file '{}': {}", output.display(), e),
    })?;

    Ok(CompileResult {
        input: input.to_path_buf(),
        output: output.to_path_buf(),
        size,
    })
}

/// Generate output path for a .rotom file.
/// Replaces .rotom extension with .bin, or appends .bin if no .rotom extension.
fn generate_output_path(input: &Path, output_dir: &Path) -> PathBuf {
    let stem = input.file_stem().unwrap_or_default();
    output_dir.join(format!("{}.bin", stem.to_string_lossy()))
}

/// Compile a path which can be either a single file or a directory.
///
/// If `input` is a file, compiles just that file.
/// If `input` is a directory, compiles all .rotom files in it (in parallel).
///
/// # Arguments
/// * `input` - Path to a .rotom file or directory containing .rotom files
/// * `output` - Path to output .bin file or directory for outputs
/// * `db` - The command database
/// * `constants` - The constant database
///
/// # Output Behavior
/// - If `input` is a file and `output` is a file: writes to that exact path
/// - If `input` is a file and `output` is a directory: writes `{stem}.bin` in that directory
/// - If `input` is a directory and `output` is a directory: writes all `{stem}.bin` files there
/// - If `input` is a directory and `output` is a file: error
pub fn compile_path(
    input: &Path,
    output: &Path,
    db: &DatabaseV2,
    constants: &ConstantDb,
) -> Result<BatchCompileResult, CompileError> {
    if input.is_file() {
        // Single file compilation
        let output_path = if output.is_dir() {
            generate_output_path(input, output)
        } else {
            output.to_path_buf()
        };

        match compile_file(input, &output_path, db, constants) {
            Ok(result) => Ok(BatchCompileResult {
                successes: vec![result],
                failures: vec![],
            }),
            Err(e) => Ok(BatchCompileResult {
                successes: vec![],
                failures: vec![(input.to_path_buf(), e)],
            }),
        }
    } else if input.is_dir() {
        // Directory compilation
        if output.exists() && !output.is_dir() {
            return Err(CompileError::Io {
                message: format!(
                    "Output must be a directory when input is a directory, got: {}",
                    output.display()
                ),
            });
        }

        // Create output directory if it doesn't exist
        if !output.exists() {
            std::fs::create_dir_all(output).map_err(|e| CompileError::Io {
                message: format!("Failed to create output directory '{}': {}", output.display(), e),
            })?;
        }

        // Collect all .rotom files in the directory
        let rotom_files: Vec<PathBuf> = std::fs::read_dir(input)
            .map_err(|e| CompileError::Io {
                message: format!("Failed to read directory '{}': {}", input.display(), e),
            })?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("rotom"))
            })
            .collect();

        if rotom_files.is_empty() {
            return Err(CompileError::Io {
                message: format!("No .rotom files found in directory: {}", input.display()),
            });
        }

        // Compile all files in parallel
        let results: Vec<Result<CompileResult, (PathBuf, CompileError)>> = rotom_files
            .par_iter()
            .map(|input_file| {
                let output_path = generate_output_path(input_file, output);
                compile_file(input_file, &output_path, db, constants)
                    .map_err(|e| (input_file.clone(), e))
            })
            .collect();

        // Partition into successes and failures
        let mut successes = Vec::new();
        let mut failures = Vec::new();

        for result in results {
            match result {
                Ok(r) => successes.push(r),
                Err((path, e)) => failures.push((path, e)),
            }
        }

        Ok(BatchCompileResult { successes, failures })
    } else {
        Err(CompileError::Io {
            message: format!("Input path does not exist: {}", input.display()),
        })
    }
}
