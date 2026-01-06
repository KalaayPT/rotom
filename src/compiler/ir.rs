use std::collections::HashMap;
use std::fmt;

use crate::compiler::ast::{FunctionHeader, ScriptFile};

use super::{
    analysis::{SymbolTable, SymbolType},
    ast::{Expression, ExpressionKind, Statement, StatementKind},
    parse_error::{ParseResult, lowering_error},
    token::TokenType,
};

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

pub struct IrFunction {
    pub headers: Vec<FunctionHeader>,
    pub instructions: Vec<IrOpcode>,
}
pub struct IrAction {
    pub name: String,
    pub instructions: Vec<IrOpcode>,
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
    pub fn jump_table_slots(&self) -> impl Iterator<Item = (u32, String)> {
        self.headers
            .iter()
            .filter(|h| h.is_public && h.id.is_some())
            .map(|h| (h.id.unwrap(), h.name.clone()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum Condition {
    Less = 0,         // 0x00
    Equal = 1,        // 0x01
    Greater = 2,      // 0x02
    LessEqual = 3,    // 0x03
    GreaterEqual = 4, // 0x04
    Different = 5,    // 0x05
}

#[derive(Debug, PartialEq)]
enum OperandType {
    Variable, // VarPointer (0x8000)
    Value,    // raw number (5)
}

#[derive(Debug, Clone)]
pub struct Lowerer<'a> {
    label_counter: usize,
    output: Vec<IrOpcode>,
    global_symbols: &'a SymbolTable,
    local_aliases: HashMap<String, i32>,
}

impl<'a> Lowerer<'a> {
    pub fn new(symbols: &'a SymbolTable) -> Self {
        Self {
            label_counter: 0,
            output: Vec::new(),
            global_symbols: symbols,
            local_aliases: HashMap::new(),
        }
    }
    fn new_label(&mut self, prefix: &str) -> String {
        self.label_counter += 1;
        format!(".{}_gen_{}", prefix, self.label_counter)
    }
    pub fn lower_script_file(
        &mut self,
        scr_file: &ScriptFile,
    ) -> ParseResult<(Vec<IrFunction>, Vec<IrAction>)> {
        let mut ir_functions = Vec::new();
        for func in &scr_file.functions {
            if let StatementKind::Function { headers, body } = &func.node {
                self.local_aliases.clear();
                let instructions = self.lower_function(body)?;
                ir_functions.push(IrFunction {
                    headers: headers.clone(),
                    instructions,
                });
            }
        }
        let mut ir_actions = Vec::new();
        for action in &scr_file.actions {
            if let StatementKind::Action { name, body } = &action.node {
                self.local_aliases.clear();
                let instructions = self.lower_function(body)?;
                ir_actions.push(IrAction {
                    name: name.clone(),
                    instructions,
                });
            }
        }
        Ok((ir_functions, ir_actions))
    }
    pub fn lower_function(&mut self, body: &[Statement]) -> ParseResult<Vec<IrOpcode>> {
        self.output.clear();
        for stmt in body {
            self.lower_statement(stmt)?;
        }
        Ok(std::mem::take(&mut self.output))
    }
    fn lower_statement(&mut self, stmt: &Statement) -> ParseResult<()> {
        match &stmt.node {
            StatementKind::IfStatement {
                condition,
                body,
                elseblock,
            } => {
                let label_end = self.new_label("end_if");
                let label_else = if elseblock.is_some() {
                    Some(self.new_label("else"))
                } else {
                    None
                };
                let jump_target = label_else.as_ref().unwrap_or(&label_end);
                self.lower_condition(condition, jump_target)?;
                for s in body {
                    self.lower_statement(s)?;
                }
                if let Some(else_b) = elseblock {
                    self.output.push(IrOpcode::Command {
                        name: "GoTo".to_string(),
                        args: vec![Arg::Pointer(label_end.clone())],
                    });
                    // unwrap is safe here because we already checked if else is some
                    self.output.push(IrOpcode::Label(label_else.unwrap()));
                    for s in else_b {
                        self.lower_statement(s)?;
                    }
                }
                self.output.push(IrOpcode::Label(label_end));
            }
            StatementKind::WhileStatement { condition, body } => {
                let label_start = self.new_label("while_start");
                let label_end = self.new_label("while_end");
                self.output.push(IrOpcode::Label(label_start.clone()));
                self.lower_condition(condition, &label_end)?;
                for s in body {
                    self.lower_statement(s)?;
                }
                self.output.push(IrOpcode::Command {
                    name: "GoTo".to_string(),
                    args: vec![Arg::Pointer(label_start.clone())],
                });
                self.output.push(IrOpcode::Label(label_end));
            }
            StatementKind::ScriptCommand { command, args } => {
                let resolved_args = self.resolve_args(args)?;
                self.output.push(IrOpcode::Command {
                    name: command.clone(),
                    args: resolved_args,
                });
            }

            StatementKind::Label(name) => self.output.push(IrOpcode::Label(name.clone())),
            StatementKind::Jump(target) => {
                if let ExpressionKind::Label(name) | ExpressionKind::Identifier(name) = &target.node
                {
                    self.output.push(IrOpcode::Command {
                        name: "GoTo".to_string(),
                        args: vec![Arg::Pointer(name.clone())],
                    });
                }
            }
            StatementKind::Return => self.output.push(IrOpcode::Command {
                name: "Return".to_string(),
                args: vec![],
            }),
            StatementKind::End => self.output.push(IrOpcode::Command {
                name: "End".to_string(),
                args: vec![],
            }),
            // Register local aliases for resolution during this function's lowering
            StatementKind::AliasStatement { name, id, .. } => {
                self.local_aliases.insert(name.clone(), *id);
            }

            _ => {}
        }
        Ok(())
    }
    fn lower_condition(&mut self, expr: &Expression, target_label: &str) -> ParseResult<()> {
        if let ExpressionKind::Infix {
            left,
            operator,
            right,
        } = &expr.node
        {
            let (left_type, left_val) = self.analyze_operand(left)?;
            let (right_type, right_val) = self.analyze_operand(right)?;
            // normalization: swapping to match script architecture (Value == Var) -> (Var == Value)
            let (final_left, final_right, swapped) = match (&left_type, &right_type) {
                (OperandType::Variable, OperandType::Value) => (left_val, right_val, false),
                (OperandType::Value, OperandType::Variable) => (right_val, left_val, true),
                (OperandType::Variable, OperandType::Variable) => (left_val, right_val, false),
                // Value == Value (Should have been constant-folded, but whatever)
                (OperandType::Value, OperandType::Value) => (left_val, right_val, false),
            };
            let cmd_name =
                if left_type == OperandType::Variable && right_type == OperandType::Variable {
                    "CompareVars"
                } else {
                    "CompareVarValue"
                };
            self.output.push(IrOpcode::Command {
                name: cmd_name.to_string(),
                args: vec![Arg::Value(final_left), Arg::Value(final_right)],
            });
            let cond = self.get_inverted_condition(operator, swapped);
            self.output.push(IrOpcode::Command {
                name: "GoToIf".to_string(),
                args: vec![
                    Arg::Value(cond as i32),
                    Arg::Pointer(target_label.to_string()),
                ],
            });
        }
        Ok(())
    }
    fn resolve_args(&self, args: &[Expression]) -> ParseResult<Vec<Arg>> {
        args.iter().map(|arg| self.resolve_arg(arg)).collect()
    }
    /// Resolve an expression to an Arg (Value or Pointer)
    fn resolve_arg(&self, expr: &Expression) -> ParseResult<Arg> {
        match &expr.node {
            ExpressionKind::Identifier(name) => {
                if let Some(&val) = self.local_aliases.get(name) {
                    return Ok(Arg::Value(val));
                }
                match self.global_symbols.resolve(name) {
                    Some(SymbolType::Variable(id)) => Ok(Arg::Value(*id)),
                    Some(SymbolType::Constant(id)) => Ok(Arg::Value(*id)),
                    Some(SymbolType::Function(_))
                    | Some(SymbolType::Label)
                    | Some(SymbolType::Action) => Ok(Arg::Pointer(name.clone())),
                    None => Err(lowering_error(format!(
                        "Symbol '{}' could not be resolved (analysis should have caught this)",
                        name
                    ))),
                }
            }
            ExpressionKind::Number(val) => Ok(Arg::Value(*val)),
            ExpressionKind::Label(name) => Ok(Arg::Pointer(name.clone())),
            // compile-time arithmetic
            ExpressionKind::Infix {
                left,
                operator,
                right,
            } => {
                let left_val = self.resolve_arg(left)?.unwrap_value();
                let right_val = self.resolve_arg(right)?.unwrap_value();
                let result = match operator {
                    TokenType::Plus => left_val + right_val,
                    TokenType::Minus => left_val - right_val,
                    TokenType::Mul => left_val * right_val,
                    _ => {
                        return Err(lowering_error(format!(
                            "Unsupported operator '{:?}' in compile-time arithmetic (only +, -, * are supported)",
                            operator
                        )));
                    }
                };
                Ok(Arg::Value(result))
            }
            ExpressionKind::Prefix { operator, id } => {
                let val = self.resolve_arg(id)?.unwrap_value();
                let result = match operator {
                    TokenType::Minus => -val,
                    _ => {
                        return Err(lowering_error(format!(
                            "Unsupported prefix operator '{:?}' (only unary minus is supported)",
                            operator
                        )));
                    }
                };
                Ok(Arg::Value(result))
            }
            _ => Err(lowering_error(format!(
                "Unsupported expression type: {:?}",
                expr.node
            ))),
        }
    }
    /// Analyze operand for conditions - needs to distinguish Variable vs Value
    fn analyze_operand(&self, expr: &Expression) -> ParseResult<(OperandType, i32)> {
        let arg = self.resolve_arg(expr)?;
        match arg {
            Arg::Value(v) => {
                if v >= 0x4000 {
                    Ok((OperandType::Variable, v))
                } else {
                    Ok((OperandType::Value, v))
                }
            }
            Arg::Pointer(name) => Err(lowering_error(format!(
                "Cannot use pointer '{}' in condition expression",
                name
            ))),
        }
    }
    // we need to invert conditions here because we are doing "jump if" logic
    fn get_inverted_condition(&self, token: &TokenType, swapped: bool) -> Condition {
        use Condition::*;

        let effective_op = if swapped {
            match token {
                TokenType::GreaterThan => TokenType::LesserThan,
                TokenType::LesserThan => TokenType::GreaterThan,
                TokenType::GreaterEqual => TokenType::LesserEqual,
                TokenType::LesserEqual => TokenType::GreaterEqual,
                _ => token.clone(),
            }
        } else {
            token.clone()
        };

        match effective_op {
            TokenType::Equal => Different,         // == -> Jump if !=
            TokenType::NotEqual => Equal,          // != -> Jump if ==
            TokenType::GreaterThan => LessEqual,   // >  -> Jump if <=
            TokenType::LesserThan => GreaterEqual, // <  -> Jump if >=
            TokenType::GreaterEqual => Less,       // >= -> Jump if <
            TokenType::LesserEqual => Greater,     // <= -> Jump if >
            _ => Different,                        // Default/Error case
        }
    }
}
