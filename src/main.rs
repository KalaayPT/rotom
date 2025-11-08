use clap::{Args, Parser, Subcommand};
use serde::Deserialize;
use std;
use std::cell::RefCell;
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
    containers: Vec<CommandContainer>,
}
impl ScriptFile {
    fn new() -> ScriptFile {
        ScriptFile {
            containers: Vec::new(),
        }
    }
}

#[derive(Debug)]
enum ContainerType {
    Script,
    Function,
    Action,
    Undefined,
}
#[derive(Debug)]
struct ContainerReference {
    id: u16,
    offset: i32,
}
#[derive(Debug)]
struct CommandContainer {
    kind: ContainerType,
    reference: ContainerReference,
    commands: Vec<ScriptCommand>,
    // called_by: Option<&'static CommandContainer>,
    // calls: Option<&'static CommandContainer>,
}
impl CommandContainer {
    fn new(kind: ContainerType, offset: i32, id: u16) -> CommandContainer {
        CommandContainer {
            kind: kind,
            reference: ContainerReference {
                id: id,
                offset: offset,
            },
            commands: Vec::new(),
        }
    }
}
#[derive(Debug)]
struct ScriptCommand {
    id: u16,
    name: String,
    parameters: Vec<i32>,
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

fn disassemble(args: CommandArgs) -> Result<(), Box<dyn Error>> {
    let db = read_json(&args.json_database)?;
    if args.script_directory.is_some() {
        parse_directory(&args.script_directory.unwrap(), &db);
    } else {
        parse_script_file_bin(&args.script_file.unwrap(), &db);
    }
    Ok(())
}

fn assemble(args: CommandArgs) -> Result<(), Box<dyn Error>> {
    let db = read_json(&args.json_database)?;
    if args.script_directory.is_some() {
        match parse_directory(&args.script_directory.unwrap(), &db) {
            Ok(()) => {}
            Err(e) => {
                println!("{}", e);
            }
        };
    } else {
        // parse_script_file_bin(&args.script_file.unwrap(), &db);
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
        parse_script_file_bin(file.to_str().expect(""), &db);
    }
    Ok(())
}

fn parse_script_file_bin(file: &str, db: &ScriptDatabase) -> Result<(), ParseError> {
    println!("parsing file {}", file);
    let mut script_file = ScriptFile::new();
    let byte_array: Vec<u8> = std::fs::read(file).unwrap();
    let jump_table_end = byte_array
        .windows(4)
        .position(|bytes| bytes[0..=1] == [0x13, 0xFD])
        .unwrap();
    // println!("{jump_table_end}");
    let jump_table = &byte_array[0..jump_table_end];
    // println!("jumptable: {jump_table:?}");
    let mut script_addresses: Vec<i32> = jump_table
        .chunks_exact(4)
        .map(|chunks| i32::from_le_bytes([chunks[0], chunks[1], chunks[2], chunks[3]]))
        .collect();
    // correct relative jumps to absolute addresses
    for i in 0..script_addresses.len() {
        script_addresses[i] = script_addresses[i] + (i as i32 + 1) * 4;
    }
    println!("script addresses: {script_addresses:x?}");
    // let script_contents = &byte_array[jump_table_end + 2..];
    // println!("{script_contents:x?}");
    let mut script_no = 1;
    let mut function_offsets: Vec<i32> = Vec::new();
    println!("parsing scripts..........");
    for script_offset in &script_addresses {
        println!("Script {script_no}:");
        let mut script = CommandContainer::new(ContainerType::Script, *script_offset, script_no);
        let mut function_offsets_temp =
            parse_script_function_bytes(&byte_array, *script_offset as usize, &db, &mut script)?;
        function_offsets.append(&mut function_offsets_temp);

        script_no += 1;
        script_file.containers.push(script);
    }

    for script_offset in &script_addresses {
        function_offsets.retain(|elem| *elem != *script_offset);
    }
    println!("functions found after parsing scripts: {function_offsets:x?}");
    // println!("{script_file:#?}");
    let mut function_no = 1;
    let mut i = 1;
    println!("parsing functions..........");
    while function_offsets.len() > 0 {
        let mut function_offsets_temp = Vec::new();
        for function_offset in &function_offsets {
            println!("Function {function_no}:");
            let mut function =
                CommandContainer::new(ContainerType::Script, *function_offset, function_no);
            function_offsets_temp.append(&mut parse_script_function_bytes(
                &byte_array,
                *function_offset as usize,
                &db,
                &mut function,
            )?);
            function_offsets_temp.dedup();

            for function_offset in &function_offsets {
                function_offsets_temp.retain(|elem| *elem != *function_offset);
            }
            for script_offset in &script_addresses {
                function_offsets_temp.retain(|elem| *elem != *script_offset);
            }

            function_no += 1;
            script_file.containers.push(function);
        }
        function_offsets = function_offsets_temp;
        i += 1;
        println!("functions left after pass {}: {function_offsets:x?}", i);
    }
    Ok(())
}

fn parse_script_function_bytes(
    byte_array: &Vec<u8>,
    mut pc: usize,
    db: &ScriptDatabase,
    current_command_container: &mut CommandContainer,
) -> Result<Vec<i32>, ParseError> {
    let mut end_condition = false;
    let mut function_offsets = Vec::new();
    'read_command_bytes: while end_condition == false {
        let command_bytes: u16 = u16::from_le_bytes([byte_array[pc], byte_array[pc + 1]]);
        // println!("command bytes: {command_bytes:x?}");
        let byte_string = format!("0x{}", format!("{command_bytes:0>4x}").to_uppercase());
        // println!("{byte_string}");
        let db_command = match db.scrcmd.get(&byte_string) {
            Some(scrcmd) => {
                pc += 2;
                // println!("pc: {pc}");
                // println!("found command: {:#?}", scrcmd.name);
                scrcmd
            }
            None => return Err(ParseError::NotACommand(command_bytes)),
        };
        let mut parameter_values = Vec::new();

        for parameter in &db_command.parameters {
            let parameter_size = *parameter as usize;
            // println!("parameter size: {parameter_size}");
            let parameter_value = match parameter_size {
                1 => byte_array[pc] as i32,
                2 => i32::from_le_bytes([byte_array[pc], byte_array[pc + 1], 0, 0]),
                3 => {
                    i32::from_le_bytes([byte_array[pc], byte_array[pc + 1], byte_array[pc + 2], 0])
                }
                4 => i32::from_le_bytes([
                    byte_array[pc],
                    byte_array[pc + 1],
                    byte_array[pc + 2],
                    byte_array[pc + 3],
                ]),
                _ => unreachable!("parameter length >4 what"),
            };
            // println!("parameter value: {parameter_value}, Hex: 0x{parameter_value:x}");
            parameter_values.push(parameter_value);
            pc += parameter_size;
        }
        let command = ScriptCommand {
            id: command_bytes,
            name: db_command.name.clone(),
            parameters: parameter_values,
        };
        if db_command.name == "End" || db_command.name == "Return" || db_command.name == "Jump" {
            end_condition = true
        }
        if db_command.name == "Jump" || db_command.name == "Call" {
            println!("found relative jump: {}", command.parameters[0] + pc as i32);
            function_offsets.push(command.parameters[0] + pc as i32);
        } else if db_command.name == "JumpIf"
            || db_command.name == "CallIf"
            || db_command.name == "JumpIfObjID"
            || db_command.name == "JumpIfEventID"
            || db_command.name == "JumpIfPlayerDir"
        {
            println!("found relative jump: {}", command.parameters[1] + pc as i32);
            function_offsets.push(command.parameters[1] + pc as i32);
        }
        println!("{} {:?}", command.name, command.parameters);
        current_command_container.commands.push(command);
    }
    Ok(function_offsets)
}
