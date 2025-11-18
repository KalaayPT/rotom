use linked_hash_set::LinkedHashSet;
use std::collections::HashMap;

use crate::database::{Enums, ScriptDatabase};

pub const CONDITIONAL_PARAM_MARKER: u8 = 255;
pub const SPECIAL_OVERWORLD_PLAYER: i32 = 255;
pub const JUMP_TABLE_END_MARKER: [u8; 2] = [0x13, 0xFD];

#[derive(Debug)]
pub struct ScriptFile {
    pub containers: Vec<CommandContainer>,
}
impl ScriptFile {
    pub fn new() -> ScriptFile {
        ScriptFile {
            containers: Vec::new(),
        }
    }
    pub fn contains_offset(&self, offset: i32) -> bool {
        self.containers
            .iter()
            .any(|command| command.reference.offset == offset)
    }
}

#[derive(Debug, Clone)]
pub enum ContainerType {
    Script,
    Function,
    Action,
    LevelScript,
}
#[derive(Debug)]
pub struct ContainerReference {
    pub id: Vec<u32>,
    pub offset: i32,
}
#[derive(Debug)]
pub struct CommandContainer {
    pub kind: ContainerType,
    pub reference: ContainerReference,
    pub commands: CommandList,
    // called_by: Option<&'static CommandContainer>,
    // calls: Option<&'static CommandContainer>,
}
impl CommandContainer {
    pub fn new(kind: ContainerType, offset: i32) -> CommandContainer {
        CommandContainer {
            commands: match kind {
                ContainerType::Script | ContainerType::Function => CommandList::Script(Vec::new()),
                ContainerType::Action => CommandList::Movement(Vec::new()),
                ContainerType::LevelScript => CommandList::Levelscript(Vec::new()),
            },
            kind: kind,
            reference: ContainerReference {
                id: Vec::new(),
                offset: offset,
            },
        }
    }
}
#[derive(Debug)]
pub enum CommandList {
    Script(Vec<ScriptCommand>),
    Movement(Vec<Movement>),
    Levelscript(Vec<LevelScriptCommand>),
}
#[derive(Debug)]
pub struct ScriptCommand {
    pub id: u16,
    pub name: String,
    pub parameters: Vec<i32>,
}
impl ScriptCommand {
    pub fn new() -> ScriptCommand {
        ScriptCommand {
            id: 0,
            name: String::new(),
            parameters: Vec::new(),
        }
    }
}
#[derive(Debug)]
pub struct Movement {
    pub id: u16,
    pub name: String,
    pub parameter: u16,
}
impl Movement {
    pub fn new() -> Movement {
        Movement {
            id: 0,
            name: String::new(),
            parameter: 0,
        }
    }
}
#[derive(Debug)]
pub struct LevelScriptCommand {
    pub name: String,
    pub parameter: Option<Vec<i32>>,
}
impl LevelScriptCommand {
    pub fn new() -> LevelScriptCommand {
        LevelScriptCommand {
            name: String::new(),
            parameter: None,
        }
    }
}

#[derive(Debug)]
pub enum ParseError {
    NotACommand(String, u32),
    InvalidParameter(usize, u32, String),
    TooManyParameters(usize, usize, u32, String),
}
impl std::error::Error for ParseError {}
impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotACommand(cmd, position) => write!(
                f,
                "failed to read command: {cmd} At line/offset: {position}"
            ),
            Self::InvalidParameter(param, position, cmd) => write!(
                f,
                "invalid parameter {param} at line/offset {position}: {cmd}"
            ),
            Self::TooManyParameters(dblen, cmdlen, position, cmd) => write!(
                f,
                "Too many parameters (excepted: {dblen}, found: {cmdlen}) at line/offset {position}: {cmd}"
            ),
        }
    }
}
pub struct ParserState {
    pub script_no: u32,
    pub func_no: u32,
    pub action_no: u32,
    pub script_offsets: Vec<i32>,
    pub function_offsets: LinkedHashSet<i32>,
    pub action_offsets: LinkedHashSet<i32>,
    pub output_string: String,
    pub relocation_table: Vec<(usize, i32, ContainerType)>,
    pub symbol_table: HashMap<i32, usize>,
    pub symbol_table_movements: HashMap<i32, usize>,
}
impl ParserState {
    pub fn new() -> ParserState {
        ParserState {
            script_no: 0,
            func_no: 0,
            action_no: 0,
            script_offsets: Vec::new(),
            function_offsets: LinkedHashSet::new(),
            action_offsets: LinkedHashSet::new(),
            output_string: String::new(),
            relocation_table: Vec::new(),
            symbol_table: HashMap::new(),
            symbol_table_movements: HashMap::new(),
        }
    }
}
pub struct ParseContext<'a> {
    pub db: &'a ScriptDatabase,
    pub enums: &'a Enums,
}
