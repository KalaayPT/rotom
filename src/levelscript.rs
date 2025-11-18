use std::path::PathBuf;

use anyhow::Result;

use crate::{
    cache::FileCache,
    database::{Enums, ScriptDatabase},
    disassembler::write_plaintext,
    parser::{
        CommandContainer, CommandList, ContainerReference, ContainerType, LevelScriptCommand,
        ParseContext, ParserState, ScriptFile,
    },
};

pub fn is_levelscript(byte_array: &Vec<u8>) -> bool {
    if byte_array.len() == 8 {
        true
    } else if byte_array.iter().nth(6) != Some(&0) {
        true
    } else {
        false
    }
}

pub fn parse_levelscript_bin(
    file: &PathBuf,
    byte_array: &Vec<u8>,
    parser: &mut ParserState,
    file_cache: &mut FileCache,
    script_file: &mut ScriptFile,
    ctx: &ParseContext,
) -> Result<()> {
    let mut pc: usize = 0;
    parser.script_no += 1;
    let has_frame_table = get_levelscripts(&mut pc, script_file, byte_array, ctx.db);
    if has_frame_table {
        get_frame_table(&mut pc, script_file, byte_array, ctx.db);
    }
    write_plaintext(file, parser, script_file, file_cache, &ctx)
}
fn get_levelscripts(
    pc: &mut usize,
    script_file: &mut ScriptFile,
    byte_array: &Vec<u8>,
    db: &ScriptDatabase,
) -> bool {
    let mut end_condition = false;
    let mut has_frame_table = false;
    let mut current_command_container = CommandContainer::new(ContainerType::LevelScript, 0);
    // levelscripts dont have containers per se; only "container" is the frame table, so
    // everything before it is id 0
    current_command_container.reference.id.push(0);
    // get init script entries before frametable
    while end_condition == false {
        let mut current_command = LevelScriptCommand::new();
        let command_byte = byte_array[*pc];
        let (db_cmdname, db_cmddata) = match db.lvlscrcmd.iter().find(|(_name, command)| {
            command.length == Some("u8".to_string()) && command.value == Some(command_byte)
        }) {
            Some((name, command)) => {
                *pc += 1;
                // println!("{name}, byte: {}", command_byte);
                if name == "InitScriptEntryEnd" {
                    end_condition = true
                }
                if name == "InitScript_OnFrameTable" {
                    has_frame_table = true
                }
                (name, command)
            }
            None => unreachable!(),
        };
        current_command.name = db_cmdname.clone();
        if !db_cmddata.parameters.is_empty() {
            current_command.parameter = Some(vec![i32::from_le_bytes([
                byte_array[*pc],
                byte_array[*pc + 1],
                byte_array[*pc + 2],
                byte_array[*pc + 3],
            ])]);
            // println!("{:?}", current_command.parameter);
            *pc += 4;
        }
        if let CommandList::Levelscript(list) = &mut current_command_container.commands {
            list.push(current_command)
        }
    }
    if !has_frame_table {
        if let CommandList::Levelscript(list) = &mut current_command_container.commands {
            list.push(LevelScriptCommand {
                name: String::from("InitScriptEnd"),
                parameter: None,
            })
        }
    }
    script_file.containers.push(current_command_container);
    has_frame_table
}

fn get_frame_table(
    pc: &mut usize,
    script_file: &mut ScriptFile,
    byte_array: &Vec<u8>,
    db: &ScriptDatabase,
) {
    let mut end_condition = false;
    let mut current_command_container = CommandContainer::new(ContainerType::LevelScript, 0);
    current_command_container.reference.id.push(1);
    'outer: while end_condition == false {
        let mut current_command = LevelScriptCommand::new();
        let (db_cmdname, db_cmddata) = match db
            .lvlscrcmd
            .iter()
            .find(|(name, _command)| *name == "InitScriptGoToIfEqual")
        {
            Some((name, command)) => (name, command),
            None => unreachable!(),
        };
        current_command.name = db_cmdname.clone();
        current_command.parameter = Some(Vec::new());
        for parameter in &db_cmddata.parameters {
            // println!("pc start of loop: {pc}");
            if byte_array[*pc] == 0 && byte_array[*pc + 1] == 0 {
                current_command.name = "InitScriptFrameTableEnd".to_string();
                current_command.parameter = None;
                *pc += *parameter as usize;
                // println!(
                //     "command: {}, parameter: {:?}, pc: {}",
                //     current_command.name, current_command.parameter, pc
                // );
                end_condition = true;
                match &mut current_command_container.commands {
                    CommandList::Levelscript(list) => {
                        list.push(current_command);
                        list.push(LevelScriptCommand {
                            name: String::from("InitScriptEnd"),
                            parameter: None,
                        });
                    }
                    _ => unreachable!(""),
                }
                continue 'outer;
            }
            if let Some(vec) = &mut current_command.parameter {
                vec.push(i32::from_le_bytes([
                    byte_array[*pc],
                    byte_array[*pc + 1],
                    0,
                    0,
                ]));
            }
            // println!("{:?}", current_command.parameter);
            *pc += *parameter as usize;
            // println!("pc: {pc}");
        }
        match &mut current_command_container.commands {
            CommandList::Levelscript(list) => list.push(current_command),
            _ => unreachable!(""),
        }
    }
    script_file.containers.push(current_command_container);
}
