//! Rotom - A Pokemon Gen 4 script compiler/decompiler
//!
//! This library provides functionality to compile Rotoscript source files
//! to binary format and decompile binary scripts back to Rotoscript.

pub mod compiler;
pub mod database;
pub mod transpiler;

// Re-export commonly used types for convenience
pub use compiler::{
    Lexer, Parser, Analyzer, Lowerer,
    parse_error::{CompileError, print_error},
};
pub use compiler::codegen::Emitter;
pub use database::{DatabaseV2, ConstantDb, GameFamily};
