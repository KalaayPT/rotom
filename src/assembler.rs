use std::{collections::HashMap, io::Write, path::PathBuf};

use anyhow::{Result, anyhow};
use chrono::Local;
use regex::{Match, Regex};

use crate::{
    CommandArgs, Directive, ParseResult,
    cache::{BuildStatus, FileCache, read_cache, write_cache},
    database::{Enums, ScrCmd, ScriptDatabase, ScriptParameter, read_jsons},
    helpers::parse_number_str,
    parse_directory,
    parser::{
        CommandContainer, CommandList, ContainerType, Movement, ParseError, ParserState,
        ScriptCommand, ScriptFile,
    },
};

pub fn assemble(args: CommandArgs) -> Result<()> {
    let start = std::time::Instant::now();
    let mut cachemap: HashMap<String, FileCache> = HashMap::new();
    let (db, enums) = read_jsons(&args.database)?;
    let cache = read_cache(&args.script_file)?;
    if args.script_file.is_dir() {
        parse_directory(
            &args.script_file,
            &db,
            &enums,
            &cache,
            Directive::Assemble,
            &mut cachemap,
        )?
    } else {
        let regex = Regex::new(r"(?m)^\w+ [\w\d#]+:")?;
        let result = parse_plaintext_file(&args.script_file, &db, &enums, &regex);
        match result {
            Ok(parse_result) => {
                cachemap.insert(parse_result.file_name, parse_result.cache_entry);
            }
            Err(e) => {
                eprintln!("Error processing {}: {}", args.script_file.display(), e);
                let name = args
                    .script_file
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                cachemap.insert(
                    name,
                    FileCache {
                        status: crate::cache::BuildStatus::Error,
                        // hash: get_hash(&args.script_file)?
                        //     .iter()
                        //     .map(|byte| format!("{byte:02x}"))
                        //     .collect(),
                        error_message: Some(e.to_string()),
                        build_time: Local::now(),
                    },
                );
            }
        }
    }
    write_cache(&args.script_file, db.game_version, cachemap)?;
    println!("success: {} ms", start.elapsed().as_millis());
    Ok(())
}

pub fn parse_plaintext_file(
    file: &PathBuf,
    db: &ScriptDatabase,
    enums: &Enums,
    regex: &Regex,
) -> Result<ParseResult> {
    // println!("file: {}", file.display());
    let file_name = file
        .file_name()
        .ok_or(anyhow!("couldnt asses file name"))?
        .to_str()
        .ok_or(anyhow!("couldnt convert file name to string"))?
        .to_string();
    let mut file_cache = FileCache::new();
    let mut script_file = ScriptFile::new();
    let mut parser = ParserState::new();
    let text = std::fs::read_to_string(file)?;
    for container in find_command_containers(&text, regex) {
        // println!("{container}");
        parse_plaintext_container(&container, &db, &enums, &mut parser, &mut script_file)?;
    }
    // print!("{script_file:#?}");
    write_binary(file, &mut parser, &mut script_file, db, &mut file_cache)?;
    Ok(ParseResult {
        file_name,
        cache_entry: file_cache,
    })
}

fn find_command_containers<'a>(
    text: &'a str,
    regex: &'a Regex,
) -> impl Iterator<Item = &'a str> + 'a {
    let matches: Vec<Match> = regex.find_iter(&text).collect();
    let mut start_indices: Vec<usize> = Vec::new();
    if !matches.is_empty() {
        start_indices.push(matches[0].start());
    }
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
    start_indices
        .into_iter()
        .zip(end_indices)
        .map(move |(start, end)| text[start..end].trim())
}

