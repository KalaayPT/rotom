use std::path::PathBuf;

use clap::{Parser as ClapParser, Subcommand};

mod compiler;
mod database;

use compiler::parse_error::CompileError;
use compiler::{Analyzer, IrFunction, Lexer, Lowerer, Parser, StatementKind};
use database::DatabaseV2;

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

fn compile(db_path: &PathBuf, input: &PathBuf, _output: Option<&PathBuf>) -> Result<(), CompileError> {
    println!("Loading database from: {}", db_path.display());
    let db = DatabaseV2::load(db_path)?;
    println!(
        "Loaded {} commands for {}",
        db.commands.len(),
        db.meta.version
    );

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
    let mut analyzer = Analyzer::new();
    analyzer.analyze(&file)?;
    println!("Analysis passed!");

    println!("\nLowering to IR...");
    for func in &file.functions {
        if let StatementKind::Function { headers, body } = &func.node {
            let func_name = headers
                .first()
                .map(|h| h.name.clone())
                .unwrap_or_else(|| "unnamed".to_string());

            let lowerer = Lowerer::new(&analyzer.symbols);
            let ir_ops = lowerer.lower_function(body)?;

            let ir_func = IrFunction {
                name: func_name,
                instructions: ir_ops,
            };
            println!("{}", ir_func);
        }
    }

    println!("Codegen not yet implemented - stopping at IR");
    Ok(())
}

fn run_test(db_path: &PathBuf) {
    let input = r#"
// === Global Aliases ===
global alias 0x800C as RESULT
global alias 0x8000 as PLAYER_X
global alias 0x8001 as COUNTER

// === Main Entry Point ===
public function Main #1
    // Local alias (shadows nothing, just for demo)
    alias 0x8002 as LOCAL_VAR

    // 1. Simple command
    SetVar PLAYER_X, 100

    // 2. Command with arithmetic expression
    SetVar LOCAL_VAR, 1 + 2 * 3

    // 3. Simple if/then/endif (variable vs literal)
    if PLAYER_X == 100 then
        Message 1
    endif

    // 4. If/else
    if RESULT != 0 then
        Message 2
    else
        Message 3
    endif

    // 5. Nested if with else
    if PLAYER_X == 100 then
        if RESULT == 1 then
            Message 10
        else
            Jump .skip_message
        endif
    endif

    // 6. While loop
    SetVar COUNTER, 5
    while COUNTER != 0 do
        SubVar COUNTER, 1
    endwhile

    // 7. Jump to local label
    Jump .end_script

    .skip_message:
    Message 99

    .end_script:
    PlaySound 0x10
End

// === Helper Function (private, uses Return) ===
function HelperFunc
    Message 50
Return

// === Action (movement only, no control flow) ===
action WalkPattern
    WalkRight 3
    WalkDown 2
    FaceUp
EndMovement
"#;

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

    // Quick sanity check - look up a few commands
    println!("\n=== Database Sanity Check ===");
    for cmd_name in &["End", "SetVar", "Message", "Jump"] {
        if let Some(cmd) = db.get_script_cmd(cmd_name) {
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
            eprintln!("Parse error: {:?}", e);
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
    let mut analyzer = Analyzer::new();
    if let Err(e) = analyzer.analyze(&file) {
        eprintln!("Analysis error: {:?}", e);
        return;
    }
    println!("Analysis passed!");

    println!("\n=== Lowering Functions to IR ===");
    for func in &file.functions {
        if let StatementKind::Function { headers, body } = &func.node {
            let func_name = headers
                .first()
                .map(|h| h.name.clone())
                .unwrap_or_else(|| "unnamed".to_string());

            let lowerer = Lowerer::new(&analyzer.symbols);
            let ir_ops = match lowerer.lower_function(body) {
                Ok(ops) => ops,
                Err(e) => {
                    eprintln!("Lowering error in {}: {:?}", func_name, e);
                    return;
                }
            };

            let ir_func = IrFunction {
                name: func_name,
                instructions: ir_ops,
            };
            println!("{}", ir_func);
        }
    }

    println!("=== Actions (no lowering needed - 1:1 with bytecode) ===");
    for action in &file.actions {
        if let StatementKind::Action { name, body } = &action.node {
            println!("Action: {}", name);
            for stmt in body {
                if let StatementKind::ScriptCommand { command, args } = &stmt.node {
                    println!("    {} ({} args)", command, args.len());
                } else if let StatementKind::End = &stmt.node {
                    println!("    EndMovement");
                }
            }
            println!();
        }
    }
}
