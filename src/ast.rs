use std::ops::Range;

use crate::token::{Token, TokenType};

#[derive(Debug)]
pub struct ScriptFile {
    pub aliases: Vec<Statement>,
    pub functions: Vec<Statement>,
    pub actions: Vec<Statement>,
}

#[derive(Debug)]
pub enum StatementKind {
    Function {
        is_public: bool,
        name: String,
        id: Option<i32>,
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
}

#[derive(Debug)]
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
    Comparison, // ==, !=, <, >, <=, >=
    Sum,        // +, -
    Product,    // *, /
    Prefix,     // !X, -X
    Call,       // Function()
}
#[derive(Debug)]
pub struct Spanned<T> {
    pub node: T,
    pub span: Range<usize>,
}

pub type Expression = Spanned<ExpressionKind>;
pub type Statement = Spanned<StatementKind>;
