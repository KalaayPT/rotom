use std::path::PathBuf;

use clap::{Parser as ClapParser, Subcommand};

// Use the library crate
use rotom::compiler::codegen::Emitter;
use rotom::compiler::parse_error::{CompileError, print_error};
use rotom::compiler::{Analyzer, Lexer, Lowerer, Parser, StatementKind};
use rotom::database::{ConstantDb, DatabaseV2, GameFamily};

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

        /// Path to a decomp project root (e.g., pokeplatinum).
        /// Loads constants from build/generated/ and include/constants/.
        /// Requires the project to have been built at least once.
        #[arg(long)]
        decomp_root: Option<PathBuf>,
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
            decomp_root,
        } => {
            if let Err(e) = compile(&database, &input, output.as_ref(), decomp_root.as_ref()) {
                eprintln!("Compilation failed: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Decompile {
            database: _database,
            input: _input,
            output: _output,
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
    decomp_root: Option<&PathBuf>,
) -> Result<(), CompileError> {
    println!("Loading database from: {}", db_path.display());
    let db = DatabaseV2::load(db_path)?;
    println!(
        "Loaded {} commands for {}",
        db.commands.len(),
        db.meta.version
    );

    // Auto-detect game family from database version
    if let Some(family) = GameFamily::from_db_version(&db.meta.version) {
        println!("Detected game family: {}", family.as_str());
    }

    // Load constants from the database
    let mut constants = ConstantDb::new();
    let const_count = constants.load_from_db(&db);
    println!("Loaded {} built-in constants", const_count);

    // Load constants from decomp project if specified
    if let Some(decomp) = decomp_root {
        println!(
            "\nLoading constants from decomp project: {}",
            decomp.display()
        );
        let decomp_count = constants.load_decomp_project(decomp)?;
        println!("Loaded {} constants from decomp project", decomp_count);

        // Load per-map event constants based on script filename
        let map_count = constants.load_map_events(decomp, input)?;
        if map_count > 0 {
            println!("Loaded {} map-specific event constants", map_count);
        }
    }

    println!("\nReading script from: {}", input.display());
    let source = std::fs::read_to_string(input).map_err(|e| CompileError::Io {
        message: format!("Failed to read input file '{}': {}", input.display(), e),
    })?;

    println!("Parsing...");
    let lexer = Lexer::new(&source);
    let mut parser = Parser::new(lexer);
    let file = parser.parse_script_file()?;
    let func_count = file
        .items
        .iter()
        .filter(|s| matches!(s.node, StatementKind::Function { .. }))
        .count();
    let action_count = file
        .items
        .iter()
        .filter(|s| matches!(s.node, StatementKind::Action { .. }))
        .count();
    println!(
        "Parsed: {} aliases, {} functions, {} actions",
        file.aliases.len(),
        func_count,
        action_count
    );

    println!("Analyzing...");
    let mut analyzer = Analyzer::with_constants(&constants);
    analyzer.analyze(&file)?;
    println!("Analysis passed!");

    println!("\nLowering to IR...");

    let mut lowerer = Lowerer::new(&analyzer.symbols, &db);
    let items = lowerer.lower_script_file(&file)?;

    let mut emitter = Emitter::new(&db);
    let byte_output = emitter.emit_script_file(&items)?;
    println!("Output: {:?}", byte_output);

    // println!("Codegen not yet implemented - stopping at IR");
    Ok(())
}

fn run_test(db_path: &PathBuf) {
    let start = std::time::Instant::now();
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
    // load db
    let db = match DatabaseV2::load(db_path) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("Failed to load database: {}", e);
            return;
        }
    };
    
    // DSPRE-derived test script (0013)
    let dspre = std::fs::read_to_string(r"C:\Users\micro\Desktop\Pokémon - Platinum Version (USA) (Rev 1)_DSPRE_contents\expanded\scripts\0002.script").unwrap();
    let decomp =
        std::fs::read_to_string(r"C:\dev\pokeplatinum\res\field\scripts\scripts_unk_0406.s")
            .unwrap();
    let input = rotom::transpiler::DSPRE::transpile(&dspre);
    let input = rotom::transpiler::decomp::transpile(&decomp, Some(&db));
    println!("{input}");
    std::fs::write("test_input.rotom", &input).unwrap();
    let input = input.as_str();
    println!("=== Loading Database ===");

    println!(
        "Loaded {} commands for {}",
        db.commands.len(),
        db.meta.version
    );



    // Load constants from the database
    let mut constants = ConstantDb::new();
    let const_count = constants.load_from_db(&db);

    // Load additional constants from JSON files in src/db/
    let db_dir = std::path::Path::new("src/db");
    for file in &["items.json", "pokemon.json", "moves.json", "trainers.json"] {
        let path = db_dir.join(file);
        if path.exists() {
            match constants.load_json(&path) {
                Ok(count) => println!("Loaded {} constants from {}", count, file),
                Err(e) => eprintln!("Warning: Failed to load {}: {}", file, e),
            }
        }
    }
    println!("Loaded {} built-in constants", const_count);

    // Load constants from pokeplatinum decomp project (hardcoded for testing)
    let decomp_root = std::path::Path::new(r"C:\dev\pokeplatinum");
    println!("\n=== Loading Decomp Constants ===");
    match constants.load_decomp_project(decomp_root) {
        Ok(count) => println!("Loaded {} constants from decomp project", count),
        Err(e) => {
            eprintln!("Failed to load decomp constants: {}", e);
            return;
        }
    }

    // Load per-map event constants for the jubilife city script
    let script_path = std::path::Path::new("scripts_common.s");
    match constants.load_map_events(decomp_root, script_path) {
        Ok(count) if count > 0 => println!("Loaded {} map-specific event constants", count),
        Ok(_) => println!("No map-specific event constants found"),
        Err(e) => eprintln!("Warning: Failed to load map events: {}", e),
    }

    // Auto-detect game family from database version
    if let Some(family) = GameFamily::from_db_version(&db.meta.version) {
        println!("\n=== Game Family: {} ===", family.as_str());
    }

    // Quick sanity check - look up a few commands
    println!("\n=== Database Sanity Check ===");
    for cmd_name in &["End", "SetVar", "Message", "Jump", "StartTrainerBattle"] {
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
    let func_count = file
        .items
        .iter()
        .filter(|s| matches!(s.node, StatementKind::Function { .. }))
        .count();
    let action_count = file
        .items
        .iter()
        .filter(|s| matches!(s.node, StatementKind::Action { .. }))
        .count();
    println!(
        "Parsed: {} aliases, {} functions, {} actions",
        file.aliases.len(),
        func_count,
        action_count
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
    let mut lowerer = Lowerer::new(&analyzer.symbols, &db);
    let items = match lowerer.lower_script_file(&file) {
        Ok(result) => result,
        Err(e) => {
            print_error("<test>", input, &e);
            return;
        }
    };
    for item in &items {
        println!("{}", item);
    }

    let pub_funcs = file
        .items
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
    let byte_output = match emitter.emit_script_file(&items) {
        Ok(output) => output,
        Err(e) => {
            print_error("<test>", input, &e);
            return;
        }
    };
    // println!("Output: {:?}", byte_output);
    println!("finished in {}ms", start.elapsed().as_millis());
    std::fs::write("test_output.bin", &byte_output).unwrap();
}
