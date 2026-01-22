//! AST to IR lowering
//!
//! The Lowerer transforms parsed AST into IR opcodes, handling:
//! - Control flow (if/else, while) → `CompareVarValue` + `GoToIf`
//! - Macro expansion with parameter substitution
//! - Default parameter application
//! - Symbol resolution (aliases, constants, labels)

use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

use crate::compiler::analysis::{SymbolTable, SymbolType};
use crate::compiler::ast::{Expression, ExpressionKind, ScriptFile, Statement, StatementKind};
use crate::compiler::parse_error::{ParseResult, lowering_error};
use crate::compiler::token::TokenType;
use crate::compiler::{Lexer, Parser};
use crate::database::{Command, DatabaseV2, ParamDef};

use super::{Arg, Condition, IrAction, IrFunction, IrOpcode, OperandType, TopLevelItem};

/// Macro condition parameter substitution: matches \paramName
static RE_MACRO_PARAM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\(\w+)").unwrap());

/// Macro condition for argument count: matches "1 arg(s)", "2 args", "3 args", etc.
static RE_ARG_COUNT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+)\s+args?\(?s?\)?$").unwrap());

/// Maximum depth for macro expansion to prevent infinite recursion
const MAX_MACRO_DEPTH: usize = 10;

#[derive(Debug, Clone)]
pub struct Lowerer<'a> {
    label_counter: usize,
    output: Vec<IrOpcode>,
    global_symbols: &'a SymbolTable,
    local_aliases: HashMap<String, i32>,
    db: &'a DatabaseV2,
    constants: Option<&'a crate::database::ConstantDb>,
}

impl<'a> Lowerer<'a> {
    pub fn new(symbols: &'a SymbolTable, db: &'a DatabaseV2) -> Self {
        Self {
            label_counter: 0,
            output: Vec::new(),
            global_symbols: symbols,
            local_aliases: HashMap::new(),
            db,
            constants: None,
        }
    }

    pub fn with_constants(
        symbols: &'a SymbolTable,
        db: &'a DatabaseV2,
        constants: &'a crate::database::ConstantDb,
    ) -> Self {
        Self {
            label_counter: 0,
            output: Vec::new(),
            global_symbols: symbols,
            local_aliases: HashMap::new(),
            db,
            constants: Some(constants),
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
                    args: vec![Arg::Pointer(label_start)],
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
            StatementKind::AliasStatement { name, id, .. } => {
                self.local_aliases.insert(name.clone(), *id);
            }

            _ => {}
        }
        Ok(())
    }

    fn lower_command(
        &mut self,
        command: &str,
        args: &[Expression],
        macro_depth: usize,
    ) -> ParseResult<()> {
        if let Ok(cmd) = self.db.get_command(command) {
            if cmd.is_macro() {
                return self.expand_macro(command, args, macro_depth);
            }

            let args_with_defaults = self.apply_defaults(command, cmd, args)?;
            let resolved_args = self.resolve_args(&args_with_defaults)?;
            self.output.push(IrOpcode::Command {
                name: command.to_string(),
                args: resolved_args,
            });
            Ok(())
        } else {
            let resolved_args = self.resolve_args(args)?;
            self.output.push(IrOpcode::Command {
                name: command.to_string(),
                args: resolved_args,
            });
            Ok(())
        }
    }

