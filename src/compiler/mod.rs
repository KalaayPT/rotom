// Compiler module - all compilation pipeline components

pub mod analysis;
pub mod ast;
pub mod codegen;
pub mod ir;
pub mod lexer;
pub mod parse_error;
pub mod parser;
pub mod token;

// Re-export commonly used types
pub use analysis::Analyzer;
pub use ast::StatementKind;
pub use ir::{IrFunction, Lowerer};
pub use lexer::Lexer;
pub use parse_error::ParseResult;
pub use parser::Parser;
