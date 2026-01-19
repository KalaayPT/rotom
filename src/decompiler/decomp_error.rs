//! Error types for the decompiler

use std::fmt;

use serde::Serialize;

/// Error type for decompilation failures
#[derive(Debug, Serialize)]
#[serde(tag = "type", content = "details")]
pub enum DecompileError {
    /// Invalid binary format (bad magic, truncated, etc.)
    InvalidFormat { message: String },

    /// Unknown opcode encountered during disassembly
    UnknownOpcode { opcode: u16, offset: usize },

    /// Invalid jump table structure
    InvalidJumpTable { message: String, offset: usize },

    /// Offset out of bounds
    OutOfBounds { offset: usize, length: usize },

    /// Database lookup failed
    Database { message: String },

    /// IO error (file not found, etc.)
    Io { message: String },
}

impl fmt::Display for DecompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecompileError::InvalidFormat { message } => {
                write!(f, "Invalid binary format: {}", message)
            }
            DecompileError::UnknownOpcode { opcode, offset } => {
                write!(
                    f,
                    "Unknown opcode 0x{:04X} at offset 0x{:04X}",
                    opcode, offset
                )
            }
            DecompileError::InvalidJumpTable { message, offset } => {
                write!(f, "Invalid jump table at 0x{:04X}: {}", offset, message)
            }
            DecompileError::OutOfBounds { offset, length } => {
                write!(
                    f,
                    "Offset 0x{:04X} out of bounds (file length: {})",
                    offset, length
                )
            }
            DecompileError::Database { message } => {
                write!(f, "Database error: {}", message)
            }
            DecompileError::Io { message } => {
                write!(f, "IO error: {}", message)
            }
        }
    }
}

impl std::error::Error for DecompileError {}

impl From<std::io::Error> for DecompileError {
    fn from(e: std::io::Error) -> Self {
        DecompileError::Io {
            message: e.to_string(),
        }
    }
}

/// Result type for decompilation operations
pub type DecompileResult<T> = Result<T, DecompileError>;

// Helper constructors
pub fn invalid_format(message: impl Into<String>) -> DecompileError {
    DecompileError::InvalidFormat {
        message: message.into(),
    }
}

pub fn unknown_opcode(opcode: u16, offset: usize) -> DecompileError {
    DecompileError::UnknownOpcode { opcode, offset }
}

pub fn invalid_jump_table(message: impl Into<String>, offset: usize) -> DecompileError {
    DecompileError::InvalidJumpTable {
        message: message.into(),
        offset,
    }
}

pub fn out_of_bounds(offset: usize, length: usize) -> DecompileError {
    DecompileError::OutOfBounds { offset, length }
}

pub fn database_error(message: impl Into<String>) -> DecompileError {
    DecompileError::Database {
        message: message.into(),
    }
}
