use std::ops::Range;

use super::token::TokenType;

#[derive(Debug)]
pub struct ScriptFile {
    pub aliases: Vec<Statement>,
    /// Top-level items (functions and actions interleaved in source order)
    pub items: Vec<Statement>,
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
        is_global: bool,
        id: i32,
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
#[derive(Debug, PartialEq, PartialOrd, Copy, Clone)]
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
