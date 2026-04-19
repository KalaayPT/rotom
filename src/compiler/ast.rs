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
    /// Whether to emit the jump table end marker (0xFD13) in the binary.
    /// Defaults to true. Set to false for scripts without `ScriptEntryEnd` directive.
    pub emit_end_marker: bool,
}
// helper struct to hold the metadata for ONE alias of a function (to support multiple jumptable
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
