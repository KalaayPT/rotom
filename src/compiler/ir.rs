use std::collections::HashMap;
use std::fmt;

use crate::compiler::ast::{FunctionHeader, ScriptFile};
use crate::database::{Command, DatabaseV2, ParamDef};

use super::{
    analysis::{SymbolTable, SymbolType},
    ast::{Expression, ExpressionKind, Statement, StatementKind},
    parse_error::{ParseResult, lowering_error},
    token::TokenType,
    Lexer, Parser,
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

#[derive(Debug, Clone)]
pub struct IrFunction {
    pub headers: Vec<FunctionHeader>,
    pub instructions: Vec<IrOpcode>,
}
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

/// Maximum depth for macro expansion to prevent infinite recursion
const MAX_MACRO_DEPTH: usize = 10;

#[derive(Debug, Clone)]
pub struct Lowerer<'a> {
    label_counter: usize,
    output: Vec<IrOpcode>,
    global_symbols: &'a SymbolTable,
    local_aliases: HashMap<String, i32>,
    db: &'a DatabaseV2,
}

impl<'a> Lowerer<'a> {
    pub fn new(symbols: &'a SymbolTable, db: &'a DatabaseV2) -> Self {
        Self {
            label_counter: 0,
            output: Vec::new(),
            global_symbols: symbols,
            local_aliases: HashMap::new(),
            db,
        }
    }
    fn new_label(&mut self, prefix: &str) -> String {
        self.label_counter += 1;
        format!(".{}_gen_{}", prefix, self.label_counter)
    }

    pub fn lower_script_file(&mut self, scr_file: &ScriptFile) -> ParseResult<Vec<TopLevelItem>> {
        let mut items = Vec::new();
        for item in &scr_file.items {
            match &item.node {
                StatementKind::Function { headers, body } => {
                    self.local_aliases.clear();
                    let instructions = self.lower_function(body)?;
                    items.push(TopLevelItem::Function(IrFunction {
                        headers: headers.clone(),
                        instructions,
                    }));
                }
                StatementKind::Action { name, body } => {
                    self.local_aliases.clear();
                    let instructions = self.lower_function(body)?;
                    items.push(TopLevelItem::Action(IrAction {
                        name: name.clone(),
                        instructions,
                    }));
                }
                _ => {}
            }
        }
        Ok(items)
    }
    pub fn lower_function(&mut self, body: &[Statement]) -> ParseResult<Vec<IrOpcode>> {
        self.output.clear();
        for stmt in body {
            self.lower_statement(stmt)?;
        }
        Ok(std::mem::take(&mut self.output))
    }
    fn lower_statement(&mut self, stmt: &Statement) -> ParseResult<()> {
        self.lower_statement_with_depth(stmt, 0)
    }

