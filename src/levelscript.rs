use crate::{
    database::ScriptDatabase,
    parser::{CommandContainer, ParserState},
};

pub fn is_levelscript(byte_array: &Vec<u8>) -> bool {
    if byte_array.len() == 8 {
        true
    } else if byte_array[6] != 0 {
        true
    } else {
        false
    }
}

pub fn parse_levelscript(byte_array: &Vec<u8>, db: &ScriptDatabase, parser: &mut ParserState) {
    let mut pc: usize = 0;
    parser.script_no += 1;
    let mut end_condition = false;
    let current_command_container = CommandContainer::new(crate::parser::ContainerType::Script, 0);
    while end_condition == false {
        let command_byte: u8;
        let db_command = match db.lvlscrcmd
    }
}
