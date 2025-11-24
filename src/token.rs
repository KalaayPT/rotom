use std::ops::Range;

#[derive(Debug, Clone, PartialEq)]
pub enum TokenType {
    Function, // "function"
    Public,   // "public"

    Num(i32),           // integer
    Identifier(String), // variable,

    True,     // "true"
    False,    // "false"
    If,       // "if"
    Then,     // "then"
    Else,     // "else"
    EndIf,    // "endif"
    While,    // "while"
    EndWhile, // "endwhile"
    End,      // "End"
    Return,   // "Return"
    Jump,     // "Jump"

    Hash,         // '#'
    Comma,        // ','
    Dot,          // '.'
    Equal,        // "=="
    Assign,       // '='
    LParen,       // '('
    RParen,       // ')'
    And,          // "&&"
    Or,           // "||"
    Not,          // "!"
    NotEqual,     // "!="
    LesserEqual,  // "<="
    GreaterEqual, // ">="
    LesserThan,   // '<'
    GreaterThan,  // '>'

    Error(String),
    EOF,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenType,
    pub span: Range<usize>,
}
