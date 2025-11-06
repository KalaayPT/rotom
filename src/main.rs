use clap::{Args, Parser, Subcommand};
use serde::Deserialize;
use std;
use std::collections::HashMap;
use std::error::Error;
use std::io;

#[derive(Debug, Parser)]
#[command(version, about = "A pokemon script assembler/disassembler", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}
#[derive(Debug, Subcommand)]
enum Commands {
    #[command(arg_required_else_help = true)]
    Assemble(CommandArgs),
    Disassemble(CommandArgs),
}
#[derive(Debug, Args)]
struct CommandArgs {
    #[arg(short, long)]
    json_database: String,
    #[arg(short = 'f', long, group = "script")]
    script_file: Option<String>,
    #[arg(short = 'd', long, group = "script")]
    script_directory: Option<String>,
}
#[derive(Debug, Deserialize)]
struct ScriptDatabase {
    movements: HashMap<String, Movements>,
    comparisonOperators: HashMap<String, String>,
    specialOverworlds: HashMap<String, String>,
    overworldDirections: HashMap<String, String>,
    scrcmd: HashMap<String, ScrCmd>,
    sounds: HashMap<String, Sounds>,
}
#[derive(Debug, Deserialize)]
struct Movements {
    name: String,
    decomp_name: String,
    description: String,
}
#[derive(Debug, Deserialize)]
struct ScrCmd {
    name: String,
    decomp_name: String,
    parameters: Vec<u8>,
    parameter_types: Vec<ScriptParameter>,
    parameter_values: Vec<String>,
    description: String,
}
#[derive(Debug, Deserialize)]
enum ScriptParameter {
    Integer,
    Variable,
    Flex,
    Overworld,
    OwMovementType,
    OwMovementDirection,
    ComparisonOperator,
    Function,
    Action,
    CMDNumber,
    Pokemon,
    Item,
    Move,
    Sound,
    Trainer,
}
#[derive(Debug, Deserialize)]
struct Sounds {
    name: String,
    used_in: String,
}
#[derive(Debug)]
struct ScriptFile {
    scripts: Vec<ScriptCommand>,
    function: Vec<ScriptCommand>,
    actions: Vec<MovementCommand>,
}
impl ScriptFile {
    fn new() -> ScriptFile {
        ScriptFile {
            scripts: Vec::new(),
            function: Vec::new(),
            actions: Vec::new(),
        }
    }
}
#[derive(Debug)]
struct ScriptCommand {
    id: u16,
    name: String,
    parameters: Vec<u32>,
}
#[derive(Debug)]
struct MovementCommand {
    id: u16,
    name: String,
    parameter: u16,
}
#[derive(Debug)]
pub enum ParseError {
    NotACommand(u16),
}
impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotACommand(cmd) => write!(f, "failed to read command: {cmd:x}"),
        }
    }
}

fn main() {
    let args = Cli::parse();
    println!("{args:?}");
    match args.command {
        Commands::Disassemble(args) => {
            disassemble(args);
        }
        Commands::Assemble(args) => {
            assemble(args).unwrap();
        }
    }
}

fn disassemble(args: CommandArgs) {
    let db = read_json(&args.json_database);
}

fn assemble(args: CommandArgs) -> Result<(), Box<dyn Error>> {
    // println!("{}", args.json_database);
    let db = read_json(&args.json_database)?;
    // println!("{db:?}");
    if args.script_directory.is_some() {
        parse_directory(&args.script_directory.unwrap(), &db);
    } else {
        parse_script_file(&args.script_file.unwrap(), &db);
    }
    // let raw_script;
    Ok(())
}

fn read_json(json_path: &str) -> Result<ScriptDatabase, Box<dyn Error>> {
    let raw_json = std::fs::read_to_string(json_path)?;
    let db: ScriptDatabase = serde_json::from_str(&raw_json)?;
    Ok(db)
}

fn parse_directory(folder: &str, db: &ScriptDatabase) -> io::Result<()> {
    let entries = std::fs::read_dir(folder)?
        .map(|res| res.map(|e| e.path()))
        .collect::<Result<Vec<_>, io::Error>>()?;
    for file in entries {
        parse_script_file(file.to_str().expect(""), &db);
    }
    Ok(())
}

fn parse_script_file(file: &str, db: &ScriptDatabase) {
    let mut script_file = ScriptFile::new();
    let byte_array: Vec<u8> = std::fs::read(file).unwrap();
    let jump_table_end = byte_array
        .windows(4)
        .position(|bytes| bytes[0..=1] == [0x13, 0xFD])
        .unwrap();
    println!("{jump_table_end}");
    let jump_table = &byte_array[0..jump_table_end];
    println!("jumptable: {jump_table:?}");
    let mut script_addresses: Vec<u32> = jump_table
        .chunks_exact(4)
        .map(|chunks| u32::from_le_bytes([chunks[0], chunks[1], chunks[2], chunks[3]]))
        .collect();
    // correct relative jumps to absolute addresses
    for i in 0..script_addresses.len() {
        script_addresses[i] = script_addresses[i] + (i as u32 + 1) * 4;
    }
    println!("script addresses: {script_addresses:x?}");
    // let script_contents = &byte_array[jump_table_end + 2..];
    // println!("{script_contents:x?}");
    for script in script_addresses {
        parse_script_function_bytes(&byte_array, script, &db, &mut script_file);
    }
    println!("{script_file:#?}");
}

fn parse_script_function_bytes(
    byte_array: &Vec<u8>,
    script_offset: u32,
    db: &ScriptDatabase,
    script_file: &mut ScriptFile,
) -> Result<(), ParseError> {
    let mut pc: usize = 0;
    let mut end_condition = false;
    let byte_array = &byte_array[script_offset as usize..];
    'read_command_bytes: while end_condition == false {
        let command_bytes: u16 = u16::from_le_bytes([byte_array[pc], byte_array[1]]);
        println!("command bytes: {command_bytes:x?}");
        let byte_string = format!("0x{command_bytes:0>4x}");
        println!("{byte_string}");
        let db_command = match db.scrcmd.get(&byte_string) {
            Some(scrcmd) => {
                pc += 2;
                println!("found command: {scrcmd:#?}");
                scrcmd
            }
            None => return Err(ParseError::NotACommand(command_bytes)),
        };
        let mut parameter_values = Vec::new();

        for parameter in &db_command.parameters {
            let parameter_size = *parameter as usize;
            // current_parameters.push(u32::from_le_bytes(
            //     byte_array[pc..pc + *parameter as usize].try_into().unwrap(),
            // ));
            parameter_values.push(match parameter_size {
                1 => byte_array[0] as u32,
                2 => u32::from_le_bytes([byte_array[0], byte_array[1], 0, 0]),
                3 => u32::from_le_bytes([byte_array[0], byte_array[1], byte_array[2], 0]),
                4 => {
                    u32::from_le_bytes([byte_array[0], byte_array[1], byte_array[2], byte_array[4]])
                }
                _ => unreachable!("parameter length >4 what"),
            })
        }
        if db_command.name == "End"
            || db_command.name == "Return"
            || db_command.name == "Jump"
            || db_command.name == "JumpIf"
        {
            end_condition = true
        }
        script_file.scripts.push(ScriptCommand {
            id: command_bytes,
            name: db_command.name.clone(),
            parameters: parameter_values,
        });
    }
    Ok(())
}
