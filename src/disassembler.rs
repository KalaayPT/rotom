use std::{error::Error, io::Write, path::PathBuf};

use crate::{
    CommandArgs, Directive,
    cache::write_cache,
    database::{Enums, Movements, ScrCmd, ScriptDatabase, ScriptParameter, read_jsons},
    levelscript::{is_levelscript, parse_levelscript},
    parse_directory,
    parser::{
        CommandContainer, CommandList, ContainerType, Movement, ParseError, ParserState,
        ScriptCommand, ScriptFile,
    },
};

pub fn disassemble(args: CommandArgs) -> Result<(), Box<dyn Error>> {
    let start = std::time::Instant::now();
    let (db, enums) = read_jsons(&args.database)?;
    if args.script_file.is_dir() {
        match parse_directory(&args.script_file, &db, &enums, Directive::Disassemble) {
            Ok(()) => {
                println!(
                    "scripts disassembled to plaintext successfully in {} ms",
                    start.elapsed().as_millis()
                );
            }
            Err(e) => {
                eprintln!("error during parsing: {e}");
            }
        };
    } else {
        match parse_script_file_bin(&args.script_file, &db, &enums) {
            Ok(()) => {
                println!(
                    "script disassembled to plaintext successfully in {} ms",
                    start.elapsed().as_millis()
                );
            }
            Err(e) => {
                eprintln!("error during parsing: {e}");
            }
        };
    }
    write_cache(&args.script_file, db.game_version);
    Ok(())
}

pub fn parse_script_file_bin(
    file: &PathBuf,
    db: &ScriptDatabase,
    enums: &Enums,
) -> Result<(), ParseError> {
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
        if is_levelscript(&byte_array) {
            parse_levelscript(&byte_array, db, &mut parser);
        }
        return Ok(());
    }
    let jump_table = &byte_array[0..jump_table_end];
    let mut script_addresses: Vec<i32> = Vec::new();
    for address in jump_table.chunks_exact(4).enumerate().map(|(i, chunks)| {
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
    for offset in &parser.script_offsets {
        parser.function_offsets.remove(offset);
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
    // println!("{:#?}", script_file);
    write_plaintext(file, &mut parser, &script_file, db, enums)?;
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
    let mut current_command_container = CommandContainer::new(cont_type.clone(), pc as i32);
    current_command_container.reference.id.push(container_id);
    // println!("current container {container_id}, offset {}", pc);
    match file
        .containers
        .iter_mut()
        .find(|container| container.reference.offset == pc as i32)
    {
        Some(container) => {
            container.reference.id.push(container_id);
            // println!("pushed id {} at offset {}", container_id, container.reference.offset);
            // println!("{container:#?}");
            if matches!(current_command_container.kind, ContainerType::Script) {
                return Ok(());
            }
        }
        None => {
            // continue with command container construction
            // println!("no match found for pc: {pc}");
        }
    }
    while end_condition == false {
        let command_bytes: u16 = u16::from_le_bytes([byte_array[pc], byte_array[pc + 1]]);
        let byte_string = format!("0x{}", format!("{command_bytes:0>4x}").to_uppercase());
        let db_command = match db.scrcmd.get(&byte_string) {
            Some(scrcmd) => {
                pc += 2;
                scrcmd
            }
            None => {
                return Err(ParseError::NotACommand(
                    format!("{command_bytes}"),
                    pc as u32,
                ));
            }
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
                // println!("inserted function offset {offset}");
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
                // println!("inserted function offset {offset}");
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
    // println!("{:#?}", current_command_container);
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
            break;
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
    let mut current_command_container = CommandContainer::new(cont_type.clone(), pc as i32);
    current_command_container
        .reference
        .id
        .push(parser.action_no);
    while end_condition == false {
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
            parameter: u16::from_le_bytes([byte_array[pc], byte_array[pc + 1]]),
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
