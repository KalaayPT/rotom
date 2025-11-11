use clap::{Args, Parser, Subcommand};
use serde::Deserialize;
use std;
use std::collections::HashMap ;
use std::fmt::format;
use linked_hash_set::LinkedHashSet;
use std::error::Error;
use std::io::{self, Write};
use std::path::PathBuf;
use regex::{Match, Regex};

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
    database: PathBuf,
    #[arg(short, long, group = "script")]
    script_file: PathBuf,
}
enum Directive {
    Assemble,
    Disassemble
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
#[derive(Debug, Deserialize)]
struct Enums {
    items: HashMap<String, String>,
    moves: HashMap<String, String>,
    pokemon: HashMap<String, String>,
    trainers: HashMap<String, String>,
}
impl Enums {
    fn new() -> Enums {
        Enums { items: HashMap::new(), moves: HashMap::new(), pokemon: HashMap::new(), trainers: HashMap::new() }
    }
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
    id: Vec<u32>,
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
                id: vec![id],
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
impl ScriptCommand {
    fn new() -> ScriptCommand {
        ScriptCommand {
            id: 0,
            name: String::new(),
            parameters: Vec::new()
        }
    }
}
#[derive(Debug)]
struct Movement {
    id: u16,
    name: String,
    parameter: i32,
}
impl Movement {
    fn new() -> Movement {
        Movement {
            id: 0,
            name: String::new(),
            parameter: 0
        }
    }
}

#[derive(Debug)]
pub enum ParseError {
    NotACommand(String, String),
}
impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotACommand(cmd, cmd2) => write!(f, "failed to read command: {cmd} At line/offset: {cmd2}"),
        }
    }
}
struct ParserState {
    script_no: u32,
    func_no: u32,
    action_no: u32,
    script_offsets: Vec<i32>,
    function_offsets: LinkedHashSet<i32>,
    action_offsets: LinkedHashSet<i32>,
    output_string: String,
}
impl ParserState {
    fn new() -> ParserState {
        ParserState {
            script_no: 0,
            func_no: 0,
            action_no: 0,
            script_offsets: Vec::new(),
            function_offsets: LinkedHashSet::new(),
            action_offsets: LinkedHashSet::new(),
            output_string: String::new(),
        }
    }
}

fn main() {
    let args = Cli::parse();
    // println!("{args:?}");
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
    let (db, enums) = read_jsons(&args.database)?;
    let directive = Directive::Disassemble;
    if args.script_file.is_dir() {
        match parse_directory(&args.script_file, &db, &enums, &directive) {
            Ok(()) => {
                println!("scripts disassembled to plaintext successfully");
            }
            Err(e) => {
                eprintln!("error during parsing: {e}");
            }
        };
    } else {
        match parse_script_file_bin(&args.script_file, &db, &enums) {
            Ok(()) => {
                println!("script disassembled to plaintext successfully");
            }
            Err(e) => {
                eprintln!("error during parsing: {e}");
            }
        };
    }
    Ok(())
}

fn assemble(args: CommandArgs) -> Result<(), Box<dyn Error>> {
    let (db, enums) = read_jsons(&args.database)?;
    let directive = Directive::Assemble;
    if args.script_file.is_dir() {
        match parse_directory(&args.script_file, &db, &enums, &directive) {
            Ok(()) => {}
            Err(e) => {
                println!("{}", e);
            }
        };
    } else {
        match parse_plaintext_file(&args.script_file, &db, &enums) {
            Ok(()) => {}
            Err(e) => {
                println!("{}", e);
            }
        };
    }
    Ok(())
}

fn read_jsons(json_path: &PathBuf) -> Result<(ScriptDatabase, Enums), Box<dyn Error>> {
    // println!("{}", format!("{}\\scrcmd_database.json",json_path));
    let db_json = std::fs::read_to_string(json_path.join("scrcmd_database.json"))?;
    let items_json = std::fs::read_to_string(json_path.join("items.json"))?;
    let pokemon_json = std::fs::read_to_string(json_path.join("pokemon.json"))?;
    let trainers_json = std::fs::read_to_string(json_path.join("trainers.json"))?;
    let moves_json = std::fs::read_to_string(json_path.join("moves.json"))?;
    let db: ScriptDatabase = serde_json::from_str(&db_json)?;
    let mut enums = Enums::new();
    enums.trainers = serde_json::from_str(&trainers_json)?;
    enums.items = serde_json::from_str(&items_json)?;
    enums.pokemon = serde_json::from_str(&pokemon_json)?;
    enums.moves = serde_json::from_str(&moves_json)?;
    Ok((db, enums))
}


