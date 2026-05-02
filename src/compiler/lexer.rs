use std::{iter::Peekable, str::Chars};

use super::token::{Token, TokenType, normalize_control_keyword};

/// A lexer for the Rotom scripting language.
///
/// Produces a stream of [`Token`]s from a source string. Whitespace and
/// comments are skipped automatically.
///
/// # Example
/// ```
/// use rotom::compiler::{Lexer, TokenType};
///
/// let lexer = Lexer::new("script Main #1:\nEnd\n");
/// let tokens: Vec<_> = lexer.tokenize();
/// assert!(tokens.iter().any(|t| matches!(t.kind, TokenType::Script)));
/// ```
pub struct Lexer<'a> {
    chars: Peekable<Chars<'a>>,
    current_pos: usize,
    finished: bool,
}

impl<'a> Lexer<'a> {
    /// Create a new lexer for the given source text.
    pub fn new(source: &'a str) -> Lexer<'a> {
        Lexer {
            chars: source.chars().peekable(),
            current_pos: 0,
            finished: false,
        }
    }

    /// Consume the lexer and return all tokens up to `EOF`.
    pub fn tokenize(self) -> Vec<Token> {
        self.collect()
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
    /// Return the next token from the source.
    ///
    /// Repeatedly calling this method (or iterating the lexer) will yield
    /// all tokens including a final `EOF` token.
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
                    let Some(first) = self.read_char() else {
                        unreachable!()
                    };
                    let name = match self.read_identifier(first) {
                        TokenType::Identifier(string) => string,
                        keyword => format!("{}", keyword),
                    };
                    // LocalLabel is just the .name part - colon is handled separately by parser
                    TokenType::LocalLabel(format!(".{}", name))
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
    /// Read an identifier (or keyword) starting with `first`.
    ///
    /// Advances the lexer past the full identifier.
    pub fn read_identifier(&mut self, first: char) -> TokenType {
        let mut name = String::from(first);
        while let Some(c) = self.chars.peek() {
            if !is_identifier_char(*c) {
                break;
            }
            name.push(*c);
            self.read_char();
        }
        normalize_control_keyword(&name).unwrap_or(TokenType::Identifier(name))
    }
    /// Read a numeric literal starting with `first`.
    ///
    /// Supports decimal and hexadecimal (`0x…`) formats.
    pub fn read_integer(&mut self, first: char) -> TokenType {
        if first == '0' && matches!(self.chars.peek(), Some('x')) {
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
/// Return true if `c` can appear inside (or as part of) an identifier.
///
/// This includes `.` because local labels like `.start` are treated as a
/// single conceptual word by editor tooling, even though the lexer handles
/// the leading `.` separately.
pub fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.' || c == '’' || c == '?'
}

impl Iterator for Lexer<'_> {
    type Item = Token;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        let token = self.next_token();
        match token.kind {
            TokenType::EOF => {
                self.finished = true;
                None
            }
            _ => Some(token),
        }
    }
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
        let source = "script myFunc123 alias_var 0 42 0x1A3F";
        let mut lexer = Lexer::new(source);
        let expected_tokens = vec![
            TokenType::Script,
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
        assert_eq!(
            token1.kind,
            TokenType::LocalLabel(".localLabel".to_string())
        );
        let token2 = lexer.next_token();
        assert_eq!(token2.kind, TokenType::Colon);
        let token3 = lexer.next_token();
        assert_eq!(
            token3.kind,
            TokenType::LocalLabel(".anotherLabel".to_string())
        );
        let eof_token = lexer.next_token();
        assert_eq!(eof_token.kind, TokenType::EOF);
    }

    #[test]
    fn test_lexer_iterator() {
        let source = "script test";
        let lexer = Lexer::new(source);
        let tokens: Vec<Token> = lexer.collect();
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].kind, TokenType::Script);
        assert_eq!(tokens[1].kind, TokenType::Identifier("test".to_string()));
    }

    #[test]
    fn test_lexer_tokenize() {
        let source = "if x then return endif";
        let tokens = Lexer::new(source).tokenize();
        assert_eq!(tokens.len(), 5);
        assert_eq!(tokens[0].kind, TokenType::If);
        assert_eq!(tokens[1].kind, TokenType::Identifier("x".to_string()));
        assert_eq!(tokens[2].kind, TokenType::Then);
        assert_eq!(tokens[3].kind, TokenType::Return);
        assert_eq!(tokens[4].kind, TokenType::EndIf);
    }

    #[test]
    fn test_lexer_keywords() {
        let source = "if then else endif while do endwhile end endmovement return jump true false";
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
    fn test_lexer_accepts_legacy_control_spellings_and_aliases() {
        let source = "End EndMovement Return Jump GoTo eNdMoVeMeNt";
        let mut lexer = Lexer::new(source);
        let expected_tokens = vec![
            TokenType::End,
            TokenType::EndMovement,
            TokenType::Return,
            TokenType::Jump,
            TokenType::Jump,
            TokenType::EndMovement,
        ];
        for expected in expected_tokens {
            let token = lexer.next_token();
            assert_eq!(token.kind, expected);
        }
        let eof_token = lexer.next_token();
        assert_eq!(eof_token.kind, TokenType::EOF);
    }

    #[test]
    fn test_lexer_accepts_case_insensitive_booleans() {
        let source = "TRUE FALSE True False";
        let mut lexer = Lexer::new(source);

        assert_eq!(lexer.next_token().kind, TokenType::True);
        assert_eq!(lexer.next_token().kind, TokenType::False);
        assert_eq!(lexer.next_token().kind, TokenType::True);
        assert_eq!(lexer.next_token().kind, TokenType::False);
        assert_eq!(lexer.next_token().kind, TokenType::EOF);
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
    fn test_lexer_does_not_promote_partial_keyword_matches() {
        let source = "goto_if endish";
        let mut lexer = Lexer::new(source);

        assert_eq!(
            lexer.next_token().kind,
            TokenType::Identifier("goto_if".to_string())
        );
        assert_eq!(
            lexer.next_token().kind,
            TokenType::Identifier("endish".to_string())
        );
        assert_eq!(lexer.next_token().kind, TokenType::EOF);
    }

    #[test]
    fn test_simple_script() {
        let source = "
        script myFunc:
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
            if token.kind == TokenType::EOF {
                break;
            }
            tokens.push(token.kind);
        }
        let expected_tokens = vec![
            TokenType::Newline,
            TokenType::Script,
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
        // Example script
        script example
            /* Initialize variable */
            count = 0
            while count < 10 do
                count = count + 1
            endwhile
        End

        script anotherExample #1
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
            if token.kind == TokenType::EOF {
                break;
            }
            tokens.push(token.kind);
        }
        let expected_tokens = vec![
            TokenType::Newline,
            TokenType::Script,
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
            TokenType::Script,
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
            TokenType::LocalLabel(".start".to_string()),
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