    fn lower_statement_with_depth(
        &mut self,
        stmt: &Statement,
        macro_depth: usize,
    ) -> ParseResult<()> {
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
                    self.lower_statement_with_depth(s, macro_depth)?;
                }
                if let Some(else_b) = elseblock {
                    self.output.push(IrOpcode::Command {
                        name: "GoTo".to_string(),
                        args: vec![Arg::Pointer(label_end.clone())],
                    });
                    // unwrap is safe here because we already checked if else is some
                    self.output.push(IrOpcode::Label(label_else.unwrap()));
                    for s in else_b {
                        self.lower_statement_with_depth(s, macro_depth)?;
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
                    self.lower_statement_with_depth(s, macro_depth)?;
                }
                self.output.push(IrOpcode::Command {
                    name: "GoTo".to_string(),
                    args: vec![Arg::Pointer(label_start.clone())],
                });
                self.output.push(IrOpcode::Label(label_end));
            }
            StatementKind::ScriptCommand { command, args } => {
                self.lower_command(command, args, macro_depth)?;
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
            StatementKind::EndMovement => self.output.push(IrOpcode::Command {
                name: "EndMovement".to_string(),
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
    
    /// Lower a command, expanding macros if needed
    fn lower_command(
        &mut self,
        command: &str,
        args: &[Expression],
        macro_depth: usize,
    ) -> ParseResult<()> {
        // Check if this is a macro by looking it up in the database
        if let Ok(cmd) = self.db.get_command(command) {
            if cmd.is_macro() {
                return self.expand_macro(command, args, macro_depth);
            }

            // Not a macro - apply defaults if needed, then emit directly
            let args_with_defaults = self.apply_defaults(command, &cmd, args)?;
            let resolved_args = self.resolve_args(&args_with_defaults)?;
            self.output.push(IrOpcode::Command {
                name: command.to_string(),
                args: resolved_args,
            });
            Ok(())
        } else {
            // Command not found in DB - emit as-is (assuming it's valid)
            let resolved_args = self.resolve_args(args)?;
            self.output.push(IrOpcode::Command {
                name: command.to_string(),
                args: resolved_args,
            });
            Ok(())
        }
    }

    /// Apply default parameter values to fill in missing arguments
    /// Arguments map to required parameter positions. Optional parameters with defaults
    /// are filled in when there are fewer arguments than parameters.
    fn apply_defaults(
        &self,
        command: &str,
        cmd: &Command,
        args: &[Expression],
    ) -> ParseResult<Vec<Expression>> {
        let params = &cmd.params;
        let arg_count = args.len();
        let param_count = params.len();

        if arg_count == param_count {
            return Ok(args.to_vec());
        }
        
        // For args < params, skip params with defaults
        // Start from the END of the params list and work backwards
        let mut result: Vec<Option<Expression>> = vec![None; param_count];
        let mut arg_idx = arg_count;
        
        // Fill from the end
        for i in (0..param_count).rev() {
            if let Some(default_str) = params[i].default.as_ref() {
                // Param has default - fill with default, don't consume arg
                let lexer = Lexer::new(default_str);
                let mut parser = Parser::new(lexer);
                let expr = parser.parse_expression(crate::compiler::ast::Precedence::Lowest)?;
                result[i] = Some(expr);
            } else if arg_idx > 0 {
                // No default, consume an arg
                result[i] = Some(args[arg_idx - 1].clone());
                arg_idx -= 1;
            } else {
                // No default and no arg available
                return Err(lowering_error(format!(
                    "Missing required argument '{}' for command '{}'",
                    params[i].name, command
                )));
            }
        }
        
        // Convert Option<Vec> to Vec
        Ok(result.into_iter().map(Option::unwrap).collect())
    }

    /// Calculate number of required parameters (those without defaults)
    fn count_required(params: &[ParamDef]) -> usize {
        params.iter().filter(|p| p.default.is_none()).count()
    }

    /// Expand a macro by substituting parameters and recursively lowering
    fn expand_macro(
        &mut self,
        macro_name: &str,
        args: &[Expression],
        depth: usize,
    ) -> ParseResult<()> {
        if depth > MAX_MACRO_DEPTH {
            return Err(lowering_error(format!(
                "Macro expansion depth exceeded (max {}) while expanding '{}'. Possible infinite recursion.",
                MAX_MACRO_DEPTH, macro_name
            )));
        }
        
        let cmd = self.db.get_command(macro_name)?;
        let params = &cmd.params;
        // Apply defaults for missing arguments
        let args_with_defaults = self.apply_defaults(macro_name, cmd, args)?;
        
        // Build parameter substitution map: param_name -> formatted value
        if args_with_defaults.len() != params.len() {
            return Err(lowering_error(format!(
                "Macro '{}' expects {} arguments, got {}",
                macro_name,
                params.len(),
                args_with_defaults.len()
            )));
        }
        
        // Check for conditional variants
        let expansion = if let Some(variants) = &cmd.variants {
            // Find the first matching variant
            let mut matched_expansion = None;
            for variant in variants {
                if let Some(condition) = &variant.condition {
                    if condition == "else" {
                        matched_expansion = variant.expansion.as_ref();
                        break;
                    }
                    
                    // Evaluate condition
                    if self.evaluate_condition(condition, &args_with_defaults, params)? {
                    matched_expansion = variant.expansion.as_ref();
                    break;
                }
            }
        }
            
            // If no match found in variants, fall back to base expansion?
            matched_expansion.or(cmd.expansion.as_ref())
        } else {
            cmd.expansion.as_ref()
        };

        let expansion = expansion.ok_or_else(|| {
            lowering_error(format!(
                "Macro '{}' has no expansion defined (and no matching variant)",
                macro_name
            ))
        })?;
        
        let mut param_map: HashMap<String, String> = HashMap::new();
        for (param, arg) in params.iter().zip(args.iter()) {
            let formatted = self.format_arg_for_substitution(arg)?;
            param_map.insert(param.name.clone(), formatted);
        }
        
        // Process each line in expansion
        for line in expansion {
            // Substitute $paramName with actual values
            let substituted = self.substitute_params(line, &param_map);
            
            // Parse as substituted line as a command
            let parsed_stmt = self.parse_expansion_line(&substituted)?;
            
            // Recursively lower (which may expand nested macros)
            self.lower_statement_with_depth(&parsed_stmt, depth + 1)?;
        }
        
        Ok(())
    }

    /// Evaluate a macro condition string (e.g., "\value < VARS_START")
    fn evaluate_condition(
        &self,
        condition: &str,
        args: &[Expression],
        params: &[crate::database::ParamDef],
    ) -> ParseResult<bool> {
        use regex::Regex;
        
        // 1. Substitute \paramName with the integer value of the argument
        let re = Regex::new(r"\\(\w+)").unwrap();
        
        let substituted = re.replace_all(condition, |caps: &regex::Captures| {
            let param_name = &caps[1];
            // Find the argument corresponding to this param name
            if let Some(pos) = params.iter().position(|p| p.name == param_name) {
                if let Some(arg) = args.get(pos) {
                    // Resolve argument to an integer value
                    match self.resolve_arg_to_int(arg) {
                        Ok(val) => val.to_string(),
                        Err(_) => "0".to_string(), // Error fallback
                    }
                } else {
                    "0".to_string()
                }
            } else {
                // Unknown param
                "0".to_string()
            }
        });
        
        // 2. Parse the substituted string as an expression
        let lexer = Lexer::new(&substituted);
        let mut parser = Parser::new(lexer);
        let expr = parser.parse_expression(crate::compiler::ast::Precedence::Lowest)?;
        
        // 3. Evaluate the expression to a boolean
        self.eval_bool_expr(&expr)
    }
    
    /// Resolve an argument expression to an integer value (for condition evaluation)
    fn resolve_arg_to_int(&self, expr: &Expression) -> ParseResult<i32> {
        match &expr.node {
            ExpressionKind::Number(n) => Ok(*n),
            ExpressionKind::Identifier(name) => {
                // Resolve identifier using global symbols (constants)
                if let Some(SymbolType::Constant(val)) = self.global_symbols.resolve(name) {
                    Ok(*val)
                } else if let Some(SymbolType::Variable(val)) = self.global_symbols.resolve(name) {
                    Ok(*val)
                } else {
                    Err(lowering_error(format!(
                        "Could not resolve '{}' to an integer for macro condition",
                        name
                    )))
                }
            }
            _ => Err(lowering_error(format!(
                "Unsupported argument type for macro condition: {:?}",
                expr.node
            ))),
        }
    }
    
    /// Evaluate an expression AST to a boolean
    fn eval_bool_expr(&self, expr: &Expression) -> ParseResult<bool> {
        match &expr.node {
            ExpressionKind::Infix {
                left,
                operator,
                right,
            } => {
                let left_val = self.eval_int_expr(left)?;
                let right_val = self.eval_int_expr(right)?;
                
                match operator {
                    TokenType::LesserThan => Ok(left_val < right_val),
                    TokenType::GreaterThan => Ok(left_val > right_val),
                    TokenType::LesserEqual => Ok(left_val <= right_val),
                    TokenType::GreaterEqual => Ok(left_val >= right_val),
                    TokenType::Equal => Ok(left_val == right_val),
                    TokenType::NotEqual => Ok(left_val != right_val),
                    _ => Err(lowering_error(format!(
                        "Unsupported operator {:?} in macro condition",
                        operator
                    ))),
                }
            }
            _ => Err(lowering_error(format!(
                "Expected comparison expression in macro condition, got {:?}",
                expr.node
            ))),
        }
    }
    
    /// Evaluate an expression AST to an integer (helper for eval_bool_expr)
    fn eval_int_expr(&self, expr: &Expression) -> ParseResult<i32> {
        match &expr.node {
            ExpressionKind::Number(n) => Ok(*n),
            ExpressionKind::Identifier(_) => self.resolve_arg_to_int(expr),
            ExpressionKind::Infix {
                left,
                operator,
                right,
            } => {
                let left_val = self.eval_int_expr(left)?;
                let right_val = self.eval_int_expr(right)?;
                match operator {
                    TokenType::Plus => Ok(left_val + right_val),
                    TokenType::Minus => Ok(left_val - right_val),
                    TokenType::Mul => Ok(left_val * right_val),
                    // TokenType::Slash => Ok(left_val / right_val),
                    _ => Err(lowering_error(format!(
                        "Unsupported arithmetic operator {:?} in macro condition",
                        operator
                    ))),
                }
            }
            _ => Err(lowering_error(format!(
                "Cannot evaluate expression to integer: {:?}",
                expr.node
            ))),
        }
    }
    
    /// Format an argument expression as a string for macro substitution
    fn format_arg_for_substitution(&self, expr: &Expression) -> ParseResult<String> {
        match &expr.node {
            ExpressionKind::Number(n) => Ok(n.to_string()),
            ExpressionKind::Identifier(name) => {
                // Keep identifiers as-is - they'll be resolved when the expanded line is parsed
                Ok(name.clone())
            }
            ExpressionKind::Label(name) => Ok(name.clone()),
            ExpressionKind::Prefix { operator, id } => {
                let inner = self.format_arg_for_substitution(id)?;
                let op_str = match operator {
                    TokenType::Minus => "-",
                    _ => {
                        return Err(lowering_error(format!(
                            "Unsupported prefix operator {:?} in macro argument",
                            operator
                        )));
                    }
                };
                Ok(format!("{}{}", op_str, inner))
            }
            ExpressionKind::Infix {
                left,
                operator,
                right,
            } => {
                let left_str = self.format_arg_for_substitution(left)?;
                let right_str = self.format_arg_for_substitution(right)?;
                let op_str = match operator {
                    TokenType::Plus => "+",
                    TokenType::Minus => "-",
                    TokenType::Mul => "*",
                    _ => {
                        return Err(lowering_error(format!(
                            "Unsupported operator {:?} in macro argument",
                            operator
                        )));
                    }
                };
                Ok(format!("{} {} {}", left_str, op_str, right_str))
            }
            _ => Err(lowering_error(format!(
                "Unsupported expression type in macro argument: {:?}",
                expr.node
            ))),
        }
    }
    
    /// Substitute $paramName placeholders with actual values
    fn substitute_params(&self, line: &str, param_map: &HashMap<String, String>) -> String {
        let mut result = line.to_string();
        for (name, value) in param_map {
            result = result.replace(&format!("${}", name), value);
        }
        result
    }
    
    /// Parse a macro expansion line into a Statement
    fn parse_expansion_line(&self, line: &str) -> ParseResult<Statement> {
        if line.trim().is_empty() {
            // Empty line - skip
            return Err(lowering_error(format!(
                "Macro expansion produced empty line"
            )));
        }
        
        // Add newline to ensure proper parsing of commands without params
        let line_with_newline = format!("{}\n", line.trim());
        
        // Parse as a single statement
        let lexer = Lexer::new(&line_with_newline);
        let mut parser = Parser::new(lexer);
        
        parser.parse_statement().map_err(|e| {
            lowering_error(format!("Failed to parse macro expansion '{}': {}", line, e))
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::ast::{Expression, ExpressionKind};
    use crate::compiler::lexer::Lexer;
    use crate::compiler::parser::Parser;
    use crate::compiler::analysis::Analyzer;
    use crate::database::{DatabaseV2, ConstantDb};

    /// Helper function to create a test database
    fn create_test_db() -> DatabaseV2 {
        DatabaseV2::load(std::path::Path::new("src/db/platinum_v2.json"))
            .expect("Test database not found at src/db/platinum_v2.json - tests require the database file")
    }

    /// Helper function to parse and analyze a simple script
    fn parse_and_analyze(source: &str) -> (ScriptFile, SymbolTable) {
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();
        
        let mut constants = ConstantDb::new();
        let db = create_test_db();
        constants.load_from_db(&db);
        
        let mut analyzer = Analyzer::with_constants(&constants);
        analyzer.analyze(&script_file).unwrap();
        
        (script_file, analyzer.symbols)
    }

    #[test]
    fn test_lower_simple_function() {
        let source = r#"
function TestFunc #1:
    Message 1
    End
"#;
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, &db);
        
        let items = lowerer.lower_script_file(&script_file).unwrap();
        let functions: Vec<_> = items.iter().filter(|i| matches!(i, TopLevelItem::Function(_))).collect();
        
        assert_eq!(functions.len(), 1);
        match &functions[0] {
            TopLevelItem::Function(ir_func) => {
                assert_eq!(ir_func.name(), "TestFunc");
                assert_eq!(ir_func.headers.len(), 1);
                assert_eq!(ir_func.headers[0].id, Some(1));
                assert!(ir_func.is_public());
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_lower_simple_command() {
        let source = r#"
function TestFunc #1:
    Message 42
    End
"#;
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, &db);
        
        let items = lowerer.lower_script_file(&script_file).unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => {
                assert_eq!(ir_func.instructions.len(), 2); // Message + End
                match &ir_func.instructions[0] {
                    IrOpcode::Command { name, args } => {
                        assert_eq!(name, "Message");
                        assert_eq!(args.len(), 1);
                        assert_eq!(args[0].unwrap_value(), 42);
                    }
                    _ => panic!("Expected command"),
                }
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_lower_label() {
        let source = r#"
function TestFunc #1:
    Message 1
    Jump .skip
    Message 2
.skip:
    Message 3
    End
"#;
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, &db);
        
        let items = lowerer.lower_script_file(&script_file).unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => {
                // Should have: Message, Jump, Message, Label, Message, End
                assert!(ir_func.instructions.len() >= 5);
                
                // Check that we have a label
                let label_count = ir_func.instructions.iter()
                    .filter(|op| matches!(op, IrOpcode::Label(_)))
                    .count();
                assert_eq!(label_count, 1);
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_lower_action() {
        let source = r#"
action TestAction
    WalkNormalNorth 3
    EndMovement
"#;
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, &db);
        
        let items = lowerer.lower_script_file(&script_file).unwrap();
        let actions: Vec<_> = items.iter().filter(|i| matches!(i, TopLevelItem::Action(_))).collect();
        
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            TopLevelItem::Action(ir_action) => {
                assert_eq!(ir_action.name, "TestAction");
                assert_eq!(ir_action.instructions.len(), 2); // WalkNormalNorth + EndMovement
            }
            _ => panic!("Expected action"),
        }
    }

    #[test]
    fn test_lower_expression_number() {
        let source = "";
        let (_script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let lowerer = Lowerer::new(&symbols, &db);
        
        // Test direct value
        let expr = Expression {
            span: 0..1,
            node: ExpressionKind::Number(42),
        };
        let arg = lowerer.resolve_arg(&expr).unwrap();
        assert_eq!(arg.unwrap_value(), 42);
    }

    #[test]
    fn test_lower_expression_hex_number() {
        let source = "";
        let (_script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let lowerer = Lowerer::new(&symbols, &db);
        
        // Test hex value
        let expr = Expression {
            span: 0..1,
            node: ExpressionKind::Number(0x42),
        };
        let arg = lowerer.resolve_arg(&expr).unwrap();
        assert_eq!(arg.unwrap_value(), 0x42);
    }

    #[test]
    fn test_lower_if_statement() {
        let source = r#"
function TestFunc #1:
    if 0x8000 == 1 then
        Message 1
    endif
    End
"#;
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, &db);
        
        let items = lowerer.lower_script_file(&script_file).unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => {
                // Should generate: CompareVarValue, GoToIf, Message, Label
                assert!(ir_func.instructions.len() >= 4);
                
                // Check that we have conditional logic
                let has_compare = ir_func.instructions.iter().any(|op| {
                    matches!(op, IrOpcode::Command { name, .. } 
                        if name == "CompareVarValue" || name == "CompareVars")
                });
                let has_jump_if = ir_func.instructions.iter().any(|op| {
                    matches!(op, IrOpcode::Command { name, .. } if name == "GoToIf")
                });
                
                assert!(has_compare, "Should have CompareVarValue instruction");
                assert!(has_jump_if, "Should have GoToIf instruction");
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_lower_while_loop() {
        let source = r#"
function TestFunc #1:
    while 0x8000 != 0 do
        SubVar 0x8000, 1
    endwhile
    End
"#;
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, &db);
        
        let items = lowerer.lower_script_file(&script_file).unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => {
                // Should generate loop logic with labels and jumps
                assert!(ir_func.instructions.len() >= 4);
                
                // Check for loop constructs
                let has_compare = ir_func.instructions.iter().any(|op| {
                    matches!(op, IrOpcode::Command { name, .. } 
                        if name == "CompareVarValue" || name == "CompareVars")
                });
                let has_jump_if = ir_func.instructions.iter().any(|op| {
                    matches!(op, IrOpcode::Command { name, .. } if name == "GoToIf")
                });
                
                assert!(has_compare, "Should have CompareVarValue instruction");
                assert!(has_jump_if, "Should have GoToIf instruction");
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_lower_if_else() {
        let source = r#"
function TestFunc #1:
    if 0x8000 == 1 then
        Message 1
    else
        Message 2
    endif
    End
"#;
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, &db);
        
        let items = lowerer.lower_script_file(&script_file).unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => {
                // Should generate: Compare, GoToIf, Message1, GoTo, Label(else), Message2, Label(end), End
                // At minimum: Compare, GoToIf, Message, GoTo, Label, Message, Label, End = 8
                assert!(ir_func.instructions.len() >= 7, 
                    "if/else should generate at least 7 instructions, got {}", 
                    ir_func.instructions.len());
                
                // Check for else branch: should have 2 labels (else and end) and a GoTo to skip else
                let label_count = ir_func.instructions.iter()
                    .filter(|op| matches!(op, IrOpcode::Label(_)))
                    .count();
                assert!(label_count >= 2, "if/else should generate at least 2 labels (else + end)");
                
                // Check for unconditional GoTo (to skip else block)
                let goto_count = ir_func.instructions.iter()
                    .filter(|op| matches!(op, IrOpcode::Command { name, .. } if name == "GoTo"))
                    .count();
                assert!(goto_count >= 1, "if/else should have GoTo to skip else block");
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_lower_condition_operand_swap() {
        // When comparing VALUE == VAR, it should be swapped to VAR == VALUE
        // because the game's CompareVarValue expects (var, value) order
        let source = r#"
function TestFunc #1:
    if 5 == 0x8000 then
        Message 1
    endif
    End
"#;
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, &db);
        
        let items = lowerer.lower_script_file(&script_file).unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => {
                // Find the CompareVarValue instruction
                let compare = ir_func.instructions.iter().find(|op| {
                    matches!(op, IrOpcode::Command { name, .. } if name == "CompareVarValue")
                });
                
                assert!(compare.is_some(), "Should have CompareVarValue instruction");
                
                if let Some(IrOpcode::Command { args, .. }) = compare {
                    // First arg should be the variable (0x8000 = 32768)
                    assert_eq!(args[0].unwrap_value(), 0x8000, 
                        "First arg should be the variable (swapped from RHS)");
                    // Second arg should be the value (5)
                    assert_eq!(args[1].unwrap_value(), 5, 
                        "Second arg should be the value (swapped from LHS)");
                }
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_lower_arithmetic_expression() {
        // Test that compile-time arithmetic expressions are evaluated
        // Use Message command since it's simple and doesn't involve macros
        let source = r#"
function TestFunc #1:
    Message 1 + 2 * 3
    End
"#;
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, &db);
        
        let items = lowerer.lower_script_file(&script_file).unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => {
                // Find the Message command
                let message = ir_func.instructions.iter().find(|op| {
                    matches!(op, IrOpcode::Command { name, .. } if name == "Message")
                });
                
                assert!(message.is_some(), "Should have Message instruction");
                
                if let Some(IrOpcode::Command { args, .. }) = message {
                    // The value should be compile-time evaluated: 1 + 2 * 3 = 7 (correct precedence) or 9 (left-to-right)
                    let value = args[0].unwrap_value();
                    assert!(value == 7 || value == 9, 
                        "Arithmetic should be evaluated at compile time, got {}", value);
                }
            }
            _ => panic!("Expected function"),
        }
    }
}