fn write_binary(
    file: &PathBuf,
    parser: &mut ParserState,
    script_file: &mut ScriptFile,
    db: &ScriptDatabase,
    file_cache: &mut FileCache,
) -> Result<()> {
    // println!("{:#?}", script_file);
    let mut pc: usize = 0;
    let mut byte_array: Vec<u8> = Vec::with_capacity(256 * 256);
    let jump_table_entries = script_file
        .containers
        .iter()
        .filter(|container| matches!(container.kind, ContainerType::Script))
        .map(|container| container.reference.id.len())
        .sum();
    let jump_table: Vec<i32> = vec![0; jump_table_entries];
    // println!("{:?}\n entries: {}", jump_table, jump_table_entries);
    byte_array.extend(jump_table.iter().flat_map(|chunk| chunk.to_le_bytes()));
    byte_array.extend([0x13, 0xfd]);
    pc += &jump_table.len() * 4 + 2;
    // println!("first script offset: {pc}");
    for container in &mut script_file.containers {
        container.reference.offset = pc as i32;
        match container.kind {
            ContainerType::Script => {
                for id in &container.reference.id {
                    parser.symbol_table.insert(*id as i32 * -1, pc);
                }
            }
            ContainerType::Function => {
                for id in &container.reference.id {
                    parser.symbol_table.insert(*id as i32, pc);
                }
            }
            ContainerType::Action => {
                parser
                    .symbol_table_movements
                    .insert(container.reference.id[0] as i32, pc);
            }
        }
        if let CommandList::Script(command_list) = &container.commands {
            for command_to_write in command_list {
                let (command_byte, db_command) = db
                    .scrcmd
                    .iter()
                    .find(|(_byte, command)| command.name == command_to_write.name)
                    .ok_or(anyhow!("command not found!"))?;
                byte_array.append(
                    &mut { parse_number_str(command_byte)? as u16 }
                        .to_le_bytes()
                        .to_vec(),
                );
                pc += 2;
                byte_array.extend(command_to_write.parameters.iter().enumerate().flat_map(
                    |(i, param)| {
                        if matches!(db_command.parameter_types[i], ScriptParameter::Function) {
                            parser.relocation_table.push((
                                pc,
                                command_to_write.parameters[i],
                                ContainerType::Function,
                            ));
                        } else if matches!(db_command.parameter_types[i], ScriptParameter::Action) {
                            parser.relocation_table.push((
                                pc,
                                command_to_write.parameters[i],
                                ContainerType::Action,
                            ));
                        }
                        let mut db_parameters = db_command.parameters.clone();
                        if db_command.parameters.first() == Some(&255) {
                            db_parameters = get_parameters_by_condition(
                                command_to_write.parameters[0] as u8,
                                db_command,
                            )
                            .unwrap();
                        }
                        let param_bytes_vec = match db_parameters[i] {
                            1 => {
                                pc += 1;
                                { *param as u8 }.to_le_bytes().to_vec()
                            }
                            2 => {
                                pc += 2;
                                { *param as u16 }.to_le_bytes().to_vec()
                            }
                            4 => {
                                pc += 4;
                                { *param as u32 }.to_le_bytes().to_vec()
                            }
                            _ => unreachable!("illegal param was: {}", db_command.parameters[i]),
                        };
                        param_bytes_vec
                    },
                ));
            }
        } else if let CommandList::Movement(command_list) = &container.commands {
            // println!("added offset for movement {}: {}", container.reference.id.first().unwrap(), container.reference.offset);
            for command_to_write in command_list {
                if let Some((command_byte, _db_command)) = db
                    .movements
                    .iter()
                    .find(|(_byte, command)| command.name == command_to_write.name)
                {
                    byte_array.extend(parse_number_str(command_byte)?.to_le_bytes());
                } else {
                    byte_array.extend(parse_number_str(&command_to_write.name)?.to_le_bytes());
                };
                pc += 2;
                byte_array.extend(command_to_write.parameter.to_le_bytes());
                pc += 2;
            }
        }
    }
    for i in 0..jump_table_entries {
        parser.relocation_table.push((
            { i * 4 },
            match script_file
                .containers
                .iter()
                .find(|container| container.reference.id.contains(&{ i as u32 + 1 }))
            {
                Some(_container) => -{ i as i32 + 1 },
                None => unreachable!(),
            },
            ContainerType::Script,
        ));
    }
    // println!("relocation table: {:#x?}", parser.relocation_table);
    // println!("symbol table: {:#x?}\nsymbol table movements: {:#x?}", parser.symbol_table, parser.symbol_table_movements);

    linker(&mut byte_array, parser)?;
    while byte_array.len() % 2 != 0 {
        byte_array.push(0);
    }

    let output_filename = format!(
        "{}",
        file.file_name()
            .ok_or(anyhow!("couldnt assess file name"))?
            .display()
            .to_string()
            .trim_end_matches(".script")
    );
    let output_dir = file
        .ancestors()
        .nth(3)
        .ok_or(anyhow!("couldnt find ROM root"))?
        .join("unpacked")
        .join("testscripts"); //TODO: 
    std::fs::create_dir_all(&output_dir).expect("couldn't create unpacked/scripts directory"); //TODO:
    let mut f = std::fs::File::create(output_dir.join(&output_filename))
        .expect("failed to create output script file");
    f.write_all(&byte_array)
        .expect("failed to write script binary to file");
    file_cache.status = BuildStatus::Success;
    file_cache.build_time = Local::now();
    // file_cache.hash = get_hash(&output_dir.join(output_filename))?
    //     .iter()
    //     .map(|byte| format!("{byte:02x}"))
    //     .collect();
    Ok(())
}

