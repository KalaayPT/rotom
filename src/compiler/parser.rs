use linked_hash_set::LinkedHashSet;
use std::collections::HashMap;

use super::{
    ast::{
        Expression, ExpressionKind, FunctionHeader, Precedence, ScriptFile, Spanned, Statement,
        StatementKind,
    },
    lexer::Lexer,
    parse_error::{parse_error, ParseResult},
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
    pub fn current_token_is_keyword(&self) -> bool {
        let kind = self.current_token.kind.clone();
        match kind {
            TokenType::Function
            | TokenType::Public
            | TokenType::Action
            | TokenType::Alias
            | TokenType::True
            | TokenType::False
            | TokenType::If
            | TokenType::Then
            | TokenType::Else
            | TokenType::EndIf
            | TokenType::While
            | TokenType::EndWhile
            | TokenType::End
            | TokenType::Return
            | TokenType::Jump
            | TokenType::And
            | TokenType::Or
            | TokenType::As => true,
            _ => false,
        }
    }
    pub fn expect_advance(&mut self, kind: TokenType) -> ParseResult<Token> {
        if std::mem::discriminant(&self.current_token.kind) == std::mem::discriminant(&kind) {
            let token = self.current_token.clone();
            self.advance();
            Ok(token)
        } else {
            Err(parse_error(
                self.current_token.span.clone(),
                format!(
                    "Unexpected Token. Expected: {}, found: {}",
                    kind, self.current_token.kind
                ),
            ))
        }
    }
    pub fn parse_script_file(&mut self) -> ParseResult<ScriptFile> {
        let mut aliases = Vec::new();
        let mut functions = Vec::new();
        let mut actions = Vec::new();
        while !self.current_token_is(TokenType::EOF) {
            if self.current_token_is(TokenType::Newline) {
                self.advance();
                continue;
            }
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
            TokenType::Label(name) => {
                self.advance();
                Spanned {
                    node: StatementKind::Label(name),
                    span: self.current_token.span.clone(),
                }
            }
            _ => {
                return Err(parse_error(
                    self.current_token.span.clone(),
                    format!(
                        "unexpected statement inside function: {}",
                        self.current_token.kind
                    ),
                ));
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
                Err(parse_error(
                    token.span,
                    format!("Expected top-level definition, found {}", token.kind),
                ))
            }
        }
    }
    pub fn parse_function(&mut self) -> ParseResult<Statement> {
        let start = self.current_token.span.start;
        let mut headers = Vec::new();
        // Consume all "Function X", "Public Function Y #123" lines (for jump-table multi-pointer
        // support)
        loop {
            if !matches!(
                self.current_token.kind,
                TokenType::Function | TokenType::Public
            ) {
                break;
            }
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
            // Optional ID logic (Only if public/explicit)
            let id = if is_public || self.current_token_is(TokenType::Hash) {
                if self.current_token_is(TokenType::Hash) {
                    self.advance(); // eat #
                    let id_token = self.expect_advance(TokenType::Num(0))?;
                    match id_token.kind {
                        TokenType::Num(num) => Some(num),
                        _ => unreachable!(),
                    }
                } else {
                    None
                }
            } else {
                None
            };
            // support "function Name:" syntax (for backwards compat maybe?)
            if self.current_token_is(TokenType::Colon) {
                self.advance();
            }

            headers.push(FunctionHeader {
                name,
                id,
                is_public,
            });
        }
        if headers.is_empty() {
            return Err(parse_error(
                self.current_token.span.clone(),
                "Expected function definition",
            ));
        }
        let mut body = self.parse_block(vec![TokenType::End, TokenType::Return])?;
        let terminator_stmt = if self.current_token_is(TokenType::Return) {
            let span = self.current_token.span.clone();
            self.advance();
            Spanned {
                node: StatementKind::Return,
                span,
            }
        } else if self.current_token_is(TokenType::End) {
            let span = self.current_token.span.clone();
            self.advance();
            Spanned {
                node: StatementKind::End,
                span,
            }
        } else {
            return Err(parse_error(
                self.current_token.span.clone(),
                "Expected 'End' or 'Return' to close function",
            ));
        };
        body.push(terminator_stmt);

        let end = self.current_token.span.start;
        Ok(Spanned {
            node: StatementKind::Function { headers, body },
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
        let mut body = self.parse_block(vec![TokenType::EndMovement])?;
        if self.current_token_is(TokenType::EndMovement) {
            let span = self.current_token.span.clone();
            self.advance();
            body.push(Spanned {
                node: StatementKind::End,
                span,
            })
        } else {
            return Err(parse_error(
                self.current_token.span.clone(),
                "Expected 'EndMovement' to close action",
            ));
        };
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
            if self.current_token_is(TokenType::Newline) {
                self.advance();
                continue;
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
        let start = self.current_token.span.start;
        self.expect_advance(TokenType::While)?;
        let condition = self.parse_expression(Precedence::Lowest)?;
        self.expect_advance(TokenType::Do)?;
        let body = self.parse_block(vec![TokenType::EndWhile])?;
        self.expect_advance(TokenType::EndWhile)?;

        let end = self.current_token.span.start;

        Ok(Spanned {
            node: StatementKind::WhileStatement {
                condition: Box::new(condition),
                body,
            },
            span: start..end,
        })
    }
    pub fn parse_command(&mut self) -> ParseResult<Statement> {
        let start = self.current_token.span.start;
        let name_token = self.expect_advance(TokenType::Identifier(String::new()))?;
        let name = match name_token.kind {
            TokenType::Identifier(s) => s,
            _ => unreachable!(),
        };
        let mut args = Vec::new();
        if !self.current_token_is(TokenType::Newline) && !self.current_token_is_keyword() {
            loop {
                args.push(self.parse_expression(Precedence::Lowest)?);
                if self.current_token_is(TokenType::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        if self.current_token_is(TokenType::Newline) {
            self.advance();
        }
        let end = self.current_token.span.start;
        Ok(Spanned {
            node: StatementKind::ScriptCommand {
                command: name,
                args,
            },
            span: start..end,
        })
    }
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
                return Err(parse_error(
                    self.current_token.span.clone(),
                    format!(
                        "Expected label or identifier as jump target, found {}",
                        self.current_token.kind
                    ),
                ));
            }
        };
        let end = self.current_token.span.start;
        Ok(Spanned {
            node: StatementKind::Jump(target_expr),
            span: start..end,
        })
    }
    fn parse_expression(&mut self, precedence: Precedence) -> ParseResult<Expression> {
        let mut left = self.parse_prefix()?;
        while !self.current_token_is(TokenType::EOF)
            && !self.current_token_is(TokenType::End)
            && !self.current_token_is(TokenType::Comma)
            && !self.current_token_is(TokenType::RParen)
            && precedence < self.cur_precedence()
        {
            left = self.parse_infix(left)?;
        }
        Ok(left)
    }
    fn parse_prefix(&mut self) -> ParseResult<Expression> {
        let start = self.current_token.span.start;
        let kind = match &self.current_token.kind {
            TokenType::Num(val) => {
                let kind = ExpressionKind::Number(*val);
                self.advance();
                kind
            }
            TokenType::Identifier(name) => {
                let kind = ExpressionKind::Identifier(name.clone());
                self.advance();
                kind
            }
            TokenType::Not | TokenType::Minus => {
                let operator = self.current_token.kind.clone();
                self.advance();
                let right = self.parse_expression(Precedence::Prefix)?;
                ExpressionKind::Prefix {
                    operator,
                    id: Box::new(right),
                }
            }
            TokenType::LParen => {
                self.advance();
                let expression = self.parse_expression(Precedence::Lowest)?;
                self.expect_advance(TokenType::RParen)?;
                return Ok(expression);
            }
            _ => {
                return Err(parse_error(
                    self.current_token.span.clone(),
                    format!("Expected expression, found {}", self.current_token.kind),
                ));
            }
        };
        let end = self.current_token.span.start;
        Ok(Spanned {
            node: kind,
            span: start..end,
        })
    }
    fn parse_infix(&mut self, left: Expression) -> ParseResult<Expression> {
        let start = left.span.start;
        if self.current_token_is(TokenType::LParen) {
            return self.parse_call_expression(left);
        }
        let operator = self.current_token.kind.clone();
        let precedence = self.cur_precedence();
        self.advance();
        let right = self.parse_expression(precedence)?;
        let end = right.span.end;
        Ok(Spanned {
            node: ExpressionKind::Infix {
                left: Box::new(left),
                operator,
                right: Box::new(right),
            },
            span: start..end,
        })
    }
    fn parse_call_expression(&mut self, function: Expression) -> ParseResult<Expression> {
        let start = function.span.start;
        self.advance();
        let mut args = Vec::new();
        if !self.current_token_is(TokenType::RParen) {
            loop {
                args.push(self.parse_expression(Precedence::Lowest)?);

                if self.current_token_is(TokenType::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        self.expect_advance(TokenType::RParen)?;
        let end = self.current_token.span.start;
        Ok(Spanned {
            node: ExpressionKind::Call {
                function: Box::new(function),
                args,
            },
            span: start..end,
        })
    }
    fn get_precedence(kind: &TokenType) -> Precedence {
        match kind {
            TokenType::Equal
            | TokenType::NotEqual
            | TokenType::LesserThan
            | TokenType::GreaterThan
            | TokenType::LesserEqual
            | TokenType::GreaterEqual => Precedence::Comparison,

            TokenType::Plus | TokenType::Minus => Precedence::Sum,

            TokenType::Mul => Precedence::Product,

            TokenType::LParen => Precedence::Call,

            _ => Precedence::Lowest,
        }
    }
    fn peek_precedence(&self) -> Precedence {
        Self::get_precedence(&self.peek_token.kind)
    }
    fn cur_precedence(&self) -> Precedence {
        Self::get_precedence(&self.current_token.kind)
    }
}

// Legacy constants - may be needed for codegen
pub const JUMP_TABLE_END_MARKER: [u8; 2] = [0x13, 0xFD];
