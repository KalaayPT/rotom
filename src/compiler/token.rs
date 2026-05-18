use std::fmt;
use std::ops::Range;

use serde::Serialize;

use super::diagnostic::serialize_range;

/// The type of a token in the Rotom language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum TokenType {
    Script,   // "script"
    Public,   // "public"
    Action,   // "action"
    Alias,    // "alias"
    Global,   // "global"

    Num(i32),           // integer
    Identifier(String), // variable,
    Label(String),      // label definition (ends with :)
    LocalLabel(String), // local label reference (.name without colon)
    String(Vec<(String, usize)>),     // string literal; each segment is (content, source_start_byte)

    // preprocessor
    Include, // "#include"
    Define,  // "#define"

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
    With,        // "with"
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

pub fn normalize_control_keyword(raw: &str) -> Option<TokenType> {
    match raw.to_ascii_lowercase().as_str() {
        "script" => Some(TokenType::Script),
        "public" => Some(TokenType::Public),
        "action" => Some(TokenType::Action),
        "alias" => Some(TokenType::Alias),
        "global" => Some(TokenType::Global),
        "true" => Some(TokenType::True),
        "false" => Some(TokenType::False),
        "if" => Some(TokenType::If),
        "then" => Some(TokenType::Then),
        "else" => Some(TokenType::Else),
        "endif" => Some(TokenType::EndIf),
        "while" => Some(TokenType::While),
        "do" => Some(TokenType::Do),
        "endwhile" => Some(TokenType::EndWhile),
        "match" => Some(TokenType::Match),
        "with" => Some(TokenType::With),
        "case" => Some(TokenType::Case),
        "endmatch" => Some(TokenType::EndMatch),
        "break" => Some(TokenType::Break),
        "end" => Some(TokenType::End),
        "endmovement" => Some(TokenType::EndMovement),
        "return" => Some(TokenType::Return),
        "jump" | "goto" => Some(TokenType::Jump),
        "and" => Some(TokenType::And),
        "or" => Some(TokenType::Or),
        "not" => Some(TokenType::Not),
        "as" => Some(TokenType::As),
        _ => None,
    }
}

impl fmt::Display for TokenType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // --- Keywords ---
            TokenType::Script => write!(f, "keyword 'script'"),
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
            TokenType::With => write!(f, "keyword 'with'"),
            TokenType::Case => write!(f, "keyword 'case'"),
            TokenType::EndMatch => write!(f, "keyword 'endmatch'"),
            TokenType::Break => write!(f, "keyword 'break'"),
            TokenType::As => write!(f, "keyword 'as'"),
            TokenType::True => write!(f, "boolean 'true'"),
            TokenType::False => write!(f, "boolean 'false'"),

            // --- Control Statements ---
            TokenType::End => write!(f, "terminator 'end'"),
            TokenType::EndMovement => write!(f, "terminator 'endmovement'"),
            TokenType::Return => write!(f, "terminator 'return'"),
            TokenType::Jump => write!(f, "control statement 'jump'"),

            // --- Literals ---
            TokenType::Identifier(s) => write!(f, "identifier '{}'", s),
            TokenType::Num(n) => write!(f, "number '{}'", n),
            TokenType::Label(l) => write!(f, "label definition '{}'", l),
            TokenType::LocalLabel(l) => write!(f, "local label reference '{}'", l),
            TokenType::String(s) => write!(f, "string \"{}\"", s.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>().join(" ")),

            // --- Preprocessor ---
            TokenType::Include => write!(f, "'#include'"),
            TokenType::Define => write!(f, "'#define'"),

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

/// A single token with its source span.
#[derive(Debug, Clone, Serialize)]
pub struct Token {
    pub kind: TokenType,
    #[serde(serialize_with = "serialize_range")]
    pub span: Range<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_control_keyword_accepts_aliases_and_case() {
        assert_eq!(normalize_control_keyword("jump"), Some(TokenType::Jump));
        assert_eq!(normalize_control_keyword("Jump"), Some(TokenType::Jump));
        assert_eq!(normalize_control_keyword("goto"), Some(TokenType::Jump));
        assert_eq!(normalize_control_keyword("GoTo"), Some(TokenType::Jump));
        assert_eq!(
            normalize_control_keyword("EndMovement"),
            Some(TokenType::EndMovement)
        );
        assert_eq!(
            normalize_control_keyword("endmovement"),
            Some(TokenType::EndMovement)
        );
        assert_eq!(normalize_control_keyword("return"), Some(TokenType::Return));
        assert_eq!(normalize_control_keyword("IF"), Some(TokenType::If));
    }
}
