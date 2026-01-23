use std::fmt;
use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenType {
    Function, // "function"
    Public,   // "public"
    Action,   // "action"
    Alias,    // "alias"
    Global,   // "global"

    Num(i32),           // integer
    Identifier(String), // variable,
    Label(String),      // label definition (ends with :)
    LocalLabel(String), // local label reference (.name without colon)

    // control flow
    True,        // "true"
    False,       // "false"
    If,          // "if"
    Then,        // "then"
    Else,        // "else"
    EndIf,       // "endif"
    While,       // "while"
    Do,          // "do"
    EndWhile,    // "endwhile"
    Match,       // "match"
    Where,       // "where"
    Case,        // "case"
    EndMatch,    // "endmatch"
    Break,       // "break"
    End,         // "End"
    EndMovement, // "EndMovement"
    Return,      // "Return"
    Jump,        // "Jump"

    // operators
    Hash,         // '#'
    Comma,        // ','
    Dot,          // '.'
    Colon,        // ':'
    Equal,        // "=="
    Assign,       // '='
    LParen,       // '('
    RParen,       // ')'
    And,          // "&&" or "and"
    Or,           // "||" or "or"
    Not,          // "!"
    NotEqual,     // "!="
    LesserEqual,  // "<="
    GreaterEqual, // ">="
    LesserThan,   // '<'
    GreaterThan,  // '>'
    As,           // "as"
    Plus,         // '+'
    Minus,        // '-'
    Mul,          // '*'

    Newline,
    Error(String),
    EOF,
}

impl fmt::Display for TokenType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // --- Keywords ---
            TokenType::Function => write!(f, "keyword 'function'"),
            TokenType::Public => write!(f, "keyword 'public'"),
            TokenType::Action => write!(f, "keyword 'action'"),
            TokenType::Alias => write!(f, "keyword 'alias'"),
            TokenType::Global => write!(f, "keyword 'global'"),
            TokenType::If => write!(f, "keyword 'if'"),
            TokenType::Then => write!(f, "keyword 'then'"),
            TokenType::Else => write!(f, "keyword 'else'"),
            TokenType::EndIf => write!(f, "keyword 'endif'"),
            TokenType::While => write!(f, "keyword 'while'"),
            TokenType::EndWhile => write!(f, "keyword 'endwhile'"),
            TokenType::Do => write!(f, "keyword 'do'"),
            TokenType::Match => write!(f, "keyword 'match'"),
            TokenType::Where => write!(f, "keyword 'where'"),
            TokenType::Case => write!(f, "keyword 'case'"),
            TokenType::EndMatch => write!(f, "keyword 'endmatch'"),
            TokenType::Break => write!(f, "keyword 'break'"),
            TokenType::As => write!(f, "keyword 'as'"),
            TokenType::True => write!(f, "boolean 'true'"),
            TokenType::False => write!(f, "boolean 'false'"),

            // --- Opcode Keywords ---
            TokenType::End => write!(f, "terminator 'End'"),
            TokenType::EndMovement => write!(f, "terminator 'EndMovement'"),
            TokenType::Return => write!(f, "terminator 'Return'"),
            TokenType::Jump => write!(f, "command 'Jump'"),

            // --- Literals ---
            TokenType::Identifier(s) => write!(f, "identifier '{}'", s),
            TokenType::Num(n) => write!(f, "number '{}'", n),
            TokenType::Label(l) => write!(f, "label definition '{}'", l),
            TokenType::LocalLabel(l) => write!(f, "local label reference '{}'", l),
            // if we add string literals for inline messages later
            // TokenType::StringLit(s) => write!(f, "string \"{}\"", s),

            // --- Symbols ---
            TokenType::Hash => write!(f, "'#'"),
            TokenType::Comma => write!(f, "','"),
            TokenType::Dot => write!(f, "'.'"),
            TokenType::Colon => write!(f, "':'"),
            TokenType::Assign => write!(f, "'='"),
            TokenType::LParen => write!(f, "'('"),
            TokenType::RParen => write!(f, "')'"),

            // --- Operators ---
            TokenType::Equal => write!(f, "'=='"),
            TokenType::NotEqual => write!(f, "'!='"),
            TokenType::LesserThan => write!(f, "'<'"),
            TokenType::GreaterThan => write!(f, "'>'"),
            TokenType::LesserEqual => write!(f, "'<='"),
            TokenType::GreaterEqual => write!(f, "'>='"),
            TokenType::Plus => write!(f, "'+'"),
            TokenType::Minus => write!(f, "'-'"),
            TokenType::Mul => write!(f, "'*'"),
            TokenType::Not => write!(f, "'!'"),
            TokenType::And => write!(f, "'&&'"),
            TokenType::Or => write!(f, "'||'"),

            // --- Special ---
            TokenType::Newline => write!(f, "newline"),
            TokenType::EOF => write!(f, "end of file"),
            TokenType::Error(e) => write!(f, "error: {}", e),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenType,
    pub span: Range<usize>,
}
