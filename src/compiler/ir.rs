use std::collections::HashMap;
use std::fmt;

use super::{
    analysis::{SymbolTable, SymbolType},
    ast::{Expression, ExpressionKind, Statement, StatementKind},
    parse_error::{lowering_error, ParseResult},
    token::TokenType,
};

pub enum IrOpcode {
    Command { name: String, args: Vec<i32> },
    Label(String),
    Jump(String),
    JumpIf { cond: Condition, label: String },
    Return,
    End,
}

impl fmt::Display for IrOpcode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IrOpcode::Command { name, args } => {
                let args_str: Vec<String> = args.iter().map(|a| format!("0x{:X}", a)).collect();
                write!(f, "    {} {}", name, args_str.join(", "))
            }
            IrOpcode::Label(name) => write!(f, "{}:", name),
            IrOpcode::Jump(target) => write!(f, "    Jump {}", target),
            IrOpcode::JumpIf { cond, label } => write!(f, "    JumpIf {:?} -> {}", cond, label),
            IrOpcode::Return => write!(f, "    Return"),
            IrOpcode::End => write!(f, "    End"),
        }
    }
}

pub struct IrFunction {
    pub name: String,
    pub instructions: Vec<IrOpcode>,
}

impl fmt::Display for IrFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "=== IR: {} ===", self.name)?;
        for op in &self.instructions {
            writeln!(f, "{}", op)?;
        }
        Ok(())
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
    fn resolve_name(&self, name: &str) -> Option<i32> {
        // Check local aliases first
        if let Some(&val) = self.local_aliases.get(name) {
            return Some(val);
        }
        match self.global_symbols.resolve(name) {
            Some(SymbolType::Variable(id)) => Some(*id),
            _ => None,
        }
    }
    pub fn lower_function(mut self, body: &[Statement]) -> ParseResult<Vec<IrOpcode>> {
        for stmt in body {
            self.lower_statement(stmt)?;
        }
        Ok(self.output)
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
                    self.output.push(IrOpcode::Jump(label_end.clone()));
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
                self.output.push(IrOpcode::Jump(label_start));
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
                    self.output.push(IrOpcode::Jump(name.clone()));
                }
            }
            StatementKind::Return => self.output.push(IrOpcode::Return),
            StatementKind::End => self.output.push(IrOpcode::End),

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
                args: vec![final_left, final_right],
            });
            let cond = self.get_inverted_condition(operator, swapped);
            self.output.push(IrOpcode::JumpIf {
                cond,
                label: target_label.to_string(),
            });
        }
        Ok(())
    }
    fn analyze_operand(&self, expr: &Expression) -> ParseResult<(OperandType, i32)> {
        match &expr.node {
            ExpressionKind::Identifier(name) => match self.resolve_name(name) {
                Some(id) => {
                    if id < 0x4000 {
                        Ok((OperandType::Value, id))
                    } else {
                        Ok((OperandType::Variable, id))
                    }
                }
                None => Err(lowering_error(format!(
                    "Symbol '{}' could not be resolved (analysis should have caught this)",
                    name
                ))),
            },
            ExpressionKind::Number(val) => Ok((OperandType::Value, *val)),
            // Handle compile-time arithmetic
            ExpressionKind::Infix {
                left,
                operator,
                right,
            } => {
                let (_, left_val) = self.analyze_operand(left)?;
                let (_, right_val) = self.analyze_operand(right)?;
                let result = match operator {
                    TokenType::Plus => left_val + right_val,
                    TokenType::Minus => left_val - right_val,
                    TokenType::Mul => left_val * right_val,
                    _ => {
                        return Err(lowering_error(format!(
                            "Unsupported operator '{:?}' in compile-time arithmetic (only +, -, * are supported)",
                            operator
                        )))
                    }
                };
                Ok((OperandType::Value, result))
            }
            ExpressionKind::Prefix { operator, id } => {
                let (_, val) = self.analyze_operand(id)?;
                let result = match operator {
                    TokenType::Minus => -val,
                    _ => {
                        return Err(lowering_error(format!(
                            "Unsupported prefix operator '{:?}' (only unary minus is supported)",
                            operator
                        )))
                    }
                };
                Ok((OperandType::Value, result))
            }
            // TODO: Handle Call expressions for conditions like `if GetPlayerGender() == 1`
            _ => Err(lowering_error(format!(
                "Unsupported expression type in operand: {:?}",
                expr.node
            ))),
        }
    }
    fn resolve_args(&self, args: &[Expression]) -> ParseResult<Vec<i32>> {
        args.iter()
            .map(|arg| {
                let (_, val) = self.analyze_operand(arg)?;
                Ok(val)
            })
            .collect()
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
