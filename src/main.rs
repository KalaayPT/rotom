use std::path::PathBuf;

use clap::{Parser as ClapParser, Subcommand};

// Use the library crate
use rotom::compile_path;
use rotom::compiler::codegen::Emitter;
use rotom::compiler::parse_error::{CompileError, print_error};
use rotom::compiler::{Analyzer, Lexer, Lowerer, Parser, StatementKind};
use rotom::database::{ConstantDb, DatabaseV2, GameFamily};
use rotom::decompile_path;

#[derive(Debug, ClapParser)]
#[command(name = "rotom")]
#[command(version, about = "A Pokemon Gen 4 script compiler/decompiler", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Compile a .rotom script to binary
    Compile {
        /// Path to the V2 database JSON file
        #[arg(short, long)]
        database: PathBuf,

        /// Input .rotom script file or directory containing .rotom files
        #[arg(short, long)]
        input: PathBuf,

        /// Output binary file or directory (defaults to input path with .bin extension)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Path to a decomp project root (e.g., pokeplatinum).
        /// Loads constants from build/generated/ and include/constants/.
        /// Requires the project to have been built at least once.
        #[arg(long)]
        decomp_root: Option<PathBuf>,

        /// Output results as JSON to stdout (suppresses other output)
        #[arg(long)]
        json: bool,
    },

    /// Decompile a binary script to .rotom source
    Decompile {
        /// Path to the V2 database JSON file
        #[arg(short, long)]
        database: PathBuf,

        /// Input binary script file
        #[arg(short, long)]
        input: PathBuf,

        /// Output .rotom file (defaults to input with .rotom extension)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Compile {
            database,
            input,
            output,
            decomp_root,
            json,
        } => {
            if let Err(e) = compile(
                &database,
                &input,
                output.as_ref(),
                decomp_root.as_ref(),
                json,
            ) {
                if !json {
                    eprintln!("Compilation failed: {}", e);
                }
                std::process::exit(1);
            }
        }
        Commands::Decompile {
            database,
            input,
            output,
        } => {
            if let Err(e) = decompile(&database, &input, output.as_ref()) {
                eprintln!("Decompilation failed: {}", e);
                std::process::exit(1);
            }
        }
    }
}

fn compile(
    db_path: &PathBuf,
    input: &PathBuf,
    output: Option<&PathBuf>,
    decomp_root: Option<&PathBuf>,
    json: bool,
) -> Result<(), CompileError> {
    let start_total = std::time::Instant::now();
    if !json {
        println!("Loading database from: {}", db_path.display());
    }
    let db = DatabaseV2::load(db_path)?;
    if !json {
        println!(
            "Loaded {} commands for {}",
            db.commands.len(),
            db.meta.version
        );

        // Auto-detect game family from database version
        if let Some(family) = GameFamily::from_db_version(&db.meta.version) {
            println!("Detected game family: {}", family.as_str());
        }
    }

    let mut constants = ConstantDb::new();
    let const_count = constants.load_from_db(&db);

    if let Some(db_dir) = db_path.parent()
        && let Ok(dir_count) = constants.load_directory(db_dir)
        && !json
        && dir_count > 0
    {
        println!(
            "Loaded {} additional constants from {}",
            dir_count,
            db_dir.display()
        );
    }

    if !json {
        println!("Loaded {} built-in constants", const_count);
    }

    // Load constants from decomp project if specified
    if let Some(decomp) = decomp_root {
        if !json {
            println!(
                "\nLoading constants from decomp project: {}",
                decomp.display()
            );
        }
        let decomp_count = constants.load_decomp_project(decomp)?;
        if !json {
            println!("Loaded {} constants from decomp project", decomp_count);
        }

        // Load per-map event constants based on script filename (only for single file)
        if input.is_file() {
            let map_count = constants.load_map_events(decomp, input)?;
            if !json && map_count > 0 {
                println!("Loaded {} map-specific event constants", map_count);
            }
        }
    }

    // Determine output path
    let output_path = match output {
        Some(p) => p.clone(),
        None => {
            if input.is_dir() {
                // Default to same directory for directory input
                input.clone()
            } else {
                // Default to .bin extension for single file
                input.with_extension("bin")
            }
        }
    };

    if !json {
        println!("\nCompiling: {}", input.display());
        println!("Output to: {}", output_path.display());
    }

    let result = compile_path(input, &output_path, &db, &constants)?;
    if !json {
        println!("Total time: {}ms", start_total.elapsed().as_millis());
    }

    if json {
        let json_output =
            serde_json::to_string_pretty(&result).map_err(|e| CompileError::Database {
                message: e.to_string(),
            })?;
        println!("{}", json_output);
    } else {
        // Report results
        for success in &result.successes {
            println!(
                "  ✓ {} -> {} ({} bytes)",
                success
                    .input
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy(),
                success
                    .output
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy(),
                success.size
            );
        }

        for failure in &result.failures {
            let filename = failure
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            if failure.source.is_empty() {
                eprintln!("  ✗ {}: {}", filename, failure.error);
            } else {
                print_error(&filename, &failure.source, &failure.error);
            }
        }

        println!(
            "\nCompilation complete: {}/{} succeeded",
            result.successes.len(),
            result.total()
        );
    }

    if result.is_success() {
        Ok(())
    } else {
        // Even if we output JSON, we should probably exit with error code if something failed
        // But for JSON consumers, the JSON itself tells the story.
        // Let's keep the standard behavior: error code 1 if any failure.
        Err(CompileError::Io {
            message: format!("{} file(s) failed to compile", result.failures.len()),
        })
    }
}

fn decompile(
    db_path: &PathBuf,
    input: &PathBuf,
    output: Option<&PathBuf>,
) -> Result<(), rotom::decompiler::decomp_error::DecompileError> {
    println!("Loading database from: {}", db_path.display());
    let db = DatabaseV2::load(db_path).map_err(|e| {
        rotom::decompiler::decomp_error::DecompileError::Io {
            message: format!("Failed to load database: {}", e),
        }
    })?;
    println!(
        "Loaded {} commands for {}",
        db.commands.len(),
        db.meta.version
    );

    let output_path = match output {
        Some(p) => p.clone(),
        None => {
            if input.is_dir() {
                input.clone()
            } else {
                input
                    .parent()
                    .map(std::path::Path::to_path_buf)
                    .unwrap_or_default()
            }
        }
    };

    println!("\nDecompiling: {}", input.display());

    let result = decompile_path(input, &output_path, &db)?;

    // Report results
    for success in &result.successes {
        println!(
            "  ✓ {} -> {} ({} bytes)",
            success
                .input
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
            success
                .output
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
            success.size
        );
    }

    for failure in &result.failures {
        let filename = failure
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        eprintln!("  ✗ {}: {}", filename, failure.error);
    }

    println!(
        "\nDecompilation complete: {}/{} succeeded",
        result.successes.len(),
        result.total()
    );

    if result.is_success() {
        Ok(())
    } else {
        Err(rotom::decompiler::decomp_error::DecompileError::Io {
            message: format!("{} file(s) failed to decompile", result.failures.len()),
        })
    }
}
