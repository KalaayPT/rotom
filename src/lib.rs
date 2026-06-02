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
pub mod progress;
pub mod project;
pub mod transpiler;

pub use project::{
    compile as project_compile, config as project_config, convert as project_convert,
    error as project_error, init as project_init,
};

pub use compile_state::{BinaryQuirk, CompileState};
pub use compiler::batch_compile::compile_batch;
pub use compiler::codegen::Emitter;
pub use compiler::{
    Analyzer, Lexer, Lowerer, Parser,
    analysis::{SymbolTable, SymbolType},
    diagnostic::{CompileError, CompileWarning, print_error},
    sourcemap::{Position, SourceMap},
};
pub use database::{ConstantDb, DatabaseV2, GameFamily, GameFamilyExt, game_family_from_hint};
pub use decompiler::{
    DecompileError, DecompileResult, Disassembler, LevelScript, LevelScriptHeaderEntry,
    LevelScriptVarConditionEntry, ScriptOutput, ScriptType, disassemble_bytes, ir_to_source,
};
pub use progress::CompileProgress;

use rayon::prelude::{IntoParallelRefIterator, ParallelIterator};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

#[derive(Debug, Serialize)]
pub struct CompiledFile {
    pub input: PathBuf,
    pub output: PathBuf,
    pub size: usize,
    /// The source code (transpiled if applicable) for codespan-reporting.
    /// Skipped in JSON output to avoid bloating machine-readable responses.
    #[serde(skip)]
    pub source: String,
    pub warnings: Vec<CompileWarning>,
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

impl CompileFailure {
    fn io_error(input: &Path, message: impl Into<String>) -> Self {
        CompileFailure {
            path: input.to_path_buf(),
            error: CompileError::Io {
                message: message.into(),
            },
            source: String::new(),
        }
    }

    fn with_source(input: &Path, source: &str, error: CompileError) -> Self {
        CompileFailure {
            path: input.to_path_buf(),
            error,
            source: source.to_string(),
        }
    }

