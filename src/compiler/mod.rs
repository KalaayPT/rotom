// Compiler module - all compilation pipeline components

pub mod analysis;
pub mod ast;
pub mod batch_compile;
pub mod codegen;
pub mod diagnostic;
pub mod ir;
pub mod lexer;
pub(crate) mod macro_condition;
pub mod parser;
pub mod sourcemap;
pub mod token;

// Re-export commonly used types
pub use analysis::{Analyzer, SymbolTable, SymbolType};
pub use ast::StatementKind;
pub use diagnostic::{CompileError, CompileWarning, ParseResult};
pub use ir::{IrFunction, Lowerer};
pub use lexer::Lexer;
pub use parser::Parser;
pub use sourcemap::{Position, SourceMap};
