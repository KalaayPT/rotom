use std::path::PathBuf;

use clap::{Parser as ClapParser, Subcommand};

mod compiler;
mod database;
mod transpiler;

use compiler::parse_error::{CompileError, print_error};
use compiler::{Analyzer, Lexer, Lowerer, Parser, StatementKind};
use database::{ConstantDb, DatabaseV2};

use crate::compiler::codegen::Emitter;
use crate::compiler::ir;

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

        /// Input .rotom script file
        #[arg(short, long)]
        input: PathBuf,

        /// Output binary file (defaults to input with .bin extension)
        #[arg(short, long)]
        output: Option<PathBuf>,
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

    /// Run a test compilation (for development)
    Test {
        /// Path to the V2 database JSON file
        #[arg(short, long)]
        database: PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Compile {
            database,
            input,
            output,
        } => {
            if let Err(e) = compile(&database, &input, output.as_ref()) {
                eprintln!("Compilation failed: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Decompile {
            database,
            input,
            output,
        } => {
            eprintln!("Decompilation not yet implemented");
            std::process::exit(1);
        }
        Commands::Test { database } => {
            run_test(&database);
        }
    }
}

fn compile(
    db_path: &PathBuf,
    input: &PathBuf,
    _output: Option<&PathBuf>,
) -> Result<(), CompileError> {
    println!("Loading database from: {}", db_path.display());
    let db = DatabaseV2::load(db_path)?;
    println!(
        "Loaded {} commands for {}",
        db.commands.len(),
        db.meta.version
    );

    // Load constants from the database
    let mut constants = ConstantDb::new();
    let const_count = constants.load_from_db(&db);
    println!("Loaded {} built-in constants", const_count);

    println!("\nReading script from: {}", input.display());
    let source = std::fs::read_to_string(input).map_err(|e| CompileError::Io {
        message: format!("Failed to read input file '{}': {}", input.display(), e),
    })?;

    println!("Parsing...");
    let lexer = Lexer::new(&source);
    let mut parser = Parser::new(lexer);
    let file = parser.parse_script_file()?;
    println!(
        "Parsed: {} aliases, {} functions, {} actions",
        file.aliases.len(),
        file.functions.len(),
        file.actions.len()
    );

    println!("Analyzing...");
    let mut analyzer = Analyzer::with_constants(&constants);
    analyzer.analyze(&file)?;
    println!("Analysis passed!");

    println!("\nLowering to IR...");

    let mut lowerer = Lowerer::new(&analyzer.symbols);
    let (ir_functions, ir_actions) = lowerer.lower_script_file(&file)?;

    let mut emitter = Emitter::new(&db);
    let byte_output = emitter.emit_script_file(&ir_functions, &ir_actions)?;
    println!("Output: {:?}", byte_output);

    // println!("Codegen not yet implemented - stopping at IR");
    Ok(())
}

fn run_test(db_path: &PathBuf) {
    let _old_input = r#"
// === Global Aliases ===
alias 0x800C as RESULT
alias 0x8000 as PLAYER_X
alias 0x8001 as COUNTER

// === Main Entry Point ===
function Main #1:
    // 1. Simple command
    SetVar PLAYER_X, 100

    // 2. Simple if/then/endif (variable vs literal)
    if PLAYER_X == 100 then
        Message 1
    endif

    // 3. If/else
    if RESULT != 0 then
        Message 2
    else
        Message 3
    endif

    // 4. Nested if with else
    if PLAYER_X == 100 then
        if RESULT == 1 then
            Message 10
        else
            Jump .skip_message
        endif
    endif

    // 5. While loop
    SetVar COUNTER, 5
    while COUNTER != 0 do
        SubVar COUNTER, 1
    endwhile

    // 6. Jump to local label
    Jump .end_script

    .skip_message:
    Message 99

    .end_script:
    PlaySound 0x10
End

// === Helper Label (uses Return) ===
HelperFunc:
    Message 50
Return

// === Action (movement only, no control flow) ===
action WalkPattern
    WalkNormalEast 3
    WalkNormalSouth 2
    FaceNorth
EndMovement
"#;

    // DSPRE-derived test script (0013)
    let dspre = std::fs::read_to_string(r"C:\dev\romhacking\renHERgade platinum\renherplat v007_DSPRE_contents\expanded\scripts\0013.script").unwrap();
    let input = transpiler::DSPRE::transpile(&dspre);
    println!("{input}");
    let input = input.as_str();
    println!("=== Loading Database ===");
    let db = match DatabaseV2::load(db_path) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("Failed to load database: {}", e);
            return;
        }
    };
    println!(
        "Loaded {} commands for {}",
        db.commands.len(),
        db.meta.version
    );

    // Load constants from the database
    let mut constants = ConstantDb::new();
    let const_count = constants.load_from_db(&db);
    println!("Loaded {} built-in constants", const_count);

    // Quick sanity check - look up a few commands
    println!("\n=== Database Sanity Check ===");
    for cmd_name in &["End", "SetVar", "Message", "Jump"] {
        if let Ok(cmd) = db.get_script_cmd(cmd_name) {
            println!(
                "  {} (id: {:?}, params: {})",
                cmd_name,
                cmd.id,
                cmd.params.len()
            );
        } else {
            println!("  {} NOT FOUND", cmd_name);
        }
    }

    println!("\n=== Parsing ===");
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let file = match parser.parse_script_file() {
        Ok(f) => f,
        Err(e) => {
            print_error("<test>", input, &e);
            return;
        }
    };
    println!(
        "Parsed: {} aliases, {} functions, {} actions",
        file.aliases.len(),
        file.functions.len(),
        file.actions.len()
    );

    println!("\n=== Analyzing ===");
    let mut analyzer = Analyzer::with_constants(&constants);
    if let Err(e) = analyzer.analyze(&file) {
        print_error("<test>", input, &e);
        return;
    }
    println!("Analysis passed!");

    println!("\n=== Lowering Functions to IR ===");

    // println!("{:#?}", file);
    let mut lowerer = Lowerer::new(&analyzer.symbols);
    let (ir_functions, ir_actions) = match lowerer.lower_script_file(&file) {
        Ok(result) => result,
        Err(e) => {
            print_error("<test>", input, &e);
            return;
        }
    };
    for ir_func in &ir_functions {
        println!("{}", ir_func);
    }
    for ir_action in &ir_actions {
        println!("{}", ir_action);
    }

    let pub_funcs = file
        .functions
        .iter()
        .filter(|f| {
            if let StatementKind::Function { headers, .. } = &f.node {
                headers.iter().any(|h| h.is_public)
            } else {
                false
            }
        })
        .count();
    println!("Public functions found: {}", pub_funcs);

    let mut emitter = Emitter::new(&db);
    let byte_output = match emitter.emit_script_file(&ir_functions, &ir_actions) {
        Ok(output) => output,
        Err(e) => {
            print_error("<test>", input, &e);
            return;
        }
    };
    println!("Output: {:?}", byte_output);
    std::fs::write("test_output.bin", &byte_output).unwrap();
}