    fn apply_defaults(
        &self,
        command: &str,
        cmd: &Command,
        args: &[Expression],
    ) -> ParseResult<Vec<Expression>> {
        let params: &[ParamDef] = if let Some(variants) = &cmd.variants {
            let mut matched: &[ParamDef] = &cmd.params;

            if let Some(first_arg) = args.first()
                && let Ok(mode) = self.resolve_arg_to_int(first_arg) {
                    let variant_params = cmd.get_variant_params(mode as u8);
                    if !variant_params.is_empty() {
                        matched = variant_params;
                    }
                }

            if std::ptr::eq(matched, &cmd.params as &[ParamDef]) {
                for variant in variants {
                    if let Some(condition) = &variant.condition {
                        if condition == "else" {
                            matched = if variant.params.is_empty() {
                                &cmd.params
                            } else {
                                &variant.params
                            };
                            break;
                        } else if matches!(self.evaluate_condition_with_arg_count(condition, args, &cmd.params), Ok(true))
                        {
                            matched = if variant.params.is_empty() {
                                &cmd.params
                            } else {
                                &variant.params
                            };
                            break;
                        }
                    }
                }
            }
            matched
        } else {
            &cmd.params
        };

        let param_count = params.len();

        if args.len() > param_count {
            return Err(lowering_error(format!(
                "Command '{}' takes at most {} arguments, but got {}",
                command,
                param_count,
                args.len()
            )));
        }

        let required_count = params
            .iter()
            .filter(|p| p.default.is_none() && !p.optional)
            .count();
        let first_optional_idx = params
            .iter()
            .position(|p| p.default.is_some() || p.optional)
            .unwrap_or(param_count);

        if args.len() < required_count {
            let min_required = first_optional_idx.min(required_count);
            if args.len() < min_required {
                return Err(lowering_error(format!(
                    "Command '{}' requires at least {} arguments, but got {}",
                    command,
                    min_required,
                    args.len()
                )));
            }
        }

        let mut result: Vec<Expression> = Vec::with_capacity(param_count);

        for (i, param) in params.iter().enumerate() {
            if i < args.len() {
                result.push(args[i].clone());
            } else if let Some(default_str) = &param.default {
                let substituted = self.substitute_default_params(default_str, params, &result)?;
                let lexer = Lexer::new(&substituted);
                let mut parser = Parser::new(lexer);
                let expr = parser.parse_expression(crate::compiler::ast::Precedence::Lowest)?;
                result.push(expr);
            } else if param.optional {
                break;
            } else {
                return Err(lowering_error(format!(
                    "Command '{}' missing required argument '{}' at position {}",
                    command, param.name, i
                )));
            }
        }

        Ok(result)
    }

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

        let args_with_defaults = self.apply_defaults(macro_name, cmd, args)?;

        if args_with_defaults.len() > params.len() {
            return Err(lowering_error(format!(
                "Macro '{}' expects at most {} arguments, got {}",
                macro_name,
                params.len(),
                args_with_defaults.len()
            )));
        }

        let expansion = if let Some(variants) = &cmd.variants {
            let mut matched_expansion = None;
            for variant in variants {
                if let Some(condition) = &variant.condition {
                    if condition == "else" {
                        matched_expansion = variant.expansion.as_ref();
                        break;
                    }

                    if self.evaluate_condition_with_arg_count(condition, args, params)? {
                        matched_expansion = variant.expansion.as_ref();
                        break;
                    }
                }
            }

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
        for (param, arg) in params.iter().zip(args_with_defaults.iter()) {
            let formatted = self.format_arg_for_substitution(arg)?;
            param_map.insert(param.name.clone(), formatted);
        }

        for line in expansion {
            let substituted = self.substitute_params(line, &param_map);
            let parsed_stmt = self.parse_expansion_line(&substituted)?;
            self.lower_statement_with_depth(&parsed_stmt, depth + 1)?;
        }

        Ok(())
    }

    fn evaluate_condition(
        &self,
        condition: &str,
        args: &[Expression],
        params: &[crate::database::ParamDef],
    ) -> ParseResult<bool> {
        let substituted = RE_MACRO_PARAM.replace_all(condition, |caps: &regex::Captures| {
            let param_name = &caps[1];
            if let Some(pos) = params.iter().position(|p| p.name == param_name) {
                if let Some(arg) = args.get(pos) {
                    match self.resolve_arg_to_int(arg) {
                        Ok(val) => val.to_string(),
                        Err(_) => "0".to_string(),
                    }
                } else {
                    "0".to_string()
                }
            } else {
                "0".to_string()
            }
        });

        let lexer = Lexer::new(&substituted);
        let mut parser = Parser::new(lexer);
        let expr = parser.parse_expression(crate::compiler::ast::Precedence::Lowest)?;

        self.eval_bool_expr(&expr)
    }

    fn evaluate_condition_with_arg_count(
        &self,
        condition: &str,
        args: &[Expression],
        params: &[crate::database::ParamDef],
    ) -> ParseResult<bool> {
        if let Some(caps) = RE_ARG_COUNT.captures(condition) {
            let expected_count: usize = caps[1].parse().unwrap_or(0);
            return Ok(args.len() == expected_count);
        }
        self.evaluate_condition(condition, args, params)
    }

