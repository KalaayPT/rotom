use super::{
    ast::{
        Expression, ExpressionKind, FunctionHeader, MatchCase, Precedence, ScriptFile, Spanned,
        Statement, StatementKind,
    },
    lexer::Lexer,
    parse_error::{ParseResult, parse_error},
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
    pub fn current_token_is(&self, kind: &TokenType) -> bool {
        self.current_token.kind == *kind
    }
    pub fn current_token_is_keyword(&self) -> bool {
        let kind = self.current_token.kind.clone();
        match kind {
            TokenType::Function
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
            | TokenType::Match
            | TokenType::With
            | TokenType::Case
            | TokenType::EndMatch
            | TokenType::Break
            | TokenType::End
            | TokenType::Return
            | TokenType::Jump
            | TokenType::And
            | TokenType::Or
            | TokenType::As => true,
            _ => false,
        }
    }

    /// Check if we're at a top-level delimiter (next function, label, action, or EOF)
    fn at_top_level_boundary(&self) -> bool {
        match &self.current_token.kind {
            TokenType::Function | TokenType::Action | TokenType::EOF => true,
            // Label: Identifier followed by Colon
            TokenType::Identifier(_) if self.peek_token.kind == TokenType::Colon => true,
            _ => false,
        }
    }
    pub fn expect_advance(&mut self, kind: &TokenType) -> ParseResult<Token> {
        if std::mem::discriminant(&self.current_token.kind) == std::mem::discriminant(kind) {
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
        let mut items: Vec<Statement> = Vec::new();
        let mut function_headers_by_name: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        while !self.current_token_is(&TokenType::EOF) {
            if self.current_token_is(&TokenType::Newline) {
                self.advance();
                continue;
            }
            let stmt = self.parse_top_level_stmt()?;
            match &stmt.node {
                StatementKind::Function { headers, .. } => {
                    // Check if we already have a function with the same name
                    if let Some(first_header) = headers.first() {
                        if function_headers_by_name.contains(&first_header.name) {
                            // Already exists - this is a duplicate body definition
                            return Err(parse_error(
                                stmt.span.clone(),
                                format!(
                                    "Duplicate definition for function '{}'. All headers for a function must be stacked together before the body.",
                                    first_header.name
                                ),
                            ));
                        }
                        // New function - add it
                        function_headers_by_name.insert(first_header.name.clone());
                        items.push(stmt);
                    } else {
                        items.push(stmt);
                    }
                }
                StatementKind::Action { .. } => items.push(stmt),
                StatementKind::AliasStatement { .. } => {
                    aliases.push(stmt.clone());
                    items.push(stmt);
                }
                _ => unreachable!("top_level_stmt should prevent other statements or errors"),
            }
        }
        Ok(ScriptFile {
            aliases,
            items,
            emit_end_marker: true,
        })
    }
    pub fn parse_statement(&mut self) -> ParseResult<Statement> {
        let statement = match self.current_token.kind.clone() {
            TokenType::If => self.parse_if()?,
            TokenType::While => self.parse_while()?,
            TokenType::Match => self.parse_match()?,
            TokenType::Break => {
                let span = self.current_token.span.clone();
                self.advance();
                Spanned {
                    node: StatementKind::Break,
                    span,
                }
            }
            TokenType::Identifier(_) => self.parse_command()?,
            TokenType::Jump => self.parse_jump()?,
            TokenType::End => {
                let span = self.current_token.span.clone();
                self.advance();
                Spanned {
                    node: StatementKind::End,
                    span,
                }
            }
            TokenType::Return => {
                let span = self.current_token.span.clone();
                self.advance();
                Spanned {
                    node: StatementKind::Return,
                    span,
                }
            }
            TokenType::Alias => self.parse_alias()?,
            TokenType::LocalLabel(name) => {
                let start = self.current_token.span.start;
                let label_name = name;
                self.advance();
                self.expect_advance(&TokenType::Colon)?;
                let end = self.current_token.span.start;
                Spanned {
                    node: StatementKind::Label(label_name),
                    span: start..end,
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
        match &self.current_token.kind {
            TokenType::Function => self.parse_function(),
            TokenType::Action => self.parse_action(),
            TokenType::Alias => self.parse_alias(),
            // Bare label: `LabelName:` at top level becomes a private function
            TokenType::Identifier(_) if self.peek_token.kind == TokenType::Colon => {
                self.parse_bare_label()
            }
            _ => {
                let token = self.current_token.clone();
                Err(parse_error(
                    token.span,
                    format!(
                        "Expected top-level definition (function, label, action, or alias), found {}",
                        token.kind
                    ),
                ))
            }
        }
    }

    /// Parse a bare label at top level: `LabelName:` followed by body
    /// This becomes a private function (no jump table entry)
    fn parse_bare_label(&mut self) -> ParseResult<Statement> {
        let start = self.current_token.span.start;
        let name_token = self.expect_advance(&TokenType::Identifier(String::new()))?;
        let TokenType::Identifier(name) = name_token.kind else {
            unreachable!()
        };
        self.expect_advance(&TokenType::Colon)?;
        let body = self.parse_function_body()?;
        let end = self.current_token.span.start;
        Ok(Spanned {
            node: StatementKind::Function {
                headers: vec![FunctionHeader {
                    name,
                    id: None,
                    is_public: false,
                }],
                body,
            },
            span: start..end,
        })
    }
    pub fn parse_function(&mut self) -> ParseResult<Statement> {
        let start = self.current_token.span.start;
        let mut headers = Vec::new();

        // Consume all stacked `function name #N:` headers
        loop {
            if !self.current_token_is(&TokenType::Function) {
                break;
            }

            self.expect_advance(&TokenType::Function)?;
            let name_token = self.expect_advance(&TokenType::Identifier(String::new()))?;
            let TokenType::Identifier(name) = name_token.kind else {
                unreachable!()
            };

            // Require #N for function (public) declarations
            self.expect_advance(&TokenType::Hash)?;
            let id_token = self.expect_advance(&TokenType::Num(0))?;
            let TokenType::Num(num) = id_token.kind else {
                unreachable!()
            };
            let id = num as u32;

            // Require colon after header
            self.expect_advance(&TokenType::Colon)?;

            headers.push(FunctionHeader {
                name,
                id: Some(id),
                is_public: true,
            });

            // Skip newlines between stacked headers
            while self.current_token_is(&TokenType::Newline) {
                self.advance();
            }
        }
        if headers.is_empty() {
            return Err(parse_error(
                self.current_token.span.clone(),
                "Expected function definition with `function name #N:`",
            ));
        }

        // Parse body until next top-level boundary
        let body = self.parse_function_body()?;

        let end = self.current_token.span.start;
        Ok(Spanned {
            node: StatementKind::Function { headers, body },
            span: start..end,
        })
    }

    /// Parse function body until we hit the next function, label, action, or EOF
    fn parse_function_body(&mut self) -> ParseResult<Vec<Statement>> {
        let mut body = Vec::new();

        while !self.current_token_is(&TokenType::EOF) {
            // Check for top-level boundary (next function, label, action)
            if self.at_top_level_boundary() {
                break;
            }

            if self.current_token_is(&TokenType::Newline) {
                self.advance();
                continue;
            }

            body.push(self.parse_statement()?);
        }

        Ok(body)
    }
    pub fn parse_action(&mut self) -> ParseResult<Statement> {
        let start = self.current_token.span.start;
        self.expect_advance(&TokenType::Action)?;
        let name_token = self.expect_advance(&TokenType::Identifier(String::new()))?;
        let TokenType::Identifier(name) = name_token.kind else {
            unreachable!()
        };
        let mut body = self.parse_block(&[TokenType::EndMovement])?;
        if self.current_token_is(&TokenType::EndMovement) {
            let span = self.current_token.span.clone();
            self.advance();
            body.push(Spanned {
                node: StatementKind::EndMovement,
                span,
            });
        } else {
            return Err(parse_error(
                self.current_token.span.clone(),
                "Expected 'EndMovement' to close action",
            ));
        }
        let end = self.current_token.span.start;
        Ok(Spanned {
            node: StatementKind::Action { name, body },
            span: start..end,
        })
    }
    pub fn parse_block(&mut self, end_condition: &[TokenType]) -> ParseResult<Vec<Statement>> {
        let mut block = Vec::new();
        while !self.current_token_is(&TokenType::EOF) {
            if end_condition.contains(&self.current_token.kind) {
                break;
            }
            if self.current_token_is(&TokenType::Newline) {
                self.advance();
                continue;
            }
            block.push(self.parse_statement()?);
        }
        Ok(block)
    }
    pub fn parse_alias(&mut self) -> ParseResult<Statement> {
        let start = self.current_token.span.start;

        self.expect_advance(&TokenType::Alias)?;
        let value = self.parse_expression(Precedence::Lowest)?;
        self.expect_advance(&TokenType::As)?;
        let name_token = self.expect_advance(&TokenType::Identifier(String::new()))?;
        let TokenType::Identifier(name) = name_token.kind else {
            unreachable!()
        };
        let end = self.current_token.span.start;
        Ok(Spanned {
            node: StatementKind::AliasStatement { value, name },
            span: start..end,
        })
    }
    pub fn parse_if(&mut self) -> ParseResult<Statement> {
        let start = self.current_token.span.start;
        self.expect_advance(&TokenType::If)?;
        let condition = self.parse_expression(Precedence::Lowest)?;
        self.expect_advance(&TokenType::Then)?;
        let then_branch = self.parse_block(&[TokenType::Else, TokenType::EndIf])?;
        let else_branch = if self.current_token_is(&TokenType::Else) {
            self.advance();
            if self.current_token_is(&TokenType::If) {
                let elseif = self.parse_if()?;
                Some(vec![elseif])
            } else {
                Some(self.parse_block(&[TokenType::EndIf])?)
            }
        } else {
            None
        };
        self.expect_advance(&TokenType::EndIf)?;
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
        self.expect_advance(&TokenType::While)?;
        let condition = self.parse_expression(Precedence::Lowest)?;
        self.expect_advance(&TokenType::Do)?;
        let body = self.parse_block(&[TokenType::EndWhile])?;
        self.expect_advance(&TokenType::EndWhile)?;

        let end = self.current_token.span.start;

        Ok(Spanned {
            node: StatementKind::WhileStatement {
                condition: Box::new(condition),
                body,
            },
            span: start..end,
        })
    }
    pub fn parse_match(&mut self) -> ParseResult<Statement> {
        let start = self.current_token.span.start;
        self.expect_advance(&TokenType::Match)?;
        let subject = self.parse_expression(Precedence::Lowest)?;
        self.expect_advance(&TokenType::With)?;

        let mut cases = Vec::new();
        let mut default = None;

        while !self.current_token_is(&TokenType::EndMatch)
            && !self.current_token_is(&TokenType::EOF)
        {
            if self.current_token_is(&TokenType::Newline) {
                self.advance();
                continue;
            }

            if self.current_token_is(&TokenType::Else) {
                self.advance();
                self.expect_advance(&TokenType::Colon)?;
                default = Some(self.parse_block(&[TokenType::EndMatch, TokenType::Case])?);
                break;
            }

            if self.current_token_is(&TokenType::Case) {
                let case_start = self.current_token.span.start;
                self.advance();

                let mut values = Vec::new();
                loop {
                    values.push(self.parse_expression(Precedence::Lowest)?);
                    if self.current_token_is(&TokenType::Comma) {
                        self.advance();
                    } else {
                        break;
                    }
                }
                self.expect_advance(&TokenType::Colon)?;
                let body =
                    self.parse_block(&[TokenType::Case, TokenType::Else, TokenType::EndMatch])?;
                let case_end = self.current_token.span.start;

                cases.push(MatchCase {
                    values,
                    body,
                    span: case_start..case_end,
                });
            } else {
                return Err(parse_error(
                    self.current_token.span.clone(),
                    format!(
                        "Expected 'case' or 'else' in match statement, found {}",
                        self.current_token.kind
                    ),
                ));
            }
        }

        self.expect_advance(&TokenType::EndMatch)?;
        let end = self.current_token.span.start;

        Ok(Spanned {
            node: StatementKind::MatchStatement {
                subject: Box::new(subject),
                cases,
                default,
            },
            span: start..end,
        })
    }
    pub fn parse_command(&mut self) -> ParseResult<Statement> {
        let start = self.current_token.span.start;
        let name_token = self.expect_advance(&TokenType::Identifier(String::new()))?;
        let TokenType::Identifier(name) = name_token.kind else {
            unreachable!()
        };
        let mut args = Vec::new();
        if !self.current_token_is(&TokenType::Newline)
            && (!self.current_token_is_keyword()
                || matches!(self.current_token.kind, TokenType::True | TokenType::False))
        {
            loop {
                args.push(self.parse_expression(Precedence::Lowest)?);
                if self.current_token_is(&TokenType::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        if self.current_token_is(&TokenType::Newline) {
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
        self.expect_advance(&TokenType::Jump)?;
        let target_expr = match &self.current_token.kind {
            TokenType::LocalLabel(name) => {
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
    pub fn parse_expression(&mut self, precedence: Precedence) -> ParseResult<Expression> {
        let mut left = self.parse_prefix()?;
        while !self.current_token_is(&TokenType::EOF)
            && !self.current_token_is(&TokenType::End)
            && !self.current_token_is(&TokenType::Comma)
            && !self.current_token_is(&TokenType::RParen)
            && !self.current_token_is(&TokenType::Then)
            && !self.current_token_is(&TokenType::Do)
            && !self.current_token_is(&TokenType::Newline)
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
            TokenType::True => {
                self.advance();
                ExpressionKind::Number(1)
            }
            TokenType::False => {
                self.advance();
                ExpressionKind::Number(0)
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
                self.expect_advance(&TokenType::RParen)?;
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
        if self.current_token_is(&TokenType::LParen) {
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
        if !self.current_token_is(&TokenType::RParen) {
            loop {
                args.push(self.parse_expression(Precedence::Lowest)?);

                if self.current_token_is(&TokenType::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        self.expect_advance(&TokenType::RParen)?;
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
            TokenType::Or => Precedence::LogicalOr,

            TokenType::And => Precedence::LogicalAnd,

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
    fn cur_precedence(&self) -> Precedence {
        Self::get_precedence(&self.current_token.kind)
    }
}

// Legacy constants - may be needed for codegen
pub const JUMP_TABLE_END_MARKER: [u8; 2] = [0x13, 0xFD];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::ast::ExpressionKind;
    use crate::compiler::token::TokenType;

    #[test]
    fn test_parser_initialization() {
        let source = "function TestFunc #1:\nEnd";
        let lexer = Lexer::new(source);
        let parser = Parser::new(lexer);
        assert_eq!(parser.current_token.kind, TokenType::Function);
        assert_eq!(
            parser.peek_token.kind,
            TokenType::Identifier("TestFunc".to_string())
        );
    }

    #[test]
    fn test_parser_advance() {
        let source = "function TestFunc #1:\nEnd";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        parser.advance();
        assert_eq!(
            parser.current_token.kind,
            TokenType::Identifier("TestFunc".to_string())
        );
        assert_eq!(parser.peek_token.kind, TokenType::Hash);
    }

    #[test]
    fn test_expect_advance_success() {
        let source = "function TestFunc #1:\nEnd";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let token = parser.expect_advance(&TokenType::Function).unwrap();
        assert_eq!(token.kind, TokenType::Function);
        assert_eq!(
            parser.current_token.kind,
            TokenType::Identifier("TestFunc".to_string())
        );
    }

    #[test]
    fn test_expect_advance_failure() {
        let source = "function TestFunc #1:\nEnd";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let result = parser.expect_advance(&TokenType::If);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_empty_script_file() {
        let source = "";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();
        assert!(script_file.aliases.is_empty());
        let functions: Vec<_> = script_file
            .items
            .iter()
            .filter(|s| matches!(s.node, StatementKind::Function { .. }))
            .collect();
        let actions: Vec<_> = script_file
            .items
            .iter()
            .filter(|s| matches!(s.node, StatementKind::Action { .. }))
            .collect();
        assert!(functions.is_empty());
        assert!(actions.is_empty());
    }

    #[test]
    fn test_parse_simple_function() {
        let source = "function TestFunc #1:\nEnd";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();
        let functions: Vec<_> = script_file
            .items
            .iter()
            .filter(|s| matches!(s.node, StatementKind::Function { .. }))
            .collect();
        assert_eq!(functions.len(), 1);
        let function = functions[0];
        match &function.node {
            StatementKind::Function { headers, body } => {
                assert_eq!(headers.len(), 1);
                assert_eq!(headers[0].name, "TestFunc");
                assert_eq!(headers[0].id, Some(1));
                assert!(headers[0].is_public);
                assert_eq!(body.len(), 1);
            }
            _ => panic!("Expected function statement"),
        }
    }

    #[test]
    fn test_parse_stacked_function_headers() {
        let source = r"
function TestFunc #1:
function TestFunc #2:
    End
";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        // Should be merged into ONE function item
        let functions: Vec<_> = script_file
            .items
            .iter()
            .filter(|s| matches!(s.node, StatementKind::Function { .. }))
            .collect();
        assert_eq!(functions.len(), 1);

        match &functions[0].node {
            StatementKind::Function { headers, body } => {
                assert_eq!(headers.len(), 2);
                assert_eq!(headers[0].id, Some(1));
                assert_eq!(headers[1].id, Some(2));
                assert_eq!(headers[0].name, "TestFunc");
                assert_eq!(headers[1].name, "TestFunc");
                assert_eq!(body.len(), 1); // Just "End"
            }
            _ => panic!("Expected function statement"),
        }
    }

    #[test]
    fn test_parse_duplicate_function_error() {
        // Test that defining the same function name in separate blocks is an error
        let source = r"
function TestFunc #1:
    Message 1

function TestFunc #2:
    Message 2
    End
";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let result = parser.parse_script_file();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Duplicate definition for function 'TestFunc'")
        );
    }

    #[test]
    fn test_parse_function_with_return() {
        let source = "function TestFunc #1:\nReturn";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();
        let functions: Vec<_> = script_file
            .items
            .iter()
            .filter(|s| matches!(s.node, StatementKind::Function { .. }))
            .collect();
        assert_eq!(functions.len(), 1);
        let function = functions[0];
        match &function.node {
            StatementKind::Function { headers, body } => {
                assert_eq!(headers.len(), 1);
                assert_eq!(headers[0].name, "TestFunc");
                assert_eq!(body.len(), 1);
                match &body[0].node {
                    StatementKind::Return => {}
                    _ => panic!("Expected return statement"),
                }
            }
            _ => panic!("Expected function statement"),
        }
    }

    #[test]
    fn test_parse_bare_label() {
        let source = "MyLabel:\n    Message 1\nEnd";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();
        let functions: Vec<_> = script_file
            .items
            .iter()
            .filter(|s| matches!(s.node, StatementKind::Function { .. }))
            .collect();
        assert_eq!(functions.len(), 1);
        let function = functions[0];
        match &function.node {
            StatementKind::Function { headers, .. } => {
                assert_eq!(headers.len(), 1);
                assert_eq!(headers[0].name, "MyLabel");
                assert_eq!(headers[0].id, None);
                assert!(!headers[0].is_public);
            }
            _ => panic!("Expected function statement"),
        }
    }

    #[test]
    fn test_parse_function_then_label() {
        let source = "function Main #1:\n    Message 1\n\nSecondLabel:\n    Message 2\nEnd";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();
        let functions: Vec<_> = script_file
            .items
            .iter()
            .filter(|s| matches!(s.node, StatementKind::Function { .. }))
            .collect();
        assert_eq!(functions.len(), 2);

        // First function
        match &functions[0].node {
            StatementKind::Function { headers, .. } => {
                assert_eq!(headers[0].name, "Main");
                assert!(headers[0].is_public);
            }
            _ => panic!("Expected function"),
        }

        // Second (bare label)
        match &functions[1].node {
            StatementKind::Function { headers, .. } => {
                assert_eq!(headers[0].name, "SecondLabel");
                assert!(!headers[0].is_public);
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_parse_fallthrough() {
        let source = "function Func1 #1:\n    Message 1\n\nFunc2Label:\n    Message 2\nEnd";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();
        let functions: Vec<_> = script_file
            .items
            .iter()
            .filter(|s| matches!(s.node, StatementKind::Function { .. }))
            .collect();
        assert_eq!(functions.len(), 2);

        // First function has no End in its body
        match &functions[0].node {
            StatementKind::Function { body, .. } => {
                // Should just have the Message command, no End
                assert_eq!(body.len(), 1);
                match &body[0].node {
                    StatementKind::ScriptCommand { command, .. } => {
                        assert_eq!(command, "Message");
                    }
                    _ => panic!("Expected command"),
                }
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_parse_if_else() {
        let source = r"
function TestFunc #1:
    if 0x8000 == 1 then
        Message 1
    else
        Message 2
    endif
    End
";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();
        let functions: Vec<_> = script_file
            .items
            .iter()
            .filter(|s| matches!(s.node, StatementKind::Function { .. }))
            .collect();
        assert_eq!(functions.len(), 1);

        match &functions[0].node {
            StatementKind::Function { body, .. } => {
                // Should have: IfStatement, End
                assert_eq!(body.len(), 2);
                match &body[0].node {
                    StatementKind::IfStatement {
                        condition,
                        body: if_body,
                        elseblock,
                    } => {
                        // Check condition is an infix expression
                        match &condition.node {
                            ExpressionKind::Infix { operator, .. } => {
                                assert_eq!(*operator, TokenType::Equal);
                            }
                            _ => panic!("Expected infix condition"),
                        }
                        // Check if body has one command
                        assert_eq!(if_body.len(), 1);
                        // Check else block exists and has one command
                        assert!(elseblock.is_some());
                        assert_eq!(elseblock.as_ref().unwrap().len(), 1);
                    }
                    _ => panic!("Expected if statement"),
                }
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_parse_while_loop() {
        let source = r"
function TestFunc #1:
    while 0x8000 != 0 do
        SubVar 0x8000, 1
    endwhile
    End
";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();
        let functions: Vec<_> = script_file
            .items
            .iter()
            .filter(|s| matches!(s.node, StatementKind::Function { .. }))
            .collect();
        assert_eq!(functions.len(), 1);

        match &functions[0].node {
            StatementKind::Function { body, .. } => {
                // Should have: WhileStatement, End
                assert_eq!(body.len(), 2);
                match &body[0].node {
                    StatementKind::WhileStatement {
                        condition,
                        body: while_body,
                    } => {
                        // Check condition is an infix expression with !=
                        match &condition.node {
                            ExpressionKind::Infix { operator, .. } => {
                                assert_eq!(*operator, TokenType::NotEqual);
                            }
                            _ => panic!("Expected infix condition"),
                        }
                        // Check while body has one command
                        assert_eq!(while_body.len(), 1);
                    }
                    _ => panic!("Expected while statement"),
                }
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_parse_action() {
        let source = r"
action TestMovement
    WalkNormalNorth 3
    FaceSouth
    EndMovement
";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();
        let actions: Vec<_> = script_file
            .items
            .iter()
            .filter(|s| matches!(s.node, StatementKind::Action { .. }))
            .collect();
        assert_eq!(actions.len(), 1);

        match &actions[0].node {
            StatementKind::Action { name, body } => {
                assert_eq!(name, "TestMovement");
                // Should have: WalkNormalNorth, FaceSouth, EndMovement
                assert_eq!(body.len(), 3);
            }
            _ => panic!("Expected action"),
        }
    }

    #[test]
    fn test_parse_alias() {
        let source = r"
alias 0x800C as VAR_RESULT
alias 0x4000 as VAR_GLOBAL
alias VAR_RESULT as VAR_CHAINED

function TestFunc #1:
    SetVar VAR_RESULT, 5
    End
";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        assert_eq!(script_file.aliases.len(), 3);
        assert_eq!(script_file.items.len(), 4);
        assert!(matches!(
            script_file.items[0].node,
            StatementKind::AliasStatement { .. }
        ));
        assert!(matches!(
            script_file.items[3].node,
            StatementKind::Function { .. }
        ));

        // First alias
        match &script_file.aliases[0].node {
            StatementKind::AliasStatement { name, value, .. } => {
                assert_eq!(name, "VAR_RESULT");
                assert!(matches!(value.node, ExpressionKind::Number(0x800C)));
            }
            _ => panic!("Expected alias statement"),
        }

        // Second alias
        match &script_file.aliases[1].node {
            StatementKind::AliasStatement { name, value, .. } => {
                assert_eq!(name, "VAR_GLOBAL");
                assert!(matches!(value.node, ExpressionKind::Number(0x4000)));
            }
            _ => panic!("Expected alias statement"),
        }

        match &script_file.aliases[2].node {
            StatementKind::AliasStatement { name, value, .. } => {
                assert_eq!(name, "VAR_CHAINED");
                assert!(matches!(
                    value.node,
                    ExpressionKind::Identifier(ref ident) if ident == "VAR_RESULT"
                ));
            }
            _ => panic!("Expected alias statement"),
        }
    }

    #[test]
    fn test_parse_match_statement() {
        let source = r"
function TestFunc #1:
    match 0x8000 with
        case 0:
            Message 1
        case 1, 2:
            Message 2
        else:
            Message 3
    endmatch
    End
";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();
        let functions: Vec<_> = script_file
            .items
            .iter()
            .filter(|s| matches!(s.node, StatementKind::Function { .. }))
            .collect();
        assert_eq!(functions.len(), 1);

        match &functions[0].node {
            StatementKind::Function { body, .. } => {
                assert_eq!(body.len(), 2);
                match &body[0].node {
                    StatementKind::MatchStatement {
                        subject,
                        cases,
                        default,
                    } => {
                        match &subject.node {
                            ExpressionKind::Number(n) => assert_eq!(*n, 0x8000),
                            _ => panic!("Expected number expression"),
                        }
                        assert_eq!(cases.len(), 2);
                        assert_eq!(cases[0].values.len(), 1);
                        assert_eq!(cases[1].values.len(), 2);
                        assert!(default.is_some());
                        assert_eq!(default.as_ref().unwrap().len(), 1);
                    }
                    _ => panic!("Expected match statement"),
                }
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_parse_match_without_else() {
        let source = r"
function TestFunc #1:
    match 0x8000 with
        case 0:
            Message 1
        case 1:
            Message 2
    endmatch
    End
";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();
        let functions: Vec<_> = script_file
            .items
            .iter()
            .filter(|s| matches!(s.node, StatementKind::Function { .. }))
            .collect();
        assert_eq!(functions.len(), 1);

        match &functions[0].node {
            StatementKind::Function { body, .. } => match &body[0].node {
                StatementKind::MatchStatement { cases, default, .. } => {
                    assert_eq!(cases.len(), 2);
                    assert!(default.is_none());
                }
                _ => panic!("Expected match statement"),
            },
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_parse_match_with_keyword() {
        let source = r"
function TestFunc #1:
    match 0x8000 with
        case 0:
            Message 1
        case 1:
            Message 2
    endmatch
    End
";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();
        let functions: Vec<_> = script_file
            .items
            .iter()
            .filter(|s| matches!(s.node, StatementKind::Function { .. }))
            .collect();
        assert_eq!(functions.len(), 1);

        match &functions[0].node {
            StatementKind::Function { body, .. } => match &body[0].node {
                StatementKind::MatchStatement { cases, default, .. } => {
                    assert_eq!(cases.len(), 2);
                    assert!(default.is_none());
                }
                _ => panic!("Expected match statement"),
            },
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_parse_break_statement() {
        let source = r"
function TestFunc #1:
    while 0x8000 != 0 do
        if 0x8000 == 5 then
            break
        endif
        SubVar 0x8000, 1
    endwhile
    End
";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();
        let functions: Vec<_> = script_file
            .items
            .iter()
            .filter(|s| matches!(s.node, StatementKind::Function { .. }))
            .collect();
        assert_eq!(functions.len(), 1);

        match &functions[0].node {
            StatementKind::Function { body, .. } => {
                assert_eq!(body.len(), 2);
                match &body[0].node {
                    StatementKind::WhileStatement {
                        body: while_body, ..
                    } => {
                        assert_eq!(while_body.len(), 2);
                        match &while_body[0].node {
                            StatementKind::IfStatement { body: if_body, .. } => {
                                assert_eq!(if_body.len(), 1);
                                assert!(matches!(&if_body[0].node, StatementKind::Break));
                            }
                            _ => panic!("Expected if statement"),
                        }
                    }
                    _ => panic!("Expected while statement"),
                }
            }
            _ => panic!("Expected function"),
        }
    }
}