fn linker(byte_array: &mut Vec<u8>, parser: &ParserState) -> Result<()> {
    for (entry, container, kind) in &parser.relocation_table {
        let chunk = &mut byte_array[*entry..*entry + 4];
        // println!("{:x?}",chunk);
        match kind {
            ContainerType::Script | ContainerType::Function => {
                // println!("container: {container:x}");
                let container_ref = {
                    *parser
                        .symbol_table
                        .get(container)
                        .ok_or(anyhow!("couldnt find container with given offset"))?
                        as i32
                        - *entry as i32
                        - 4
                }
                .to_le_bytes();
                // println!("bytes: {container_ref:?}");
                chunk.copy_from_slice(&container_ref);
            }
            ContainerType::Action => {
                let container_ref = {
                    *parser
                        .symbol_table_movements
                        .get(container)
                        .ok_or(anyhow!("couldnt find container with given offset"))?
                        as i32
                        - *entry as i32
                        - 4
                }
                .to_le_bytes();
                chunk.copy_from_slice(&container_ref);
            }
        }
    }
    Ok(())
}

fn parse_plaintext_container(
    container_str: &str,
    db: &ScriptDatabase,
    enums: &Enums,
    parser: &mut ParserState,
    script_file: &mut ScriptFile,
) -> Result<()> {
    let mut current_command_container = CommandContainer::new(ContainerType::Script, 0);
    let mut end_condition = false;
    let mut header_index = 0;
    while !end_condition {
        if let Some(container_line) = container_str.lines().nth(header_index) {
            // println!("{container_line}");
            end_condition =
                parse_container_header(container_line, &mut current_command_container, parser)?;
            header_index += 1;
        } else {
            break;
        }
    }
    // println!("{}",parser.script_no);
    for (i, line) in container_str.lines().skip(header_index - 1).enumerate() {
        parse_command_str(line, &mut current_command_container, &db, i, &enums)?;
    }
    // println!("{current_command_container:#?}");
    script_file.containers.push(current_command_container);
    Ok(())
}

fn parse_container_header(
    container_line: &str,
    current_command_container: &mut CommandContainer,
    parser: &mut ParserState,
) -> Result<bool> {
    // println!("header line: {container_line}");
    match container_line.split_whitespace().next().unwrap() {
        "Script" => {
            // parser.script_no += 1;
            current_command_container.kind = ContainerType::Script;
            current_command_container.reference.id.push(
                container_line
                    .split_whitespace()
                    .nth(1)
                    .ok_or(anyhow!("container number not found"))?
                    .trim_end_matches(":")
                    .parse()?,
            );
        }
        "Function" => {
            parser.func_no += 1;
            current_command_container.kind = ContainerType::Function;
            current_command_container.reference.id.push(parser.func_no);
        }
        "Action" => {
            parser.action_no += 1;
            current_command_container.kind = ContainerType::Action;
            current_command_container
                .reference
                .id
                .push(parser.action_no);
            current_command_container.commands = CommandList::Movement(Vec::new())
        }
        _ => return Ok(true),
    };
    Ok(false)
}

