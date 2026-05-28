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

impl Spanned<ExpressionKind> {
    pub fn to_constant_eval_source(&self) -> Result<String, String> {
        self.format_source_like(true, "constant expression", "constant evaluation")
    }

    pub fn to_macro_arg_source(&self) -> Result<String, String> {
        self.format_source_like(false, "macro argument", "macro argument")
    }

    fn format_source_like(
        &self,
        parenthesize_infix: bool,
        domain: &str,
        err_domain: &str,
    ) -> Result<String, String> {
        match &self.node {
            ExpressionKind::Number(n) => Ok(n.to_string()),
            ExpressionKind::Identifier(s) | ExpressionKind::Label(s) => Ok(s.clone()),
            ExpressionKind::String(segs) => Ok(segs
                .iter()
                .map(|(s, _)| s.as_str())
                .collect::<Vec<_>>()
                .join(" ")),
            ExpressionKind::Prefix { operator, id } => {
                let inner = id.format_source_like(parenthesize_infix, domain, err_domain)?;
                let op = match operator {
                    TokenType::Minus => "-",
                    TokenType::Plus => "+",
                    TokenType::Not => "!",
                    other => {
                        return Err(format!("Unsupported prefix operator {other:?} in {domain}"));
                    }
                };
                Ok(format!("{op}{inner}"))
            }
            ExpressionKind::Infix {
                left,
                operator,
                right,
            } => {
                let left_str = left.format_source_like(parenthesize_infix, domain, err_domain)?;
                let right_str = right.format_source_like(parenthesize_infix, domain, err_domain)?;
                let op = match operator {
                    TokenType::Plus => "+",
                    TokenType::Minus => "-",
                    TokenType::Mul => "*",
                    TokenType::LesserThan => "<",
                    TokenType::GreaterThan => ">",
                    TokenType::LesserEqual => "<=",
                    TokenType::GreaterEqual => ">=",
                    TokenType::Equal => "==",
                    TokenType::NotEqual => "!=",
                    TokenType::And => "&&",
                    TokenType::Or => "||",
                    other => return Err(format!("Unsupported operator {other:?} in {domain}")),
                };
                let formatted = format!("{left_str} {op} {right_str}");
                if parenthesize_infix {
                    Ok(format!("({formatted})"))
                } else {
                    Ok(formatted)
                }
            }
            ExpressionKind::Call { function, args } => {
                let ExpressionKind::Identifier(name) = &function.node else {
                    return Err(format!("Call-like {err_domain} must use a simple name"));
                };

                let mut formatted_args = Vec::with_capacity(args.len());
                for arg in args {
                    formatted_args.push(arg.format_source_like(
                        parenthesize_infix,
                        domain,
                        err_domain,
                    )?);
                }

                Ok(format!("{}({})", name, formatted_args.join(", ")))
            }
            ExpressionKind::Error => Err(format!("Invalid expression in {err_domain}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infix_constant_eval_parenthesizes_macro_arg_does_not() {
        let expr = Expression {
            node: ExpressionKind::Infix {
                left: Box::new(Expression {
                    node: ExpressionKind::Number(1),
                    span: 0..1,
                }),
                operator: TokenType::Plus,
                right: Box::new(Expression {
                    node: ExpressionKind::Number(2),
                    span: 2..3,
                }),
            },
            span: 0..3,
        };
        assert_eq!(expr.to_constant_eval_source().unwrap(), "(1 + 2)");
        assert_eq!(expr.to_macro_arg_source().unwrap(), "1 + 2");
    }

    #[test]
    fn test_prefix_formatting() {
        let expr = Expression {
            node: ExpressionKind::Prefix {
                operator: TokenType::Minus,
                id: Box::new(Expression {
                    node: ExpressionKind::Number(42),
                    span: 1..3,
                }),
            },
            span: 0..3,
        };
        assert_eq!(expr.to_constant_eval_source().unwrap(), "-42");
        assert_eq!(expr.to_macro_arg_source().unwrap(), "-42");
    }

    #[test]
    fn test_call_expression_formatting() {
        let expr = Expression {
            node: ExpressionKind::Call {
                function: Box::new(Expression {
                    node: ExpressionKind::Identifier("TEST".to_string()),
                    span: 0..4,
                }),
                args: vec![
                    Expression {
                        node: ExpressionKind::Number(1),
                        span: 5..6,
                    },
                    Expression {
                        node: ExpressionKind::Number(2),
                        span: 8..9,
                    },
                ],
            },
            span: 0..10,
        };
        assert_eq!(expr.to_constant_eval_source().unwrap(), "TEST(1, 2)");
        assert_eq!(expr.to_macro_arg_source().unwrap(), "TEST(1, 2)");
    }

    #[test]
    fn test_call_non_identifier_function_contains_context() {
        let expr = Expression {
            node: ExpressionKind::Call {
                function: Box::new(Expression {
                    node: ExpressionKind::Number(42),
                    span: 0..2,
                }),
                args: vec![],
            },
            span: 0..2,
        };
        let const_err = expr.to_constant_eval_source().unwrap_err();
        assert!(
            const_err.contains("constant evaluation"),
            "expected 'constant evaluation' in error, got: {const_err}"
        );
        let macro_err = expr.to_macro_arg_source().unwrap_err();
        assert!(
            macro_err.contains("macro argument"),
            "expected 'macro argument' in error, got: {macro_err}"
        );
    }

    #[test]
    fn test_string_segment_formatting() {
        let expr = Expression {
            node: ExpressionKind::String(vec![("hello".to_string(), 0), ("world".to_string(), 6)]),
            span: 0..11,
        };
        assert_eq!(expr.to_constant_eval_source().unwrap(), "hello world");
        assert_eq!(expr.to_macro_arg_source().unwrap(), "hello world");
    }

    #[test]
    fn test_error_variant_contains_contextual_message() {
        let expr = Expression {
            node: ExpressionKind::Error,
            span: 0..0,
        };
        let const_err = expr.to_constant_eval_source().unwrap_err();
        assert!(
            const_err.contains("constant evaluation"),
            "expected 'constant evaluation' in error, got: {const_err}"
        );
        let macro_err = expr.to_macro_arg_source().unwrap_err();
        assert!(
            macro_err.contains("macro argument"),
            "expected 'macro argument' in error, got: {macro_err}"
        );
    }

    #[test]
    fn test_unsupported_prefix_contains_context() {
        let expr = Expression {
            node: ExpressionKind::Prefix {
                operator: TokenType::Mul,
                id: Box::new(Expression {
                    node: ExpressionKind::Number(5),
                    span: 1..2,
                }),
            },
            span: 0..2,
        };
        let const_err = expr.to_constant_eval_source().unwrap_err();
        assert!(
            const_err.contains("constant expression"),
            "expected 'constant expression' in error, got: {const_err}"
        );
        let macro_err = expr.to_macro_arg_source().unwrap_err();
        assert!(
            macro_err.contains("macro argument"),
            "expected 'macro argument' in error, got: {macro_err}"
        );
    }

    #[test]
    fn test_unsupported_infix_contains_context() {
        let expr = Expression {
            node: ExpressionKind::Infix {
                left: Box::new(Expression {
                    node: ExpressionKind::Number(1),
                    span: 0..1,
                }),
                operator: TokenType::Assign,
                right: Box::new(Expression {
                    node: ExpressionKind::Number(2),
                    span: 2..3,
                }),
            },
            span: 0..3,
        };
        let const_err = expr.to_constant_eval_source().unwrap_err();
        assert!(
            const_err.contains("constant expression"),
            "expected 'constant expression' in error, got: {const_err}"
        );
        let macro_err = expr.to_macro_arg_source().unwrap_err();
        assert!(
            macro_err.contains("macro argument"),
            "expected 'macro argument' in error, got: {macro_err}"
        );
    }

    #[test]
    fn test_label_formats_like_identifier() {
        let expr = Expression {
            node: ExpressionKind::Label("target".to_string()),
            span: 0..6,
        };
        assert_eq!(expr.to_constant_eval_source().unwrap(), "target");
        assert_eq!(expr.to_macro_arg_source().unwrap(), "target");
    }

    #[test]
    fn test_number_formatting() {
        let expr = Expression {
            node: ExpressionKind::Number(42),
            span: 0..2,
        };
        assert_eq!(expr.to_constant_eval_source().unwrap(), "42");
        assert_eq!(expr.to_macro_arg_source().unwrap(), "42");
    }
}