fn parse_directory(folder: &PathBuf, db: &ScriptDatabase, enums: &Enums, directive: &Directive) -> io::Result<()> {
    let entries = std::fs::read_dir(folder)?
        .map(|res| res.map(|e| e.path()))
        .collect::<Result<Vec<_>, io::Error>>()?;
    for file in entries {
        match directive {
            Directive::Assemble => parse_plaintext_file(&file, &db, &enums).unwrap(),
            Directive::Disassemble => parse_script_file_bin(&file, &db, &enums).unwrap()
        }
    }
    Ok(())
}

fn parse_plaintext_file(file: &PathBuf, db: &ScriptDatabase, enums: &Enums) -> Result<(), ParseError> {
    println!("file: {}", file.display());
    let mut script_file = ScriptFile::new();
    let mut parser = ParserState::new();
    let text = std::fs::read_to_string(file).unwrap();
    let regex = Regex::new(r"(?m)^\w+ [\w\d#]+:").unwrap();
    let matches: Vec<Match> = regex.find_iter(&text).collect();
    let mut start_indices: Vec<usize> = Vec::new();
    start_indices.push(matches[0].start());
    for i in 1..matches.len() {
            let prev_match = &matches[i - 1];
            let current_match = &matches[i];
            let text_between = &text[prev_match.end()..current_match.start()];
            if !text_between.trim().is_empty() {
                start_indices.push(current_match.start());
            }
        }
    let mut end_indices: Vec<usize> = start_indices.iter().skip(1).copied().collect();
    end_indices.push(text.len());
    // println!("{:?}", start_indices);
    let command_containers: Vec<&str> = start_indices
        .iter()
        .zip(end_indices)
        .map(|(&start,end)| text[start..end].trim())
        .collect();
    
    for container in &command_containers {
        // println!("{container}");
        parse_plaintext_container(&container, &db, &enums, &mut parser, &mut script_file);
    }    
    print!("{script_file:#?}");
    // write_binary();
    Ok(())
}

fn write_binary(
    file: &PathBuf,
    parser: &mut ParserState,
    script_file: &ScriptFile,
    db: &ScriptDatabase,
){

}

fn parse_plaintext_container(container_str: &str, db: &ScriptDatabase, enums: &Enums, parser: &mut ParserState, script_file: &mut ScriptFile){
    let header_line = container_str.lines().next().unwrap();
    let mut current_command_container = CommandContainer::new(ContainerType::Script, 0, 0);
    match header_line.split_whitespace().next().unwrap() {
        "Script" => {
            parser.script_no += 1;
            current_command_container.kind = ContainerType::Script;
            current_command_container.reference.id.push(parser.script_no);
        },
        "Function" => {
            parser.func_no += 1;
            current_command_container.kind = ContainerType::Function;
            current_command_container.reference.id.push(parser.func_no);

        },
        "Action" => {
            parser.action_no += 1;
            current_command_container.kind = ContainerType::Action;
            current_command_container.reference.id.push(parser.action_no);
            current_command_container.commands = CommandList::Movement(Vec::new())
        },
        _ => unreachable!("")
    };
    for (i, line) in container_str.lines().skip(1).enumerate() {
        parse_command_str(line, &mut current_command_container, &db, i, &enums);
    }
    // println!("{current_command_container:#?}");
    script_file.containers.push(current_command_container);
}

