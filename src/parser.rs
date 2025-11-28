use linked_hash_set::LinkedHashSet;
use std::collections::HashMap;

use crate::{
    ast::{ScriptFile, Spanned, Statement, StatementKind},
    database::{Enums, ScriptDatabase},
    lexer::Lexer,
    parse_error::{ParseError, ParseResult},
    token::{Token, TokenType},
};

pub struct Parser<'a> {
    lexer: Lexer<'a>,
    current_token: Token,
    peek_token: Token,
}
impl<'a> Parser<'a> {
    pub fn new(mut lexer: Lexer<'a>) -> Parser<'a> {
        let first = lexer.next_token();
        let second = lexer.next_token();
        Parser {
            lexer,
            current_token: first,
            peek_token: second,
        }
    }
    pub fn advance(&mut self) {
        self.current_token = self.peek_token.clone();
        self.peek_token = self.lexer.next_token();
    }
    pub fn current_token_is(&self, kind: TokenType) -> bool {
        self.current_token.kind == kind
    }
    pub fn expect_advance(&mut self, kind: TokenType) -> ParseResult<Token> {
        if self.current_token_is(kind.clone()) {
            let token = self.current_token.clone();
            self.advance();
            Ok(token)
        } else {
            Err(ParseError {
                span: self.current_token.span.clone(),
                message: format!(
                    "Unexpected Token. Expected: {:?}, found: {:?}",
                    kind, self.current_token.kind
                ),
            })
        }
    }
    pub fn parse_script_file(&mut self) -> ParseResult<ScriptFile> {
        let mut aliases = Vec::new();
        let mut functions = Vec::new();
        let mut actions = Vec::new();
        while !self.current_token_is(TokenType::EOF) {
            let stmt = self.parse_top_level_stmt()?;
            match &stmt.node {
                StatementKind::Function { .. } => functions.push(stmt),
                StatementKind::Action { .. } => actions.push(stmt),
                StatementKind::AliasStatement { .. } => aliases.push(stmt),
                _ => unreachable!("top_level_stmt should prevent other statements or errors"),
            }
        }
        Ok(ScriptFile {
            aliases,
            functions,
            actions,
        })
    }
    pub fn parse_top_level_stmt(&mut self) -> ParseResult<Statement> {
        match self.current_token.kind {
            TokenType::Function | TokenType::Public => self.parse_function(),
            TokenType::Action => self.parse_action(),
            TokenType::Global => self.parse_alias(),
            _ => {
                let token = self.current_token.clone();
                Err(ParseError {
                    span: token.span,
                    message: format!("Expected top-level definition, found {:?}", token.kind),
                })
            }
        }
    }
    pub fn parse_function(&mut self) -> ParseResult<Statement> {
        let start = self.current_token.span.start;
        let is_public = if self.current_token_is(TokenType::Public) {
            self.advance();
            true
        } else {
            false
        };
        self.expect_advance(TokenType::Function)?;
        let name_token = self.expect_advance(TokenType::Identifier(String::new()))?;
        let name = match name_token.kind {
            TokenType::Identifier(name) => name,
            _ => unreachable!(),
        };
        let id = match is_public {
            true => {
                self.expect_advance(TokenType::Hash)?;
                let id_token = self.expect_advance(TokenType::Num(0))?;
                match id_token.kind {
                    TokenType::Num(num) => Some(num),
                    _ => unreachable!(),
                }
            }
            false => None,
        };
        let body = self.parse_block(vec![TokenType::End, TokenType::Return])?;

        let end = self.current_token.span.start;
        Ok(Spanned {
            node: StatementKind::Function {
                is_public,
                name,
                id,
                body,
            },
            span: start..end,
        })
    }
    pub fn parse_action(&mut self) -> ParseResult<Statement> {
        let start = self.current_token.span.start;
        self.expect_advance(TokenType::Action)?;
        let name_token = self.expect_advance(TokenType::Identifier(String::new()))?;
        let name = match name_token.kind {
            TokenType::Identifier(name) => name,
            _ => unreachable!(),
        };
        let body = self.parse_block(vec![TokenType::End])?;

        let end = self.current_token.span.start;
        Ok(Spanned {
            node: StatementKind::Action { name, body },
            span: start..end,
        })
    }
    pub fn parse_block(&mut self, end_condition: Vec<TokenType>) -> ParseResult<Vec<Statement>> {
        let block = Vec::new();
        while !end_condition.contains(&self.current_token.kind) {}
        Ok(block)
    }
    pub fn parse_alias(&mut self) -> ParseResult<Statement> {
        let start = self.current_token.span.start;
        let is_global = if self.current_token_is(TokenType::Global) {
            self.advance();
            true
        } else {
            false
        };
        self.expect_advance(TokenType::Alias)?;
        let id_token = self.expect_advance(TokenType::Num(0))?;
        let id = match id_token.kind {
            TokenType::Num(num) => num,
            _ => unreachable!(),
        };
        self.expect_advance(TokenType::As)?;
        let name_token = self.expect_advance(TokenType::Identifier(String::new()))?;
        let name = match name_token.kind {
            TokenType::Identifier(name) => name,
            _ => unreachable!(),
        };
        let end = self.current_token.span.start;
        Ok(Spanned {
            node: StatementKind::AliasStatement {
                is_global,
                id,
                name,
            },
            span: start..end,
        })
    }
}

pub const CONDITIONAL_PARAM_MARKER: u8 = 255;
pub const JUMP_TABLE_END_MARKER: [u8; 2] = [0x13, 0xFD];

#[derive(Debug)]
pub struct _ScriptFile {
    pub containers: Vec<CommandContainer>,
}
impl _ScriptFile {
    pub fn new() -> _ScriptFile {
        _ScriptFile {
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
