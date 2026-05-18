use std::ops::Range;

use super::token::TokenType;

#[derive(Debug, Clone)]
pub struct MatchCase {
    pub values: Vec<Expression>,
    pub body: Vec<Statement>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct ScriptFile {
    pub aliases: Vec<Statement>,
    /// Top-level statements in source order.
    /// This includes functions, actions, and top-level aliases.
    pub items: Vec<Statement>,
    /// Number of jump table end markers (0xFD13) to emit in the binary.
    /// Defaults to 1. Set to 0 for scripts without `ScriptEntryEnd` directive.
    pub jump_table_end_marker_count: u8,
}
// helper struct to hold the metadata for ONE alias of a script (to support multiple jumptable
// entries for one function)
#[derive(Debug, Clone)]
pub struct FunctionHeader {
    pub name: String,
    pub id: Option<u32>,
    pub is_public: bool,
}

#[derive(Debug, Clone)]
pub enum StatementKind {
    Function {
        headers: Vec<FunctionHeader>,
        body: Vec<Statement>,
    },
    Action {
        name: String,
        body: Vec<Statement>,
    },
    AliasStatement {
        value: Expression,
        name: String,
    },
    Include {
        path: String,
    },
    Define {
        name: String,
        value: Expression,
    },
    IfStatement {
        condition: Box<Expression>,
        body: Vec<Statement>,
        elseblock: Option<Vec<Statement>>,
    },
    WhileStatement {
        condition: Box<Expression>,
        body: Vec<Statement>,
    },
    MatchStatement {
        subject: Box<Expression>,
        cases: Vec<MatchCase>,
        default: Option<Vec<Statement>>,
    },
    Break,
    ScriptCommand {
        command: String,
        args: Vec<Expression>,
    },
    Label(String),
    Jump(Expression),
    Return,
    End,
    EndMovement,
    /// A placeholder for a statement that could not be parsed.
    ///
    /// Used by the error-tolerant parser to preserve AST structure around
    /// syntax errors.
    Error,
}

#[derive(Debug, Clone)]
pub enum ExpressionKind {
    Number(i32),
    Identifier(String),
    Prefix {
        operator: TokenType,
        id: Box<Expression>,
    },
    Infix {
        left: Box<Expression>,
        operator: TokenType,
        right: Box<Expression>,
    },
    Label(String),
    Call {
        function: Box<Expression>,
        args: Vec<Expression>,
    },
    String(Vec<(String, usize)>),
    /// A placeholder for an expression that could not be parsed.
    ///
    /// Used by the error-tolerant parser to preserve AST structure around
    /// syntax errors inside expressions.
    Error,
}
#[derive(Debug, PartialEq, Eq, PartialOrd, Copy, Clone)]
pub enum Precedence {
    Lowest,
    LogicalOr,  // ||, or
    LogicalAnd, // &&, and
    Comparison, // ==, !=, <, >, <=, >=
    Sum,        // +, -
    Product,    // *, /
    Prefix,     // !X, -X
    Call,       // Function()
}
#[derive(Debug, Clone)]
pub struct Spanned<T> {
    pub node: T,
    pub span: Range<usize>,
}

pub type Expression = Spanned<ExpressionKind>;
pub type Statement = Spanned<StatementKind>;