fn parse_command_str(command: &str, current_command_container: &mut CommandContainer, db: &ScriptDatabase, line_index: usize, enums: &Enums) -> Result<(), ParseError>{
    println!("{command}");
    if matches!(current_command_container.kind, ContainerType::Action) {
        parse_movement_str(&command, current_command_container, &db, &line_index)?;
        return Ok(());
    } 
    let mut current_command = ScriptCommand::new();    
    let db_command: &ScrCmd;
    let command_elements: Vec<&str> = command.split_whitespace().collect(); 
    match db.scrcmd.iter().find(|(_byte_code,command)| command.name == command_elements[0]) {
        Some((byte_code, scrcmd)) => {
            current_command.name = scrcmd.name.clone();
            // println!("{byte_code}");
            current_command.id = u16::from_str_radix(&byte_code.trim_start_matches("0x"), 16).unwrap();
            db_command = scrcmd;
        },
        None => return Err(ParseError::NotACommand(format!("{}", command_elements[0]), format!("{line_index}")))
    }
    let mut db_parameters = db_command.parameters.clone();
    if db_command.parameters.first() == Some(&255) {
        db_parameters = get_parameters_by_condition(&command_elements, db_command)?;
    }
    for (i, _parameter ) in db_parameters.iter().enumerate() {
        if command_elements.len() == 1 {
            continue;
        }
        // println!("command parameter: {}", command_elements[i+1]);
        match db_command.parameter_types[i] {
            ScriptParameter::Sound => {
                match db.sounds.iter().find(|(_byte,sound)| sound.name == command_elements[i+1]) {
                    Some((byte, _sound)) => {
                        current_command.parameters.push(byte.parse().unwrap());
                    },
                    None => return Err(ParseError::NotACommand(format!("{}", command_elements[i+1]), format!("{}", line_index)))
                }
            },
            ScriptParameter::Variable => {
                current_command.parameters.push(i32::from_str_radix(command_elements[i+1].trim_start_matches("0x"), 16).unwrap())
            },
            ScriptParameter::Flex => {
                if command_elements[i+1].parse::<i32>().is_err() {
                    current_command.parameters.push(i32::from_str_radix(command_elements[i+1].trim_start_matches("0x"), 16).unwrap())
                } else {
                    current_command.parameters.push(command_elements[i+1].parse().unwrap());
                }
            },
            ScriptParameter::ComparisonOperator => {
                match db.comparisonOperators.iter().find(|(_byte, string)| *string == command_elements[i+1]){
                    Some((byte, _string)) => current_command.parameters.push(
                    i32::from_str_radix(byte.trim_start_matches("0x"), 16).unwrap()
                    ),
                    None => unreachable!()
                }
            },
            ScriptParameter::Function => {
                let (first, _last) = command_elements[i+1].split_at(4);
                match first {
                    // negative for scripts, positive for functions, later gets corrected to
                    // actual offsets
                "Func" => current_command.parameters.push(command_elements[i+1].trim_start_matches("Function#").parse().unwrap()),
                "Scri" => current_command.parameters.push(-command_elements[i+1].trim_start_matches("Script#").parse::<i32>().unwrap()),
                _ => unreachable!()
                }
            },
            ScriptParameter::Item => {
                match enums.items.iter().find(|(_byte, string)| *string == command_elements[i+1]) {
                    Some((byte, _string)) => current_command.parameters.push(byte.parse().unwrap()),
                    None => {
                        if command_elements[i+1].parse::<i32>().is_err() {
                            current_command.parameters.push(i32::from_str_radix(command_elements[i+1].trim_start_matches("0x"), 16).unwrap())
                        } else {
                            current_command.parameters.push(command_elements[i+1].parse().unwrap());
                        }
                    },  
                }
            },
            ScriptParameter::Trainer => {
                match enums.trainers.iter().find(|(_byte, string)| *string == command_elements[i+1]) {
                    Some((byte, _string)) => current_command.parameters.push(byte.parse().unwrap()),
                    None => {
                        if command_elements[i+1].parse::<i32>().is_err() {
                            current_command.parameters.push(i32::from_str_radix(command_elements[i+1].trim_start_matches("0x"), 16).unwrap())
                        } else {
                            current_command.parameters.push(command_elements[i+1].parse().unwrap());
                        }
                    },  
                }
            },
            ScriptParameter::Pokemon => {
                match enums.pokemon.iter().find(|(_byte, string)| *string == command_elements[i+1]) {
                    Some((byte, _string)) => current_command.parameters.push(byte.parse().unwrap()),
                    None => {
                        if command_elements[i+1].parse::<i32>().is_err() {
                            current_command.parameters.push(i32::from_str_radix(command_elements[i+1].trim_start_matches("0x"), 16).unwrap())
                        } else {
                            current_command.parameters.push(command_elements[i+1].parse().unwrap());
                        }
                    },  
                }
            },
            ScriptParameter::Move => {
                match enums.moves.iter().find(|(_byte, string)| *string == command_elements[i+1]) {
                    Some((byte, _string)) => current_command.parameters.push(byte.parse().unwrap()),
                    None => {
                        if command_elements[i+1].parse::<i32>().is_err() {
                            current_command.parameters.push(i32::from_str_radix(command_elements[i+1].trim_start_matches("0x"), 16).unwrap())
                        } else {
                            current_command.parameters.push(command_elements[i+1].parse().unwrap());
                        }
                    },  
                }
            },
            ScriptParameter::Overworld => {
                if command_elements[i+1] == "Player" {
                    current_command.parameters.push(255);
                } else if command_elements[i+1].starts_with("0x") {
                    current_command.parameters.push(i32::from_str_radix(command_elements[i+1].trim_start_matches("0x"), 16).unwrap())
                } else {
                    current_command.parameters.push(command_elements[i+1].trim_start_matches("Overworld.").parse().unwrap()); 
                }
            },
            ScriptParameter::OwMovementType => {
                if command_elements[i+1].starts_with("0x") {
                    current_command.parameters.push(i32::from_str_radix(command_elements[i+1].trim_start_matches("0x"), 16).unwrap())
                } else {
                    current_command.parameters.push(command_elements[i+1].trim_start_matches("Move.").parse().unwrap()); 
                }
            },
            ScriptParameter::Action => {
                let (first, _last) = command_elements[i+1].split_at(7);
                match first {
                    "Action#" => current_command.parameters.push(command_elements[i+1].trim_start_matches("Action#").parse().unwrap()),
                    _ => unreachable!()
                }
            },
            ScriptParameter::OwMovementDirection => {
                if command_elements[i+1].starts_with("0x") {
                    current_command.parameters.push(i32::from_str_radix(command_elements[i+1].trim_start_matches("0x"), 16).unwrap())
                } else if command_elements[i+1].parse::<i32>().is_err() {
                    match db.overworldDirections.iter().find(|(_byte, string)| *string == command_elements[i+1]){
                        Some((byte, _string)) => current_command.parameters.push(
                            i32::from_str_radix(byte.trim_start_matches("0x"), 16).unwrap()
                        ),
                        None => unreachable!()
                    }
                } else {
                    current_command.parameters.push(command_elements[i+1].parse().unwrap());
                }
            },
            _ => {
                current_command.parameters.push(command_elements[i+1].parse().unwrap());
            }
        }
    }
    // println!("command: {current_command:#?}");
    if let CommandList::Script(commandlist) = &mut current_command_container.commands {
        commandlist.push(current_command);
    }
    Ok(())
}

