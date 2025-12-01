use linked_hash_set::LinkedHashSet;
use std::collections::HashMap;

use crate::{
    ast::{ExpressionKind, ScriptFile, Spanned, Statement, StatementKind},
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
    pub fn next_token_is_comparison_op(&self) -> bool {
        if matches!(self.peek_token.kind, TokenType::Equal)
            || matches!(self.peek_token.kind, TokenType::NotEqual)
            || matches!(self.peek_token.kind, TokenType::LesserThan)
            || matches!(self.peek_token.kind, TokenType::GreaterThan)
            || matches!(self.peek_token.kind, TokenType::LesserEqual)
            || matches!(self.peek_token.kind, TokenType::GreaterEqual)
        {
            true
        } else {
            false
        }
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
    pub fn parse_statement(&mut self) -> ParseResult<Statement> {
        let statement = match self.current_token.kind.clone() {
            TokenType::If => self.parse_if()?,
            TokenType::While => self.parse_while()?,
            TokenType::Alias => self.parse_alias()?,
            TokenType::Identifier(_) => self.parse_command()?,
            TokenType::Jump => self.parse_jump()?,
            _ => {
                return Err(ParseError {
                    span: self.current_token.span.clone(),
                    message: format!(
                        "unexpected statement inside function: {:?}",
                        self.current_token.kind
                    ),
                });
            }
        };
        Ok(statement)
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
        // eat terminator
        self.advance();
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
        let mut block = Vec::new();
        while !self.current_token_is(TokenType::EOF) {
            if end_condition.contains(&self.current_token.kind) {
                break;
            }
            block.push(self.parse_statement()?);
        }
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
    pub fn parse_if(&mut self) -> ParseResult<Statement> {
        let start = self.current_token.span.start;
        self.expect_advance(TokenType::If)?;
        let condition = self.parse_expression(Precedence::Lowest)?;
        self.expect_advance(TokenType::Then)?;
        let then_branch = self.parse_block(vec![TokenType::Else, TokenType::EndIf])?;
        let else_branch = if self.current_token_is(TokenType::Else) {
            self.advance();
            if self.current_token_is(TokenType::If) {
                let elseif = self.parse_if()?;
                Some(vec![elseif])
            } else {
                Some(self.parse_block(vec![TokenType::EndIf])?)
            }
        } else {
            None
        };
        self.expect_advance(TokenType::EndIf)?;
        let end = self.current_token.span.start;

        Ok(Spanned {
            node: StatementKind::IfStatement {
                condition: Box::new(condition),
                body: then_branch,
                elseblock: else_branch,
            },
            span: start..end,
        })
    }
    pub fn parse_while(&mut self) -> ParseResult<Statement> {
        Ok(Spanned {
            node: StatementKind::WhileStatement {
                condition: (),
                body: (),
            },
            span: (),
        })
    }
    pub fn parse_command(&mut self) -> ParseResult<Statement> {}
    fn parse_jump(&mut self) -> ParseResult<Statement> {
        let start = self.current_token.span.start;
        self.expect_advance(TokenType::Jump)?;
        let target_expr = match &self.current_token.kind {
            TokenType::Label(name) => {
                let span = self.current_token.span.clone();
                let name_clone = name.clone();
                self.advance();
                Spanned {
                    node: ExpressionKind::Label(name_clone),
                    span,
                }
            }
            TokenType::Identifier(name) => {
                let span = self.current_token.span.clone();
                let name_clone = name.clone();
                self.advance();
                Spanned {
                    node: ExpressionKind::Identifier(name_clone),
                    span,
                }
            }
            _ => {
                return Err(ParseError {
                    span: self.current_token.span.clone(),
                    message: format!(
                        "Expected Label or Function Name, found {:?}",
                        self.current_token.kind
                    ),
                });
            }
        };
        let end = self.current_token.span.start;
        Ok(Spanned {
            node: StatementKind::Jump(target_expr),
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