    fn resolve_arg_to_int(&self, expr: &Expression) -> ParseResult<i32> {
        match &expr.node {
            ExpressionKind::Number(n) => Ok(*n),
            ExpressionKind::Identifier(name) => {
                if let Some(SymbolType::Constant(val)) = self.global_symbols.resolve(name) {
                    return Ok(*val);
                } else if let Some(SymbolType::Variable(val)) = self.global_symbols.resolve(name) {
                    return Ok(*val);
                }

                if let Some(db) = self.constants
                    && let Some(val) = db.get(name) {
                        return Ok(val);
                    }

                Err(lowering_error(format!(
                    "Could not resolve '{}' to an integer for macro condition",
                    name
                )))
            }
            _ => Err(lowering_error(format!(
                "Unsupported argument type for macro condition: {:?}",
                expr.node
            ))),
        }
    }

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

    fn eval_int_expr(&self, expr: &Expression) -> ParseResult<i32> {
        match &expr.node {
            ExpressionKind::Number(n) => Ok(*n),
            ExpressionKind::Identifier(_) => self.resolve_arg_to_int(expr),
            ExpressionKind::Prefix { operator, id } => {
                let val = self.eval_int_expr(id)?;
                match operator {
                    TokenType::Minus => Ok(-val),
                    _ => Err(lowering_error(format!(
                        "Unsupported prefix operator {:?} in expression",
                        operator
                    ))),
                }
            }
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

    fn format_arg_for_substitution(&self, expr: &Expression) -> ParseResult<String> {
        match &expr.node {
            ExpressionKind::Number(n) => Ok(n.to_string()),
            ExpressionKind::Identifier(name) => Ok(name.clone()),
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

    fn substitute_params(&self, line: &str, param_map: &HashMap<String, String>) -> String {
        let mut result = line.to_string();
        for (name, value) in param_map {
            result = result.replace(&format!("${}", name), value);
        }
        result
    }

    fn substitute_default_params(
        &self,
        default_str: &str,
        params: &[ParamDef],
        resolved_args: &[Expression],
    ) -> ParseResult<String> {
        let mut result = default_str.to_string();

        for (i, param) in params.iter().enumerate() {
            let placeholder = format!("${}", param.name);
            if result.contains(&placeholder)
                && i < resolved_args.len() {
                    let formatted = self.format_arg_for_substitution(&resolved_args[i])?;
                    result = result.replace(&placeholder, &formatted);
                }
        }

        Ok(result)
    }

    fn parse_expansion_line(&self, line: &str) -> ParseResult<Statement> {
        if line.trim().is_empty() {
            return Err(lowering_error("Macro expansion produced empty line".to_string()));
        }

        let line_with_newline = format!("{}\n", line.trim());

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
            let (final_left, final_right, swapped) = match (&left_type, &right_type) {
                (OperandType::Variable, OperandType::Value) => (left_val, right_val, false),
                (OperandType::Value, OperandType::Variable) => (right_val, left_val, true),
                (OperandType::Variable, OperandType::Variable) => (left_val, right_val, false),
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
            Ok(())
        } else {
            Err(lowering_error(format!(
                "Condition must be a comparison expression (e.g., 'x == 1'), found {:?}. Boolean expressions are not yet supported.",
                expr.node
            )))
        }
    }

    fn resolve_args(&self, args: &[Expression]) -> ParseResult<Vec<Arg>> {
        args.iter().map(|arg| self.resolve_arg(arg)).collect()
    }

    fn resolve_arg(&self, expr: &Expression) -> ParseResult<Arg> {
        match &expr.node {
            ExpressionKind::Identifier(name) => {
                if let Some(&val) = self.local_aliases.get(name) {
                    return Ok(Arg::Value(val));
                }

                match self.global_symbols.resolve(name) {
                    Some(SymbolType::Variable(id)) => return Ok(Arg::Value(*id)),
                    Some(SymbolType::Constant(id)) => return Ok(Arg::Value(*id)),
                    Some(SymbolType::Function(_) | SymbolType::Label | SymbolType::Action) => return Ok(Arg::Pointer(name.clone())),
                    None => {}
                }

                if let Some(db) = self.constants
                    && let Some(val) = db.get(name) {
                        return Ok(Arg::Value(val));
                    }

                Err(lowering_error(format!(
                    "Symbol '{}' could not be resolved (analysis should have caught this)",
                    name
                )))
            }
            ExpressionKind::Number(val) => Ok(Arg::Value(*val)),
            ExpressionKind::Label(name) => Ok(Arg::Pointer(name.clone())),
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

    fn get_inverted_condition(&self, token: &TokenType, swapped: bool) -> Condition {
        use Condition::{Different, Equal, LessEqual, GreaterEqual, Less, Greater};

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
            TokenType::Equal => Different,
            TokenType::NotEqual => Equal,
            TokenType::GreaterThan => LessEqual,
            TokenType::LesserThan => GreaterEqual,
            TokenType::GreaterEqual => Less,
            TokenType::LesserEqual => Greater,
            _ => Different,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::analysis::Analyzer;
    use crate::compiler::ast::{Expression, ExpressionKind};
    use crate::compiler::lexer::Lexer;
    use crate::compiler::parser::Parser;
    use crate::database::{ConstantDb, DatabaseV2};

    fn create_test_db() -> DatabaseV2 {
        DatabaseV2::load(std::path::Path::new("src/db/platinum_v2.json")).expect(
            "Test database not found at src/db/platinum_v2.json - tests require the database file",
        )
    }

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
        let functions: Vec<_> = items
            .iter()
            .filter(|i| matches!(i, TopLevelItem::Function(_)))
            .collect();

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
                assert_eq!(ir_func.instructions.len(), 2);
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
                assert!(ir_func.instructions.len() >= 5);

                let label_count = ir_func
                    .instructions
                    .iter()
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
        let actions: Vec<_> = items
            .iter()
            .filter(|i| matches!(i, TopLevelItem::Action(_)))
            .collect();

        assert_eq!(actions.len(), 1);
        match &actions[0] {
            TopLevelItem::Action(ir_action) => {
                assert_eq!(ir_action.name, "TestAction");
                assert_eq!(ir_action.instructions.len(), 2);
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
                assert!(ir_func.instructions.len() >= 4);

                let has_compare = ir_func.instructions.iter().any(|op| {
                    matches!(op, IrOpcode::Command { name, .. }
                        if name == "CompareVarValue" || name == "CompareVars")
                });
                let has_jump_if = ir_func
                    .instructions
                    .iter()
                    .any(|op| matches!(op, IrOpcode::Command { name, .. } if name == "GoToIf"));

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
                assert!(ir_func.instructions.len() >= 4);

                let has_compare = ir_func.instructions.iter().any(|op| {
                    matches!(op, IrOpcode::Command { name, .. }
                        if name == "CompareVarValue" || name == "CompareVars")
                });
                let has_jump_if = ir_func
                    .instructions
                    .iter()
                    .any(|op| matches!(op, IrOpcode::Command { name, .. } if name == "GoToIf"));

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
                assert!(
                    ir_func.instructions.len() >= 7,
                    "if/else should generate at least 7 instructions, got {}",
                    ir_func.instructions.len()
                );

                let label_count = ir_func
                    .instructions
                    .iter()
                    .filter(|op| matches!(op, IrOpcode::Label(_)))
                    .count();
                assert!(
                    label_count >= 2,
                    "if/else should generate at least 2 labels (else + end)"
                );

                let goto_count = ir_func
                    .instructions
                    .iter()
                    .filter(|op| matches!(op, IrOpcode::Command { name, .. } if name == "GoTo"))
                    .count();
                assert!(
                    goto_count >= 1,
                    "if/else should have GoTo to skip else block"
                );
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_lower_condition_operand_swap() {
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
                let compare = ir_func.instructions.iter().find(
                    |op| matches!(op, IrOpcode::Command { name, .. } if name == "CompareVarValue"),
                );

                assert!(compare.is_some(), "Should have CompareVarValue instruction");

                if let Some(IrOpcode::Command { args, .. }) = compare {
                    assert_eq!(
                        args[0].unwrap_value(),
                        0x8000,
                        "First arg should be the variable (swapped from RHS)"
                    );
                    assert_eq!(
                        args[1].unwrap_value(),
                        5,
                        "Second arg should be the value (swapped from LHS)"
                    );
                }
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_lower_arithmetic_expression() {
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
                let message = ir_func
                    .instructions
                    .iter()
                    .find(|op| matches!(op, IrOpcode::Command { name, .. } if name == "Message"));

                assert!(message.is_some(), "Should have Message instruction");

                if let Some(IrOpcode::Command { args, .. }) = message {
                    let value = args[0].unwrap_value();
                    assert!(
                        value == 7 || value == 9,
                        "Arithmetic should be evaluated at compile time, got {}",
                        value
                    );
                }
            }
            _ => panic!("Expected function"),
        }
    }
}