fn parse_command_str(
    command: &str,
    current_command_container: &mut CommandContainer,
    db: &ScriptDatabase,
    line_index: usize,
    enums: &Enums,
) -> Result<()> {
    // println!("{command}");
    if matches!(current_command_container.kind, ContainerType::Action) {
        parse_movement_str(&command, current_command_container, &db, &line_index)?;
        return Ok(());
    }
    let mut current_command = ScriptCommand::new();
    let db_command: &ScrCmd;
    let command_elements: Vec<&str> = command.split_whitespace().collect();
    match db
        .scrcmd
        .iter()
        .find(|(_byte_code, command)| command.name == command_elements[0])
    {
        Some((byte_code, scrcmd)) => {
            current_command.name = scrcmd.name.clone();
            // println!("{byte_code}");
            current_command.id = parse_number_str(&byte_code)? as u16;
            db_command = scrcmd;
        }
        None => {
            return Err(ParseError::NotACommand(
                format!("{}", command_elements[0]),
                line_index as u32,
            )
            .into());
        }
    }
    let mut db_parameters = db_command.parameters.clone();
    if db_command.parameters.first() == Some(&255) {
        db_parameters = get_parameters_by_condition(command_elements[1].parse()?, db_command)?;
    }
    // println!("line {line_index}: {command}");
    if command_elements.len() > db_parameters.len() + 1 {
        return Err(ParseError::TooManyParameters(
            db_parameters.len(),
            command_elements.len() - 1,
            line_index as u32,
            command.to_string(),
        )
        .into());
    }
    for (i, _parameter) in db_parameters.iter().enumerate() {
        if command_elements.len() == 1 {
            continue;
        }
        // println!("command parameter: {}", command_elements[i+1]);
        match db_command.parameter_types[i] {
            ScriptParameter::Sound => {
                match db
                    .sounds
                    .iter()
                    .find(|(_byte, sound)| sound.name == command_elements[i + 1])
                {
                    Some((byte, _sound)) => {
                        current_command.parameters.push(byte.parse()?);
                    }
                    None => {
                        current_command
                            .parameters
                            .push(parse_number_str(command_elements[i + 1])?);
                    }
                }
            }
            ScriptParameter::Variable | ScriptParameter::Flex => current_command
                .parameters
                .push(parse_number_str(command_elements[i + 1])?),
            ScriptParameter::ComparisonOperator => {
                match db
                    .comparisonOperators
                    .iter()
                    .find(|(_byte, string)| *string == command_elements[i + 1])
                {
                    Some((byte, _string)) => {
                        current_command.parameters.push(parse_number_str(byte)?)
                    }
                    None => unreachable!(),
                }
            }
            ScriptParameter::Function => {
                let (first, _last) = command_elements[i + 1].split_at(4);
                match first {
                    // negative for scripts, positive for functions, later gets corrected to
                    // actual offsets
                    "Func" => current_command.parameters.push(
                        command_elements[i + 1]
                            .trim_start_matches("Function#")
                            .parse()?,
                    ),
                    "Scri" => current_command.parameters.push(
                        -command_elements[i + 1]
                            .trim_start_matches("Script#")
                            .parse::<i32>()?,
                    ),
                    _ => unreachable!(),
                }
            }
            ScriptParameter::Item => {
                match enums
                    .items
                    .iter()
                    .find(|(_byte, string)| *string == command_elements[i + 1])
                {
                    Some((byte, _string)) => current_command.parameters.push(byte.parse()?),
                    None => current_command
                        .parameters
                        .push(parse_number_str(command_elements[i + 1])?),
                }
            }
            ScriptParameter::Trainer => {
                match enums
                    .trainers
                    .iter()
                    .find(|(_byte, string)| *string == command_elements[i + 1])
                {
                    Some((byte, _string)) => current_command.parameters.push(byte.parse()?),
                    None => current_command
                        .parameters
                        .push(parse_number_str(command_elements[i + 1])?),
                }
            }
            ScriptParameter::Pokemon => {
                match enums
                    .pokemon
                    .iter()
                    .find(|(_byte, string)| *string == command_elements[i + 1])
                {
                    Some((byte, _string)) => current_command.parameters.push(byte.parse()?),
                    None => current_command
                        .parameters
                        .push(parse_number_str(command_elements[i + 1])?),
                }
            }
            ScriptParameter::Move => {
                match enums
                    .moves
                    .iter()
                    .find(|(_byte, string)| *string == command_elements[i + 1])
                {
                    Some((byte, _string)) => current_command.parameters.push(byte.parse()?),
                    None => current_command
                        .parameters
                        .push(parse_number_str(command_elements[i + 1])?),
                }
            }
            ScriptParameter::Overworld => {
                if command_elements[i + 1] == "Player" {
                    current_command.parameters.push(255);
                } else if command_elements[i + 1].starts_with("0x") {
                    current_command
                        .parameters
                        .push(parse_number_str(command_elements[i + 1])?)
                } else {
                    current_command.parameters.push(
                        command_elements[i + 1]
                            .trim_start_matches("Overworld.")
                            .parse()?,
                    );
                }
            }
            ScriptParameter::OwMovementType => {
                if command_elements[i + 1].starts_with("0x") {
                    current_command
                        .parameters
                        .push(parse_number_str(command_elements[i + 1])?)
                } else {
                    current_command.parameters.push(
                        command_elements[i + 1]
                            .trim_start_matches("Move.")
                            .parse()?,
                    );
                }
            }
            ScriptParameter::Action => {
                let (first, _last) = command_elements[i + 1].split_at(7);
                match first {
                    "Action#" => current_command.parameters.push(
                        command_elements[i + 1]
                            .trim_start_matches("Action#")
                            .parse()?,
                    ),
                    _ => unreachable!(),
                }
            }
            ScriptParameter::OwMovementDirection => {
                if command_elements[i + 1].starts_with("0x") {
                    current_command
                        .parameters
                        .push(parse_number_str(command_elements[i + 1])?)
                } else {
                    match db
                        .overworldDirections
                        .iter()
                        .find(|(_byte, string)| *string == command_elements[i + 1])
                    {
                        Some((byte, _string)) => current_command
                            .parameters
                            .push(i32::from_str_radix(byte.trim_start_matches("0x"), 16)?),
                        None => unreachable!(),
                    }
                }
            }
            _ => current_command
                .parameters
                .push(parse_number_str(command_elements[i + 1])?),
        }
    }
    // println!("command: {current_command:#?}");
    if let CommandList::Script(commandlist) = &mut current_command_container.commands {
        commandlist.push(current_command);
    }
    Ok(())
}

