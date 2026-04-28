use std::fmt;
use std::ops::Range;

use codespan_reporting::diagnostic::Severity;
use serde::Serialize;

use super::{render_diagnostic, serialize_range};

/// Unified error type for the compiler
#[derive(Debug, Serialize)]
#[serde(tag = "type", content = "details")]
pub enum CompileError {
    /// Error during lexing/parsing with source location
    Parse {
        #[serde(serialize_with = "serialize_range")]
        span: Range<usize>,
        message: String,
    },

    /// Error during semantic analysis
    Analysis {
        #[serde(serialize_with = "serialize_range")]
        span: Range<usize>,
        message: String,
    },

    /// Error during IR lowering
    Lowering { message: String },

    /// Error during code generation
    Codegen { message: String },

    /// Error during source transpilation/conversion
    Transpile { message: String },

    /// Error loading database
    Database { message: String },

    /// IO error (file not found, etc.)
    Io { message: String },
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompileError::Parse { message, .. } => write!(f, "Parse error: {}", message),
            CompileError::Analysis { message, .. } => write!(f, "Analysis error: {}", message),
            CompileError::Lowering { message } => write!(f, "Lowering error: {}", message),
            CompileError::Codegen { message } => write!(f, "Codegen error: {}", message),
            CompileError::Transpile { message } => write!(f, "Transpile error: {}", message),
            CompileError::Database { message } => write!(f, "Database error: {}", message),
            CompileError::Io { message } => write!(f, "IO error: {}", message),
        }
    }
}

impl std::error::Error for CompileError {}

impl From<std::io::Error> for CompileError {
    fn from(e: std::io::Error) -> Self {
        CompileError::Io {
            message: e.to_string(),
        }
    }
}

impl From<serde_json::Error> for CompileError {
    fn from(e: serde_json::Error) -> Self {
        CompileError::Database {
            message: e.to_string(),
        }
    }
}

pub type ParseResult<T> = Result<T, CompileError>;

/// Helper to create a parse error
pub fn parse_error(span: Range<usize>, message: impl Into<String>) -> CompileError {
    CompileError::Parse {
        span,
        message: message.into(),
    }
}

/// Helper to create an analysis error
pub fn analysis_error(span: Range<usize>, message: impl Into<String>) -> CompileError {
    CompileError::Analysis {
        span,
        message: message.into(),
    }
}

/// Helper to create a lowering error
pub fn lowering_error(message: impl Into<String>) -> CompileError {
    CompileError::Lowering {
        message: message.into(),
    }
}

/// Helper to create a codegen error
pub fn codegen_error(message: impl Into<String>) -> CompileError {
    CompileError::Codegen {
        message: message.into(),
    }
}

pub fn database_error(message: impl Into<String>) -> CompileError {
    CompileError::Database {
        message: message.into(),
    }
}

/// Print a compile error with source context
pub fn print_error(filename: &str, source: &str, error: &CompileError) {
    let (span, message, error_type) = match error {
        CompileError::Parse { span, message } => {
            (Some(span.clone()), message.as_str(), "Parse error")
        }
        CompileError::Analysis { span, message } => {
            (Some(span.clone()), message.as_str(), "Analysis error")
        }
        CompileError::Lowering { message } => (None, message.as_str(), "Lowering error"),
        CompileError::Codegen { message } => (None, message.as_str(), "Codegen error"),
        CompileError::Transpile { message } => (None, message.as_str(), "Transpile error"),
        CompileError::Database { message } => (None, message.as_str(), "Database error"),
        CompileError::Io { message } => (None, message.as_str(), "IO error"),
    };

    if let Some(span) = span {
        render_diagnostic(
            filename,
            source,
            Severity::Error,
            error_type,
            span,
            message.to_string(),
        );
    } else {
        eprintln!("{}: {}: {}", filename, error_type, message);
    }
}
