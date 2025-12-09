use std::{iter::Peekable, str::Chars};

use crate::token::{Token, TokenType};

pub struct Lexer<'a> {
    pub source: &'a str,
    pub chars: Peekable<Chars<'a>>,
    pub current_pos: usize,
}
impl<'a> Lexer<'a> {
    pub fn new(source: &'a str) -> Lexer<'a> {
        Lexer {
            source,
            chars: source.chars().peekable(),
            current_pos: 0,
        }
    }
    fn read_char(&mut self) -> Option<char> {
        let char = self.chars.next()?;
        self.current_pos += char.len_utf8();
        Some(char)
    }
    fn peek_next(&self) -> Option<char> {
        let mut cloned = self.chars.clone();
        cloned.next()?;
        Some(cloned.next()?)
    }
    fn skip_whitespace_and_comments(&mut self) {
        loop {
            match self.chars.peek() {
                Some(' ') | Some('\t') | Some('\r') => {
                    self.read_char();
                }
                Some('/') => match self.peek_next() {
                    Some('/') => self.skip_line(),
                    Some('*') => self.skip_block_comment(),
                    _ => return,
                },
                _ => return,
            };
        }
    }
    fn skip_line(&mut self) {
        while let Some(&c) = self.chars.peek() {
            if c == '\n' {
                break;
            }
            self.read_char();
        }
        self.read_char();
    }
    fn skip_block_comment(&mut self) {
        self.read_char();
        self.read_char();
        loop {
            match self.chars.peek() {
                Some('*') => {
                    if let Some('/') = self.peek_next() {
                        self.read_char();
                        self.read_char();
                        break;
                    } else {
                        self.read_char();
                    }
                }
                Some(_) => {
                    self.read_char();
                }
                None => {
                    // TODO: EOF
                    break;
                }
            }
        }
    }
    pub fn next_token(&mut self) -> Token {
        self.skip_whitespace_and_comments();
        let start = self.current_pos;
        let kind = match self.read_char() {
            Some('#') => TokenType::Hash,
            Some(',') => TokenType::Comma,
            Some('.') => {
                if let Some(&c) = self.chars.peek()
                    && is_identifier_start(c)
                {
                    // self.read_char();
                    let first = match self.read_char() {
                        Some(char) => char,
                        _ => unreachable!(),
                    };
                    let name = match self.read_identifier(first) {
                        TokenType::Identifier(string) => string,
                        keyword => format!("{}", keyword),
                    };
                    if self.chars.peek() == Some(&':') {
                        self.read_char();
                        TokenType::Label(name)
                    } else {
                        TokenType::Label(name)
                    }
                } else {
                    TokenType::Dot
                }
            }
            Some(':') => TokenType::Colon,
            Some('=') => {
                if let Some('=') = self.chars.peek() {
                    self.read_char();
                    TokenType::Equal
                } else {
                    TokenType::Assign
                }
            }
            Some('(') => TokenType::LParen,
            Some(')') => TokenType::RParen,
            Some('&') => {
                if let Some('&') = self.chars.peek() {
                    self.read_char();
                    TokenType::And
                } else {
                    TokenType::Error(String::from("invalid token"))
                }
            }
            Some('|') => {
                if let Some('|') = self.chars.peek() {
                    self.read_char();
                    TokenType::And
                } else {
                    TokenType::Error(String::from("invalid token"))
                }
            }
            Some('!') => {
                if let Some('=') = self.chars.peek() {
                    self.read_char();
                    TokenType::NotEqual
                } else {
                    TokenType::Not
                }
            }
            Some('<') => {
                if let Some('=') = self.chars.peek() {
                    self.read_char();
                    TokenType::LesserEqual
                } else {
                    TokenType::LesserThan
                }
            }
            Some('>') => {
                if let Some('=') = self.chars.peek() {
                    self.read_char();
                    TokenType::GreaterEqual
                } else {
                    TokenType::GreaterThan
                }
            }
            Some('+') => TokenType::Plus,
            Some('-') => TokenType::Minus,
            Some('*') => TokenType::Mul,
            Some('\n') => TokenType::Newline,
            Some(c) if is_identifier_start(c) => self.read_identifier(c),
            Some(c) if c.is_ascii_digit() => self.read_integer(c),
            None => TokenType::EOF,
            Some(c) => TokenType::Error(format!("unexpected token: {c}")),
        };
        let end = self.current_pos;
        Token {
            kind,
            span: start..end,
        }
    }
    pub fn read_identifier(&mut self, first: char) -> TokenType {
        let mut name = String::from(first);
        while let Some(c) = self.chars.peek() {
            if !is_identifier_char(*c) {
                break;
            }
            name.push(*c);
            self.read_char();
        }
        match name.as_str() {
            "function" => return TokenType::Function,
            "public" => return TokenType::Public,
            "action" => return TokenType::Action,
            "alias" => return TokenType::Alias,
            "global" => return TokenType::Global,
            "true" => return TokenType::True,
            "false" => return TokenType::False,
            "if" => return TokenType::If,
            "then" => return TokenType::Then,
            "else" => return TokenType::Else,
            "endif" => return TokenType::EndIf,
            "while" => return TokenType::While,
            "do" => return TokenType::Do,
            "endwhile" => return TokenType::EndWhile,
            "End" => return TokenType::End,
            "Return" => return TokenType::Return,
            "Jump" => return TokenType::Jump,
            "and" => return TokenType::And,
            "or" => return TokenType::Or,
            "not" => return TokenType::Not,
            "as" => return TokenType::As,
            id => return TokenType::Identifier(String::from(id)),
        }
    }
    pub fn read_integer(&mut self, first: char) -> TokenType {
        if first == '0'
            && let Some('x') = self.chars.peek()
        {
            self.read_char();
            let mut hex_string = String::new();
            while let Some(c) = self.chars.peek() {
                if !c.is_ascii_hexdigit() {
                    break;
                }
                hex_string.push(*c);
                self.read_char();
            }
            if hex_string.is_empty() {
                return TokenType::Error("Invalid hex literal".to_string());
            }
            match i32::from_str_radix(&hex_string, 16) {
                Ok(num) => TokenType::Num(num),
                Err(e) => TokenType::Error(e.to_string()),
            }
            // TokenType::Num(i32::from_str_radix(&hex_string, 16))
        } else {
            let mut num_string = String::from(first);
            while let Some(c) = self.chars.peek() {
                if !c.is_ascii_digit() {
                    break;
                }
                num_string.push(*c);
                self.read_char();
            }
            match num_string.parse() {
                Ok(num) => TokenType::Num(num),
                Err(e) => TokenType::Error(e.to_string()),
            }
        }
    }
}

fn is_identifier_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}
fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
