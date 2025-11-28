use std::ops::Range;

use crate::token::{Token, TokenType};

pub struct ScriptFile {
    pub aliases: Vec<Statement>,
    pub functions: Vec<Statement>,
    pub actions: Vec<Statement>,
}

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
    ScriptCommand {
        command: String,
        args: Vec<Expression>,
    },
}

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
}

pub struct Spanned<T> {
    pub node: T,
    pub span: Range<usize>,
}

pub type Expression = Spanned<ExpressionKind>;
pub type Statement = Spanned<StatementKind>;
