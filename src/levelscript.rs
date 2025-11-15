use crate::{database::ScriptDatabase, parser::ParserState};

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
    ()
}
