use clap::{Args, Parser, Subcommand};
use serde::Deserialize;
use std;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::io;
use std::path::PathBuf;

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
    #[arg(short, long, group = "script")]
    script_file: PathBuf,
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
    fn contains_offset(&self, offset: i32) -> bool {
        self.containers
            .iter()
            .any(|command| command.reference.offset == offset)
    }
}

#[derive(Debug, Clone)]
enum ContainerType {
    Script,
    Function,
    Action,
}
#[derive(Debug)]
struct ContainerReference {
    id: u32,
    offset: i32,
}
#[derive(Debug)]
struct CommandContainer {
    kind: ContainerType,
    reference: ContainerReference,
    commands: CommandList,
    // called_by: Option<&'static CommandContainer>,
    // calls: Option<&'static CommandContainer>,
}
impl CommandContainer {
    fn new(kind: ContainerType, offset: i32, id: u32) -> CommandContainer {
        CommandContainer {
            commands: match kind {
                ContainerType::Script | ContainerType::Function => CommandList::Script(Vec::new()),
                ContainerType::Action => CommandList::Movement(Vec::new()),
            },
            kind: kind,
            reference: ContainerReference {
                id: id,
                offset: offset,
            },
        }
    }
}
#[derive(Debug)]
enum CommandList {
    Script(Vec<ScriptCommand>),
    Movement(Vec<Movement>),
}
#[derive(Debug)]
struct ScriptCommand {
    id: u16,
    name: String,
    parameters: Vec<i32>,
}
#[derive(Debug)]
struct Movement {
    id: u16,
    name: String,
    parameter: i32,
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
struct ParserState {
    script_no: u32,
    func_no: u32,
    action_no: u32,
    script_offsets: HashSet<i32>,
    function_offsets: HashSet<i32>,
    action_offsets: HashSet<i32>,
    output_string: String,
}
impl ParserState {
    fn new() -> ParserState {
        ParserState {
            script_no: 0,
            func_no: 0,
            action_no: 0,
            script_offsets: HashSet::new(),
            function_offsets: HashSet::new(),
            action_offsets: HashSet::new(),
            output_string: String::new(),
        }
    }
}

fn main() {
    let args = Cli::parse();
    println!("{args:?}");
    match args.command {
        Commands::Disassemble(args) => {
            disassemble(args).unwrap();
        }
        Commands::Assemble(args) => {
            assemble(args).unwrap();
        }
    }
}

fn disassemble(args: CommandArgs) -> Result<(), Box<dyn Error>> {
    let db = read_json(&args.json_database)?;
    if args.script_file.is_dir() {
        parse_directory(&args.script_file, &db).unwrap();
    } else {
        parse_script_file_bin(&args.script_file, &db).unwrap();
    }
    Ok(())
}

fn assemble(args: CommandArgs) -> Result<(), Box<dyn Error>> {
    let db = read_json(&args.json_database)?;
    if args.script_file.exists() {
        match parse_directory(&args.script_file, &db) {
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

fn parse_directory(folder: &PathBuf, db: &ScriptDatabase) -> io::Result<()> {
    let entries = std::fs::read_dir(folder)?
        .map(|res| res.map(|e| e.path()))
        .collect::<Result<Vec<_>, io::Error>>()?;
    for file in entries {
        parse_script_file_bin(&file, &db).unwrap();
    }
    Ok(())
}

fn parse_script_file_bin(file: &PathBuf, db: &ScriptDatabase) -> Result<(), ParseError> {
    println!("parsing file {:?}", file);
    let mut parser = ParserState::new();
    let mut script_file = ScriptFile::new();
    let byte_array: Vec<u8> = std::fs::read(file).unwrap();
    let jump_table_end = match byte_array
        .windows(4)
        .position(|bytes| bytes[0..=1] == [0x13, 0xFD])
    {
        Some(end) => end,
        None => 0,
    };
    if jump_table_end == 0 {
        return Ok(());
    }
    // println!("{jump_table_end}");
    let jump_table = &byte_array[0..jump_table_end];
    // println!("jumptable: {jump_table:?}");
    let script_addresses: HashSet<i32> = jump_table
        .chunks_exact(4)
        .enumerate()
        .map(|(i, chunks)| {
            let rel_address = i32::from_le_bytes([chunks[0], chunks[1], chunks[2], chunks[3]]);
            let abs_address = rel_address + (i as i32 + 1) * 4;
            abs_address
        })
        .collect();
    parser.script_offsets = script_addresses;
    println!("script addresses: {:x?}", parser.script_offsets);
    // println!("{script_contents:x?}");
    parser.script_no += 1;
    println!("parsing scripts..........");
    for script_offset in parser.script_offsets.clone() {
        println!("\nScript {}:", parser.script_no);
        parse_script_function_bytes(
            &byte_array,
            script_offset as usize,
            &db,
            &mut script_file,
            &mut parser,
            ContainerType::Script,
        )?;

        parser.script_no += 1;
    }
    println!(
        "functions found after parsing scripts: {:x?}",
        parser.function_offsets
    );
    // println!("{script_file:#?}");
    parser.func_no += 1;
    let mut i = 1;
    println!("parsing functions..........");
    while parser.function_offsets.len() > 0 {
        for function_offset in parser.function_offsets.clone() {
            println!("\nFunction {}:", parser.func_no);
            parse_script_function_bytes(
                &byte_array,
                function_offset as usize,
                &db,
                &mut script_file,
                &mut parser,
                ContainerType::Function,
            )?;
            parser.func_no += 1;
            _ = parser.function_offsets.remove(&function_offset)
        }
        i += 1;
        println!(
            "functions left after pass {}: {:x?}",
            i, parser.function_offsets
        );
    }
    println!("movements found: {:x?}", parser.action_offsets);
    parser.action_no += 1;
    while parser.action_offsets.len() > 0 {
        for action_offset in parser.action_offsets.clone() {
            println!("\nAction {}:", parser.action_no);
            parse_action_bytes(
                &byte_array,
                action_offset as usize,
                &db,
                &mut script_file,
                &mut parser,
                ContainerType::Action,
            )?;
            parser.action_no += 1;
            _ = parser.action_offsets.remove(&action_offset)
        }
    }

    Ok(())
}

fn parse_script_function_bytes(
    byte_array: &Vec<u8>,
    mut pc: usize,
    db: &ScriptDatabase,
    file: &mut ScriptFile,
    parser: &mut ParserState,
    cont_type: ContainerType,
) -> Result<(), ParseError> {
    let mut end_condition = false;
    let mut current_command_container =
        CommandContainer::new(cont_type.clone(), pc as i32, parser.func_no);
    'read_command_bytes: while end_condition == false {
        let command_bytes: u16 = u16::from_le_bytes([byte_array[pc], byte_array[pc + 1]]);
        // println!("command bytes: {command_bytes:x?}");
        let byte_string = format!("0x{}", format!("{command_bytes:0>4x}").to_uppercase());
        // println!("{byte_string}");
        let db_command = match db.scrcmd.get(&byte_string) {
            Some(scrcmd) => {
                pc += 2;
                scrcmd
            }
            None => return Err(ParseError::NotACommand(command_bytes)),
        };
        let mut parameter_values = Vec::new();
        if db_command.parameters.first() == Some(&255) {
            parse_conditional_parameters(&mut parameter_values, db_command, byte_array, &mut pc);
        } else {
            for parameter in &db_command.parameters {
                let parameter_size = *parameter as usize;
                // println!("parameter size: {parameter_size}");
                let parameter_value = match parameter_size {
                    1 => byte_array[pc] as i32,
                    2 => i32::from_le_bytes([byte_array[pc], byte_array[pc + 1], 0, 0]),
                    3 => i32::from_le_bytes([
                        byte_array[pc],
                        byte_array[pc + 1],
                        byte_array[pc + 2],
                        0,
                    ]),
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
            let offset = command.parameters[0] + pc as i32;
            println!("found relative jump: {}", offset);
            if !file.contains_offset(offset) {
                parser.function_offsets.insert(offset);
            };
        } else if db_command.name == "JumpIf"
            || db_command.name == "CallIf"
            || db_command.name == "JumpIfObjID"
            || db_command.name == "JumpIfEventID"
            || db_command.name == "JumpIfPlayerDir"
        {
            let offset = command.parameters[1] + pc as i32;
            // println!("found relative jump: {}", offset);
            if !file.contains_offset(offset) {
                parser.function_offsets.insert(offset);
            };
        }
        if db_command.name == "Movement" {
            let offset = command.parameters[1] + pc as i32;
            println!("found action: {}", offset);
            if !file.contains_offset(offset) {
                parser.action_offsets.insert(offset);
            };
        }
        println!("{} {:?}", command.name, command.parameters);
        match &mut current_command_container.commands {
            CommandList::Script(list) => list.push(command),
            _ => unreachable!(""),
        }
    }
    file.containers.push(current_command_container);
    Ok(())
}

fn parse_conditional_parameters(
    parameter_values: &mut Vec<i32>,
    db_command: &ScrCmd,
    byte_array: &Vec<u8>,
    pc: &mut usize,
) {
    let mut paramcounter = 1;
    let condition_param = &db_command.parameters[paramcounter];
    println!("condition param: {condition_param}");
    paramcounter += 1;
    let mut conditional_paramlist = Vec::new();
    conditional_paramlist.push(*condition_param);
    'find_conditional_params: loop {
        if &db_command.parameters[paramcounter] == &byte_array[*pc] {
            println!("conditon found!");
            println!("condition: {}", db_command.parameters[paramcounter]);
            paramcounter += 2;
            conditional_paramlist.append(
                &mut db_command.parameters[paramcounter..paramcounter + {
                    db_command.parameters[paramcounter - 1] as usize
                }]
                    .to_vec(),
            );
            println!("conditional paramlist: {conditional_paramlist:?}");
            break 'find_conditional_params;
        }
        paramcounter += db_command.parameters[paramcounter + 1] as usize + 2;
    }
    for parameter in &conditional_paramlist {
        let parameter_size = *parameter as usize;
        // println!("parameter size: {parameter_size}");
        let parameter_value = match parameter_size {
            1 => byte_array[*pc] as i32,
            2 => i32::from_le_bytes([byte_array[*pc], byte_array[*pc + 1], 0, 0]),
            3 => i32::from_le_bytes([byte_array[*pc], byte_array[*pc + 1], byte_array[*pc + 2], 0]),
            4 => i32::from_le_bytes([
                byte_array[*pc],
                byte_array[*pc + 1],
                byte_array[*pc + 2],
                byte_array[*pc + 3],
            ]),
            _ => unreachable!("parameter length >4 what"),
        };
        // println!("parameter value: {parameter_value}, Hex: 0x{parameter_value:x}");
        parameter_values.push(parameter_value);
        *pc += parameter_size;
    }
}

fn parse_action_bytes(
    byte_array: &Vec<u8>,
    mut pc: usize,
    db: &ScriptDatabase,
    file: &mut ScriptFile,
    parser: &mut ParserState,
    cont_type: ContainerType,
) -> Result<(), ParseError> {
    let mut end_condition = false;
    let mut current_command_container =
        CommandContainer::new(cont_type.clone(), pc as i32, parser.func_no);
    'read_command_bytes: while end_condition == false {
        let command_bytes: u16 = u16::from_le_bytes([byte_array[pc], byte_array[pc + 1]]);
        // println!("command bytes: {command_bytes:x?}");
        let byte_string = format!("0x{}", format!("{command_bytes:0>4x}").to_uppercase());
        // println!("{byte_string}");
        let db_command = match db.movements.get(&byte_string) {
            Some(movement) => {
                pc += 2;
                movement
            }
            None => {
                pc += 2;
                &Movements {
                    name: byte_string,
                    decomp_name: "".to_string(),
                    description: "".to_string(),
                }
            }
        };
        let movement = Movement {
            id: command_bytes,
            name: db_command.name.clone(),
            parameter: i32::from_le_bytes([byte_array[pc], byte_array[pc + 1], 0, 0]),
        };
        pc += 2;
        if db_command.name == "End" {
            end_condition = true
        }
        println!("{} {}", movement.name, movement.parameter);
        match &mut current_command_container.commands {
            CommandList::Movement(list) => list.push(movement),
            _ => unreachable!(""),
        }
    }
    file.containers.push(current_command_container);
    Ok(())
}
