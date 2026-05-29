//! AST to IR lowering
//!
//! The Lowerer transforms parsed AST into IR opcodes, handling:
//! - Control flow (if/else, while) → `CompareVarValue` + `JumpIf`
//! - Macro expansion with parameter substitution
//! - Default parameter application
//! - Symbol resolution (aliases, constants, labels)
//! - Autovar commands in conditions (commands with destVar that default to `VAR_RESULT`)

use std::collections::HashMap;
use std::sync::Arc;

use crate::autovar::{VAR_RESULT, autovar_param_index};
use crate::compiler::analysis::{SymbolTable, SymbolType};
use crate::compiler::ast::{Expression, ExpressionKind, ScriptFile, Statement, StatementKind};
use crate::compiler::diagnostic::{ParseResult, lowering_error};
use crate::compiler::token::TokenType;
use crate::compiler::{Lexer, Parser};
use crate::database::{Command, ComparisonOperator, DatabaseV2, ParamDef, ResolvedCommandShape};

use super::{Arg, IrAction, IrFunction, IrOpcode, OperandType, TopLevelItem};

/// Maximum depth for macro expansion to prevent infinite recursion
const MAX_MACRO_DEPTH: usize = 10;

#[derive(Clone)]
pub struct Lowerer<'a> {
    label_counter: usize,
    output: Vec<IrOpcode>,
    global_symbols: &'a SymbolTable,
    active_aliases: HashMap<String, i32>,
    db: &'a DatabaseV2,
    constants: Option<&'a crate::database::ConstantDb>,
    break_targets: Vec<String>,
    workspace: Arc<uxie::Workspace>,
    /// File stem of the script being compiled, used to resolve the text archive
    /// for string literal arguments (e.g. `"0003"` for DSPRE, `"acuity_cavern"` for decomp).
    source_stem: String,
}

impl<'a> Lowerer<'a> {
    fn empty_workspace() -> Arc<uxie::Workspace> {
        static EMPTY: std::sync::OnceLock<Arc<uxie::Workspace>> = std::sync::OnceLock::new();
        Arc::clone(EMPTY.get_or_init(|| {
            Arc::new(uxie::Workspace::new(
                std::path::PathBuf::new(),
                uxie::game::Game::Platinum,
            ))
        }))
    }

    pub fn new(symbols: &'a SymbolTable, db: &'a DatabaseV2) -> Self {
        Self {
            label_counter: 0,
            output: Vec::new(),
            global_symbols: symbols,
            active_aliases: HashMap::new(),
            db,
            constants: None,
            break_targets: Vec::new(),
            workspace: Self::empty_workspace(),
            source_stem: String::new(),
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
            active_aliases: HashMap::new(),
            db,
            constants: Some(constants),
            break_targets: Vec::new(),
            workspace: Self::empty_workspace(),
            source_stem: String::new(),
        }
    }

