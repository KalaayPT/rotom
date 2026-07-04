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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_formats_all_variants() {
        assert_eq!(
            DecompileError::InvalidFormat {
                message: "too short".to_string(),
            }
            .to_string(),
            "Invalid binary format: too short"
        );
        assert_eq!(
            DecompileError::UnknownOpcode {
                opcode: 0x1234,
                offset: 0x20,
            }
            .to_string(),
            "Unknown opcode 0x1234 at offset 0x0020"
        );
        assert_eq!(
            DecompileError::InvalidJumpTable {
                message: "bad pointer".to_string(),
                offset: 0x10,
            }
            .to_string(),
            "Invalid jump table at 0x0010: bad pointer"
        );
        assert_eq!(
            DecompileError::OutOfBounds {
                offset: 0x30,
                length: 16,
            }
            .to_string(),
            "Offset 0x0030 out of bounds (file length: 16)"
        );
        assert_eq!(
            DecompileError::Database {
                message: "missing opcode".to_string(),
            }
            .to_string(),
            "Database error: missing opcode"
        );
        assert_eq!(
            DecompileError::from(std::io::Error::other("disk")).to_string(),
            "IO error: disk"
        );
    }
}