    fn with_path(input: &Path, error: CompileError) -> Self {
        CompileFailure {
            path: input.to_path_buf(),
            error,
            source: String::new(),
        }
    }
}

/// Result of compiling a path (file or directory)
#[derive(Debug, Serialize)]
pub struct BatchCompileResult {
    pub successes: Vec<CompiledFile>,
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
    pub quirks: BinaryQuirk,
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

/// Compilation output including emitted bytes and any warnings.
pub struct CompiledBytes {
    pub bytes: Vec<u8>,
    pub warnings: Vec<CompileWarning>,
}

/// Compile Rotom source to bytes.
///
/// Parses the source, runs semantic analysis, lowers to IR, and emits binary.
pub fn compile(
    source: &str,
    db: &DatabaseV2,
    constants: &ConstantDb,
    binary_quirks: BinaryQuirk,
) -> Result<CompiledBytes, CompileError> {
    static EMPTY: OnceLock<Arc<uxie::Workspace>> = OnceLock::new();
    // No file context — family is irrelevant since project_path is empty and
    // find_text_archive_path returns None for any archive ID.
    let workspace = EMPTY.get_or_init(|| {
        Arc::new(uxie::Workspace::new(
            std::path::PathBuf::new(),
            uxie::game::Game::Platinum,
        ))
    });
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let file = parser.parse_script_file()?;
    emit_script_file(&file, db, constants, binary_quirks, workspace, "")
}

/// Runs semantic analysis, lowers to IR, and emits binary bytes for a parsed script.
fn emit_script_file(
    file: &compiler::ast::ScriptFile,
    db: &DatabaseV2,
    constants: &ConstantDb,
    binary_quirks: BinaryQuirk,
    workspace: &Arc<uxie::Workspace>,
    source_stem: &str,
) -> Result<CompiledBytes, CompileError> {
    let mut analyzer = Analyzer::with_database(constants, db);
    analyzer.analyze(file)?;
    let mut lowerer = Lowerer::for_file(
        &analyzer.symbols,
        db,
        constants,
        workspace.clone(),
        source_stem.to_string(),
    );
    let items = lowerer.lower_script_file(file)?;
    let mut emitter = Emitter::new(db);
    let jump_table_end_marker_count = binary_quirks.jump_table_end_marker_count.unwrap_or(1);
    let bytes = emitter.emit_script_file(&items, jump_table_end_marker_count)?;
    Ok(CompiledBytes {
        bytes,
        warnings: analyzer.warnings,
    })
}

/// Compile a levelscript decomp source string to binary bytes.
///
/// Accepts the legacy `.s` levelscript syntax and emits the raw binary
/// levelscript format used by the game engine.
pub fn compile_levelscript_assembly_to_bytes(
    source: &str,
    constants: &ConstantDb,
) -> Result<Vec<u8>, CompileError> {
    let result = transpiler::transpile_levelscript(source, Some(constants)).map_err(|e| {
        CompileError::Transpile {
            message: format!("Levelscript transpile error: {}", e),
        }
    })?;

    let mut bytes = result.levelscript.to_bytes();

    bytes.extend(std::iter::repeat_n(0u8, result.extra_padding as usize));

    Ok(bytes)
}

/// Compile a levelscript from its JSON representation to binary bytes.
///
/// `levelscript_padding` is appended after normal 4-byte alignment. It is
/// stored in compile state as a [`BinaryQuirk`] rather than in the JSON
/// so that users never need to edit padding bytes by hand.
pub fn compile_levelscript_json_to_bytes(
    source: &str,
    binary_quirks: BinaryQuirk,
) -> Result<Vec<u8>, CompileError> {
    let levelscript = LevelScript::from_json(source).map_err(|e| CompileError::Transpile {
        message: format!("Failed to parse levelscript JSON: {}", e),
    })?;

    let mut bytes = levelscript.to_bytes();
    if let Some(padding) = binary_quirks.levelscript_padding {
        bytes.extend(std::iter::repeat_n(0, padding as usize));
    }
    Ok(bytes)
}

/// Returns true when the path follows a known levelscript naming convention.
pub fn is_levelscript_path(path: &Path) -> bool {
    let stem = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("");

    (stem.contains("_init_") && !stem.contains("_init_new_game")) || stem.ends_with("_hdr")
}

/// Disassemble binary script bytes into an intermediate representation.
///
/// The returned [`ScriptOutput`] can be fed to [`ir_to_source()`] to obtain
/// human-readable Rotoscript.
pub fn decompile_to_ir(bytes: Vec<u8>, db: &DatabaseV2) -> DecompileResult<ScriptOutput> {
    disassemble_bytes(db, bytes)
}

fn count_jump_table_lines(source: &str) -> usize {
    let mut n = 0;
    let mut past_includes = false;
    for line in source.lines() {
        let t = line.trim();
        if t.starts_with(';') || t.starts_with('@') || t.starts_with("//") {
            continue;
        }
        if t.starts_with("#include") {
            past_includes = true;
            continue;
        }
        if !past_includes {
            continue;
        }
        if t == "ScriptEntryEnd" || t == "ScrDefEnd" {
            return n + 1;
        }
        if t.is_empty() || t.starts_with("ScriptEntry ") || t.starts_with("ScrDef ") {
            n += 1;
        } else {
            break;
        }
    }
    n
}

/// Reads a file from disk and compiles it, handling translation and preprocessor directives.
///
/// Supports `.rotom`, `.script`, `.s`, and `.json` inputs.
#[allow(clippy::too_many_lines)]
pub(crate) fn compile_file_internal(
    input: &Path,
    output: &Path,
    db: &DatabaseV2,
    constants: &ConstantDb,
    load_file_constants: bool,
    binary_quirks: BinaryQuirk,
    workspace: &std::sync::Arc<uxie::Workspace>,
) -> Result<CompiledFile, CompileFailure> {
    let source = std::fs::read_to_string(input).map_err(|e| {
        CompileFailure::io_error(
            input,
            format!("Failed to read input file '{}': {}", input.display(), e),
        )
    })?;
    let extension = input
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let is_levelscript = is_levelscript_path(input)
        || (extension == "s" && transpiler::is_levelscript_source(&source));

    let (bytes, warnings, warning_source) = if extension == "json" {
        (
            compile_levelscript_json_to_bytes(&source, binary_quirks)
                .map_err(|e| CompileFailure::with_source(input, &source, e))?,
            Vec::new(),
            source.clone(),
        )
    } else if is_levelscript && extension == "s" {
        let file_constants = if load_file_constants {
            let mut cloned = constants.clone();
            cloned
                .load_script_constants(input)
                .map_err(|e| CompileFailure::with_path(input, e))?;
            Some(cloned)
        } else {
            None
        };
        let constants = file_constants.as_ref().unwrap_or(constants);
        (
            compile_levelscript_assembly_to_bytes(&source, constants)
                .map_err(|e| CompileFailure::with_source(input, &source, e))?,
            Vec::new(),
            source.clone(),
        )
    } else {
        let (rotom_source, binary_quirks) = match extension.as_str() {
            "rotom" => (source.clone(), BinaryQuirk::default()),
            "script" => (
                transpiler::transpile_dspre(&source, Some(db)),
                BinaryQuirk::default(),
            ),
            "s" => {
                let result = transpiler::transpile_decomp(&source, Some(db)).map_err(|e| {
                    CompileFailure::with_source(
                        input,
                        &source,
                        CompileError::Transpile {
                            message: format!("Decomp transpile error at line {}: {}", e.line, e),
                        },
                    )
                })?;
                (result.source, result.binary_quirks)
            }
            _ => {
                return Err(CompileFailure::io_error(
                    input,
                    format!("Unsupported file extension: .{}", extension),
                ));
            }
        };

        let source_stem = input
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();

        // Warm step: pre-load text archive so message_ids is populated before
        // ConstantDb::get is called during analysis/lowering.
        if let Some(archive_id) = workspace.text_archive_for_script_file(&source_stem) {
            workspace.ensure_archive_loaded(archive_id).map_err(|e| {
                CompileFailure::with_source(
                    input,
                    &rotom_source,
                    CompileError::Io {
                        message: format!("failed to load text archive {archive_id}: {e}"),
                    },
                )
            })?;
        }

        if extension == "rotom" && load_file_constants {
            let lexer = compiler::Lexer::new(&rotom_source);
            let mut parser = compiler::Parser::new(lexer);
            let file = parser
                .parse_script_file()
                .map_err(|e| CompileFailure::with_source(input, &rotom_source, e))?;

            let mut cloned = constants.clone();
            let script_dir = input.parent().unwrap_or_else(|| Path::new("."));
            cloned
                .apply_directives(script_dir, &rotom_source, &file.items)
                .map_err(|e| CompileFailure::with_path(input, e))?;

            let output =
                emit_script_file(&file, db, &cloned, binary_quirks, workspace, &source_stem)
                    .map_err(|e| CompileFailure::with_source(input, &rotom_source, e))?;
            (output.bytes, output.warnings, rotom_source)
        } else {
            let file_constants = if load_file_constants {
                let mut cloned = constants.clone();
                cloned
                    .load_script_constants(input)
                    .map_err(|e| CompileFailure::with_path(input, e))?;
                Some(cloned)
            } else {
                None
            };
            let constants = file_constants.as_ref().unwrap_or(constants);

            let file = {
                let lexer = compiler::Lexer::new(&rotom_source);
                let mut parser = compiler::Parser::new(lexer);
                parser
                    .parse_script_file()
                    .map_err(|e| CompileFailure::with_source(input, &rotom_source, e))?
            };
            let output =
                emit_script_file(&file, db, constants, binary_quirks, workspace, &source_stem)
                    .map_err(|e| {
                        if extension.as_str() == "s" {
                            let n = count_jump_table_lines(&source);
                            if n > 0 {
                                let (tm, om) =
                                    (SourceMap::new(&rotom_source), SourceMap::new(&source));
                                let shift = |b: usize| {
                                    let p = tm.byte_to_position(b);
                                    om.position_to_byte(Position {
                                        line: p.line + n as u32,
                                        character: p.character,
                                    })
                                };
                                let e = match e {
                                    CompileError::Parse { span, message } => CompileError::Parse {
                                        span: shift(span.start)..shift(span.end),
                                        message,
                                    },
                                    CompileError::Analysis { span, message } => {
                                        CompileError::Analysis {
                                            span: shift(span.start)..shift(span.end),
                                            message,
                                        }
                                    }
                                    e => e,
                                };
                                return CompileFailure::with_source(input, &source, e);
                            }
                        }
                        CompileFailure::with_source(input, &rotom_source, e)
                    })?;
            (output.bytes, output.warnings, rotom_source)
        }
    };
    let size = bytes.len();

    write_compiled_bytes(input, output, &bytes)?;

    Ok(CompiledFile {
        input: input.to_path_buf(),
        output: output.to_path_buf(),
        size,
        source: warning_source,
        warnings,
    })
}

fn write_compiled_bytes(input: &Path, output: &Path, bytes: &[u8]) -> Result<(), CompileFailure> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            CompileFailure::io_error(
                input,
                format!(
                    "Failed to create output directory '{}': {}",
                    parent.display(),
                    e
                ),
            )
        })?;
    }

    std::fs::write(output, bytes).map_err(|e| {
        CompileFailure::io_error(
            input,
            format!("Failed to write output file '{}': {}", output.display(), e),
        )
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

/// Compile a file or directory of scripts to binary.
///
/// Supports `.rotom`, `.script`, `.s`, and `.json` inputs. When `input` is a
/// directory, all supported files inside it are compiled in parallel.
///
/// # Arguments
/// * `input` - Path to a single file or a directory of source files
/// * `output` - Output file path (for single file) or output directory
/// * `db` - The command database
/// * `constants` - The constant database
///
/// # Errors
/// Returns an error when the input path does not exist, when the output is
/// not a directory for directory-mode compilation, or when output path
/// collisions are detected.
#[allow(clippy::too_many_lines)]
pub fn compile_path(
    input: &Path,
    output: &Path,
    db: &DatabaseV2,
    constants: &ConstantDb,
    workspace: &Arc<uxie::Workspace>,
) -> Result<BatchCompileResult, CompileError> {
    if input.is_file() {
        let output_path = if output.is_dir() {
            generate_output_path_compile(input, output)
        } else {
            output.to_path_buf()
        };

        let result = compile_file_internal(
            input,
            &output_path,
            db,
            constants,
            true,
            BinaryQuirk::default(),
            workspace,
        );
        workspace
            .flush_pending_messages()
            .map_err(|e| CompileError::Io {
                message: format!("Failed to flush text archives: {e}"),
            })?;
        Ok(match result {
            Ok(success) => BatchCompileResult {
                successes: vec![success],
                failures: vec![],
            },
            Err(e) => BatchCompileResult {
                successes: vec![],
                failures: vec![e],
            },
        })
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

        let work: Vec<_> = files
            .into_iter()
            .map(|f| crate::compiler::batch_compile::CompileWorkItem {
                input: f.clone(),
                output: generate_output_path_compile(&f, output),
                quirks: BinaryQuirk::default(),
            })
            .collect();

        let result = compile_batch(&work, db, constants, true, None, workspace);
        workspace
            .flush_pending_messages()
            .map_err(|e| CompileError::Io {
                message: format!("Failed to flush text archives: {e}"),
            })?;
        Ok(result)
    } else {
        Err(CompileError::Io {
            message: format!("Input path does not exist: {}", input.display()),
        })
    }
}

/// Writes decompiled text for a binary already disassembled into [`ScriptOutput`] (quirks,
/// [`ir_to_source`], output path stem, disk write—all aligned with [`decompile_file_internal`]).
///
/// `input` is the binary path used for diagnostics and for [`DecompileFileResult::input`].
pub(crate) fn decompile_file_from_ir(
    input: &Path,
    script_output: &ScriptOutput,
    output_file: Option<&Path>,
    output_dir: Option<&Path>,
    db: &DatabaseV2,
    constants: Option<&ConstantDb>,
) -> Result<DecompileFileResult, DecompileFailure> {
    let mut quirks = BinaryQuirk::default();
    match &script_output {
        ScriptOutput::Normal {
            jump_table_end_marker_count,
            ..
        } if *jump_table_end_marker_count != 1 => {
            quirks.jump_table_end_marker_count = Some(*jump_table_end_marker_count);
        }
        ScriptOutput::Levelscript(ls) if ls.padding > 0 => {
            quirks.levelscript_padding = Some(ls.padding);
        }
        _ => {}
    }

    let is_levelscript = matches!(&script_output, ScriptOutput::Levelscript(_));

    let output_path = resolve_decompile_output_path(input, output_file, output_dir, is_levelscript);

    let source_text = ir_to_source(script_output, db, constants);
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

fn decompile_file_internal(
    input: &Path,
    output_file: Option<&Path>,
    output_dir: Option<&Path>,
    db: &DatabaseV2,
    constants: Option<&ConstantDb>,
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

    decompile_file_from_ir(
        input,
        &script_output,
        output_file,
        output_dir,
        db,
        constants,
    )
}

/// Decompile a file or directory of binary scripts to Rotoscript source.
///
/// When `input` is a directory, all `.bin` files (and extensionless binaries)
/// inside it are decompiled in parallel.
///
/// # Arguments
/// * `input` - Path to a single `.bin` file or a directory
/// * `output` - Output file path (for single file) or output directory
/// * `db` - The command database
/// * `constants` - Optional constant database for symbolic argument resolution
/// * `progress` - Optional progress tracker for batch decompilation
///
/// # Errors
/// Returns an error when the input path does not exist or when no `.bin` files
/// are found in directory mode.
pub fn decompile_path(
    input: &Path,
    output: &Path,
    db: &DatabaseV2,
    constants: Option<&ConstantDb>,
    progress: Option<&crate::progress::CompileProgress>,
) -> Result<BatchDecompileResult, DecompileError> {
    if input.is_file() {
        let (output_file, output_dir) = if output.is_dir() {
            (None, Some(output))
        } else {
            (Some(output), None)
        };

        match decompile_file_internal(input, output_file, output_dir, db, constants) {
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
            .map(|input_file| {
                let result = decompile_file_internal(input_file, None, Some(output), db, constants);
                match &result {
                    Ok(_) => {
                        if let Some(p) = progress {
                            p.inc_completed();
                        }
                    }
                    Err(_) => {
                        if let Some(p) = progress {
                            p.inc_failed();
                        }
                    }
                }
                result
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

#[cfg(test)]
mod warning_tests {
    use super::*;

    #[test]
    fn batch_compile_result_serializes_warnings() {
        let warning = CompileWarning::UnusedAlias {
            name: "foo".to_string(),
            span: 0..14,
        };
        let result = BatchCompileResult {
            successes: vec![CompiledFile {
                input: PathBuf::from("script.rotom"),
                output: PathBuf::from("script.bin"),
                size: 0,
                source: "alias 1 as foo".to_string(),
                warnings: vec![warning],
            }],
            failures: Vec::new(),
        };

        let json = serde_json::to_string(&result).unwrap();

        assert!(json.contains("\"warnings\""));
        assert!(json.contains("UnusedAlias"));
        assert!(json.contains("foo"));
        assert!(!json.contains("alias 1 as foo"));
    }
}

#[cfg(test)]
mod tests {
    use crate::BinaryQuirk;

    use super::{
        ConstantDb, DatabaseV2, compile, compile_path, decompile_path,
        detect_compile_output_collisions, resolve_decompile_output_path,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(name: &str) -> PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before UNIX_EPOCH")
            .as_nanos();
        std::env::temp_dir().join(format!("rotom_{}_{}_{}", name, std::process::id(), now))
    }

    fn load_test_db() -> &'static DatabaseV2 {
        DatabaseV2::test_platinum()
    }

    fn minimal_script_source() -> &'static str {
        "script Main #1:\nEnd\n"
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

        let result = compile_path(
            &input_path,
            &output_path,
            db,
            &constants,
            &Arc::new(uxie::Workspace::new(
                std::path::PathBuf::new(),
                uxie::game::Game::Platinum,
            )),
        )
        .expect("compile_path should return a batch result");
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

        let result = compile_path(
            &input_dir,
            &output_file,
            db,
            &constants,
            &Arc::new(uxie::Workspace::new(
                std::path::PathBuf::new(),
                uxie::game::Game::Platinum,
            )),
        );
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
        let bytes = compile(
            minimal_script_source(),
            db,
            &constants,
            BinaryQuirk::default(),
        )
        .expect("failed to compile seed script")
        .bytes;

        let input_path = temp_dir.join("input.bin");
        let output_path = temp_dir.join("custom_out.rotom");
        fs::write(&input_path, bytes).expect("failed to write input binary");

        let result = decompile_path(&input_path, &output_path, db, None, None)
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
        let bytes = compile(
            minimal_script_source(),
            db,
            &constants,
            BinaryQuirk::default(),
        )
        .expect("failed to compile seed script")
        .bytes;

        fs::write(input_dir.join("0000"), bytes).expect("failed to write extensionless binary");

        let result = decompile_path(&input_dir, &output_dir, db, None, None)
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
script Main #1:
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

        let result = compile_path(
            &input_path,
            &output_path,
            db,
            &constants,
            &Arc::new(uxie::Workspace::new(
                std::path::PathBuf::new(),
                uxie::game::Game::Platinum,
            )),
        )
        .expect("compile_path should return a batch result");
        assert!(result.is_success(), "compile_path should succeed");

        let compiled = fs::read(&output_path).expect("failed to read compiled output");
        let expected = compile(
            "script Main #1:\n    Message 7\n    Message 7\n    End\n",
            db,
            &ConstantDb::new(),
            BinaryQuirk::default(),
        )
        .expect("expected source should compile")
        .bytes;
        fs::remove_dir_all(&temp_dir).ok();

        assert_eq!(compiled, expected);
    }

    #[test]
    fn compile_to_bytes_supports_symbolic_alias_chains() {
        let db = load_test_db();
        let constants = ConstantDb::new();

        let compiled = compile(
            "alias 7 as FOO
alias FOO as BAR

script Main #1:
    Message BAR
    End
",
            db,
            &constants,
            BinaryQuirk::default(),
        )
        .expect("symbolic alias chain should compile")
        .bytes;

        let expected = compile(
            "script Main #1:\n    Message 7\n    End\n",
            db,
            &ConstantDb::new(),
            BinaryQuirk::default(),
        )
        .expect("expected source should compile")
        .bytes;

        assert_eq!(compiled, expected);
    }

    #[test]
    fn compile_to_bytes_supports_aliases_from_earlier_function_bodies() {
        let db = load_test_db();
        let constants = ConstantDb::new();

        let compiled = compile(
            "script DefineAlias #1:
    alias 7 as SHARED
    End

script Main #2:
    Message SHARED
    End
",
            db,
            &constants,
            BinaryQuirk::default(),
        )
        .expect("alias from earlier script body should compile")
        .bytes;

        let expected = compile(
            "script DefineAlias #1:\n    End\n\nscript Main #2:\n    Message 7\n    End\n",
            db,
            &ConstantDb::new(),
            BinaryQuirk::default(),
        )
        .expect("expected source should compile")
        .bytes;

        assert_eq!(compiled, expected);
    }

    #[test]
    fn compile_to_bytes_supports_top_level_alias_redefinition_in_source_order() {
        let db = load_test_db();
        let constants = ConstantDb::new();

        let compiled = compile(
            "alias 7 as VALUE
script First #1:
    Message VALUE
    End

alias 9 as VALUE
script Second #2:
    Message VALUE
    End
",
            db,
            &constants,
            BinaryQuirk::default(),
        )
        .expect("top-level alias redefinition should compile")
        .bytes;

        let expected = compile(
            "script First #1:\n    Message 7\n    End\n\nscript Second #2:\n    Message 9\n    End\n",
            db,
            &ConstantDb::new(),
            BinaryQuirk::default(),
        )
        .expect("expected source should compile")
        .bytes;

        assert_eq!(compiled, expected);
    }

    #[test]
    fn compile_to_bytes_rejects_forward_alias_reference_in_source_order() {
        let db = load_test_db();
        let constants = ConstantDb::new();

        let result = compile(
            "script Main #1:
    Message SHARED
    End

alias 7 as SHARED
",
            db,
            &constants,
            BinaryQuirk::default(),
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
script Main #1:
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

        let result = compile_path(
            &input_path,
            &output_path,
            db,
            &constants,
            &Arc::new(uxie::Workspace::new(
                std::path::PathBuf::new(),
                uxie::game::Game::Platinum,
            )),
        )
        .expect("compile_path should return a batch result");
        assert!(result.is_success(), "compile_path should succeed");

        let compiled = fs::read(&output_path).expect("failed to read compiled output");
        let expected = compile(
            "script Main #1:\n    Message 7\n    End\n",
            db,
            &ConstantDb::new(),
            BinaryQuirk::default(),
        )
        .expect("expected source should compile")
        .bytes;
        fs::remove_dir_all(&temp_dir).ok();

        assert_eq!(compiled, expected);
    }

    #[test]
    fn compile_path_rotom_reports_text_archive_preload_errors() {
        let temp_dir = unique_temp_dir("compile_path_reports_text_archive_preload_errors");
        write_test_decomp_project(&temp_dir);

        // Numeric stem "1" maps to text archive ID 17 via Platinum's hardcoded
        // global script table (min_script_id=2500, script_file_id=1, text_archive_id=17).
        // That archive doesn't exist in the minimal fixture, so preload fails.
        let input_path = temp_dir.join("1.rotom");
        let output_path = temp_dir.join("1.bin");
        fs::write(&input_path, minimal_script_source()).expect("failed to write input script");

        let db = load_test_db();
        let mut constants = ConstantDb::new();
        let workspace = Arc::new(
            uxie::Workspace::open_decomp(&temp_dir).expect("failed to open decomp workspace"),
        );
        constants.set_message_ids(workspace.shared_message_ids());

        let result = compile_path(&input_path, &output_path, db, &constants, &workspace)
            .expect("compile_path should return a batch result");
        fs::remove_dir_all(&temp_dir).ok();

        assert_eq!(result.successes.len(), 0);
        assert_eq!(result.failures.len(), 1);
        assert!(
            result.failures[0]
                .error
                .to_string()
                .contains("failed to load text archive"),
            "unexpected error: {}",
            result.failures[0].error
        );
    }

    #[test]
    fn compile_path_rotom_reports_unresolved_includes() {
        let temp_dir = unique_temp_dir("compile_path_rotom_reports_unresolved_includes");
        write_test_decomp_project(&temp_dir);

        let input_path = temp_dir.join("res/field/scripts/test.rotom");
        fs::write(
            &input_path,
            r#"#include "constants/missing.h"
script Main #1:
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

        let result = compile_path(
            &input_path,
            &output_path,
            db,
            &constants,
            &Arc::new(uxie::Workspace::new(
                std::path::PathBuf::new(),
                uxie::game::Game::Platinum,
            )),
        )
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