    /// Construct a lowerer for a specific project file with full workspace context.
    ///
    /// `source_stem` is the file stem of the script being compiled (e.g. `"0003"`
    /// for DSPRE, `"acuity_cavern"` for decomp). It is used to resolve which text
    /// archive string literal arguments should be written to.
    pub fn for_file(
        symbols: &'a SymbolTable,
        db: &'a DatabaseV2,
        constants: &'a crate::database::ConstantDb,
        workspace: Arc<uxie::Workspace>,
        source_stem: String,
    ) -> Self {
        Self {
            label_counter: 0,
            output: Vec::new(),
            global_symbols: symbols,
            active_aliases: HashMap::new(),
            db,
            constants: Some(constants),
            break_targets: Vec::new(),
            workspace,
            source_stem,
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
                StatementKind::AliasStatement { .. } => {
                    self.lower_statement(item)?;
                }
                StatementKind::Function { headers, body } => {
                    let instructions = self.lower_function(body)?;
                    items.push(TopLevelItem::Function(IrFunction {
                        headers: headers.clone(),
                        instructions,
                    }));
                }
                StatementKind::Action { name, body } => {
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
                let label_else = elseblock.as_ref().map(|_| self.new_label("else"));
                let jump_target = label_else.as_ref().unwrap_or(&label_end);

                self.lower_condition(condition, jump_target)?;
                for s in body {
                    self.lower_statement_with_depth(s, macro_depth)?;
                }

                if let (Some(else_b), Some(label_else)) = (elseblock, label_else) {
                    self.output.push(IrOpcode::Command {
                        name: "Jump".to_string(),
                        args: vec![Arg::Pointer(label_end.clone())],
                    });
                    self.output.push(IrOpcode::Label(label_else));
                    for s in else_b {
                        self.lower_statement_with_depth(s, macro_depth)?;
                    }
                }

                self.output.push(IrOpcode::Label(label_end));
            }
            StatementKind::WhileStatement { condition, body } => {
                let label_start = self.new_label("while_start");
                let label_end = self.new_label("while_end");
                self.break_targets.push(label_end.clone());
                self.output.push(IrOpcode::Label(label_start.clone()));
                self.lower_condition(condition, &label_end)?;
                for s in body {
                    self.lower_statement_with_depth(s, macro_depth)?;
                }
                self.output.push(IrOpcode::Command {
                    name: "Jump".to_string(),
                    args: vec![Arg::Pointer(label_start)],
                });
                self.output.push(IrOpcode::Label(label_end));
                self.break_targets.pop();
            }
            StatementKind::MatchStatement {
                subject,
                cases,
                default,
            } => {
                let effective_subject = self.lower_subject_to_effective_subject(subject)?;
                self.lower_match_with_per_case_optimization(
                    &effective_subject,
                    cases,
                    default.as_deref(),
                    macro_depth,
                )?;
            }
            StatementKind::Break => {
                let Some(target) = self.break_targets.last() else {
                    return Err(lowering_error(
                        "break statement outside of loop".to_string(),
                    ));
                };
                self.output.push(IrOpcode::Command {
                    name: "Jump".to_string(),
                    args: vec![Arg::Pointer(target.clone())],
                });
            }
            StatementKind::ScriptCommand { command, args } => {
                self.lower_command(command, args, macro_depth)?;
            }

            StatementKind::Label(name) => self.output.push(IrOpcode::Label(name.clone())),
            StatementKind::Jump(target) => {
                if let ExpressionKind::Label(name) | ExpressionKind::Identifier(name) = &target.node
                {
                    self.output.push(IrOpcode::Command {
                        name: "Jump".to_string(),
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
            StatementKind::AliasStatement { name, value, .. } => {
                let resolved = self.resolve_arg_to_int(value)?;
                self.active_aliases.insert(name.clone(), resolved);
            }

            _ => {}
        }
        Ok(())
    }

    fn lower_subject_to_effective_subject(
        &mut self,
        subject: &Expression,
    ) -> ParseResult<Expression> {
        if let Some((function, args)) = self.command_call_parts(subject) {
            self.lower_autovar_call(function, args)?;
            return Ok(Expression {
                node: ExpressionKind::Number(VAR_RESULT),
                span: subject.span.clone(),
            });
        }

        Ok(subject.clone())
    }

    fn can_optimize_case_to_gotoif(case: &crate::compiler::ast::MatchCase) -> Option<String> {
        if let [_value] = case.values.as_slice()
            && let [stmt] = case.body.as_slice()
        {
            match &stmt.node {
                StatementKind::ScriptCommand { command, args } => {
                    if command == "Call" {
                        if let Some(first_arg) = args.first() {
                            if let ExpressionKind::Identifier(name) | ExpressionKind::Label(name) =
                                &first_arg.node
                            {
                                Some(name.clone())
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
                StatementKind::Jump(target) => {
                    if let ExpressionKind::Identifier(name) | ExpressionKind::Label(name) =
                        &target.node
                    {
                        Some(name.clone())
                    } else {
                        None
                    }
                }
                _ => None,
            }
        } else {
            None
        }
    }

    fn lower_match_with_per_case_optimization(
        &mut self,
        effective_subject: &Expression,
        cases: &[crate::compiler::ast::MatchCase],
        default: Option<&[Statement]>,
        macro_depth: usize,
    ) -> ParseResult<()> {
        let subject_val = self.resolve_arg(effective_subject)?.value("match subject")?;
        let label_end = self.new_label("end_match");
        let mut need_end_label = false;

        for (i, case) in cases.iter().enumerate() {
            if let Some(call_target) = Self::can_optimize_case_to_gotoif(case) {
                let case_value = self.resolve_arg(&case.values[0])?.value("match case value")?;
                self.output.push(IrOpcode::Command {
                    name: "CompareVarValue".to_string(),
                    args: vec![Arg::Value(subject_val), Arg::Value(case_value)],
                });
                self.output.push(IrOpcode::Command {
                    name: "JumpIf".to_string(),
                    args: vec![
                        Arg::Value(ComparisonOperator::Equal as i32),
                        Arg::Pointer(call_target),
                    ],
                });
            } else {
                need_end_label = true;
                let label_next = if i + 1 < cases.len() {
                    self.new_label("match_next")
                } else if default.is_some() {
                    self.new_label("match_default")
                } else {
                    label_end.clone()
                };

                let label_body = self.new_label("match_case");

                for (vi, value) in case.values.iter().enumerate() {
                    let is_last_value = vi + 1 >= case.values.len();
                    let jump_target = if is_last_value {
                        &label_next
                    } else {
                        &label_body
                    };

                    let condition = Expression {
                        node: ExpressionKind::Infix {
                            left: Box::new(effective_subject.clone()),
                            operator: TokenType::Equal,
                            right: Box::new(value.clone()),
                        },
                        span: value.span.clone(),
                    };

                    if is_last_value {
                        self.lower_condition(&condition, jump_target)?;
                    } else {
                        self.lower_condition_non_inverted(&condition, jump_target)?;
                    }
                }

                self.output.push(IrOpcode::Label(label_body));
                for s in &case.body {
                    self.lower_statement_with_depth(s, macro_depth)?;
                }
                self.output.push(IrOpcode::Command {
                    name: "Jump".to_string(),
                    args: vec![Arg::Pointer(label_end.clone())],
                });

                if i + 1 < cases.len() || default.is_some() {
                    self.output.push(IrOpcode::Label(label_next));
                }
            }
        }

        if let Some(default_body) = default {
            need_end_label = true;
            for s in default_body {
                self.lower_statement_with_depth(s, macro_depth)?;
            }
        }

        if need_end_label {
            self.output.push(IrOpcode::Label(label_end));
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

            let materialized_args = self.materialize_command_args(command, cmd, args)?;
            let resolved_args =
                self.resolve_args_for_command(command, &cmd.params, &materialized_args)?;
            self.output.push(IrOpcode::Command {
                name: command.to_string(),
                args: resolved_args,
            });
        } else {
            let resolved_args = self.resolve_args(args)?;
            self.output.push(IrOpcode::Command {
                name: command.to_string(),
                args: resolved_args,
            });
        }
        Ok(())
    }

    /// Pick which param list this call should use before defaults or `emit_args` run.
    fn select_command_shape<'b>(
        &self,
        cmd: &'b Command,
        args: &[Expression],
    ) -> ResolvedCommandShape<'b> {
        let first_arg_u8 = args
            .first()
            .and_then(|arg| self.resolve_arg_to_int(arg).ok())
            .and_then(|value| u8::try_from(value).ok());

        cmd.resolve_source_call_shape(first_arg_u8, |condition, params| {
            self.evaluate_condition_with_arg_count(condition, args, params)
                .unwrap_or(false)
        })
    }

    /// Build the final arg list lowering will emit for a command.
    ///
    /// Order:
    /// 1. pick a call shape
    /// 2. apply defaults to that shape
    /// 3. if the shape defines `emit_args`, rewrite those args
    fn materialize_command_args(
        &self,
        command: &str,
        cmd: &Command,
        args: &[Expression],
    ) -> ParseResult<Vec<Expression>> {
        let shape = self.select_command_shape(cmd, args);
        let source_args = Self::apply_defaults_to_params(command, shape.params, args)?;
        Self::apply_emit_args(shape, &source_args)
    }

    fn apply_defaults_to_params(
        command: &str,
        params: &[ParamDef],
        args: &[Expression],
    ) -> ParseResult<Vec<Expression>> {
        let param_count = params.len();

        if args.len() > param_count {
            return Err(lowering_error(format!(
                "Command '{}' takes at most {} arguments, but got {}",
                command,
                param_count,
                args.len()
            )));
        }

        let mut required_indices: Vec<usize> = Vec::new();
        let mut optional_indices: Vec<usize> = Vec::new();

        for (i, p) in params.iter().enumerate() {
            if p.default.is_none() && !p.optional {
                required_indices.push(i);
            } else {
                optional_indices.push(i);
            }
        }

        let required_count = required_indices.len();

        if args.len() < required_count {
            return Err(lowering_error(format!(
                "Command '{}' requires {} arguments, but got {}",
                command,
                required_count,
                args.len()
            )));
        }

        if args.len() > param_count {
            return Err(lowering_error(format!(
                "Command '{}' takes at most {} arguments, but got {}",
                command,
                param_count,
                args.len()
            )));
        }

        let has_optional_before_required = optional_indices
            .iter()
            .any(|&opt_idx| required_indices.iter().any(|&req_idx| opt_idx < req_idx));

        let mut result: Vec<Option<Expression>> = vec![None; param_count];

        if has_optional_before_required && args.len() < param_count {
            for (arg_idx, &param_idx) in required_indices.iter().enumerate() {
                if arg_idx < args.len() {
                    result[param_idx] = Some(args[arg_idx].clone());
                }
            }

            let args_for_optionals = args.len().saturating_sub(required_count);
            for (opt_num, &param_idx) in optional_indices.iter().enumerate() {
                if opt_num < args_for_optionals {
                    let arg_idx = required_count + opt_num;
                    if arg_idx < args.len() {
                        result[param_idx] = Some(args[arg_idx].clone());
                    }
                }
            }
        } else {
            for (i, arg) in args.iter().enumerate() {
                result[i] = Some(arg.clone());
            }
        }

        for (i, param) in params.iter().enumerate() {
            if result[i].is_none() {
                if let Some(default_str) = &param.default {
                    let substituted =
                        Self::substitute_default_params_sparse(default_str, params, &result[..i])?;
                    let lexer = Lexer::new(&substituted);
                    let mut parser = Parser::new(lexer);
                    let expr = parser.parse_expression(crate::compiler::ast::Precedence::Lowest)?;
                    result[i] = Some(expr);
                } else if param.optional {
                    break;
                } else {
                    return Err(lowering_error(format!(
                        "Command '{}' missing required argument '{}' at position {}",
                        command, param.name, i
                    )));
                }
            }
        }

        Ok(result.into_iter().flatten().collect())
    }

    /// Rewrite the chosen args with `emit_args`.
    ///
    /// The expressions run after `$param` substitution on the already-defaulted args. If there is
    /// no rewrite, the args are used as-is.
    fn apply_emit_args(
        shape: ResolvedCommandShape<'_>,
        source_args: &[Expression],
    ) -> ParseResult<Vec<Expression>> {
        let Some(emit_args) = shape.emit_args else {
            return Ok(source_args.to_vec());
        };

        let mut param_map: HashMap<String, String> = HashMap::new();
        for (param, arg) in shape.params.iter().zip(source_args.iter()) {
            param_map.insert(
                param.name.clone(),
                arg.to_macro_arg_source().map_err(lowering_error)?,
            );
        }

        let mut rewritten = Vec::with_capacity(emit_args.len());
        for expr in emit_args {
            let substituted = Self::substitute_params(expr, &param_map);
            rewritten.push(Self::parse_expression_text(&substituted)?);
        }

        Ok(rewritten)
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
        let shape = self.select_command_shape(cmd, args);
        let params = shape.params;

        let args_with_defaults = Self::apply_defaults_to_params(macro_name, params, args)?;

        if args_with_defaults.len() > params.len() {
            return Err(lowering_error(format!(
                "Macro '{}' expects at most {} arguments, got {}",
                macro_name,
                params.len(),
                args_with_defaults.len()
            )));
        }

        let expansion = shape.macro_expansion.ok_or_else(|| {
            lowering_error(format!(
                "Macro '{}' has no expansion defined (and no matching variant)",
                macro_name
            ))
        })?;

        let mut param_map: HashMap<String, String> = HashMap::new();
        for (param, arg) in params.iter().zip(args_with_defaults.iter()) {
            let formatted = arg.to_macro_arg_source().map_err(lowering_error)?;
            param_map.insert(param.name.clone(), formatted);
        }

        for line in expansion {
            let substituted = Self::substitute_params(line, &param_map);
            let parsed_stmt = Self::parse_expansion_line(&substituted)?;
            self.lower_statement_with_depth(&parsed_stmt, depth + 1)?;
        }

        Ok(())
    }

    fn evaluate_condition_with_arg_count(
        &self,
        condition: &str,
        args: &[Expression],
        params: &[ParamDef],
    ) -> ParseResult<bool> {
        crate::compiler::macro_condition::evaluate_macro_variant_condition(
            condition,
            args,
            params,
            |expr| self.resolve_arg_to_int(expr).ok(),
            |name| match name {
                "VARS_START" => Some(0x4000),
                "SCRIPT_LOCAL_VARS_START" => Some(0x8000),
                "VARS_END" | "SCRIPT_LOCAL_VARS_END" => Some(0x800D),
                _ => {
                    if let Some(cond) = ComparisonOperator::parse(name) {
                        return Some(i64::from(cond as i32));
                    }
                    if let Some(&val) = self.active_aliases.get(name) {
                        return Some(i64::from(val));
                    }
                    if let Some(SymbolType::Constant(val)) = self.global_symbols.resolve(name) {
                        return Some(i64::from(*val));
                    }
                    if let Some(SymbolType::Variable(val)) = self.global_symbols.resolve(name) {
                        return Some(i64::from(*val));
                    }
                    if let Some(db) = self.constants
                        && let Some(val) = db.get(name)
                    {
                        return Some(i64::from(val));
                    }
                    None
                }
            },
        )
        .map_err(|_| {
            lowering_error(format!(
                "Failed to evaluate macro condition '{}'",
                condition
            ))
        })
    }

    fn resolve_arg_to_int(&self, expr: &Expression) -> ParseResult<i32> {
        match &expr.node {
            ExpressionKind::Number(n) => Ok(*n),
            ExpressionKind::Identifier(name) => match name.as_str() {
                "VARS_START" => Ok(0x4000),
                "SCRIPT_LOCAL_VARS_START" => Ok(0x8000),
                "VARS_END" | "SCRIPT_LOCAL_VARS_END" => Ok(0x800D),
                _ => {
                    if let Some(&val) = self.active_aliases.get(name) {
                        return Ok(val);
                    }
                    if let Some(SymbolType::Constant(val)) = self.global_symbols.resolve(name) {
                        return Ok(*val);
                    } else if let Some(SymbolType::Variable(val)) =
                        self.global_symbols.resolve(name)
                    {
                        return Ok(*val);
                    }

                    if let Some(db) = self.constants
                        && let Some(val) = db.get(name)
                    {
                        return Ok(val);
                    }

                    Err(lowering_error(format!(
                        "Could not resolve '{}' to an integer for macro condition",
                        name
                    )))
                }
            },
            ExpressionKind::Prefix { operator, id } => {
                let val = self.resolve_arg_to_int(id)?;
                match operator {
                    TokenType::Minus => Ok(-val),
                    TokenType::Plus => Ok(val),
                    _ => Err(lowering_error(format!(
                        "Unsupported prefix operator {:?} in macro condition",
                        operator
                    ))),
                }
            }
            ExpressionKind::Infix {
                left,
                operator,
                right,
            } => {
                let left = self.resolve_arg_to_int(left)?;
                let right = self.resolve_arg_to_int(right)?;
                match operator {
                    TokenType::Plus => left.checked_add(right).ok_or_else(|| {
                        lowering_error(format!(
                            "Integer overflow while evaluating macro expression: {} + {}",
                            left, right
                        ))
                    }),
                    TokenType::Minus => left.checked_sub(right).ok_or_else(|| {
                        lowering_error(format!(
                            "Integer overflow while evaluating macro expression: {} - {}",
                            left, right
                        ))
                    }),
                    TokenType::Mul => left.checked_mul(right).ok_or_else(|| {
                        lowering_error(format!(
                            "Integer overflow while evaluating macro expression: {} * {}",
                            left, right
                        ))
                    }),
                    _ => Err(lowering_error(format!(
                        "Unsupported infix operator {:?} in macro condition",
                        operator
                    ))),
                }
            }
            _ => Err(lowering_error(format!(
                "Unsupported argument type for macro condition: {:?}",
                expr.node
            ))),
        }
    }

    fn command_call_parts<'b>(
        &self,
        expr: &'b Expression,
    ) -> Option<(&'b Expression, &'b [Expression])> {
        match &expr.node {
            ExpressionKind::Call { function, args } if matches!(&function.node, ExpressionKind::Identifier(name) if self.db.commands.contains_key(name)) => {
                Some((function.as_ref(), args.as_slice()))
            }
            _ => None,
        }
    }

    fn substitute_params(line: &str, param_map: &HashMap<String, String>) -> String {
        let mut result = line.to_string();
        for (name, value) in param_map {
            result = result.replace(&format!("${}", name), value);
        }
        result
    }

    fn substitute_default_params(
        default_str: &str,
        params: &[ParamDef],
        resolved_args: &[Expression],
    ) -> ParseResult<String> {
        let sparse_args: Vec<Option<Expression>> =
            resolved_args.iter().cloned().map(Some).collect();
        Self::substitute_default_params_sparse(default_str, params, &sparse_args)
    }

    fn substitute_default_params_sparse(
        default_str: &str,
        params: &[ParamDef],
        resolved_args: &[Option<Expression>],
    ) -> ParseResult<String> {
        let mut result = default_str.to_string();

        for (i, param) in params.iter().enumerate() {
            let placeholder = format!("${}", param.name);
            if result.contains(&placeholder)
                && let Some(arg) = resolved_args.get(i).and_then(|arg| arg.as_ref())
            {
                let formatted = arg.to_macro_arg_source().map_err(lowering_error)?;
                result = result.replace(&placeholder, &formatted);
            }
        }

        Ok(result)
    }

    /// Parse one expression with the normal Rotom parser.
    ///
    /// Defaults and `emit_args` use this so they follow the same expression rules as source code.
    fn parse_expression_text(expr: &str) -> ParseResult<Expression> {
        let expr_with_newline = format!("{}\n", expr.trim());
        let lexer = Lexer::new(&expr_with_newline);
        let mut parser = Parser::new(lexer);
        parser.parse_expression(crate::compiler::ast::Precedence::Lowest)
    }

    fn parse_expansion_line(line: &str) -> ParseResult<Statement> {
        if line.trim().is_empty() {
            return Err(lowering_error(
                "Macro expansion produced empty line".to_string(),
            ));
        }

        let line_with_newline = format!("{}\n", line.trim());

        let lexer = Lexer::new(&line_with_newline);
        let mut parser = Parser::new(lexer);

        parser.parse_statement().map_err(|e| {
            lowering_error(format!("Failed to parse macro expansion '{}': {}", line, e))
        })
    }

    fn lower_condition(&mut self, expr: &Expression, target_label: &str) -> ParseResult<()> {
        self.lower_condition_inner(expr, target_label, true)
    }

    fn lower_condition_non_inverted(
        &mut self,
        expr: &Expression,
        target_label: &str,
    ) -> ParseResult<()> {
        self.lower_condition_inner(expr, target_label, false)
    }

    fn lower_condition_inner(
        &mut self,
        expr: &Expression,
        target_label: &str,
        invert: bool,
    ) -> ParseResult<()> {
        if let Some((function, args)) = self.command_call_parts(expr) {
            self.lower_autovar_call(function, args)?;
            self.output.push(IrOpcode::Command {
                name: "CompareVarValue".to_string(),
                args: vec![Arg::Value(VAR_RESULT), Arg::Value(1)],
            });
            let cond = if invert {
                ComparisonOperator::Different
            } else {
                ComparisonOperator::Equal
            };
            self.output.push(IrOpcode::Command {
                name: "JumpIf".to_string(),
                args: vec![
                    Arg::Value(cond as i32),
                    Arg::Pointer(target_label.to_string()),
                ],
            });
            return Ok(());
        }

        if let ExpressionKind::Infix {
            left,
            operator,
            right,
        } = &expr.node
        {
            let left_type = self.lower_operand_if_call(left)?;
            let right_type = self.lower_operand_if_call(right)?;

            let (left_type, left_val) = self.analyze_operand_after_call(left, left_type)?;
            let (right_type, right_val) = self.analyze_operand_after_call(right, right_type)?;
            let (final_left, final_right, swapped) = match (&left_type, &right_type) {
                (OperandType::Value, OperandType::Variable) => (right_val, left_val, true),
                _ => (left_val, right_val, false),
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
            let cond = if invert {
                Self::get_inverted_condition(operator, swapped)
            } else {
                Self::get_condition(operator, swapped)
            };
            self.output.push(IrOpcode::Command {
                name: "JumpIf".to_string(),
                args: vec![
                    Arg::Value(cond as i32),
                    Arg::Pointer(target_label.to_string()),
                ],
            });
            Ok(())
        } else {
            Err(lowering_error(format!(
                "Condition must be a comparison expression (e.g., 'x == 1') or an autovar command call, found {:?}.",
                expr.node
            )))
        }
    }

    fn lower_operand_if_call(&mut self, expr: &Expression) -> ParseResult<Option<i32>> {
        if let Some((function, args)) = self.command_call_parts(expr) {
            self.lower_autovar_call(function, args)?;
            Ok(Some(VAR_RESULT))
        } else {
            Ok(None)
        }
    }

    fn analyze_operand_after_call(
        &self,
        expr: &Expression,
        call_result: Option<i32>,
    ) -> ParseResult<(OperandType, i32)> {
        if let Some(var_result) = call_result {
            Ok((OperandType::Variable, var_result))
        } else {
            self.analyze_operand(expr)
        }
    }

    fn lower_autovar_call(
        &mut self,
        function: &Expression,
        args: &[Expression],
    ) -> ParseResult<()> {
        let ExpressionKind::Identifier(name) = &function.node else {
            return Err(lowering_error(
                "Autovar call must use a command name".to_string(),
            ));
        };

        let cmd = self.db.get_command(name)?;

        let autovar_index = autovar_param_index(cmd);

        let mut final_args: Vec<Expression> = args.to_vec();

        if let Some(idx) = autovar_index {
            while final_args.len() < idx {
                let param = &cmd.params[final_args.len()];
                if let Some(default_str) = &param.default {
                    let substituted =
                        Self::substitute_default_params(default_str, &cmd.params, &final_args)?;
                    let lexer = Lexer::new(&substituted);
                    let mut parser = Parser::new(lexer);
                    let expr = parser.parse_expression(crate::compiler::ast::Precedence::Lowest)?;
                    final_args.push(expr);
                } else {
                    return Err(lowering_error(format!(
                        "Command '{}' missing required argument '{}' at position {}",
                        name,
                        param.name,
                        final_args.len()
                    )));
                }
            }

            final_args.push(Expression {
                node: ExpressionKind::Number(VAR_RESULT),
                span: 0..0,
            });
        }

        let shape = self.select_command_shape(cmd, &final_args);
        let args_with_defaults = Self::apply_defaults_to_params(name, shape.params, &final_args)?;

        if cmd.is_macro() {
            let expansion = shape.macro_expansion.ok_or_else(|| {
                lowering_error(format!(
                    "Macro '{}' has no expansion defined (and no matching variant)",
                    name
                ))
            })?;

            let mut param_map: HashMap<String, String> = HashMap::new();
            for (param, arg) in shape.params.iter().zip(args_with_defaults.iter()) {
                let formatted = arg.to_macro_arg_source().map_err(lowering_error)?;
                param_map.insert(param.name.clone(), formatted);
            }

            for line in expansion {
                let substituted = Self::substitute_params(line, &param_map);
                let parsed_stmt = Self::parse_expansion_line(&substituted)?;
                self.lower_statement_with_depth(&parsed_stmt, 1)?;
            }
        } else {
            let resolved_args = self.resolve_args(&args_with_defaults)?;
            self.output.push(IrOpcode::Command {
                name: name.clone(),
                args: resolved_args,
            });
        }

        Ok(())
    }

    fn resolve_args(&self, args: &[Expression]) -> ParseResult<Vec<Arg>> {
        args.iter().map(|arg| self.resolve_arg(arg)).collect()
    }

    /// Resolve args, routing string literals for `text_slot` params into the
    /// correct text archive for the command (e.g. the menu archive for `AddMenuEntryImm`).
    fn resolve_args_for_command(
        &self,
        command: &str,
        params: &[crate::database::ParamDef],
        args: &[Expression],
    ) -> ParseResult<Vec<Arg>> {
        let archive_override = self.msg_archive_for_command(command);
        args.iter()
            .enumerate()
            .map(|(i, arg)| {
                let override_for_arg = if archive_override.is_some()
                    && params.get(i).is_some_and(|p| p.name == "text_slot")
                {
                    archive_override
                } else {
                    None
                };
                self.resolve_arg_with_archive(arg, override_for_arg)
            })
            .collect()
    }

    /// Return the fixed text archive ID for a command, or `None` to use the
    /// script file's default archive.
    fn msg_archive_for_command(&self, command: &str) -> Option<u16> {
        uxie::Workspace::menu_entry_id(command, self.workspace.family)
    }

    fn resolve_arg(&self, expr: &Expression) -> ParseResult<Arg> {
        self.resolve_arg_with_archive(expr, None)
    }

    #[allow(clippy::too_many_lines)]
    fn resolve_arg_with_archive(
        &self,
        expr: &Expression,
        archive_override: Option<u16>,
    ) -> ParseResult<Arg> {
        match &expr.node {
            ExpressionKind::Identifier(name) => {
                if let Some(&val) = self.active_aliases.get(name) {
                    return Ok(Arg::Value(val));
                }

                match self.global_symbols.resolve(name) {
                    Some(SymbolType::Variable(id) | SymbolType::Constant(id)) => {
                        return Ok(Arg::Value(*id));
                    }
                    Some(SymbolType::Function(_) | SymbolType::Label | SymbolType::Action) => {
                        return Ok(Arg::Pointer(name.clone()));
                    }
                    None => {}
                }

                if let Some(db) = self.constants
                    && let Some(val) = db.get(name)
                {
                    return Ok(Arg::Value(val));
                }

                if let Some(cond) = ComparisonOperator::parse(name) {
                    return Ok(Arg::Value(cond as i32));
                }

                Err(lowering_error(format!(
                    "Symbol '{}' could not be resolved (analysis should have caught this)",
                    name
                )))
            }
            ExpressionKind::Number(val) => Ok(Arg::Value(*val)),
            ExpressionKind::Label(name) => Ok(Arg::Pointer(name.clone())),
            ExpressionKind::String(segs) => {
                let flat = segs
                    .iter()
                    .map(|(s, _)| s.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                let archive_id = if let Some(id) = archive_override {
                    id
                } else {
                    self.workspace
                        .text_archive_for_script_file(&self.source_stem)
                        .ok_or_else(|| {
                            lowering_error(format!(
                                "cannot resolve text archive for '{}': string literals require \
                                     a project workspace with script-to-archive mapping",
                                self.source_stem
                            ))
                        })?
                };
                let index = self
                    .workspace
                    .find_or_add_message(archive_id, &flat)
                    .map_err(|e| {
                        lowering_error(format!(
                            "failed to write message to archive {archive_id}: {e}"
                        ))
                    })?;
                Ok(Arg::Value(i32::from(index)))
            }
            ExpressionKind::Infix {
                left,
                operator,
                right,
            } => {
                let left_val = self.resolve_arg(left)?.value("left arithmetic operand")?;
                let right_val = self.resolve_arg(right)?.value("right arithmetic operand")?;
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
                let val = self.resolve_arg(id)?.value("prefix operand")?;
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
            ExpressionKind::Call { function, args }
                if matches!(&function.node, ExpressionKind::Identifier(n) if n == "format")
                    && args.len() == 1
                    && matches!(&args[0].node, ExpressionKind::String(_)) =>
            {
                let ExpressionKind::String(segs) = &args[0].node else {
                    return Err(lowering_error("format() requires a string literal argument"));
                };
                let wrapped = uxie::format_message(
                    &segs
                        .iter()
                        .map(|(s, _)| s.as_str())
                        .collect::<Vec<_>>()
                        .join(" "),
                )
                .map_err(|e| lowering_error(format!("failed to format message: {e}")))?;
                let archive_id = if let Some(id) = archive_override {
                    id
                } else {
                    self.workspace
                        .text_archive_for_script_file(&self.source_stem)
                        .ok_or_else(|| {
                            lowering_error(format!(
                                "cannot resolve text archive for '{}': format() requires \
                                     a project workspace with script-to-archive mapping",
                                self.source_stem
                            ))
                        })?
                };
                let index = self
                    .workspace
                    .find_or_add_message(archive_id, &wrapped)
                    .map_err(|e| {
                        lowering_error(format!(
                            "failed to write message to archive {archive_id}: {e}"
                        ))
                    })?;
                Ok(Arg::Value(i32::from(index)))
            }
            ExpressionKind::Call { function, .. } => {
                if self.command_call_parts(expr).is_some() {
                    let ExpressionKind::Identifier(name) = &function.node else {
                        return Err(lowering_error(
                            "command call expression must use an identifier name",
                        ));
                    };
                    return Err(lowering_error(format!(
                        "Command '{}' cannot be used as a plain value here",
                        name
                    )));
                }

                let expr_text = expr.to_constant_eval_source().map_err(lowering_error)?;
                if let Some(constants) = self.constants
                    && let Some(value) = constants.evaluate_expression(&expr_text)
                {
                    return Ok(Arg::Value(value));
                }

                Err(lowering_error(format!(
                    "Could not resolve '{}' as a constant expression",
                    expr_text
                )))
            }
            ExpressionKind::Error => Err(lowering_error(
                "Invalid expression in argument resolution".to_string(),
            )),
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

    fn swap_operator(token: &TokenType) -> TokenType {
        match token {
            TokenType::GreaterThan => TokenType::LesserThan,
            TokenType::LesserThan => TokenType::GreaterThan,
            TokenType::GreaterEqual => TokenType::LesserEqual,
            TokenType::LesserEqual => TokenType::GreaterEqual,
            _ => token.clone(),
        }
    }

    fn get_condition(token: &TokenType, swapped: bool) -> ComparisonOperator {
        use ComparisonOperator::{Different, Equal, Greater, GreaterEqual, Less, LessEqual};

        let effective_op = if swapped {
            Self::swap_operator(token)
        } else {
            token.clone()
        };

        match effective_op {
            TokenType::NotEqual => Different,
            TokenType::GreaterThan => Greater,
            TokenType::LesserThan => Less,
            TokenType::GreaterEqual => GreaterEqual,
            TokenType::LesserEqual => LessEqual,
            _ => Equal,
        }
    }

    fn get_inverted_condition(token: &TokenType, swapped: bool) -> ComparisonOperator {
        use ComparisonOperator::{Different, Equal, Greater, GreaterEqual, Less, LessEqual};

        let effective_op = if swapped {
            Self::swap_operator(token)
        } else {
            token.clone()
        };

        match effective_op {
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
    use crate::database::{CommandType, ConstantDb, DatabaseMeta, DatabaseV2, ParamType, Variant};
    use std::collections::HashMap;

    fn create_test_db() -> &'static DatabaseV2 {
        DatabaseV2::test_platinum()
    }

    fn create_view_rankings_shape_db() -> DatabaseV2 {
        let mut commands = HashMap::new();
        commands.insert(
            "ViewRankings".to_string(),
            Command {
                cmd_type: CommandType::ScriptCmd,
                id: Some(378),
                legacy_name: None,
                description: None,
                params: vec![
                    ParamDef {
                        name: "packed_page".to_string(),
                        param_type: ParamType::U16,
                        const_value: None,
                        default: None,
                        optional: false,
                    },
                    ParamDef {
                        name: "record".to_string(),
                        param_type: ParamType::Var,
                        const_value: None,
                        default: None,
                        optional: false,
                    },
                ],
                variants: Some(vec![Variant {
                    params: vec![
                        ParamDef {
                            name: "scope".to_string(),
                            param_type: ParamType::U16,
                            const_value: None,
                            default: None,
                            optional: false,
                        },
                        ParamDef {
                            name: "page".to_string(),
                            param_type: ParamType::U16,
                            const_value: None,
                            default: None,
                            optional: false,
                        },
                        ParamDef {
                            name: "record".to_string(),
                            param_type: ParamType::Var,
                            const_value: None,
                            default: None,
                            optional: false,
                        },
                    ],
                    desc: Some("decomp source form".to_string()),
                    condition: Some("3 args".to_string()),
                    expansion: None,
                    emit_args: Some(vec![
                        "$scope * 3 + $page".to_string(),
                        "$record".to_string(),
                    ]),
                }]),
                expansion: None,
            },
        );

        DatabaseV2 {
            meta: DatabaseMeta {
                version: "test".to_string(),
                generated_at: None,
                generated_from: None,
            },
            commands,
            sounds: HashMap::new(),
            overworld_directions: HashMap::new(),
            special_overworlds: HashMap::new(),
            id_index: std::sync::OnceLock::new(),
        }
    }

    fn parse_and_analyze(source: &str) -> (ScriptFile, SymbolTable) {
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut constants = ConstantDb::new();
        let db = create_test_db();
        constants.load_from_db(db);

        let mut analyzer = Analyzer::with_database(&constants, db);
        analyzer.analyze(&script_file).unwrap();

        (script_file, analyzer.symbols)
    }

    fn parse_and_analyze_with_db(source: &str, db: &DatabaseV2) -> (ScriptFile, SymbolTable) {
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let constants = ConstantDb::new();
        let mut analyzer = Analyzer::with_database(&constants, db);
        analyzer.analyze(&script_file).unwrap();

        (script_file, analyzer.symbols)
    }

    #[test]
    fn test_lower_simple_function() {
        let source = r"
script TestFunc #1:
    Message 1
    End
";
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, db);

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
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_lower_simple_command() {
        let source = r"
script TestFunc #1:
    Message 42
    End
";
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, db);

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
                    IrOpcode::Label(_) => panic!("Expected command"),
                }
            }
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_lower_command_variant_emit_args_rewrites_to_canonical_args() {
        let source = r"
script TestFunc #1:
    ViewRankings 1, 2, 0x800C
    End
";
        let db = create_view_rankings_shape_db();
        let (script_file, symbols) = parse_and_analyze_with_db(source, &db);
        let mut lowerer = Lowerer::new(&symbols, &db);

        let items = lowerer.lower_script_file(&script_file).unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => match &ir_func.instructions[0] {
                IrOpcode::Command { name, args } => {
                    assert_eq!(name, "ViewRankings");
                    assert_eq!(args.len(), 2);
                    assert_eq!(args[0].unwrap_value(), 5);
                    assert_eq!(args[1].unwrap_value(), 0x800C);
                }
                IrOpcode::Label(_) => panic!("Expected command"),
            },
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_lower_command_variant_emit_args_preserves_canonical_shape() {
        let source = r"
script TestFunc #1:
    ViewRankings 5, 0x800C
    End
";
        let db = create_view_rankings_shape_db();
        let (script_file, symbols) = parse_and_analyze_with_db(source, &db);
        let mut lowerer = Lowerer::new(&symbols, &db);

        let items = lowerer.lower_script_file(&script_file).unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => match &ir_func.instructions[0] {
                IrOpcode::Command { name, args } => {
                    assert_eq!(name, "ViewRankings");
                    assert_eq!(args.len(), 2);
                    assert_eq!(args[0].unwrap_value(), 5);
                    assert_eq!(args[1].unwrap_value(), 0x800C);
                }
                IrOpcode::Label(_) => panic!("Expected command"),
            },
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_lower_label() {
        let source = r"
script TestFunc #1:
    Message 1
    Jump .skip
    Message 2
.skip:
    Message 3
    End
";
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, db);

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
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_lower_action() {
        let source = r"
action TestAction:
    WalkNormalNorth 3
    EndMovement
";
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, db);

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
            TopLevelItem::Function(_) => panic!("Expected action"),
        }
    }

    #[test]
    fn test_lower_expression_number() {
        let source = "";
        let (_script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let lowerer = Lowerer::new(&symbols, db);

        let expr = Expression {
            node: ExpressionKind::Number(42),
            span: 0..1,
        };
        let arg = lowerer.resolve_arg(&expr).unwrap();
        assert_eq!(arg.unwrap_value(), 42);
    }

    #[test]
    fn test_lower_expression_hex_number() {
        let source = "";
        let (_script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let lowerer = Lowerer::new(&symbols, db);

        let expr = Expression {
            node: ExpressionKind::Number(0x42),
            span: 0..1,
        };
        let arg = lowerer.resolve_arg(&expr).unwrap();
        assert_eq!(arg.unwrap_value(), 0x42);
    }

    #[test]
    fn test_lower_if_statement() {
        let source = r"
script TestFunc #1:
    if 0x8000 == 1 then
        Message 1
    endif
    End
";
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, db);

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
                    .any(|op| matches!(op, IrOpcode::Command { name, .. } if name == "JumpIf"));

                assert!(has_compare, "Should have CompareVarValue instruction");
                assert!(has_jump_if, "Should have JumpIf instruction");
            }
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_lower_while_loop() {
        let source = r"
script TestFunc #1:
    while 0x8000 != 0 do
        SubVar 0x8000, 1
    endwhile
    End
";
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, db);

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
                    .any(|op| matches!(op, IrOpcode::Command { name, .. } if name == "JumpIf"));

                assert!(has_compare, "Should have CompareVarValue instruction");
                assert!(has_jump_if, "Should have JumpIf instruction");
            }
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_lower_if_else() {
        let source = r"
script TestFunc #1:
    if 0x8000 == 1 then
        Message 1
    else
        Message 2
    endif
    End
";
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, db);

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
                    .filter(|op| matches!(op, IrOpcode::Command { name, .. } if name == "Jump"))
                    .count();
                assert!(
                    goto_count >= 1,
                    "if/else should have Jump to skip else block"
                );
            }
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_lower_condition_operand_swap() {
        let source = r"
script TestFunc #1:
    if 5 == 0x8000 then
        Message 1
    endif
    End
";
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, db);

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
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_lower_arithmetic_expression() {
        let source = r"
script TestFunc #1:
    Message 1 + 2 * 3
    End
";
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, db);

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
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_lower_match_statement() {
        let source = r"
script TestFunc #1:
    match 0x8000 with
        case 0:
            Message 1
        case 1, 2:
            Message 2
        else:
            Message 3
    endmatch
    End
";
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, db);

        let items = lowerer.lower_script_file(&script_file).unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => {
                let compare_count = ir_func
                    .instructions
                    .iter()
                    .filter(|op| {
                        matches!(op, IrOpcode::Command { name, .. } if name == "CompareVarValue")
                    })
                    .count();
                assert!(
                    compare_count >= 3,
                    "Should have at least 3 CompareVarValue for cases 0, 1, 2. Got {}",
                    compare_count
                );

                let goto_count = ir_func
                    .instructions
                    .iter()
                    .filter(|op| matches!(op, IrOpcode::Command { name, .. } if name == "Jump"))
                    .count();
                assert!(
                    goto_count >= 2,
                    "Should have Jump jumps to skip to end after each case. Got {}",
                    goto_count
                );

                let message_count = ir_func
                    .instructions
                    .iter()
                    .filter(|op| matches!(op, IrOpcode::Command { name, .. } if name == "Message"))
                    .count();
                assert_eq!(message_count, 3, "Should have 3 Message commands");
            }
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_lower_break_statement() {
        let source = r"
script TestFunc #1:
    while 0x8000 != 0 do
        if 0x8000 == 5 then
            break
        endif
        SubVar 0x8000, 1
    endwhile
    End
";
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, db);

        let items = lowerer.lower_script_file(&script_file).unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => {
                let labels: Vec<_> = ir_func
                    .instructions
                    .iter()
                    .filter_map(|op| {
                        if let IrOpcode::Label(name) = op {
                            Some(name.clone())
                        } else {
                            None
                        }
                    })
                    .collect();

                let while_end_label = labels
                    .iter()
                    .find(|l| l.contains("while_end"))
                    .expect("Should have while_end label");

                let goto_to_while_end = ir_func.instructions.iter().any(|op| {
                    matches!(op, IrOpcode::Command { name, args }
                    if name == "Jump" && args.iter().any(|a| {
                        matches!(a, Arg::Pointer(p) if p == while_end_label)
                    }))
                });

                assert!(
                    goto_to_while_end,
                    "Break should generate Jump to while_end label"
                );
            }
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_break_outside_loop_fails() {
        let source = r"
script TestFunc #1:
    break
    End
";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut constants = ConstantDb::new();
        let db = create_test_db();
        constants.load_from_db(db);

        let mut analyzer = Analyzer::with_database(&constants, db);
        let result = analyzer.analyze(&script_file);

        assert!(
            result.is_err(),
            "Break outside loop should fail during analysis"
        );
        let err = result.unwrap_err();
        assert!(
            format!("{:?}", err).contains("break statement can only be used inside a while loop"),
            "Error should mention break outside loop"
        );
    }

    #[test]
    fn test_lower_autovar_call_bare_condition() {
        let source = r"
script TestFunc #1:
    if CheckPlayerOnBike() then
        Message 1
    endif
    End
";
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, db);

        let items = lowerer.lower_script_file(&script_file).unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => {
                let has_check_bike = ir_func.instructions.iter().any(|op| {
                    matches!(op, IrOpcode::Command { name, .. } if name == "CheckPlayerOnBike")
                });
                assert!(has_check_bike, "Should emit CheckPlayerOnBike command");

                let has_compare = ir_func.instructions.iter().any(|op| {
                    matches!(op, IrOpcode::Command { name, args }
                        if name == "CompareVarValue"
                        && args.len() == 2
                        && matches!(&args[0], Arg::Value(0x800C))
                        && matches!(&args[1], Arg::Value(1)))
                });
                assert!(
                    has_compare,
                    "Should emit CompareVarValue with VAR_RESULT and 1"
                );
            }
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_lower_autovar_call_with_comparison() {
        let source = r"
script TestFunc #1:
    if ShowYesNoMenu() == 0 then
        Message 1
    endif
    End
";
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, db);

        let items = lowerer.lower_script_file(&script_file).unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => {
                let has_yesno = ir_func.instructions.iter().any(
                    |op| matches!(op, IrOpcode::Command { name, .. } if name == "ShowYesNoMenu"),
                );
                assert!(has_yesno, "Should emit ShowYesNoMenu command");

                let has_compare = ir_func.instructions.iter().any(|op| {
                    matches!(op, IrOpcode::Command { name, args }
                        if name == "CompareVarValue"
                        && args.len() == 2
                        && matches!(&args[0], Arg::Value(0x800C))
                        && matches!(&args[1], Arg::Value(0)))
                });
                assert!(
                    has_compare,
                    "Should emit CompareVarValue with VAR_RESULT and 0"
                );
            }
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_lower_autovar_in_match() {
        let source = r"
script TestFunc #1:
    match ShowYesNoMenu() with
        case 0:
            Message 1
        case 1:
            Message 2
    endmatch
    End
";
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, db);

        let items = lowerer.lower_script_file(&script_file).unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => {
                let has_yesno = ir_func.instructions.iter().any(
                    |op| matches!(op, IrOpcode::Command { name, .. } if name == "ShowYesNoMenu"),
                );
                assert!(has_yesno, "Should emit ShowYesNoMenu command");

                let compare_count = ir_func
                    .instructions
                    .iter()
                    .filter(|op| {
                        matches!(op, IrOpcode::Command { name, .. } if name == "CompareVarValue")
                    })
                    .count();
                assert!(
                    compare_count >= 2,
                    "Should have at least 2 CompareVarValue for cases 0 and 1. Got {}",
                    compare_count
                );
            }
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_lower_autovar_in_match_emits_once() {
        let source = r"
script TestFunc #1:
    match ShowYesNoMenu() with
        case 0:
            Message 1
        case 1:
            Message 2
        case 2:
            Message 3
    endmatch
    End
";
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, db);

        let items = lowerer.lower_script_file(&script_file).unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => {
                let yesno_count = ir_func
                    .instructions
                    .iter()
                    .filter(|op| {
                        matches!(op, IrOpcode::Command { name, .. } if name == "ShowYesNoMenu")
                    })
                    .count();
                assert_eq!(
                    yesno_count, 1,
                    "ShowYesNoMenu should be emitted exactly once, got {}",
                    yesno_count
                );

                let compare_count = ir_func
                    .instructions
                    .iter()
                    .filter(|op| {
                        matches!(op, IrOpcode::Command { name, .. } if name == "CompareVarValue")
                    })
                    .count();
                assert!(
                    compare_count >= 3,
                    "Should have at least 3 CompareVarValue for cases 0, 1, 2. Got {}",
                    compare_count
                );
            }
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_lower_autovar_call_with_args() {
        let source = r"
script TestFunc #1:
    if AddItem(1, 5) then
        Message 1
    endif
    End
";
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, db);

        let items = lowerer.lower_script_file(&script_file).unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => {
                let has_add_item = ir_func.instructions.iter().any(|op| {
                    matches!(op, IrOpcode::Command { name, args }
                        if name == "AddItem"
                        && args.len() == 3
                        && matches!(&args[0], Arg::Value(1))
                        && matches!(&args[1], Arg::Value(5))
                        && matches!(&args[2], Arg::Value(0x800C)))
                });
                assert!(
                    has_add_item,
                    "Should emit AddItem with item=1, amount=5, destVarID=VAR_RESULT"
                );

                let has_compare = ir_func.instructions.iter().any(|op| {
                    matches!(op, IrOpcode::Command { name, args }
                        if name == "CompareVarValue"
                        && matches!(&args[0], Arg::Value(0x800C))
                        && matches!(&args[1], Arg::Value(1)))
                });
                assert!(
                    has_compare,
                    "Should emit CompareVarValue with VAR_RESULT and 1"
                );
            }
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_lower_autovar_call_on_right_side() {
        let source = r"
script TestFunc #1:
    if 1 == CheckPlayerOnBike() then
        Message 1
    endif
    End
";
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, db);

        let items = lowerer.lower_script_file(&script_file).unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => {
                let has_check_bike = ir_func.instructions.iter().any(|op| {
                    matches!(op, IrOpcode::Command { name, .. } if name == "CheckPlayerOnBike")
                });
                assert!(has_check_bike, "Should emit CheckPlayerOnBike command");

                let has_compare = ir_func.instructions.iter().any(|op| {
                    matches!(op, IrOpcode::Command { name, args }
                        if name == "CompareVarValue"
                        && args.len() == 2
                        && matches!(&args[0], Arg::Value(0x800C)))
                });
                assert!(has_compare, "Should emit CompareVarValue with VAR_RESULT");
            }
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_defaults_can_reference_prior_defaulted_param() {
        let source = r"
alias 0x800C as VAR_RESULT

script TestFunc #1:
    ShowCurrentFloor 1, 2
    End
";
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, db);

        let items = lowerer.lower_script_file(&script_file).unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => {
                let has_show_current_floor = ir_func.instructions.iter().any(|op| {
                    matches!(op, IrOpcode::Command { name, args }
                        if name == "ShowCurrentFloor"
                        && args.len() == 4
                        && matches!(&args[0], Arg::Value(1))
                        && matches!(&args[1], Arg::Value(2))
                        && matches!(&args[2], Arg::Value(0x800C))
                        && matches!(&args[3], Arg::Value(0x800C)))
                });
                assert!(
                    has_show_current_floor,
                    "ShowCurrentFloor should default both destVarID params to VAR_RESULT"
                );
            }
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_lower_match_with_keyword() {
        let source = r"
script TestFunc #1:
    match 0x8000 with
        case 0:
            Message 1
        case 1:
            Message 2
    endmatch
    End
";
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, db);

        let items = lowerer.lower_script_file(&script_file).unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => {
                let compare_count = ir_func
                    .instructions
                    .iter()
                    .filter(|op| {
                        matches!(op, IrOpcode::Command { name, .. } if name == "CompareVarValue")
                    })
                    .count();
                assert!(
                    compare_count >= 2,
                    "Should have at least 2 CompareVarValue. Got {}",
                    compare_count
                );
            }
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_lower_optimized_match_single_calls() {
        let source = r"
script TestFunc #1:
    match 0x8000 with
        case 0:
            Call func_a
        case 1:
            Call func_b
        case 2:
            Call func_c
    endmatch
    End

func_a:
    Return

func_b:
    Return

func_c:
    Return
";
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, db);

        let items = lowerer.lower_script_file(&script_file).unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => {
                let compare_count = ir_func
                    .instructions
                    .iter()
                    .filter(|op| {
                        matches!(op, IrOpcode::Command { name, .. } if name == "CompareVarValue")
                    })
                    .count();
                assert_eq!(
                    compare_count, 3,
                    "Should have exactly 3 CompareVarValue for optimized match. Got {}",
                    compare_count
                );

                let gotoif_count = ir_func
                    .instructions
                    .iter()
                    .filter(|op| matches!(op, IrOpcode::Command { name, .. } if name == "JumpIf"))
                    .count();
                assert_eq!(
                    gotoif_count, 3,
                    "Should have exactly 3 JumpIf for optimized match. Got {}",
                    gotoif_count
                );

                let goto_count = ir_func
                    .instructions
                    .iter()
                    .filter(|op| matches!(op, IrOpcode::Command { name, .. } if name == "Jump"))
                    .count();
                assert_eq!(
                    goto_count, 0,
                    "Optimized match should have no Jump commands. Got {}",
                    goto_count
                );

                let label_count = ir_func
                    .instructions
                    .iter()
                    .filter(|op| matches!(op, IrOpcode::Label(name) if name.starts_with('.')))
                    .count();
                assert_eq!(
                    label_count, 0,
                    "Optimized match should have no generated labels. Got {}",
                    label_count
                );
            }
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_lower_optimized_match_single_jumps() {
        let source = r"
script TestFunc #1:
    match 0x8000 with
        case 0:
            Jump label_a
        case 1:
            Jump label_b
        case 2:
            Jump label_c
    endmatch
    End

label_a:
    Return

label_b:
    Return

label_c:
    Return
";
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, db);

        let items = lowerer.lower_script_file(&script_file).unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => {
                let compare_count = ir_func
                    .instructions
                    .iter()
                    .filter(|op| {
                        matches!(op, IrOpcode::Command { name, .. } if name == "CompareVarValue")
                    })
                    .count();
                assert_eq!(
                    compare_count, 3,
                    "Should have exactly 3 CompareVarValue for optimized jump match. Got {}",
                    compare_count
                );

                let gotoif_count = ir_func
                    .instructions
                    .iter()
                    .filter(|op| matches!(op, IrOpcode::Command { name, .. } if name == "JumpIf"))
                    .count();
                assert_eq!(
                    gotoif_count, 3,
                    "Should have exactly 3 JumpIf for optimized jump match. Got {}",
                    gotoif_count
                );

                let goto_count = ir_func
                    .instructions
                    .iter()
                    .filter(|op| matches!(op, IrOpcode::Command { name, .. } if name == "Jump"))
                    .count();
                assert_eq!(
                    goto_count, 0,
                    "Optimized jump match should have no Jump commands. Got {}",
                    goto_count
                );

                let label_count = ir_func
                    .instructions
                    .iter()
                    .filter(|op| matches!(op, IrOpcode::Label(name) if name.starts_with('.')))
                    .count();
                assert_eq!(
                    label_count, 0,
                    "Optimized jump match should have no generated labels. Got {}",
                    label_count
                );
            }
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_lower_mixed_match_per_case_optimization() {
        let source = r"
script TestFunc #1:
    match 0x8000 with
        case 0:
            Call func_a
        case 1:
            Message 1
            Message 2
        case 2:
            Call func_b
    endmatch
    End

func_a:
    Return

func_b:
    Return
";
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, db);

        let items = lowerer.lower_script_file(&script_file).unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => {
                let gotoif_count = ir_func
                    .instructions
                    .iter()
                    .filter(|op| matches!(op, IrOpcode::Command { name, .. } if name == "JumpIf"))
                    .count();
                assert_eq!(
                    gotoif_count, 3,
                    "Should have 2 optimized JumpIf + 1 standard JumpIf. Got {}",
                    gotoif_count
                );

                let goto_count = ir_func
                    .instructions
                    .iter()
                    .filter(|op| matches!(op, IrOpcode::Command { name, .. } if name == "Jump"))
                    .count();
                assert!(
                    goto_count >= 1,
                    "Non-optimized case should have at least 1 Jump. Got {}",
                    goto_count
                );

                let message_count = ir_func
                    .instructions
                    .iter()
                    .filter(|op| matches!(op, IrOpcode::Command { name, .. } if name == "Message"))
                    .count();
                assert_eq!(message_count, 2, "Should have 2 Message commands");
            }
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_lower_condition_identifier() {
        let source = r"
script TestFunc #1:
    CompareVarValue 0x8000, 5
    JumpIf EQUAL, some_label
    End
some_label:
    Return
";
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, db);

        let items = lowerer.lower_script_file(&script_file).unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => {
                let has_jumpif_equal = ir_func.instructions.iter().any(|op| {
                    matches!(op, IrOpcode::Command { name, args }
                        if name == "JumpIf"
                        && args.len() == 2
                        && matches!(&args[0], Arg::Value(1)))
                });
                assert!(
                    has_jumpif_equal,
                    "JumpIf should have condition value 1 (EQUAL)"
                );
            }
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_lower_all_condition_identifiers() {
        let source = r"
script TestFunc #1:
    CompareVarValue 0x8000, 5
    JumpIf LESS, label1
    JumpIf EQUAL, label2
    JumpIf GREATER, label3
    JumpIf LESS_EQUAL, label4
    JumpIf GREATER_EQUAL, label5
    JumpIf DIFFERENT, label6
    End
label1:
label2:
label3:
label4:
label5:
label6:
    Return
";
        let (script_file, symbols) = parse_and_analyze(source);
        let db = create_test_db();
        let mut lowerer = Lowerer::new(&symbols, db);

        let items = lowerer.lower_script_file(&script_file).unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => {
                let jumpif_conditions: Vec<i32> = ir_func
                    .instructions
                    .iter()
                    .filter_map(|op| {
                        if let IrOpcode::Command { name, args } = op
                            && name == "JumpIf"
                            && !args.is_empty()
                            && let Arg::Value(v) = &args[0]
                        {
                            return Some(*v);
                        }
                        None
                    })
                    .collect();

                assert_eq!(jumpif_conditions, vec![0, 1, 2, 3, 4, 5]);
            }
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_macro_condition_with_constants() {
        let db = create_test_db();
        let mut constants = crate::database::ConstantDb::new();
        constants.load_from_db(db);

        let source = r"
script Test #1:
    CompareVar 0x8000, 100
    End
";
        let (script_file, symbols) = parse_and_analyze(source);
        let mut lowerer = Lowerer::with_constants(&symbols, db, &constants);

        let items = lowerer.lower_script_file(&script_file).unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => {
                let first_cmd = ir_func.instructions.iter().find(|op| {
                    matches!(op, IrOpcode::Command { name, .. } if name == "CompareVarToValue")
                });
                assert!(
                    first_cmd.is_some(),
                    "CompareVar with value < VARS_START should expand to CompareVarToValue. Got: {:?}",
                    ir_func.instructions
                );
            }
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_macro_condition_with_var_reference() {
        let db = create_test_db();
        let mut constants = crate::database::ConstantDb::new();
        constants.load_from_db(db);

        let source = r"
script Test #1:
    CompareVar 0x8000, 0x8001
    End
";
        let (script_file, symbols) = parse_and_analyze(source);
        let mut lowerer = Lowerer::with_constants(&symbols, db, &constants);

        let items = lowerer.lower_script_file(&script_file).unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => {
                let first_cmd = ir_func.instructions.iter().find(
                    |op| matches!(op, IrOpcode::Command { name, .. } if name == "CompareVarToVar"),
                );
                assert!(
                    first_cmd.is_some(),
                    "CompareVar with value >= VARS_START should expand to CompareVarToVar. Got: {:?}",
                    ir_func.instructions
                );
            }
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_nested_macro_expansion_gotoifge() {
        let db = create_test_db();
        let mut constants = crate::database::ConstantDb::new();
        constants.load_from_db(db);

        let source = r"
script Test #1:
    GoToIfGe 0x8000, 100, TestLabel
TestLabel:
    End
";
        let (script_file, symbols) = parse_and_analyze(source);
        let mut lowerer = Lowerer::with_constants(&symbols, db, &constants);

        let result = lowerer.lower_script_file(&script_file);
        assert!(
            result.is_ok(),
            "GoToIfGe with literal value should compile successfully. Error: {:?}",
            result.err()
        );

        let items = result.unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => {
                let has_compare_to_value = ir_func.instructions.iter().any(
                    |op| matches!(op, IrOpcode::Command { name, .. } if name == "CompareVarToValue"),
                );
                assert!(
                    has_compare_to_value,
                    "GoToIfGe with value < VARS_START should expand through CompareVar to CompareVarToValue. Got: {:?}",
                    ir_func.instructions
                );
            }
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_nested_macro_expansion_gotoifinrange_with_incrementing_expression() {
        let db = create_test_db();
        let mut constants = crate::database::ConstantDb::new();
        constants.load_from_db(db);

        let source = r"
script Test #1:
    GoToIfInRange 0x8000, 80, 85, TestLabel
TestLabel:
    End
";
        let (script_file, symbols) = parse_and_analyze(source);
        let mut lowerer = Lowerer::with_constants(&symbols, db, &constants);

        let result = lowerer.lower_script_file(&script_file);
        assert!(
            result.is_ok(),
            "GoToIfInRange with recursive $lower + 1 expansion should compile successfully. Error: {:?}",
            result.err()
        );

        let items = result.unwrap();
        match &items[0] {
            TopLevelItem::Function(ir_func) => {
                let compare_values: Vec<i32> = ir_func
                    .instructions
                    .iter()
                    .filter_map(|op| match op {
                        IrOpcode::Command { name, args }
                            if name == "CompareVarToValue" && args.len() >= 2 =>
                        {
                            match &args[1] {
                                Arg::Value(v) => Some(*v),
                                Arg::Pointer(_) => None,
                            }
                        }
                        _ => None,
                    })
                    .collect();

                assert_eq!(
                    compare_values,
                    vec![80, 81, 82, 83, 84, 85],
                    "GoToIfInRange should evaluate incremented macro arguments at compile time"
                );
            }
            TopLevelItem::Action(_) => panic!("Expected script"),
        }
    }

    #[test]
    fn test_resolve_arg_to_int_supports_infix_macro_expressions() {
        let db = create_test_db();
        let constants = crate::database::ConstantDb::new();
        let (_, symbols) = parse_and_analyze(
            r"
script Test #1:
    End
",
        );
        let lowerer = Lowerer::with_constants(&symbols, db, &constants);

        let expr = Expression {
            node: ExpressionKind::Infix {
                left: Box::new(Expression {
                    node: ExpressionKind::Number(80),
                    span: 0..0,
                }),
                operator: TokenType::Plus,
                right: Box::new(Expression {
                    node: ExpressionKind::Number(1),
                    span: 0..0,
                }),
            },
            span: 0..0,
        };

        let resolved = lowerer.resolve_arg_to_int(&expr).unwrap();
        assert_eq!(resolved, 81);
    }

    #[test]
    fn test_nested_macro_with_named_constant() {
        use std::path::Path;

        let db = crate::database::DatabaseV2::test_platinum();
        let mut constants = crate::database::ConstantDb::new();
        constants.load_from_db(db);

        let decomp_root = Path::new("C:/dev/pokeplatinum");
        if !decomp_root.exists() {
            return;
        }
        let _ = constants.load_decomp_project(decomp_root);

        let required_constants = ["TRAINER_CARD_LEVEL_GOLD"];
        if required_constants
            .iter()
            .any(|name| constants.get(name).is_none())
        {
            return;
        }

        let source = r"
script Test #1:
    GoToIfGe 0x800C, TRAINER_CARD_LEVEL_GOLD, TestLabel
TestLabel:
    End
";
        let lexer = crate::compiler::Lexer::new(source);
        let mut parser = crate::compiler::Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = crate::compiler::Analyzer::with_database(&constants, db);
        analyzer.analyze(&script_file).unwrap();

        let mut lowerer = Lowerer::with_constants(&analyzer.symbols, db, &constants);

        let result = lowerer.lower_script_file(&script_file);
        assert!(
            result.is_ok(),
            "GoToIfGe with named constant should compile successfully. Error: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_nested_macro_with_var_constant() {
        use std::path::Path;

        let db = crate::database::DatabaseV2::test_platinum();
        let mut constants = crate::database::ConstantDb::new();
        constants.load_from_db(db);

        let decomp_root = Path::new("C:/dev/pokeplatinum");
        if !decomp_root.exists() {
            return;
        }
        let _ = constants.load_decomp_project(decomp_root);

        let required_constants = ["VAR_ELEVATOR_FLOORS_ABOVE"];
        if required_constants
            .iter()
            .any(|name| constants.get(name).is_none())
        {
            return;
        }

        // VAR_ELEVATOR_FLOORS_ABOVE = 16590 (0x40CE) which is >= VARS_START (0x4000)
        // So this should expand to CompareVarToVar
        let source = r"
script Test #1:
    GoToIfEq VAR_ELEVATOR_FLOORS_ABOVE, 3, TestLabel
TestLabel:
    End
";
        let lexer = crate::compiler::Lexer::new(source);
        let mut parser = crate::compiler::Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = crate::compiler::Analyzer::with_database(&constants, db);
        analyzer.analyze(&script_file).unwrap();

        let mut lowerer = Lowerer::with_constants(&analyzer.symbols, db, &constants);

        let result = lowerer.lower_script_file(&script_file);
        assert!(
            result.is_ok(),
            "GoToIfEq with VAR constant should compile successfully. Error: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_compile_scripts_common() {
        use std::path::Path;

        let db = crate::database::DatabaseV2::test_platinum();
        let mut constants = crate::database::ConstantDb::new();
        constants.load_from_db(db);

        let decomp_root = Path::new("C:/dev/pokeplatinum");
        if !decomp_root.exists() {
            return; // Skip if decomp not available
        }
        let _ = constants.load_decomp_project(decomp_root);

        let script_path = decomp_root.join("res/field/scripts/scripts_common.s");
        if !script_path.exists() {
            return;
        }

        let source = std::fs::read_to_string(&script_path).unwrap();
        let transpiled = crate::transpiler::decomp::transpile(&source, Some(db))
            .expect("decomp transpile should succeed");

        let lexer = crate::compiler::Lexer::new(&transpiled.source);
        let mut parser = crate::compiler::Parser::new(lexer);
        let script_file = match parser.parse_script_file() {
            Ok(f) => f,
            Err(e) => panic!("Parse error: {:?}", e),
        };

        let mut analyzer = crate::compiler::Analyzer::with_database(&constants, db);
        if let Err(e) = analyzer.analyze(&script_file) {
            panic!("Analysis error: {:?}", e);
        }

        let mut lowerer = Lowerer::with_constants(&analyzer.symbols, db, &constants);
        let result = lowerer.lower_script_file(&script_file);
        assert!(
            result.is_ok(),
            "scripts_common.s should compile successfully. Error: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_callif_macro_with_named_constant() {
        use std::path::Path;

        let db = crate::database::DatabaseV2::test_platinum();
        let mut constants = crate::database::ConstantDb::new();
        constants.load_from_db(db);

        let decomp_root = Path::new("C:/dev/pokeplatinum");
        if !decomp_root.exists() {
            return;
        }
        let _ = constants.load_decomp_project(decomp_root);

        let required_constants = ["VAR_RESULT", "TRAINER_CARD_LEVEL_GOLD"];
        if required_constants
            .iter()
            .any(|name| constants.get(name).is_none())
        {
            return;
        }

        // Test CallIfGe with TRAINER_CARD_LEVEL_GOLD (value 4, which is < VARS_START)
        let source = r"
script Test #1:
    CallIfGe VAR_RESULT, TRAINER_CARD_LEVEL_GOLD, TestLabel
    CallIfLt VAR_RESULT, TRAINER_CARD_LEVEL_GOLD, TestLabel
TestLabel:
    End
";
        let lexer = crate::compiler::Lexer::new(source);
        let mut parser = crate::compiler::Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = crate::compiler::Analyzer::with_database(&constants, db);
        analyzer.analyze(&script_file).unwrap();

        let mut lowerer = Lowerer::with_constants(&analyzer.symbols, db, &constants);

        let result = lowerer.lower_script_file(&script_file);
        assert!(
            result.is_ok(),
            "CallIfGe/CallIfLt with named constant should compile successfully. Error: {:?}",
            result.err()
        );
    }
}
