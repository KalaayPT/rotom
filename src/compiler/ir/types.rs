//! Core IR types for the Rotom compiler
//!
//! This module defines the intermediate representation used between
//! the parser/analyzer and the code generator.

use std::fmt;

use crate::compiler::ast::FunctionHeader;

/// An IR opcode - either a command or a label
#[derive(Debug, Clone)]
pub enum IrOpcode {
    Command { name: String, args: Vec<Arg> },
    Label(String),
}

impl fmt::Display for IrOpcode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IrOpcode::Command { name, args } => {
                let args_str: Vec<String> = args.iter().map(|a| format!("{}", a)).collect();
                write!(f, "    {} {}", name, args_str.join(", "))
            }
            IrOpcode::Label(name) => write!(f, "{}:", name),
        }
    }
}

/// An argument to an IR command
#[derive(Debug, Clone)]
pub enum Arg {
    Value(i32),
    Pointer(String),
}

impl fmt::Display for Arg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Arg::Value(v) => write!(f, "0x{:X}", v),
            Arg::Pointer(s) => write!(f, "Pointer({})", s),
        }
    }
}

impl Arg {
    pub fn unwrap_value(&self) -> i32 {
        match self {
            Arg::Value(v) => *v,
            _ => panic!("called unwrap_value on {:?}", self),
        }
    }
    pub fn unwrap_pointer(&self) -> String {
        match self {
            Arg::Pointer(s) => s.clone(),
            _ => panic!("called unwrap_pointer on {:?}", self),
        }
    }
}

/// A function in the IR, with one or more headers (for stacked entry points)
#[derive(Debug, Clone)]
pub struct IrFunction {
    pub headers: Vec<FunctionHeader>,
    pub instructions: Vec<IrOpcode>,
}

/// An action (movement sequence) in the IR
#[derive(Debug, Clone)]
pub struct IrAction {
    pub name: String,
    pub instructions: Vec<IrOpcode>,
}

/// A top-level item in a script (preserves ordering of functions and actions)
#[derive(Debug, Clone)]
pub enum TopLevelItem {
    Function(IrFunction),
    Action(IrAction),
}

impl fmt::Display for TopLevelItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TopLevelItem::Function(func) => write!(f, "{}", func),
            TopLevelItem::Action(action) => write!(f, "{}", action),
        }
    }
}

impl fmt::Display for IrFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "=== IR: {} ===", self.headers[0].name)?;
        for op in &self.instructions {
            writeln!(f, "{}", op)?;
        }
        Ok(())
    }
}

impl fmt::Display for IrAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "=== IR Action: {} ===", self.name)?;
        for op in &self.instructions {
            writeln!(f, "{}", op)?;
        }
        Ok(())
    }
}

impl IrFunction {
    pub fn name(&self) -> &str {
        &self.headers[0].name
    }
    pub fn is_public(&self) -> bool {
        self.headers.iter().any(|h| h.is_public)
    }
    pub fn jump_table_slots(&self) -> impl Iterator<Item = (u32, String)> + '_ {
        self.headers
            .iter()
            .filter(|h| h.is_public)
            .filter_map(|h| h.id.map(|id| (id, h.name.clone())))
    }
}

/// Condition codes for comparison jumps
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Condition {
    Less = 0,         // 0x00
    Equal = 1,        // 0x01
    Greater = 2,      // 0x02
    LessEqual = 3,    // 0x03
    GreaterEqual = 4, // 0x04
    Different = 5,    // 0x05
}

#[derive(Debug, PartialEq, Eq)]
pub enum OperandType {
    Variable, // VarPointer (0x8000)
    Value,    // raw number (5)
}
