use std::path::PathBuf;

use clap::{Parser as ClapParser, Subcommand};

// Use the library crate
use rotom::compile_path;
use rotom::compiler::parse_error::{CompileError, print_error};
use rotom::database::{ConstantDb, DatabaseV2, GameFamilyExt, game_family_from_hint};
use rotom::decompile_path;
use rotom::project::command::{
    compile_mode as compile_project_mode, convert_mode, decompile_mode as decompile_project_mode,
    init_mode,
};
use rotom::project::error::ProjectError;

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
        database: Option<PathBuf>,

        /// Input .rotom script file or directory containing .rotom files
        #[arg(short, long)]
        input: Option<PathBuf>,

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

        /// Rebuild all project files, ignoring any future incremental state
        #[arg(long)]
        force: bool,
    },

    /// Decompile a binary script to .rotom source
    Decompile {
        /// Path to the V2 database JSON file
        #[arg(short, long)]
        database: Option<PathBuf>,

        /// Input binary script file
        #[arg(short, long)]
        input: Option<PathBuf>,

        /// Output .rotom file (defaults to input with .rotom extension)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    Init {
        /// Project root (defaults to current directory)
        #[arg(value_name = "ROOT")]
        root: Option<PathBuf>,

        /// Skip any interactive prompts
        #[arg(long)]
        non_interactive: bool,
    },

    Convert {
        /// Project root (defaults to current directory or nearest rotom.toml ancestor)
        #[arg(value_name = "ROOT")]
        root: Option<PathBuf>,

        /// Preview conversions without writing files
        #[arg(long)]
        dry_run: bool,
    },
}

fn main() {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Compile {
            database,
            input,
            output,
            decomp_root,
            json,
            force,
        } => handle_compile_command(
            database.as_deref(),
            input.as_deref(),
            output.as_deref(),
            decomp_root.as_deref(),
            *json,
            *force,
        ),
        Commands::Decompile {
            database,
            input,
            output,
        } => handle_decompile_command(database.as_deref(), input.as_deref(), output.as_deref()),
        Commands::Init {
            root,
            non_interactive,
        } => handle_init_command(root.as_deref(), *non_interactive),
        Commands::Convert { root, dry_run } => handle_convert_command(root.as_deref(), *dry_run),
    }
}

fn handle_compile_command(
    database: Option<&std::path::Path>,
    input: Option<&std::path::Path>,
    output: Option<&std::path::Path>,
    decomp_root: Option<&std::path::Path>,
    json: bool,
    force: bool,
) {
    let result = if database.is_none() && input.is_none() {
        if output.is_some() || decomp_root.is_some() {
            Err(ProjectError::UnsupportedProjectCompileArgs)
        } else {
            compile_project_mode(force).and_then(|result| {
                report_compile_result(&result, json).map_err(ProjectError::from)?;
                if result.is_success() {
                    Ok(())
                } else {
                    Err(ProjectError::CompileFailures(result.failures.len()))
                }
            })
        }
    } else {
        let database = database.ok_or(ProjectError::MissingCompileArgs);
        let input = input.ok_or(ProjectError::MissingCompileArgs);
        match (database, input) {
            (Ok(database), Ok(input)) => compile(database, input, output, decomp_root, json)
                .map_err(ProjectError::from),
            (Err(error), _) | (_, Err(error)) => Err(error),
        }
    };

    if let Err(error) = result {
        if !json {
            eprintln!("Compilation failed: {}", error);
        }
        std::process::exit(1);
    }
}

fn handle_decompile_command(
    database: Option<&std::path::Path>,
    input: Option<&std::path::Path>,
    output: Option<&std::path::Path>,
) {
    let result = if database.is_none() && input.is_none() {
        if output.is_some() {
            Err(ProjectError::UnsupportedProjectDecompileArgs)
        } else {
            decompile_project_mode().and_then(|result| {
                report_decompile_result(&result);
                if result.is_success() {
                    Ok(())
                } else {
                    Err(ProjectError::DecompileFailures(result.failures.len()))
                }
            })
        }
    } else {
        let database = database.ok_or(ProjectError::MissingDecompileArgs);
        let input = input.ok_or(ProjectError::MissingDecompileArgs);
        match (database, input) {
            (Ok(database), Ok(input)) => {
                decompile(database, input, output).map_err(ProjectError::from)
            }
            (Err(error), _) | (_, Err(error)) => Err(error),
        }
    };

    if let Err(error) = result {
        eprintln!("Decompilation failed: {}", error);
        std::process::exit(1);
    }
}

