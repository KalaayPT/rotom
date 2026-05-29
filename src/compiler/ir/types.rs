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
    pub fn value(&self, context: &str) -> crate::compiler::ParseResult<i32> {
        match self {
            Arg::Value(v) => Ok(*v),
            Arg::Pointer(target) => Err(crate::compiler::diagnostic::lowering_error(format!(
                "expected numeric value for {context}, found pointer to '{target}'"
            ))),
        }
    }

    #[cfg(test)]
    pub fn unwrap_value(&self) -> i32 {
        match self {
            Arg::Value(v) => *v,
            Arg::Pointer(_) => panic!("called unwrap_value on {:?}", self),
        }
    }

    #[cfg(test)]
    pub fn unwrap_pointer(&self) -> String {
        match self {
            Arg::Pointer(s) => s.clone(),
            Arg::Value(_) => panic!("called unwrap_pointer on {:?}", self),
        }
    }
}

/// A script in the IR, with one or more headers (for stacked entry points)
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

#[derive(Debug, PartialEq, Eq)]
pub enum OperandType {
    Variable, // VarPointer (0x8000)
    Value,    // raw number (5)
}

#[cfg(test)]
mod tests {
    use super::Arg;

    #[test]
    fn value_returns_inner_number() {
        assert_eq!(Arg::Value(42).value("test context").unwrap(), 42);
    }

    #[test]
    fn value_rejects_pointer_with_context() {
        let error = Arg::Pointer("target".to_string())
            .value("match subject")
            .expect_err("pointer should not be accepted as a value");

        assert!(
            error.to_string().contains("match subject"),
            "error should include caller context: {error}"
        );
    }
}
