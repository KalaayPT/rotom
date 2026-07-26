use std::{iter::Peekable, str::Chars};

use super::token::{Token, TokenType, normalize_control_keyword};

/// A lexer for the Rotom scripting language.
///
/// Produces a stream of [`Token`]s from a source string. Whitespace and
/// comments are skipped automatically.
///
/// # Example
/// ```
/// use rotom::compiler::Lexer;
/// use rotom::compiler::token::TokenType;
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
        self.chars.clone().nth(1)
    }
    /// Skip trivia before a token, returning an error token for malformed comments.
    fn skip_whitespace_and_comments(&mut self) -> Option<Token> {
        loop {
            let start = self.current_pos;
            match self.chars.peek() {
                Some(' ' | '\t' | '\r') => {
                    self.read_char();
                }
                Some('/') => match self.peek_next() {
                    Some('/') => self.skip_line(),
                    Some('*') => {
                        if !self.skip_block_comment() {
                            return Some(Token {
                                kind: TokenType::Error("unclosed block comment".to_string()),
                                span: start..self.current_pos,
                            });
                        }
                    }
                    _ => return None,
                },
                _ => return None,
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
    /// Skip a block comment, returning `false` if EOF is reached before `*/`.
    fn skip_block_comment(&mut self) -> bool {
        self.read_char();
        self.read_char();
        loop {
            match self.chars.peek() {
                Some('*') => {
                    if self.peek_next() == Some('/') {
                        self.read_char();
                        self.read_char();
                        return true;
                    }
                    self.read_char();
                }
                Some(_) => {
                    self.read_char();
                }
                None => return false,
            }
        }
    }
    /// Return the next token from the source.
    ///
    /// Repeatedly calling this method (or iterating the lexer) will yield
    /// all tokens including a final `EOF` token.
    #[allow(clippy::too_many_lines)]
    pub fn next_token(&mut self) -> Token {
        if let Some(error) = self.skip_whitespace_and_comments() {
            return error;
        }
        let start = self.current_pos;
        let kind = match self.read_char() {
            Some('#') => match self.chars.peek() {
                Some(&c) if is_identifier_start(c) => {
                    let first = self.read_char().unwrap();
                    match self.read_identifier(first) {
                        TokenType::Identifier(s) => match s.as_str() {
                            "include" => TokenType::Include,
                            "define" => TokenType::Define,
                            _ => TokenType::Hash,
                        },
                        keyword => {
                            // If read_identifier returned a keyword (e.g.
                            // "if" after #), treat the # as standalone.
                            let _ = keyword;
                            TokenType::Hash
                        }
                    }
                }
                _ => TokenType::Hash,
            },
            Some('"') => {
                // current_pos is now just past the opening "; that's where content starts.
                let mut segments: Vec<(String, usize)> = vec![(String::new(), self.current_pos)];
                while let Some(c) = self.read_char() {
                    if c == '\\' && self.chars.peek() == Some(&'"') {
                        // \" → chatot alias ["] for the curly-quote character (charmap 0x01B4)
                        segments.last_mut().unwrap().0.push_str("[\"]");
                        self.read_char();
                    } else if c == '"' {
                        break;
                    } else if c == '\n' {
                        // Real newline: start a new segment, stripping any continuation indent
                        // so editor auto-indent doesn't pollute the string content.
                        while matches!(self.chars.peek(), Some(' ' | '\t')) {
                            self.read_char();
                        }
                        segments.push((String::new(), self.current_pos));
                    } else {
                        segments.last_mut().unwrap().0.push(c);
                    }
                }
                TokenType::String(segments)
            }
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
                    // Dot-prefixed names (.foo) are normalised to plain Identifier(".foo").
                    // The dot is preserved in the name string for round-trip fidelity but
                    // carries no semantic weight — there is no separate "inline label" scope.
                    TokenType::Identifier(format!(".{}", name))
                } else {
                    TokenType::Dot
                }
            }
            Some(':') => {
                if matches!(self.chars.peek(), Some(':')) {
                    self.read_char();
                    TokenType::DoubleColon
                } else {
                    TokenType::Colon
                }
            }
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
            Some('[') => TokenType::LBracket,
            Some(']') => TokenType::RBracket,
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
            Some('-') => {
                if matches!(self.chars.peek(), Some('>')) {
                    self.read_char();
                    TokenType::Arrow
                } else {
                    TokenType::Minus
                }
            }
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
            // DSPRE script filenames are numeric stems (e.g. `0211`). Preserve
            // the original spelling when the digits form the module side of a
            // `module::label` reference instead of collapsing them to a number.
            if matches!(self.chars.peek(), Some(':')) && self.peek_next() == Some(':') {
                return TokenType::Identifier(num_string);
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
        let source = "# , . : :: = == ( ) && || ! != < <= > >= + - * \n";
        let mut lexer = Lexer::new(source);
        let expected_tokens = vec![
            TokenType::Hash,
            TokenType::Comma,
            TokenType::Dot,
            TokenType::Colon,
            TokenType::DoubleColon,
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
    fn test_numeric_module_stem_preserves_leading_zeroes() {
        let tokens = Lexer::new("0211::NewGame").tokenize();
        assert_eq!(
            tokens.iter().map(|token| &token.kind).collect::<Vec<_>>(),
            vec![
                &TokenType::Identifier("0211".to_string()),
                &TokenType::DoubleColon,
                &TokenType::Identifier("NewGame".to_string()),
            ]
        );
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
    fn test_lexer_dot_names() {
        // Dot-prefixed names are normalised to Identifier with the dot in the name.
        let source = ".localLabel: .anotherLabel";
        let mut lexer = Lexer::new(source);
        let token1 = lexer.next_token();
        assert_eq!(
            token1.kind,
            TokenType::Identifier(".localLabel".to_string())
        );
        let token2 = lexer.next_token();
        assert_eq!(token2.kind, TokenType::Colon);
        let token3 = lexer.next_token();
        assert_eq!(
            token3.kind,
            TokenType::Identifier(".anotherLabel".to_string())
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
    fn test_unclosed_block_comment_is_error() {
        let mut lexer = Lexer::new("script Main #1:\n    /* no terminator\n    End\n");

        assert_eq!(lexer.next_token().kind, TokenType::Script);
        assert_eq!(
            lexer.next_token().kind,
            TokenType::Identifier("Main".to_string())
        );
        assert_eq!(lexer.next_token().kind, TokenType::Hash);
        assert_eq!(lexer.next_token().kind, TokenType::Num(1));
        assert_eq!(lexer.next_token().kind, TokenType::Colon);
        assert_eq!(lexer.next_token().kind, TokenType::Newline);
        assert_eq!(
            lexer.next_token().kind,
            TokenType::Error("unclosed block comment".to_string())
        );
        assert_eq!(lexer.next_token().kind, TokenType::EOF);
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
            TokenType::Identifier(".start".to_string()),
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

    fn seg_contents(kind: TokenType) -> Vec<String> {
        match kind {
            TokenType::String(segs) => segs.into_iter().map(|(s, _)| s).collect(),
            _ => panic!("expected String token"),
        }
    }

    #[test]
    fn string_literal_plain() {
        let mut lexer = Lexer::new(r#""hello world""#);
        assert_eq!(seg_contents(lexer.next_token().kind), vec!["hello world"]);
        assert_eq!(lexer.next_token().kind, TokenType::EOF);
    }

    #[test]
    fn string_literal_escaped_quote_emits_bracket_alias() {
        // \" inside a string must produce the chatot alias ["] for U+201C,
        // not a raw ASCII double-quote (which has no charmap entry).
        let mut lexer = Lexer::new(r#""say \" something""#);
        assert_eq!(
            seg_contents(lexer.next_token().kind),
            vec![r#"say ["] something"#]
        );
        assert_eq!(lexer.next_token().kind, TokenType::EOF);
    }

    #[test]
    fn string_literal_multiline_produces_two_segments() {
        // Real newlines split the string into segments so each can be
        // measured independently for dialog width warnings.
        let src = "\"hello\nworld\"";
        let mut lexer = Lexer::new(src);
        assert_eq!(
            seg_contents(lexer.next_token().kind),
            vec!["hello", "world"]
        );
        assert_eq!(lexer.next_token().kind, TokenType::EOF);
    }

    #[test]
    fn string_literal_multiline_strips_continuation_indent() {
        // Leading whitespace on the continuation line is consumed, so editor
        // auto-indent doesn't pollute the segment content.
        let src = "\"first line\n    second line\"";
        let mut lexer = Lexer::new(src);
        assert_eq!(
            seg_contents(lexer.next_token().kind),
            vec!["first line", "second line"]
        );
        assert_eq!(lexer.next_token().kind, TokenType::EOF);
    }

    #[test]
    fn string_literal_explicit_escape_before_newline_preserved() {
        // An explicit chatot escape (\r) before a real newline ends the current
        // segment; the escape stays with that segment.
        let src = "\"line one\\r\n    line two\"";
        let mut lexer = Lexer::new(src);
        assert_eq!(
            seg_contents(lexer.next_token().kind),
            vec![r"line one\r", "line two"]
        );
        assert_eq!(lexer.next_token().kind, TokenType::EOF);
    }

    #[test]
    fn string_literal_multiline_segment_offsets_are_recorded() {
        // Verify that each segment carries the correct source byte offset so
        // the analyser can produce precise warning spans.
        let src = "\"abc\ndef\"";
        //          0123456789
        //          "abc\ndef"
        //           ^      ^-- closing " at 8
        //           1 = start of first segment (after opening ")
        //           5 = start of second segment (after \n at 4)
        let mut lexer = Lexer::new(src);
        if let TokenType::String(segs) = lexer.next_token().kind {
            assert_eq!(segs[0].1, 1); // 'a' is at byte 1
            assert_eq!(segs[1].1, 5); // 'd' is at byte 5 (after " a b c \n)
        } else {
            panic!("expected String token");
        }
    }

    #[test]
    fn string_literal_escaped_quote_terminates_correctly() {
        // The character after \" must still be part of the same string, not
        // start a new token.
        let mut lexer = Lexer::new(r#""a\"b" End"#);
        assert_eq!(seg_contents(lexer.next_token().kind), vec![r#"a["]b"#]);
        assert_eq!(lexer.next_token().kind, TokenType::End);
        assert_eq!(lexer.next_token().kind, TokenType::EOF);
    }
}