fn handle_init_command(root: Option<&std::path::Path>, non_interactive: bool) {
    match init_mode(root, non_interactive) {
        Ok(report) => {
            println!("Init successful.");

            if report.used_embedded_database {
                println!(
                    "Used baked-in database snapshot because the latest release download was unavailable."
                );
            }

            if !report.reused_paths.is_empty() {
                println!("Reused existing: {}.", report.reused_paths.join(", "));
            }

            if report.converted_files > 0 {
                println!("Converted {} file(s) to .rotom.", report.converted_files);
            } else if report.convertible_files_detected > 0 {
                println!(
                    "Detected {} convertible file(s). Run `rotom convert` to convert them.",
                    report.convertible_files_detected
                );
            }
        }
        Err(error) => {
            eprintln!("Init failed: {}", error);
            std::process::exit(1);
        }
    }
}

fn handle_convert_command(root: Option<&std::path::Path>, dry_run: bool) {
    match convert_mode(root, dry_run) {
        Ok(report) => {
            if dry_run {
                println!("Planned {} conversion(s):", report.plans.len());
                for plan in &report.plans {
                    println!(
                        "  {} -> {} (backup: {})",
                        plan.input.display(),
                        plan.output.display(),
                        plan.backup.display()
                    );
                }
                return;
            }

            println!("Converted {} file(s).", report.converted);
            if let Some(backup_dir) = report.backup_dir {
                println!("Backups written to {}.", backup_dir.display());
            }
        }
        Err(error) => {
            eprintln!("Convert failed: {}", error);
            std::process::exit(1);
        }
    }
}

fn compile(
    db_path: &std::path::Path,
    input: &std::path::Path,
    output: Option<&std::path::Path>,
    decomp_root: Option<&std::path::Path>,
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
        if let Some(family) = game_family_from_hint(&db.meta.version) {
            println!("Detected game family: {}", family.display_name());
        }
    }

    let mut constants = ConstantDb::new();
    let const_count = constants.load_from_db(&db);

    if let Some(db_dir) = db_path.parent() {
        let dir_count = constants.load_directory(db_dir)?;
        if !json && dir_count > 0 {
            println!(
                "Loaded {} additional constants from {}",
                dir_count,
                db_dir.display()
            );
        }
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
    }

    // Determine output path
    let output_path = match output {
        Some(path) => path.to_path_buf(),
        None => {
            if input.is_dir() {
                // Default to same directory for directory input
                input.to_path_buf()
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

    report_compile_result(&result, json)?;

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
    db_path: &std::path::Path,
    input: &std::path::Path,
    output: Option<&std::path::Path>,
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
        Some(path) => path.to_path_buf(),
        None => {
            if input.is_dir() {
                input.to_path_buf()
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
    report_decompile_result(&result);

    if result.is_success() {
        Ok(())
    } else {
        Err(rotom::decompiler::decomp_error::DecompileError::Io {
            message: format!("{} file(s) failed to decompile", result.failures.len()),
        })
    }
}

fn report_compile_result(
    result: &rotom::BatchCompileResult,
    json: bool,
) -> Result<(), CompileError> {
    if json {
        let json_output =
            serde_json::to_string_pretty(result).map_err(|e| CompileError::Database {
                message: e.to_string(),
            })?;
        println!("{}", json_output);
        return Ok(());
    }

    for success in &result.successes {
        println!(
            "  ✓ {} -> {} ({} bytes)",
            success.input.display(),
            success.output.display(),
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
    Ok(())
}

fn report_decompile_result(result: &rotom::BatchDecompileResult) {
    for success in &result.successes {
        println!(
            "  ✓ {} -> {} ({} bytes)",
            success.input.display(),
            success.output.display(),
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
}
