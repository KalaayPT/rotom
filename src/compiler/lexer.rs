use std::{iter::Peekable, str::Chars};

use super::token::{Token, TokenType};

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
        cloned.next()
    }
    fn skip_whitespace_and_comments(&mut self) {
        loop {
            match self.chars.peek() {
                Some(' ' | '\t' | '\r') => {
                    self.read_char();
                }
                Some('/') => match self.peek_next() {
                    Some('/') => self.skip_line(),
                    Some('*') => self.skip_block_comment(),
                    _ => return,
                },
                _ => return,
            }
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
                    if self.peek_next() == Some('/') {
                        self.read_char();
                        self.read_char();
                        break;
                    }
                    self.read_char();
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
                    let first = match self.read_char() {
                        Some(char) => char,
                        _ => unreachable!(),
                    };
                    let name = match self.read_identifier(first) {
                        TokenType::Identifier(string) => string,
                        keyword => format!("{}", keyword),
                    };
                    // Preserve the dot prefix for local labels
                    let full_name = format!(".{}", name);
                    if self.chars.peek() == Some(&':') {
                        self.read_char();
                        TokenType::Label(full_name)
                    } else {
                        TokenType::Label(full_name)
                    }
                } else {
                    TokenType::Dot
                }
            }
            Some(':') => TokenType::Colon,
            Some('=') => {
                if matches!(self.chars.peek(), Some('=')) {
                    self.read_char();
                    TokenType::Equal
                } else {
                    TokenType::Assign
                }
            }
            Some('(') => TokenType::LParen,
            Some(')') => TokenType::RParen,
            Some('&') => {
                if matches!(self.chars.peek(), Some('&')) {
                    self.read_char();
                    TokenType::And
                } else {
                    TokenType::Error(String::from("invalid token"))
                }
            }
            Some('|') => {
                if matches!(self.chars.peek(), Some('|')) {
                    self.read_char();
                    TokenType::Or
                } else {
                    TokenType::Error(String::from("invalid token"))
                }
            }
            Some('!') => {
                if matches!(self.chars.peek(), Some('=')) {
                    self.read_char();
                    TokenType::NotEqual
                } else {
                    TokenType::Not
                }
            }
            Some('<') => {
                if matches!(self.chars.peek(), Some('=')) {
                    self.read_char();
                    TokenType::LesserEqual
                } else {
                    TokenType::LesserThan
                }
            }
            Some('>') => {
                if matches!(self.chars.peek(), Some('=')) {
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
            "function" => TokenType::Function,
            "public" => TokenType::Public,
            "action" => TokenType::Action,
            "alias" => TokenType::Alias,
            "global" => TokenType::Global,
            "true" => TokenType::True,
            "false" => TokenType::False,
            "if" => TokenType::If,
            "then" => TokenType::Then,
            "else" => TokenType::Else,
            "endif" => TokenType::EndIf,
            "while" => TokenType::While,
            "do" => TokenType::Do,
            "endwhile" => TokenType::EndWhile,
            "End" => TokenType::End,
            "EndMovement" => TokenType::EndMovement,
            "Return" => TokenType::Return,
            "Jump" => TokenType::Jump,
            "and" => TokenType::And,
            "or" => TokenType::Or,
            "not" => TokenType::Not,
            "as" => TokenType::As,
            _ => TokenType::Identifier(name),
        }
    }
    pub fn read_integer(&mut self, first: char) -> TokenType {
        if first == '0'
            && matches!(self.chars.peek(), Some('x'))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lexer_basic_tokens() {
        let source = "# , . : = == ( ) && || ! != < <= > >= + - * \n";
        let mut lexer = Lexer::new(source);
        let expected_tokens = vec![
            TokenType::Hash,
            TokenType::Comma,
            TokenType::Dot,
            TokenType::Colon,
            TokenType::Assign,
            TokenType::Equal,
            TokenType::LParen,
            TokenType::RParen,
            TokenType::And,
            TokenType::Or,
            TokenType::Not,
            TokenType::NotEqual,
            TokenType::LesserThan,
            TokenType::LesserEqual,
            TokenType::GreaterThan,
            TokenType::GreaterEqual,
            TokenType::Plus,
            TokenType::Minus,
            TokenType::Mul,
            TokenType::Newline,
        ];
        for expected in expected_tokens {
            let token = lexer.next_token();
            assert_eq!(token.kind, expected);
        }
        let eof_token = lexer.next_token();
        assert_eq!(eof_token.kind, TokenType::EOF);
    }

    #[test]
    fn test_lexer_identifiers_and_numbers() {
        let source = "function myFunc123 alias_var 0 42 0x1A3F";
        let mut lexer = Lexer::new(source);
        let expected_tokens = vec![
            TokenType::Function,
            TokenType::Identifier("myFunc123".to_string()),
            TokenType::Identifier("alias_var".to_string()),
            TokenType::Num(0),
            TokenType::Num(42),
            TokenType::Num(6719),
        ];
        for expected in expected_tokens {
            let token = lexer.next_token();
            assert_eq!(token.kind, expected);
        }
        let eof_token = lexer.next_token();
        assert_eq!(eof_token.kind, TokenType::EOF);
    }

    #[test]
    fn test_lexer_comments_and_whitespace() {
        let source = "  // This is a comment\n  /* Block \n comment */  identifier  ";
        let mut lexer = Lexer::new(source);
        let token = lexer.next_token();
        assert_eq!(token.kind, TokenType::Identifier("identifier".to_string()));
        let eof_token = lexer.next_token();
        assert_eq!(eof_token.kind, TokenType::EOF);
    }

    #[test]
    fn test_lexer_labels() {
        let source = ".localLabel: .anotherLabel";
        let mut lexer = Lexer::new(source);
        let token1 = lexer.next_token();
        assert_eq!(token1.kind, TokenType::Label(".localLabel".to_string()));
        let token2 = lexer.next_token();
        assert_eq!(token2.kind, TokenType::Label(".anotherLabel".to_string()));
        let eof_token = lexer.next_token();
        assert_eq!(eof_token.kind, TokenType::EOF);
    }

    #[test]
    fn test_lexer_keywords() {
        let source = "if then else endif while do endwhile End EndMovement Return Jump true false";
        let mut lexer = Lexer::new(source);
        let expected_tokens = vec![
            TokenType::If,
            TokenType::Then,
            TokenType::Else,
            TokenType::EndIf,
            TokenType::While,
            TokenType::Do,
            TokenType::EndWhile,
            TokenType::End,
            TokenType::EndMovement,
            TokenType::Return,
            TokenType::Jump,
            TokenType::True,
            TokenType::False,
        ];
        for expected in expected_tokens {
            let token = lexer.next_token();
            assert_eq!(token.kind, expected);
        }
        let eof_token = lexer.next_token();
        assert_eq!(eof_token.kind, TokenType::EOF);
    }

    #[test]
    fn test_lexer_errors() {
        let source = "@ $ ^";
        let mut lexer = Lexer::new(source);
        let expected_errors = vec![
            "unexpected token: @",
            "unexpected token: $",
            "unexpected token: ^",
        ];
        for expected_msg in expected_errors {
            let token = lexer.next_token();
            match token.kind {
                TokenType::Error(msg) => assert_eq!(msg, expected_msg),
                _ => panic!("Expected an error token"),
            }
        }
        let eof_token = lexer.next_token();
        assert_eq!(eof_token.kind, TokenType::EOF);
    }

    #[test]
    fn test_simple_function() {
        let source = "
        function myFunc:
            alias_var = 42
            if alias_var >= 10 then
                Return
            endif
        End
        ";
        let mut lexer = Lexer::new(source);
        let mut tokens = Vec::new();
        loop {
            let token = lexer.next_token();
            if let TokenType::EOF = token.kind {
                break;
            }
            tokens.push(token.kind);
        }
        let expected_tokens = vec![
            TokenType::Newline,
            TokenType::Function,
            TokenType::Identifier("myFunc".to_string()),
            TokenType::Colon,
            TokenType::Newline,
            TokenType::Identifier("alias_var".to_string()),
            TokenType::Assign,
            TokenType::Num(42),
            TokenType::Newline,
            TokenType::If,
            TokenType::Identifier("alias_var".to_string()),
            TokenType::GreaterEqual,
            TokenType::Num(10),
            TokenType::Then,
            TokenType::Newline,
            TokenType::Return,
            TokenType::Newline,
            TokenType::EndIf,
            TokenType::Newline,
            TokenType::End,
            TokenType::Newline,
        ];
        assert_eq!(tokens, expected_tokens);
    }

    #[test]
    fn test_full_example_file() {
        let source = "
        // Example function
        function example
            /* Initialize variable */
            count = 0
            while count < 10 do
                count = count + 1
            endwhile
        End

        public function anotherExample #1
            alias 0xFF as maxCount
            if maxCount == 255 then
                Jump .start
            else
                ApplyMovement sampleMovement
                WaitMovement
                Return
            endif
        End

        movement sampleMovement
            WalkNorth 10
        EndMovement
        ";
        let mut lexer = Lexer::new(source);
        let mut tokens = Vec::new();
        loop {
            let token = lexer.next_token();
            if let TokenType::EOF = token.kind {
                break;
            }
            tokens.push(token.kind);
        }
        let expected_tokens = vec![
            TokenType::Newline,
            TokenType::Function,
            TokenType::Identifier("example".to_string()),
            TokenType::Newline,
            TokenType::Newline,
            TokenType::Identifier("count".to_string()),
            TokenType::Assign,
            TokenType::Num(0),
            TokenType::Newline,
            TokenType::While,
            TokenType::Identifier("count".to_string()),
            TokenType::LesserThan,
            TokenType::Num(10),
            TokenType::Do,
            TokenType::Newline,
            TokenType::Identifier("count".to_string()),
            TokenType::Assign,
            TokenType::Identifier("count".to_string()),
            TokenType::Plus,
            TokenType::Num(1),
            TokenType::Newline,
            TokenType::EndWhile,
            TokenType::Newline,
            TokenType::End,
            TokenType::Newline,
            TokenType::Newline,
            TokenType::Public,
            TokenType::Function,
            TokenType::Identifier("anotherExample".to_string()),
            TokenType::Hash,
            TokenType::Num(1),
            TokenType::Newline,
            TokenType::Alias,
            TokenType::Num(255),
            TokenType::As,
            TokenType::Identifier("maxCount".to_string()),
            TokenType::Newline,
            TokenType::If,
            TokenType::Identifier("maxCount".to_string()),
            TokenType::Equal,
            TokenType::Num(255),
            TokenType::Then,
            TokenType::Newline,
            TokenType::Jump,
            TokenType::Label(".start".to_string()),
            TokenType::Newline,
            TokenType::Else,
            TokenType::Newline,
            TokenType::Identifier("ApplyMovement".to_string()),
            TokenType::Identifier("sampleMovement".to_string()),
            TokenType::Newline,
            TokenType::Identifier("WaitMovement".to_string()),
            TokenType::Newline,
            TokenType::Return,
            TokenType::Newline,
            TokenType::EndIf,
            TokenType::Newline,
            TokenType::End,
            TokenType::Newline,
            TokenType::Newline,
            TokenType::Identifier("movement".to_string()),
            TokenType::Identifier("sampleMovement".to_string()),
            TokenType::Newline,
            TokenType::Identifier("WalkNorth".to_string()),
            TokenType::Num(10),
            TokenType::Newline,
            TokenType::EndMovement,
            TokenType::Newline,
        ];
        assert_eq!(tokens, expected_tokens);
    }
}