fn get_parameters_by_condition(command_elements: &Vec<&str>, db_command: &ScrCmd) -> Result<Vec<u8>, ParseError>{
    let mut corrected_parameters: Vec<u8> = vec![2];
    // println!("condition marker: {} condition parameter: {}", db_command.parameters[0], db_command.parameters[1]);
    let mut paramcounter = 2;
    let condition_param = command_elements[1];
    loop {
        if db_command.parameters[paramcounter] == condition_param.parse::<u8>().unwrap() {
            if db_command.parameters[paramcounter+1] > 0 {
                for i in 0..db_command.parameters[paramcounter+1] {
                    corrected_parameters.push(db_command.parameters[paramcounter+2+i as usize])
                }
            }
                        // corrected_parameters.append(&mut db_command.parameters[paramcounter..paramcounter + {
            //         db_command.parameters[paramcounter - 1] as usize
            //     }].to_vec());
            break
        }
        paramcounter += db_command.parameters[paramcounter +1] as usize + 2;
    }
    // println!("{:?}", corrected_parameters);
    Ok(corrected_parameters)
}

fn parse_movement_str(command: &str, current_command_container: &mut CommandContainer, db: &ScriptDatabase, line_index: &usize) -> Result<(), ParseError>{

    let mut current_command = Movement::new();    
    let db_command: &Movements;
    let command_elements: Vec<&str> = command.split_whitespace().collect(); 
    match db.movements.iter().find(|(_byte_code,command)| command.name == command_elements[0]) {
        Some((byte_code, movement)) => {
            current_command.name = movement.name.clone();
            // println!("{byte_code}");
            current_command.id = u16::from_str_radix(&byte_code.trim_start_matches("0x"), 16).unwrap();
            db_command = movement;
        },
        None => return Err(ParseError::NotACommand(format!("{}", command_elements[0]), format!("{line_index}")))
    }
    if command_elements.len() == 2 {
        current_command.parameter = command_elements[1].parse().unwrap();
    }
    if let CommandList::Movement(commandlist) = &mut current_command_container.commands {
        commandlist.push(current_command);
    }
    Ok(())
}


