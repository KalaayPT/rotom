use super::{
    ast::{
        Expression, ExpressionKind, FunctionHeader, MatchCase, MenuConfig, MenuEntry, Precedence,
        ScriptFile, Spanned, Statement, StatementKind,
    },
    diagnostic::{CompileError, ParseResult, parse_error},
    lexer::Lexer,
    token::{Token, TokenType},
};

pub struct Parser<'a> {
    lexer: Lexer<'a>,
    current_token: Token,
    peek_token: Token,
    /// When `true`, the parser recovers from syntax errors by inserting
    /// `StatementKind::Error` nodes and collecting diagnostics in `errors`.
    recover: bool,
    /// Collected diagnostics when `recover` is `true`.
    pub errors: Vec<CompileError>,
}

impl<'a> Parser<'a> {
    pub fn new(mut lexer: Lexer<'a>) -> Parser<'a> {
        let first = lexer.next_token();
        let second = lexer.next_token();
        Parser {
            lexer,
            current_token: first,
            peek_token: second,
            recover: false,
            errors: Vec::new(),
        }
    }

    /// Create a parser with error recovery enabled.
    ///
    /// When recovery is on, `parse_statement` and friends insert
    /// `StatementKind::Error` nodes into the AST and continue parsing
    /// instead of returning `Err`.  Collected diagnostics can be retrieved
    /// with `std::mem::take(&mut parser.errors)` after parsing finishes.
    pub fn new_fallible(mut lexer: Lexer<'a>) -> Parser<'a> {
        let first = lexer.next_token();
        let second = lexer.next_token();
        Parser {
            lexer,
            current_token: first,
            peek_token: second,
            recover: true,
            errors: Vec::new(),
        }
    }

    pub fn advance(&mut self) {
        self.current_token = self.peek_token.clone();
        self.peek_token = self.lexer.next_token();
    }
    pub fn current_token_is(&self, kind: &TokenType) -> bool {
        self.current_token.kind == *kind
    }
    /// Consume newlines where the menu builder grammar permits line breaks.
    fn skip_newlines(&mut self) {
        while self.current_token_is(&TokenType::Newline) {
            self.advance();
        }
    }
    pub fn current_token_is_keyword(&self) -> bool {
        matches!(
            self.current_token.kind,
            TokenType::Script
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
                | TokenType::As
        )
    }

    /// Check if we're at a top-level delimiter (next script, label, action, or EOF)
    fn at_top_level_boundary(&self) -> bool {
        match &self.current_token.kind {
            TokenType::Script | TokenType::Action | TokenType::EOF => true,
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
        let mut script_headers_by_name: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        while !self.current_token_is(&TokenType::EOF) {
            if self.current_token_is(&TokenType::Newline) {
                self.advance();
                continue;
            }
            let stmt = match self.parse_top_level_stmt() {
                Ok(s) => s,
                Err(e) => {
                    if self.recover {
                        self.errors.push(e);
                        self.synchronize_top_level();
                        continue;
                    }
                    return Err(e);
                }
            };
            match &stmt.node {
                StatementKind::Function { headers, .. } => {
                    if let Some(first_header) = headers.first() {
                        if script_headers_by_name.contains(&first_header.name) {
                            let msg = format!(
                                "Duplicate definition for script '{}'. All headers for a script must be stacked together before the body.",
                                first_header.name
                            );
                            if self.recover {
                                self.errors.push(parse_error(stmt.span.clone(), msg));
                            } else {
                                return Err(parse_error(stmt.span.clone(), msg));
                            }
                        } else {
                            script_headers_by_name.insert(first_header.name.clone());
                        }
                    }
                    items.push(stmt);
                }
                StatementKind::Action { .. }
                | StatementKind::Include { .. }
                | StatementKind::Define { .. } => items.push(stmt),
                StatementKind::AliasStatement { .. } => {
                    aliases.push(stmt.clone());
                    items.push(stmt);
                }
                _ if self.recover => items.push(stmt),
                _ => unreachable!("top_level_stmt should prevent other statements or errors"),
            }
        }
        Ok(ScriptFile {
            aliases,
            items,
            jump_table_end_marker_count: 1,
        })
    }

    /// Skip tokens until we reach a safe top-level boundary.
    fn synchronize_top_level(&mut self) {
        while !self.current_token_is(&TokenType::EOF) {
            if self.at_top_level_boundary() {
                return;
            }
            self.advance();
        }
    }

    /// Skip tokens until we reach a safe statement boundary.
    fn synchronize_statement(&mut self) {
        while !self.current_token_is(&TokenType::EOF) {
            if self.at_top_level_boundary() {
                return;
            }
            match &self.current_token.kind {
                TokenType::If
                | TokenType::While
                | TokenType::Match
                | TokenType::Break
                | TokenType::End
                | TokenType::Return
                | TokenType::EndMovement
                | TokenType::Alias
                | TokenType::Jump
                | TokenType::Newline => {
                    self.advance();
                    return;
                }
                _ => self.advance(),
            }
        }
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
            TokenType::Identifier(name)
                if (name == "Menu" || name == "MenuGlobal")
                    && self.peek_token.kind == TokenType::LParen =>
            {
                self.parse_menu_builder()?
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
            _ => {
                return Err(parse_error(
                    self.current_token.span.clone(),
                    format!(
                        "unexpected statement inside script/function: {}",
                        self.current_token.kind
                    ),
                ));
            }
        };
        Ok(statement)
    }
    pub fn parse_top_level_stmt(&mut self) -> ParseResult<Statement> {
        match &self.current_token.kind {
            TokenType::Script => self.parse_function(),
            TokenType::Action => self.parse_action(),
            TokenType::Alias => self.parse_alias(),
            TokenType::Include => self.parse_include(),
            TokenType::Define => self.parse_define(),
            // Bare label: `LabelName:` at top level becomes a private helper
            TokenType::Identifier(_) if self.peek_token.kind == TokenType::Colon => {
                self.parse_bare_label()
            }
            _ => {
                let token = self.current_token.clone();
                Err(parse_error(
                    token.span,
                    format!(
                        "Expected top-level definition (script, label, action, or alias), found {}",
                        token.kind
                    ),
                ))
            }
        }
    }

    /// Parse a bare label at top level: `LabelName:` followed by body
    /// This becomes a private helper (no jump table entry)
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

        // Consume all stacked `script name #N:` headers
        loop {
            if !self.current_token_is(&TokenType::Script) {
                break;
            }

            self.expect_advance(&TokenType::Script)?;
            let name_token = self.expect_advance(&TokenType::Identifier(String::new()))?;
            let TokenType::Identifier(name) = name_token.kind else {
                unreachable!()
            };

            // Require #N or #[list] for script declarations
            self.expect_advance(&TokenType::Hash)?;

            let ids: Vec<u32> = if self.current_token_is(&TokenType::LBracket) {
                // #[N, M-P, Q, ...] — bracketed list with optional ranges
                self.advance(); // consume '['
                let mut ids = Vec::new();
                loop {
                    let lo_token = self.expect_advance(&TokenType::Num(0))?;
                    let TokenType::Num(lo) = lo_token.kind else {
                        unreachable!()
                    };
                    let lo = lo as u32;
                    if self.current_token_is(&TokenType::Minus) {
                        self.advance(); // consume '-'
                        let hi_token = self.expect_advance(&TokenType::Num(0))?;
                        let TokenType::Num(hi) = hi_token.kind else {
                            unreachable!()
                        };
                        let hi = hi as u32;
                        if hi < lo {
                            return Err(parse_error(
                                lo_token.span,
                                format!("Range {lo}-{hi} is empty: end must be >= start"),
                            ));
                        }
                        ids.extend(lo..=hi);
                    } else {
                        ids.push(lo);
                    }
                    if self.current_token_is(&TokenType::Comma) {
                        self.advance(); // consume ','
                    } else {
                        break;
                    }
                }
                self.expect_advance(&TokenType::RBracket)?;
                ids
            } else {
                let id_token = self.expect_advance(&TokenType::Num(0))?;
                let TokenType::Num(num) = id_token.kind else {
                    unreachable!()
                };
                vec![num as u32]
            };

            // Require colon after header
            self.expect_advance(&TokenType::Colon)?;

            for id in ids {
                headers.push(FunctionHeader {
                    name: name.clone(),
                    id: Some(id),
                    is_public: true,
                });
            }

            // Skip newlines between stacked headers
            while self.current_token_is(&TokenType::Newline) {
                self.advance();
            }
        }
        if headers.is_empty() {
            return Err(parse_error(
                self.current_token.span.clone(),
                "Expected script definition with `script name #N:`",
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

    /// Parse function body until we hit the next function, label, action, or EOF.
    fn parse_function_body(&mut self) -> ParseResult<Vec<Statement>> {
        let mut body = Vec::new();

        while !self.current_token_is(&TokenType::EOF) {
            if self.at_top_level_boundary() {
                break;
            }

            if self.current_token_is(&TokenType::Newline) {
                self.advance();
                continue;
            }

            match self.parse_statement() {
                Ok(stmt) => body.push(stmt),
                Err(e) if self.recover => {
                    let span = self.current_token.span.clone();
                    self.errors.push(e);
                    body.push(Spanned {
                        node: StatementKind::Error,
                        span,
                    });
                    self.synchronize_statement();
                }
                Err(e) => return Err(e),
            }
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
        self.expect_advance(&TokenType::Colon)?;
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
            // If we hit a top-level keyword the block was never properly closed.
            if matches!(
                self.current_token.kind,
                TokenType::Script | TokenType::Action
            ) {
                break;
            }
            if self.current_token_is(&TokenType::Newline) {
                self.advance();
                continue;
            }
            match self.parse_statement() {
                Ok(stmt) => block.push(stmt),
                Err(e) if self.recover => {
                    let span = self.current_token.span.clone();
                    self.errors.push(e);
                    block.push(Spanned {
                        node: StatementKind::Error,
                        span,
                    });
                    self.synchronize_statement();
                }
                Err(e) => return Err(e),
            }
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
    pub fn parse_include(&mut self) -> ParseResult<Statement> {
        let start = self.current_token.span.start;
        self.expect_advance(&TokenType::Include)?;
        let path_token = self.expect_advance(&TokenType::String(vec![]))?;
        let TokenType::String(path_segs) = path_token.kind else {
            unreachable!()
        };
        let path = path_segs.into_iter().map(|(s, _)| s).collect::<String>();
        let end = self.current_token.span.start;
        Ok(Spanned {
            node: StatementKind::Include { path },
            span: start..end,
        })
    }
    pub fn parse_define(&mut self) -> ParseResult<Statement> {
        let start = self.current_token.span.start;
        self.expect_advance(&TokenType::Define)?;
        let name_token = self.expect_advance(&TokenType::Identifier(String::new()))?;
        let TokenType::Identifier(name) = name_token.kind else {
            unreachable!()
        };
        let value = self.parse_expression(Precedence::Lowest)?;
        let end = self.current_token.span.start;
        Ok(Spanned {
            node: StatementKind::Define { name, value },
            span: start..end,
        })
    }
    pub fn parse_if(&mut self) -> ParseResult<Statement> {
        let start = self.current_token.span.start;
        self.expect_advance(&TokenType::If)?;
        let condition = self.parse_expression(Precedence::Lowest)?;
        self.expect_advance(&TokenType::Then)?;
        let then_branch = self.parse_block(&[TokenType::Else, TokenType::EndIf])?;
        let (else_branch, chain_continued) = if self.current_token_is(&TokenType::Else) {
            self.advance();
            if self.current_token_is(&TokenType::If) {
                // `else if` continues the chain: the recursive call consumes
                // the single shared `endif` that terminates the whole chain, so
                // this frame must not expect another one.
                let elseif = self.parse_if()?;
                (Some(vec![elseif]), true)
            } else {
                (Some(self.parse_block(&[TokenType::EndIf])?), false)
            }
        } else {
            (None, false)
        };
        if !chain_continued {
            self.expect_advance(&TokenType::EndIf)?;
        }
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
        if self.current_token_is(&TokenType::LParen) {
            // Call-style: CommandName(arg1, arg2)
            self.advance();
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
        } else if !self.current_token_is(&TokenType::Newline)
            && (!self.current_token_is_keyword()
                || matches!(self.current_token.kind, TokenType::True | TokenType::False))
        {
            // Space-separated: CommandName arg1, arg2
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
    /// Parse a `Menu(...)` / `MenuGlobal(...)` builder with a `.method()` chain.
    fn parse_menu_builder(&mut self) -> ParseResult<Statement> {
        let start = self.current_token.span.start;
        let name_token = self.expect_advance(&TokenType::Identifier(String::new()))?;
        let TokenType::Identifier(name) = name_token.kind else {
            unreachable!()
        };
        let is_global = name == "MenuGlobal";

        self.expect_advance(&TokenType::LParen)?;
        let mut entries = Vec::new();
        self.skip_newlines();
        while !self.current_token_is(&TokenType::RParen) {
            let (label, hover) = self.parse_menu_entry_label()?;
            self.expect_advance(&TokenType::Arrow)?;
            let target = self.parse_expression(Precedence::Lowest)?;
            entries.push(MenuEntry {
                label,
                hover,
                target,
            });
            if self.current_token_is(&TokenType::Comma) {
                self.advance();
                self.skip_newlines();
            } else {
                break;
            }
        }
        self.skip_newlines();
        self.expect_advance(&TokenType::RParen)?;

        let mut config = MenuConfig::default();
        self.skip_newlines();
        // The lexer folds `.foo` into a single dot-prefixed `Identifier(".foo")`
        // token (for label syntax), so a builder method chain `.method(args)`
        // arrives as `Identifier(".method")` followed by `(`. A dot-prefixed
        // identifier followed by `:` is a label definition, not a method call,
        // so it ends the chain.
        while matches!(
            &self.current_token.kind,
            TokenType::Identifier(name)
                if name.starts_with('.') && name.len() > 1 && self.peek_token.kind == TokenType::LParen
        ) {
            let method = match &self.current_token.kind {
                TokenType::Identifier(name) => name[1..].to_string(),
                _ => unreachable!(),
            };
            self.advance();
            self.expect_advance(&TokenType::LParen)?;
            self.apply_menu_method(&method, &mut config, &mut entries)?;
            self.skip_newlines();
            self.expect_advance(&TokenType::RParen)?;
            self.skip_newlines();
        }

        let end = self.current_token.span.start;
        Ok(Spanned {
            node: StatementKind::MenuBuilder {
                is_global,
                entries,
                config: Box::new(config),
            },
            span: start..end,
        })
    }
    /// Parse the label part of a menu entry: `"text"` or `("text", "hover")`.
    fn parse_menu_entry_label(&mut self) -> ParseResult<(Expression, Option<Expression>)> {
        if self.current_token_is(&TokenType::LParen) {
            self.advance();
            let label = self.parse_expression(Precedence::Lowest)?;
            self.expect_advance(&TokenType::Comma)?;
            self.skip_newlines();
            let hover = self.parse_expression(Precedence::Lowest)?;
            self.skip_newlines();
            self.expect_advance(&TokenType::RParen)?;
            Ok((label, Some(hover)))
        } else {
            let label = self.parse_expression(Precedence::Lowest)?;
            Ok((label, None))
        }
    }
    /// Parse one menu builder method into its configuration and entries.
    fn apply_menu_method(
        &mut self,
        method: &str,
        config: &mut MenuConfig,
        entries: &mut Vec<MenuEntry>,
    ) -> ParseResult<()> {
        match method {
            "anchor" => {
                let side_token = self.expect_advance(&TokenType::Identifier(String::new()))?;
                let TokenType::Identifier(side) = side_token.kind else {
                    unreachable!()
                };
                let right = match side.as_str() {
                    "left" => false,
                    "right" => true,
                    other => {
                        return Err(parse_error(
                            side_token.span,
                            format!("anchor expects 'left' or 'right', got '{other}'"),
                        ));
                    }
                };
                config.anchor = Some(right);
            }
            "scrollable" => {
                config.scrollable = Some(if self.current_token_is(&TokenType::RParen) {
                    true
                } else {
                    self.parse_bool_arg()?
                });
            }
            "position" => {
                let x = self.parse_expression(Precedence::Lowest)?;
                self.expect_advance(&TokenType::Comma)?;
                self.skip_newlines();
                let y = self.parse_expression(Precedence::Lowest)?;
                config.position = Some((x, y));
            }
            "cursor" => config.cursor = Some(self.parse_expression(Precedence::Lowest)?),
            "prompt" => config.prompt = Some(self.parse_expression(Precedence::Lowest)?),
            "width" => config.width = Some(self.parse_expression(Precedence::Lowest)?),
            "columns" => config.columns = Some(self.parse_expression(Precedence::Lowest)?),
            "cancel" => {
                let (label, hover) = self.parse_menu_entry_label()?;
                if self.current_token_is(&TokenType::Arrow) {
                    self.advance();
                    let target = self.parse_expression(Precedence::Lowest)?;
                    config.cancel = Some(target.clone());
                    entries.push(MenuEntry {
                        label,
                        hover,
                        target,
                    });
                } else {
                    if hover.is_some() {
                        return Err(parse_error(
                            self.current_token.span.clone(),
                            "cancel hover text requires '-> target'".to_string(),
                        ));
                    }
                    config.cancel = Some(label);
                }
            }
            other => {
                return Err(parse_error(
                    self.current_token.span.clone(),
                    format!("unknown Menu builder method '.{other}'"),
                ));
            }
        }
        Ok(())
    }
    /// Parse a literal boolean menu builder argument.
    fn parse_bool_arg(&mut self) -> ParseResult<bool> {
        if self.current_token_is(&TokenType::True) {
            self.advance();
            Ok(true)
        } else if self.current_token_is(&TokenType::False) {
            self.advance();
            Ok(false)
        } else {
            Err(parse_error(
                self.current_token.span.clone(),
                "expected 'true' or 'false'".to_string(),
            ))
        }
    }
    fn parse_jump(&mut self) -> ParseResult<Statement> {
        let start = self.current_token.span.start;
        self.expect_advance(&TokenType::Jump)?;
        let target_expr = match &self.current_token.kind {
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
                let name = name.clone();
                self.advance();
                // Cross-file script reference `file::label`: an identifier
                // immediately followed by `::` and a second identifier.
                if self.current_token_is(&TokenType::DoubleColon) {
                    self.advance();
                    match &self.current_token.kind {
                        TokenType::Identifier(label) => {
                            let label = label.clone();
                            self.advance();
                            ExpressionKind::ModuleRef {
                                module: name,
                                label,
                            }
                        }
                        _ => {
                            return Err(parse_error(
                                self.current_token.span.clone(),
                                format!(
                                    "Expected a label after '::', found {}",
                                    self.current_token.kind
                                ),
                            ));
                        }
                    }
                } else {
                    ExpressionKind::Identifier(name)
                }
            }
            TokenType::String(s) => {
                let kind = ExpressionKind::String(s.clone());
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
        let source = "script TestFunc #1:\nEnd";
        let lexer = Lexer::new(source);
        let parser = Parser::new(lexer);
        assert_eq!(parser.current_token.kind, TokenType::Script);
        assert_eq!(
            parser.peek_token.kind,
            TokenType::Identifier("TestFunc".to_string())
        );
    }

    #[test]
    fn test_parser_advance() {
        let source = "script TestFunc #1:\nEnd";
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
        let source = "script TestFunc #1:\nEnd";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let token = parser.expect_advance(&TokenType::Script).unwrap();
        assert_eq!(token.kind, TokenType::Script);
        assert_eq!(
            parser.current_token.kind,
            TokenType::Identifier("TestFunc".to_string())
        );
    }

    #[test]
    fn test_expect_advance_failure() {
        let source = "script TestFunc #1:\nEnd";
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
        assert!(
            !script_file
                .items
                .iter()
                .any(|s| matches!(s.node, StatementKind::Function { .. }))
        );
        assert!(
            !script_file
                .items
                .iter()
                .any(|s| matches!(s.node, StatementKind::Action { .. }))
        );
    }

    #[test]
    fn test_parse_simple_script() {
        let source = "script TestFunc #1:\nEnd";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();
        let functions: Vec<_> = script_file
            .items
            .iter()
            .filter(|s| matches!(s.node, StatementKind::Function { .. }))
            .collect();
        assert_eq!(functions.len(), 1);
        let script = functions[0];
        match &script.node {
            StatementKind::Function { headers, body } => {
                assert_eq!(headers.len(), 1);
                assert_eq!(headers[0].name, "TestFunc");
                assert_eq!(headers[0].id, Some(1));
                assert!(headers[0].is_public);
                assert_eq!(body.len(), 1);
            }
            _ => panic!("Expected script statement"),
        }
    }

    #[test]
    fn test_parse_stacked_script_headers() {
        let source = r"
script TestFunc #1:
script TestFunc #2:
    End
";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        // Should be merged into ONE script item
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
            _ => panic!("Expected script statement"),
        }
    }

    #[test]
    fn test_parse_duplicate_function_error() {
        // Test that defining the same script name in separate blocks is an error
        let source = r"
script TestFunc #1:
    Message 1

script TestFunc #2:
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
                .contains("Duplicate definition for script 'TestFunc'")
        );
    }

    #[test]
    fn test_parse_function_with_return() {
        let source = "script TestFunc #1:\nReturn";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();
        let functions: Vec<_> = script_file
            .items
            .iter()
            .filter(|s| matches!(s.node, StatementKind::Function { .. }))
            .collect();
        assert_eq!(functions.len(), 1);
        let script = functions[0];
        match &script.node {
            StatementKind::Function { headers, body } => {
                assert_eq!(headers.len(), 1);
                assert_eq!(headers[0].name, "TestFunc");
                assert_eq!(body.len(), 1);
                match &body[0].node {
                    StatementKind::Return => {}
                    _ => panic!("Expected return statement"),
                }
            }
            _ => panic!("Expected script statement"),
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
        let script = functions[0];
        match &script.node {
            StatementKind::Function { headers, .. } => {
                assert_eq!(headers.len(), 1);
                assert_eq!(headers[0].name, "MyLabel");
                assert_eq!(headers[0].id, None);
                assert!(!headers[0].is_public);
            }
            _ => panic!("Expected script statement"),
        }
    }

    #[test]
    fn test_parse_function_then_label() {
        let source = "script Main #1:\n    Message 1\n\nSecondLabel:\n    Message 2\nEnd";
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
            _ => panic!("Expected script"),
        }

        // Second (bare label)
        match &functions[1].node {
            StatementKind::Function { headers, .. } => {
                assert_eq!(headers[0].name, "SecondLabel");
                assert!(!headers[0].is_public);
            }
            _ => panic!("Expected script"),
        }
    }

    #[test]
    fn test_parse_fallthrough() {
        let source = "script Func1 #1:\n    Message 1\n\nFunc2Label:\n    Message 2\nEnd";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();
        let functions: Vec<_> = script_file
            .items
            .iter()
            .filter(|s| matches!(s.node, StatementKind::Function { .. }))
            .collect();
        assert_eq!(functions.len(), 2);

        // First script has no End in its body
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
            _ => panic!("Expected script"),
        }
    }

    #[test]
    fn test_parse_if_else() {
        let source = r"
script TestFunc #1:
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
            _ => panic!("Expected script"),
        }
    }

    #[test]
    fn test_parse_if_elseif_chain() {
        // `else if` chains share a single terminating `endif`, regardless of
        // how many branches the chain has.
        let source = r"
script TestFunc #1:
    if 0x80C == 0 then
        GoTo TestInBed
    else if 0x40F == 0 then
        GoTo HalloweenEventInit
    else if 0x40F == 1 then
        GoTo TestInBed
    else
        GoTo TestInBed
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
                // IfStatement, End
                assert_eq!(body.len(), 2);
                // Walk the else-if chain: each `else if` nests an IfStatement
                // in the else block. Three `if`s -> three nested IfStatements,
                // terminated by a single shared `endif`.
                let mut depth = 0usize;
                let mut node = &body[0];
                while let StatementKind::IfStatement { elseblock, .. } = &node.node {
                    depth += 1;
                    match elseblock {
                        Some(branch) if branch.len() == 1 => node = &branch[0],
                        _ => break,
                    }
                }
                assert_eq!(depth, 3);
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_parse_while_loop() {
        let source = r"
script TestFunc #1:
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
            _ => panic!("Expected script"),
        }
    }

    #[test]
    fn test_parse_action() {
        let source = r"
action TestMovement:
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

script TestFunc #1:
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
script TestFunc #1:
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
            _ => panic!("Expected script"),
        }
    }

    #[test]
    fn test_parse_match_without_else() {
        let source = r"
script TestFunc #1:
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
            _ => panic!("Expected script"),
        }
    }

    #[test]
    fn test_parse_match_with_keyword() {
        let source = r"
script TestFunc #1:
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
            _ => panic!("Expected script"),
        }
    }

    #[test]
    fn test_parse_break_statement() {
        let source = r"
script TestFunc #1:
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
            _ => panic!("Expected script"),
        }
    }

    // ------------------------------------------------------------------
    // Error-tolerant parsing tests
    // ------------------------------------------------------------------

    #[test]
    fn test_fallible_parser_recovers_from_bad_statement() {
        let source = r"
script TestFunc #1:
    Message 1
    garbage_token !@#
    Message 2
    End
";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new_fallible(lexer);
        let file = parser.parse_script_file().unwrap();
        let errors = std::mem::take(&mut parser.errors);

        // Should still produce a function with three statements (including error)
        assert!(!errors.is_empty(), "expected at least one error");
        let functions: Vec<_> = file
            .items
            .iter()
            .filter(|s| matches!(s.node, StatementKind::Function { .. }))
            .collect();
        assert_eq!(functions.len(), 1);
        match &functions[0].node {
            StatementKind::Function { body, .. } => {
                // Message 1, Error, Message 2, End
                assert_eq!(body.len(), 4);
                assert!(matches!(&body[1].node, StatementKind::Error));
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_fallible_parser_recovers_at_top_level() {
        // `!!!` is not a valid token sequence, so it will cause a parse error
        // that the fallible parser must recover from.
        let source = r"
script First #1:
    End

!!! invalid garbage

script Second #2:
    End
";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new_fallible(lexer);
        let file = parser.parse_script_file().unwrap();
        let errors = std::mem::take(&mut parser.errors);

        assert!(!errors.is_empty(), "expected at least one error");
        assert_eq!(
            file.items
                .iter()
                .filter(|s| matches!(s.node, StatementKind::Function { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn test_fallible_parser_empty_source() {
        let lexer = Lexer::new("");
        let mut parser = Parser::new_fallible(lexer);
        let file = parser.parse_script_file().unwrap();
        let errors = std::mem::take(&mut parser.errors);
        assert!(errors.is_empty());
        assert!(file.items.is_empty());
    }

    #[test]
    fn test_fallible_parser_recovers_from_missing_endwhile() {
        // Missing `endwhile` followed by another top-level script used to
        // cause an infinite loop because `synchronize_statement` would
        // return without advancing past the `script` boundary token.
        let source = r"
script First #1:
    while true do
        Message 1
script Second #2:
    End
";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new_fallible(lexer);
        let file = parser.parse_script_file().unwrap();
        let errors = std::mem::take(&mut parser.errors);

        assert!(
            !errors.is_empty(),
            "expected at least one error for missing endwhile"
        );
        assert_eq!(
            file.items
                .iter()
                .filter(|s| matches!(s.node, StatementKind::Function { .. }))
                .count(),
            2,
            "both scripts should still be parsed"
        );
    }

    #[test]
    fn test_parse_command_call_style_multiple_args() {
        // Call-style commands like TeachMovesScreen(1, 1) should parse
        // correctly with multiple parenthesized arguments.
        let source = r"
script Test #1:
    TeachMovesScreen(1, 1)
    End
";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let file = parser.parse_script_file().unwrap();
        let functions: Vec<_> = file
            .items
            .iter()
            .filter(|s| matches!(s.node, StatementKind::Function { .. }))
            .collect();
        assert_eq!(functions.len(), 1);
        match &functions[0].node {
            StatementKind::Function { body, .. } => {
                assert_eq!(body.len(), 2); // command + End
                match &body[0].node {
                    StatementKind::ScriptCommand { command, args } => {
                        assert_eq!(command, "TeachMovesScreen");
                        assert_eq!(args.len(), 2);
                    }
                    _ => panic!("Expected ScriptCommand"),
                }
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_parse_module_ref_command_argument() {
        let source = r"
script Test #1:
    CallCommonScript scripts_common::NewGame
    End
";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let file = parser.parse_script_file().unwrap();

        let StatementKind::Function { body, .. } = &file.items[0].node else {
            panic!("Expected function");
        };
        let StatementKind::ScriptCommand { command, args } = &body[0].node else {
            panic!("Expected script command");
        };
        assert_eq!(command, "CallCommonScript");
        assert!(matches!(
            &args[0].node,
            ExpressionKind::ModuleRef { module, label }
                if module == "scripts_common" && label == "NewGame"
        ));
        assert_eq!(
            args[0].to_macro_arg_source().unwrap(),
            "scripts_common::NewGame"
        );
    }

    #[test]
    fn test_parse_numeric_module_ref_preserves_filename_stem() {
        let source = "script Test #1:\n    CallCommonScript 0211::NewGame\n    End\n";
        let mut parser = Parser::new(Lexer::new(source));
        let file = parser.parse_script_file().unwrap();

        let StatementKind::Function { body, .. } = &file.items[0].node else {
            panic!("Expected function");
        };
        let StatementKind::ScriptCommand { args, .. } = &body[0].node else {
            panic!("Expected script command");
        };
        assert!(matches!(
            &args[0].node,
            ExpressionKind::ModuleRef { module, label }
                if module == "0211" && label == "NewGame"
        ));
    }

    #[test]
    fn test_parse_menu_builder_allows_multiline_chain_and_hover_tuple() {
        let source = r#"
script Test #1:
    Menu(
        ("Option", "Hover"
        ) -> Option,
        10 -> Other,
    )
    .scrollable()
    .cursor(1)
    .cancel(("Cancel", "Back") -> Cancel)
    End

Option:
    End
Other:
    End
Cancel:
    End
"#;
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let StatementKind::Function { body, .. } = &script_file.items[0].node else {
            panic!("Expected function");
        };
        let StatementKind::MenuBuilder {
            entries, config, ..
        } = &body[0].node
        else {
            panic!("Expected menu builder");
        };
        assert_eq!(entries.len(), 3);
        assert!(entries[0].hover.is_some());
        assert!(entries[2].hover.is_some());
        assert_eq!(config.scrollable, Some(true));
        assert!(matches!(
            config.cancel.as_ref().map(|expr| &expr.node),
            Some(ExpressionKind::Identifier(label)) if label == "Cancel"
        ));
        assert!(matches!(
            config.cursor.as_ref().map(|expr| &expr.node),
            Some(ExpressionKind::Number(1))
        ));
    }

    #[test]
    fn test_parse_menu_builder_scrollable_accepts_explicit_false() {
        let source = r"
script Test #1:
    Menu(10 -> Target).scrollable(false)
    End

Target:
    End
";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let StatementKind::Function { body, .. } = &script_file.items[0].node else {
            panic!("Expected function");
        };
        let StatementKind::MenuBuilder { config, .. } = &body[0].node else {
            panic!("Expected menu builder");
        };
        assert_eq!(config.scrollable, Some(false));
    }

    #[test]
    fn test_parse_preprocessor_directives() {
        // #include and #define should be skipped at the top level.
        let source = "#include \"stdpoke.txt\"\n#define SOME_CONST 42\n\nscript Test #1:\n    Message SOME_CONST\n    End\n";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let file = parser.parse_script_file().unwrap();
        assert!(
            parser.errors.is_empty(),
            "expected no errors for preprocessor directives"
        );
        assert_eq!(
            file.items
                .iter()
                .filter(|s| matches!(s.node, StatementKind::Function { .. }))
                .count(),
            1,
            "expected one script after preprocessor directives"
        );
    }
}