fn get_parameters_by_condition(condition_param: u8, db_command: &ScrCmd) -> Result<Vec<u8>> {
    let mut corrected_parameters: Vec<u8> = vec![2];
    // println!("condition marker: {} condition parameter: {}", db_command.parameters[0], db_command.parameters[1]);
    let mut paramcounter = 2;
    // let condition_param = command_elements[1];
    loop {
        if db_command.parameters[paramcounter] == condition_param {
            if db_command.parameters[paramcounter + 1] > 0 {
                for i in 0..db_command.parameters[paramcounter + 1] {
                    corrected_parameters.push(db_command.parameters[paramcounter + 2 + i as usize])
                }
            }
            break;
        }
        paramcounter += db_command.parameters[paramcounter + 1] as usize + 2;
    }
    // println!("{:?}", corrected_parameters);
    Ok(corrected_parameters)
}

fn parse_movement_str(
    command: &str,
    current_command_container: &mut CommandContainer,
    db: &ScriptDatabase,
    line_index: &usize,
) -> Result<()> {
    let mut current_command = Movement::new();
    // let db_command: &Movements;
    let command_elements: Vec<&str> = command.split_whitespace().collect();
    match db
        .movements
        .iter()
        .find(|(_byte_code, command)| command.name == command_elements[0])
    {
        Some((byte_code, movement)) => {
            current_command.name = movement.name.clone();
            // println!("{byte_code}");
            current_command.id = parse_number_str(&byte_code)? as u16;
        }
        None => {
            current_command.name = command_elements[0].to_string();
            current_command.id = parse_number_str(command_elements[1])? as u16;
        }
    }
    if command_elements.len() == 2 {
        current_command.parameter = command_elements[1].parse()?;
    }
    if let CommandList::Movement(commandlist) = &mut current_command_container.commands {
        commandlist.push(current_command);
    }
    Ok(())
}