fn parse_script_file_bin(file: &PathBuf, db: &ScriptDatabase, enums: &Enums) -> Result<(), ParseError> {
    // println!("{}", file.display());
    let mut parser = ParserState::new();
    let mut script_file = ScriptFile::new();
    let byte_array: Vec<u8> = std::fs::read(file).unwrap();
    let jump_table_end = match byte_array
        .windows(4)
        .position(|bytes| bytes[0..=1] == [0x13, 0xFD])
    {
        Some(end) => end,
        None => {
            // println!("levelscript or corrupted script detected");
            // jumpt_table_end being 0 prevents rotom from parsing the file at all as it thinks
            // there are no scripts
            0
        }
    };
    if jump_table_end == 0 {
        return Ok(());
    }
    let jump_table = &byte_array[0..jump_table_end];
    let mut script_addresses: Vec<i32> = Vec::new();
    for address in jump_table
        .chunks_exact(4)
        .enumerate()
        .map(|(i, chunks)| {
            let rel_address = i32::from_le_bytes([chunks[0], chunks[1], chunks[2], chunks[3]]);
            let abs_address = rel_address + (i as i32 + 1) * 4;
            abs_address
        }) {
        script_addresses.push(address);
    }
    
    parser.script_offsets = script_addresses;
    parser.script_no += 1;
    for script_offset in parser.script_offsets.clone() {
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
    parser.func_no += 1;
    while parser.function_offsets.len() > 0 {
        for function_offset in parser.function_offsets.clone() {
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
    }
    parser.action_no += 1;
    while parser.action_offsets.len() > 0 {
        for action_offset in parser.action_offsets.clone() {
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
    write_plaintext(file, &mut parser, &script_file, db, enums)?;
    Ok(())
}

fn write_plaintext(
    file: &PathBuf,
    parser: &mut ParserState,
    script_file: &ScriptFile,
    db: &ScriptDatabase,
    enums: &Enums
) -> Result<(), ParseError> {
    let output_filename = format!("{}.script", file.file_name().unwrap().display());
    let output_dir = file
        .ancestors()
        .nth(3)
        .unwrap()
        .join("expanded")
        .join("scripts");
    if !output_dir.exists() {
        std::fs::create_dir_all(&output_dir).expect("couldn't create expanded/scripts directory");
    }
    for container in &script_file.containers {
        parser.output_string.push_str(
            match &container.kind {
                ContainerType::Script => {
                    let mut str = String::new();
                    str.push_str("\n\n");                    
                    for id in &container.reference.id {
                        str.push_str(format!("Script {}:\n", id).as_str())
                    }
                    str = str.trim_end_matches("\n").to_string();
                    str
                }
                ContainerType::Function => {
                    let mut str = String::new();
                    str.push_str("\n\n");                    
                    for id in &container.reference.id {
                        str.push_str(format!("Function {}:\n", id).as_str())
                    }
                    str = str.trim_end_matches("\n").to_string();
                    str
                }
                ContainerType::Action => {
                    let mut str = String::new();
                    str.push_str("\n\n");                    
                    for id in &container.reference.id {
                        str.push_str(format!("Action {}:\n", id).as_str())
                    }
                    str = str.trim_end_matches("\n").to_string();
                    str
                }
            }
            .as_str(),
        );

        match &container.commands {
            CommandList::Script(commands) => {
                for command in commands {
                    let byte_string =
                        format!("0x{}", format!("{:0>4x}", command.id).to_uppercase());
                    let db_command = match db.scrcmd.get(&byte_string) {
                        Some(scrcmd) => scrcmd,
                        _ => unreachable!(""),
                    };
                    if command.name == "Jump" || command.name == "End" || command.name == "Return" {
                        parser
                            .output_string
                            .push_str(format!("\n{} ", command.name).as_str());
                    } else {
                        parser
                            .output_string
                            .push_str(format!("\n\t{} ", command.name).as_str());
                    }
                    for (i, parameter) in command.parameters.iter().enumerate() {
                        let mut formatted_parameter = String::new();
                        // println!(
                        //     "i: {} name: {} command parameters: {:?} db_command parameter types: {:?}, parameter: {}",
                        //     i, command.name, command.parameters, db_command.parameter_types, parameter
                        // );
                        if !db_command.parameter_types.is_empty() {
                            formatted_parameter = match db_command.parameter_types[i] {
                                ScriptParameter::Function => {
                                    match script_file
                                        .containers
                                        .iter()
                                        .find(|command| command.reference.offset == *parameter)
                                        .map(|cmd| cmd)
                                        .expect("finding function/script failed: no container with given offset found")
                                        .kind {
                                            ContainerType::Script => {
                                                format!(
                                            "Script#{}",
                                            script_file
                                            .containers
                                            .iter()
                                            .find(|command| command.reference.offset == *parameter)
                                            .map(|command| command.reference.id.first().unwrap())
                                            .expect(
                                                format!(
                                                    "function offset not found: {}",
                                                    *parameter
                                                )
                                                .as_str()
                                            )
                                            )
                                        },
                                            ContainerType::Function => {format!(
                                        "Function#{}",
                                        script_file
                                            .containers
                                            .iter()
                                            .find(|command| command.reference.offset == *parameter)
                                            .map(|command| command.reference.id.first().unwrap())
                                            .expect(
                                                format!(
                                                    "function offset not found: {}",
                                                    *parameter
                                                )
                                                .as_str()
                                            )
                                    )
                                },
                                            ContainerType::Action => unreachable!("")
                                        }
                                                                    }
                                ScriptParameter::Action => {
                                    format!(
                                        "Action#{}",
                                        script_file
                                            .containers
                                            .iter()
                                            .find(|command| command.reference.offset == *parameter)
                                            .map(|command| command.reference.id.first().unwrap())
                                            .expect(
                                                format!(
                                                    "function offset not found: {}\ncommand: {:?}",
                                                    *parameter,
                                                    command
                                                )
                                                .as_str()
                                            )
                                    )
                                },
                                ScriptParameter::Variable => format!("0x{}", format!("{:x}", parameter).to_uppercase()),
                                ScriptParameter::Integer => format!("{parameter}"),
                                ScriptParameter::ComparisonOperator =>  {   
                                    let byte_string =
                                    format!("0x{}", format!("{:0>4x}", parameter).to_uppercase());
                                    let mut formatted = String::new();
                                    if let Some(val) = db.comparisonOperators.get(&byte_string) {
                                        formatted = val.clone();

                                    } 
                                    formatted
                                },
                                ScriptParameter::OwMovementDirection =>  {   
                                    if parameter < &4 {
                                        let byte_string =
                                        format!("0x{}", format!("{:0>4x}", parameter).to_uppercase());
                                        let mut formatted = String::new();
                                        if let Some(val) = db.overworldDirections.get(&byte_string) {
                                            formatted = val.clone();
                                        } 
                                        formatted
                                    } else if parameter >= &0x4000 {
                                        format!("0x{parameter:x}")
                                    } else {
                                        format!("{}", parameter)
                                    }
                                },
                                ScriptParameter::Sound => {
                                    match db.sounds.get(format!("{}",parameter).as_str()) {
                                        Some(sound) => sound.name.clone(),
                                        None => format!("{parameter}")
                                    } 
                                },
                                ScriptParameter::Overworld => {
                                    if parameter == &255 {
                                        format!("Player")
                                    } else if parameter > &0x4000 {
                                        format!("0x{parameter:x}")
                                    } else {
                                        format!("Overworld.{}", parameter)
                                    }
                                },
                                ScriptParameter::OwMovementType => {
                                    if parameter > &0x4000 {
                                        format!("0x{parameter:x}")
                                    } else {
                                        format!("Move.{}", parameter)
                                    }
                                },
                                ScriptParameter::Trainer => {
                                    match enums.trainers.get(format!("{}", parameter).as_str()) {
                                        Some(trainer) => format!("{trainer}"),
                                        None => format!("{parameter}")
                                    }
                                },
                                ScriptParameter::Item => {
                                    match enums.items.get(format!("{}", parameter).as_str()) {
                                        Some(item) => format!("{item}"),
                                        None => format!("{parameter}")
                                    }
                                },
                                ScriptParameter::Pokemon => {
                                    match enums.pokemon.get(format!("{}", parameter).as_str()) {
                                        Some(pokemon) => format!("{pokemon}"),
                                        None => format!("{parameter}")
                                    }
                                },
                                ScriptParameter::Move => {
                                    match enums.moves.get(format!("{}", parameter).as_str()) {
                                        Some(moves) => format!("{moves}"),
                                        None => format!("{parameter}")
                                    }
                                },
                                _ => {
                                    let str;
                                    if *parameter >= 0x4000 { 
                                        str = format!("0x{}", format!("{:x}", parameter).to_uppercase());
                                    } else { 
                                        str = format!("{parameter}");}
                                    str
                                }
                            };
                        }
                        parser
                            .output_string
                            .push_str(&format!("{} ", formatted_parameter));
                    }
                }
            }
            CommandList::Movement(commands) => {
                for command in commands {
                    if command.name == "End" {
                    parser
                        .output_string
                        .push_str(format!("\n{}", command.name).as_str());
                    } else {
                    parser
                        .output_string
                        .push_str(format!("\n\t{} {}", command.name, command.parameter).as_str());}
                }
            }
        };
    }
    let mut f = std::fs::File::create(output_dir.join(output_filename))
        .expect("failed to create output script file");
    f.write_all(parser.output_string.as_bytes())
        .expect("failed to write script text to file");
    // println!("{}", parser.output_string);
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
    let container_id = match cont_type {
        ContainerType::Script => parser.script_no,
        ContainerType::Function => parser.func_no,
        _ => unreachable!("actions cant be passed to parse_script_function_bytes"),
    };
    let mut current_command_container =
        CommandContainer::new(cont_type.clone(), pc as i32, container_id);
    match file.containers.iter_mut().find(|container| container.reference.offset == pc as i32 ) {
        Some(container) => {
            container.reference.id.push(container_id);
            // println!("{container:#?}");
            return Ok(())
        },
        None => {
            // continue with command container construction
            // println!("no match found for pc: {pc}");
        }
    }
    'read_command_bytes: while end_condition == false {
        let command_bytes: u16 = u16::from_le_bytes([byte_array[pc], byte_array[pc + 1]]);
        let byte_string = format!("0x{}", format!("{command_bytes:0>4x}").to_uppercase());
        let db_command = match db.scrcmd.get(&byte_string) {
            Some(scrcmd) => {
                pc += 2;
                scrcmd
            }
            None => return Err(ParseError::NotACommand(format!("{command_bytes}"), format!("{pc}"))),
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
        let mut command = ScriptCommand {
            id: command_bytes,
            name: db_command.name.clone(),
            parameters: parameter_values,
        };
        if db_command.name == "End" || db_command.name == "Return" || db_command.name == "Jump" {
            end_condition = true
        }
        if db_command.name == "Jump" || db_command.name == "Call" {
            let offset = command.parameters[0] + pc as i32;
            command.parameters[0] = offset;
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
            command.parameters[1] = offset;
            if !file.contains_offset(offset) {
                parser.function_offsets.insert(offset);
            };
        }
        if db_command.name == "Movement" {
            let offset = command.parameters[1] + pc as i32;
            command.parameters[1] = offset;
            if !file.contains_offset(offset) {
                parser.action_offsets.insert(offset);
            };
        }
        // println!("{} {:?}", command.name, command.parameters);
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
    // println!("condition param: {condition_param}");
    paramcounter += 1;
    let mut conditional_paramlist = Vec::new();
    conditional_paramlist.push(*condition_param);
    loop {
        if &db_command.parameters[paramcounter] == &byte_array[*pc] {
            // println!("conditon found!");
            // println!("condition: {}", db_command.parameters[paramcounter]);
            paramcounter += 2;
            conditional_paramlist.append(
                &mut db_command.parameters[paramcounter..paramcounter + {
                    db_command.parameters[paramcounter - 1] as usize
                }]
                    .to_vec(),
            );
            // println!("conditional paramlist: {conditional_paramlist:?}");
            break
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
        CommandContainer::new(cont_type.clone(), pc as i32, parser.action_no);
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
        match &mut current_command_container.commands {
            CommandList::Movement(list) => list.push(movement),
            _ => unreachable!(""),
        }
    }
    file.containers.push(current_command_container);
    Ok(())
}
