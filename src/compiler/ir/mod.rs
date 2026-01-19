//! Intermediate Representation (IR) for the Rotom compiler
//!
//! This module contains:
//! - Core IR types (IrOpcode, Arg, IrFunction, IrAction, TopLevelItem)
//! - The Lowerer that transforms AST → IR

mod lowerer;
mod types;

pub use lowerer::Lowerer;
pub(crate) use types::OperandType;
pub use types::{Arg, Condition, IrAction, IrFunction, IrOpcode, TopLevelItem};
