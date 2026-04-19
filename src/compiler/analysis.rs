use dashmap::DashMap;
use regex::Regex;
use std::{collections::HashMap, ops::Range, sync::LazyLock};
use uxie::c_parser::defines::eval_expr_with_parent;

use crate::database::{
    Command, ComparisonOperator, ConstantDb, DatabaseV2, ParamDef, ParamType, ResolvedCommandShape,
};

use super::{
    ast::{Expression, ExpressionKind, ScriptFile, Statement, StatementKind},
    parse_error::{CompileError, ParseResult, analysis_error},
};

/// Macro/variant arg-count condition matcher: `1 arg`, `2 args`, `3 arg(s)`, etc.
static RE_ARG_COUNT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\d+)\s+args?\(?s?\)?$").expect("static regex pattern is valid")
});

#[derive(Debug, Clone)]
pub enum SymbolType {
    Function(Option<u32>),
    Action,
    Label,
    Variable(i32),
    Constant(i32),
}

#[derive(Debug, Clone)]
pub struct SymbolTable {
    scopes: Vec<HashMap<String, (SymbolType, Range<usize>)>>,
}

impl Default for SymbolTable {
    fn default() -> Self {
        Self::new()
    }
}

impl SymbolTable {
    pub fn new() -> SymbolTable {
        SymbolTable {
            scopes: vec![HashMap::new()],
        }
    }
    pub fn enter_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }
    pub fn exit_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }
    pub fn define_global(
        &mut self,
        name: String,
        kind: SymbolType,
        span: Range<usize>,
    ) -> ParseResult<()> {
        if let Some((existing_kind, original_span)) = self.scopes[0].get(&name) {
            // Allow user definitions to shadow database constants
            if matches!(existing_kind, SymbolType::Constant(_)) {
                // Overwrite the constant with the user's definition
                self.scopes[0].insert(name, (kind, span));
                return Ok(());
            }
            return Err(analysis_error(
                span,
                format!(
                    "Symbol '{}' is already defined globally. (Previous definition at {:?})",
                    name, original_span
                ),
            ));
        }
        self.scopes[0].insert(name, (kind, span));
        Ok(())
    }
    pub fn define_alias(
        &mut self,
        name: String,
        value: i32,
        span: Range<usize>,
    ) -> ParseResult<()> {
        if matches!(
            self.resolve(&name),
            Some(SymbolType::Function(_) | SymbolType::Action | SymbolType::Label)
        ) {
            return Err(analysis_error(
                span,
                format!(
                    "Alias '{}' conflicts with an existing non-alias symbol",
                    name
                ),
            ));
        }

        self.scopes[0].insert(name, (SymbolType::Variable(value), span));
        Ok(())
    }
    pub fn define_scoped(
        &mut self,
        name: String,
        kind: SymbolType,
        span: Range<usize>,
    ) -> Result<(), CompileError> {
        let current_scope = self.scopes.last_mut().expect(
            "scopes is never empty; initialized with one element and protected by exit_scope guard",
        );

        if let Some((_, original_span)) = current_scope.get(&name) {
            return Err(analysis_error(
                span,
                format!(
                    "Symbol '{}' is already defined in this scope. (Previous definition at {:?})",
                    name, original_span
                ),
            ));
        }

        current_scope.insert(name, (kind, span));
        Ok(())
    }
    pub fn resolve(&self, name: &str) -> Option<&SymbolType> {
        for scope in self.scopes.iter().rev() {
            if let Some((kind, _)) = scope.get(name) {
                return Some(kind);
            }
        }
        None
    }
}

pub struct Analyzer<'a> {
    pub symbols: SymbolTable,
    constants: Option<&'a ConstantDb>,
    database: Option<&'a DatabaseV2>,
    loop_depth: u32,
}

impl Default for Analyzer<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> Analyzer<'a> {
    pub fn new() -> Analyzer<'a> {
        Analyzer {
            symbols: SymbolTable::new(),
            constants: None,
            database: None,
            loop_depth: 0,
        }
    }

    /// Create an analyzer with a constants database
    pub fn with_constants(constants: &'a ConstantDb) -> Analyzer<'a> {
        Analyzer {
            symbols: SymbolTable::new(),
            constants: Some(constants),
            database: None,
            loop_depth: 0,
        }
    }

    /// Create an analyzer with both constants and command database for full validation
    pub fn with_database(constants: &'a ConstantDb, database: &'a DatabaseV2) -> Analyzer<'a> {
        Analyzer {
            symbols: SymbolTable::new(),
            constants: Some(constants),
            database: Some(database),
            loop_depth: 0,
        }
    }

    pub fn analyze(&mut self, file: &ScriptFile) -> ParseResult<()> {
        for item in &file.items {
            match &item.node {
                StatementKind::Function { .. } => {
                    self.register_function_names(item)?;
                }
                StatementKind::Action { .. } => {
                    self.register_action_name(item)?;
                }
                _ => {}
            }
        }
        for item in &file.items {
            match &item.node {
                StatementKind::AliasStatement { .. } => {
                    self.register_alias(item)?;
                }
                StatementKind::Function { .. } => {
                    self.validate_function_body(item)?;
                }
                StatementKind::Action { .. } => {
                    self.validate_action_body(item)?;
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn register_alias(&mut self, stmt: &Statement) -> ParseResult<()> {
        if let StatementKind::AliasStatement { name, value, .. } = &stmt.node {
            let id = self.resolve_expression_to_int(value)?;
            self.symbols
                .define_alias(name.clone(), id, stmt.span.clone())?;
        }
        Ok(())
    }
    fn register_function_names(&mut self, func: &Statement) -> ParseResult<()> {
        if let StatementKind::Function { headers, .. } = &func.node {
            // Track which function names we've already registered
            let mut registered_names: std::collections::HashSet<String> =
                std::collections::HashSet::new();

            for header in headers {
                // Only define the function name once (even if it has multiple headers)
                if !registered_names.contains(&header.name) {
                    registered_names.insert(header.name.clone());
                    self.symbols.define_global(
                        header.name.clone(),
                        SymbolType::Function(header.id),
                        func.span.clone(),
                    )?;
                }
            }
        }
        Ok(())
    }
    fn register_action_name(&mut self, stmt: &Statement) -> ParseResult<()> {
        if let StatementKind::Action { name, .. } = &stmt.node {
            self.symbols
                .define_global(name.clone(), SymbolType::Action, stmt.span.clone())?;
        }
        Ok(())
    }
    fn validate_function_body(&mut self, func: &Statement) -> ParseResult<()> {
        if let StatementKind::Function { body, .. } = &func.node {
            self.symbols.enter_scope();
            self.loop_depth = 0;
            self.register_labels_in_block(body)?;
            self.validate_block(body)?;
            self.symbols.exit_scope();
        }
        Ok(())
    }
    fn validate_action_body(&self, action: &Statement) -> ParseResult<()> {
        if let StatementKind::Action { body, .. } = &action.node {
            for stmt in body {
                match &stmt.node {
                    StatementKind::ScriptCommand { args, .. } => {
                        for arg in args {
                            self.validate_expression(arg)?;
                        }
                    }
                    StatementKind::End | StatementKind::EndMovement => {}
                    _ => {
                        return Err(analysis_error(
                            stmt.span.clone(),
                            "Actions can only contain Commands and 'End'. Logic (If/Jump) and Aliases are not allowed.",
                        ));
                    }
                }
            }
        }
        Ok(())
    }
    fn register_labels_in_block(&mut self, block: &[Statement]) -> ParseResult<()> {
        for stmt in block {
            match &stmt.node {
                StatementKind::Label(name) => {
                    if name.starts_with('.') {
                        self.symbols.define_scoped(
                            name.clone(),
                            SymbolType::Label,
                            stmt.span.clone(),
                        )?;
                    } else {
                        self.symbols.define_global(
                            name.clone(),
                            SymbolType::Label,
                            stmt.span.clone(),
                        )?;
                    }
                }
                // recursively find labels inside if and while blocks and add to function scope
                StatementKind::IfStatement {
                    body, elseblock, ..
                } => {
                    self.register_labels_in_block(body)?;
                    if let Some(else_b) = elseblock {
                        self.register_labels_in_block(else_b)?;
                    }
                }
                StatementKind::WhileStatement { body, .. } => {
                    self.register_labels_in_block(body)?;
                }
                StatementKind::MatchStatement { cases, default, .. } => {
                    for case in cases {
                        self.register_labels_in_block(&case.body)?;
                    }
                    if let Some(default_body) = default {
                        self.register_labels_in_block(default_body)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn validate_block(&mut self, block: &[Statement]) -> ParseResult<()> {
        for stmt in block {
            self.validate_statement(stmt)?;
        }
        Ok(())
    }
    fn validate_statement(&mut self, stmt: &Statement) -> ParseResult<()> {
        match &stmt.node {
            StatementKind::Jump(target_expr) => {
                self.validate_jump_target(target_expr)?;
            }
            StatementKind::IfStatement {
                condition,
                body,
                elseblock,
            } => {
                self.validate_expression(condition)?;
                self.validate_block(body)?;

                if let Some(else_b) = elseblock {
                    self.validate_block(else_b)?;
                }
            }
            StatementKind::WhileStatement { condition, body } => {
                self.validate_expression(condition)?;
                self.loop_depth += 1;
                self.validate_block(body)?;
                self.loop_depth -= 1;
            }
            StatementKind::MatchStatement {
                subject,
                cases,
                default,
            } => {
                self.validate_match_subject(subject, &stmt.span)?;
                for case in cases {
                    for value in &case.values {
                        self.validate_expression(value)?;
                    }
                    self.validate_block(&case.body)?;
                }
                if let Some(default_body) = default {
                    self.validate_block(default_body)?;
                }
            }
            StatementKind::Break => {
                if self.loop_depth == 0 {
                    return Err(analysis_error(
                        stmt.span.clone(),
                        "break statement can only be used inside a while loop".to_string(),
                    ));
                }
            }
            StatementKind::ScriptCommand { command, args } => {
                self.validate_command(command, args, &stmt.span)?;
                for arg in args {
                    self.validate_expression(arg)?;
                }
            }
            StatementKind::AliasStatement { .. } => {
                self.register_alias(stmt)?;
            }
            // labels already gathered, skip
            StatementKind::Label(_) | StatementKind::Return | StatementKind::End => {}
            _ => {
                return Err(analysis_error(
                    stmt.span.clone(),
                    format!("unexpected statement: {:?}", stmt.node),
                ));
            }
        }
        Ok(())
    }

    /// Check if a symbol exists (in symbol table OR constants db)
    fn resolve_symbol(&self, name: &str) -> Option<SymbolType> {
        // 1. Check local/global symbols
        if let Some(kind) = self.symbols.resolve(name) {
            return Some(kind.clone());
        }

        // 2. Fallback: Check constants DB
        if let Some(db) = self.constants
            && let Some(value) = db.get(name)
        {
            return Some(SymbolType::Constant(value));
        }

        // 3. Fallback: Check condition identifiers (EQUAL, LESS, etc.)
        if let Some(cond) = ComparisonOperator::from_str(name) {
            return Some(SymbolType::Constant(cond as i32));
        }

        None
    }

    /// Look up a command in the database
    fn get_command(&self, name: &str) -> Option<&Command> {
        self.database?.get_command(name).ok()
    }

    fn is_command_call(&self, expr: &Expression) -> bool {
        matches!(
            &expr.node,
            ExpressionKind::Call { function, .. }
                if matches!(&function.node, ExpressionKind::Identifier(name) if self.get_command(name).is_some())
        )
    }

    /// Pick which param list this call should be checked against.
    ///
    /// This keeps analysis and lowering in sync.
    fn select_command_shape<'b>(
        &self,
        cmd: &'b Command,
        args: &[Expression],
    ) -> ResolvedCommandShape<'b> {
        let first_arg_u8 = args
            .first()
            .and_then(|arg| self.resolve_expression_to_int(arg).ok())
            .and_then(|value| u8::try_from(value).ok());

        cmd.resolve_source_call_shape(first_arg_u8, |condition, params| {
            self.evaluate_variant_condition_with_arg_count(condition, args, params)
                .unwrap_or(false)
        })
    }

    /// Check if a command is autovar-compatible (has a result parameter with `VAR_RESULT` default)
    fn is_autovar_command(&self, name: &str) -> bool {
        self.get_autovar_param_index(name).is_some()
    }

    /// Get the autovar parameter index for a command (the parameter that stores the result)
    fn get_autovar_param_index(&self, name: &str) -> Option<usize> {
        let cmd = self.get_command(name)?;
        crate::autovar::autovar_param_index(cmd)
    }

    /// Validate that a Call expression is to an autovar-compatible command
    fn validate_autovar_call(
        &self,
        function: &Expression,
        args: &[Expression],
        span: &Range<usize>,
    ) -> ParseResult<()> {
        let ExpressionKind::Identifier(name) = &function.node else {
            return Err(analysis_error(
                span.clone(),
                "Function calls in conditions must use a command name".to_string(),
            ));
        };

        if !self.is_autovar_command(name) {
            return Err(analysis_error(
                span.clone(),
                format!(
                    "Command '{}' cannot be used in a condition because it does not return a result. \
                     Only commands with a destVar/destVarID parameter that defaults to VAR_RESULT can be used this way.",
                    name
                ),
            ));
        }

        let Some(autovar_index) = self.get_autovar_param_index(name) else {
            return Err(analysis_error(
                span.clone(),
                format!(
                    "Internal error: could not find autovar parameter for '{}'",
                    name
                ),
            ));
        };

        if args.len() > autovar_index {
            return Err(analysis_error(
                span.clone(),
                format!(
                    "When using '{}' in a condition, do not specify the result variable - it will automatically use VAR_RESULT. \
                     Provide only the first {} argument(s).",
                    name, autovar_index
                ),
            ));
        }

        for arg in args {
            self.validate_expression(arg)?;
        }

        Ok(())
    }

    /// Validate a match statement subject (must be a variable or autovar call)
    fn validate_match_subject(
        &self,
        subject: &Expression,
        stmt_span: &Range<usize>,
    ) -> ParseResult<()> {
        match &subject.node {
            ExpressionKind::Identifier(name) => match self.resolve_symbol(name) {
                Some(SymbolType::Variable(_) | SymbolType::Constant(_)) => Ok(()),
                Some(_) => Err(analysis_error(
                    subject.span.clone(),
                    format!(
                        "match subject '{}' must be a variable, not a label or function",
                        name
                    ),
                )),
                None => Err(analysis_error(
                    subject.span.clone(),
                    format!("Undefined symbol '{}' in match subject", name),
                )),
            },
            ExpressionKind::Number(n) => {
                if *n >= 0x4000 {
                    Ok(())
                } else {
                    Err(analysis_error(
                        subject.span.clone(),
                        format!(
                            "match subject must be a variable (>= 0x4000), got literal value {}",
                            n
                        ),
                    ))
                }
            }
            ExpressionKind::Call { function, args } => {
                self.validate_autovar_call(function, args, &subject.span)?;
                Ok(())
            }
            _ => Err(analysis_error(
                stmt_span.clone(),
                "match subject must be a variable or an autovar command call".to_string(),
            )),
        }
    }

    /// Validate command parameters (count and types)
    fn validate_command(
        &self,
        command: &str,
        args: &[Expression],
        span: &Range<usize>,
    ) -> ParseResult<()> {
        let Some(cmd) = self.get_command(command) else {
            return Ok(());
        };

        // Analysis checks the args against the chosen source shape. Defaults and `emit_args`
        // happen later during lowering.
        let shape = self.select_command_shape(cmd, args);
        let params = shape.params;
        let actual_count = args.len();
        let is_macro = cmd.is_macro();
        let required_count = params
            .iter()
            .filter(|p| !p.optional && p.default.is_none())
            .count();
        let max_count = params.len();

        if actual_count < required_count {
            return Err(analysis_error(
                span.clone(),
                format!(
                    "Command '{}' requires at least {} argument(s), got {}",
                    command, required_count, actual_count
                ),
            ));
        }

        if actual_count > max_count {
            return Err(analysis_error(
                span.clone(),
                format!(
                    "Command '{}' accepts at most {} argument(s), got {}",
                    command, max_count, actual_count
                ),
            ));
        }

        // Skip type validation for macros since their param types are hints,
        // not actual requirements (the expanded commands determine the real types)
        if !is_macro {
            for (i, (arg, param)) in args.iter().zip(params.iter()).enumerate() {
                self.validate_argument_type(arg, &param.param_type, command, &param.name, i)?;
            }
        }

        Ok(())
    }

    fn evaluate_variant_condition_with_arg_count(
        &self,
        condition: &str,
        args: &[Expression],
        params: &[ParamDef],
    ) -> ParseResult<bool> {
        if let Some(caps) = RE_ARG_COUNT.captures(condition) {
            let expected_count: usize = caps[1].parse().unwrap_or(0);
            return Ok(args.len() == expected_count);
        }
        self.evaluate_variant_condition(condition, args, params)
    }

    fn evaluate_variant_condition(
        &self,
        condition: &str,
        args: &[Expression],
        params: &[ParamDef],
    ) -> ParseResult<bool> {
        let exprs: HashMap<String, String> = HashMap::new();
        let mut resolved: HashMap<String, i64> = HashMap::new();
        let cache: DashMap<String, i64> = DashMap::new();

        for (pos, param) in params.iter().enumerate() {
            if let Some(arg) = args.get(pos)
                && let Ok(value) = self.resolve_expression_to_int(arg)
            {
                resolved.insert(param.name.clone(), i64::from(value));
            }
        }

        let parent_resolver = |name: &str| -> Option<i64> {
            match name {
                "VARS_START" => Some(0x4000),
                "VARS_END" | "SCRIPT_LOCAL_VARS_END" => Some(0x800D),
                "SCRIPT_LOCAL_VARS_START" => Some(0x8000),
                _ => {
                    if let Some(SymbolType::Constant(val) | SymbolType::Variable(val)) =
                        self.resolve_symbol(name)
                    {
                        return Some(i64::from(val));
                    }
                    None
                }
            }
        };

        eval_expr_with_parent(condition, &exprs, &resolved, &cache, &parent_resolver)
            .map(|value| value != 0)
            .ok_or_else(|| {
                analysis_error(
                    0..0,
                    format!("Failed to evaluate variant condition '{}'", condition),
                )
            })
    }

    fn resolve_expression_to_int(&self, expr: &Expression) -> ParseResult<i32> {
        match &expr.node {
            ExpressionKind::Number(n) => Ok(*n),
            ExpressionKind::Identifier(name) => match name.as_str() {
                "VARS_START" => Ok(0x4000),
                "VARS_END" | "SCRIPT_LOCAL_VARS_END" => Ok(0x800D),
                "SCRIPT_LOCAL_VARS_START" => Ok(0x8000),
                _ => match self.resolve_symbol(name) {
                    Some(SymbolType::Constant(val) | SymbolType::Variable(val)) => Ok(val),
                    _ => Err(analysis_error(
                        expr.span.clone(),
                        format!("Could not resolve '{}' to an integer expression", name),
                    )),
                },
            },
            ExpressionKind::Prefix { operator, id } => {
                let value = self.resolve_expression_to_int(id)?;
                match operator {
                    super::token::TokenType::Minus => Ok(-value),
                    super::token::TokenType::Plus => Ok(value),
                    _ => Err(analysis_error(
                        expr.span.clone(),
                        format!(
                            "Unsupported prefix operator {:?} in integer expression",
                            operator
                        ),
                    )),
                }
            }
            ExpressionKind::Infix {
                left,
                operator,
                right,
            } => {
                let left = self.resolve_expression_to_int(left)?;
                let right = self.resolve_expression_to_int(right)?;
                match operator {
                    super::token::TokenType::Plus => left.checked_add(right),
                    super::token::TokenType::Minus => left.checked_sub(right),
                    super::token::TokenType::Mul => left.checked_mul(right),
                    _ => None,
                }
                .ok_or_else(|| {
                    analysis_error(
                        expr.span.clone(),
                        format!(
                            "Unsupported or overflowing operator {:?} in integer expression",
                            operator
                        ),
                    )
                })
            }
            ExpressionKind::Call { function, .. } => {
                let ExpressionKind::Identifier(name) = &function.node else {
                    return Err(analysis_error(
                        expr.span.clone(),
                        "Function-like constant expressions must use a simple name".to_string(),
                    ));
                };

                if self.get_command(name).is_some() {
                    return Err(analysis_error(
                        expr.span.clone(),
                        format!("Command '{}' cannot be used as a constant expression", name),
                    ));
                }

                let expr_text = Self::format_expression_for_constant_eval(expr)?;
                if let Some(constants) = self.constants
                    && let Some(value) = constants.evaluate_expression(&expr_text)
                {
                    return Ok(value);
                }

                Err(analysis_error(
                    expr.span.clone(),
                    format!("Could not resolve '{}' as a constant expression", expr_text),
                ))
            }
            ExpressionKind::Label(_) => Err(analysis_error(
                expr.span.clone(),
                format!(
                    "Unsupported expression {:?} in integer expression",
                    expr.node
                ),
            )),
        }
    }

    fn format_expression_for_constant_eval(expr: &Expression) -> ParseResult<String> {
        match &expr.node {
            ExpressionKind::Number(n) => Ok(n.to_string()),
            ExpressionKind::Identifier(name) | ExpressionKind::Label(name) => Ok(name.clone()),
            ExpressionKind::Prefix { operator, id } => {
                let inner = Self::format_expression_for_constant_eval(id)?;
                let op = match operator {
                    super::token::TokenType::Minus => "-",
                    super::token::TokenType::Plus => "+",
                    super::token::TokenType::Not => "!",
                    _ => {
                        return Err(analysis_error(
                            expr.span.clone(),
                            format!(
                                "Unsupported prefix operator {:?} in constant expression",
                                operator
                            ),
                        ));
                    }
                };
                Ok(format!("{}{}", op, inner))
            }
            ExpressionKind::Infix {
                left,
                operator,
                right,
            } => {
                let left_str = Self::format_expression_for_constant_eval(left)?;
                let right_str = Self::format_expression_for_constant_eval(right)?;
                let op = match operator {
                    super::token::TokenType::Plus => "+",
                    super::token::TokenType::Minus => "-",
                    super::token::TokenType::Mul => "*",
                    super::token::TokenType::LesserThan => "<",
                    super::token::TokenType::GreaterThan => ">",
                    super::token::TokenType::LesserEqual => "<=",
                    super::token::TokenType::GreaterEqual => ">=",
                    super::token::TokenType::Equal => "==",
                    super::token::TokenType::NotEqual => "!=",
                    super::token::TokenType::And => "&&",
                    super::token::TokenType::Or => "||",
                    _ => {
                        return Err(analysis_error(
                            expr.span.clone(),
                            format!("Unsupported operator {:?} in constant expression", operator),
                        ));
                    }
                };
                Ok(format!("({} {} {})", left_str, op, right_str))
            }
            ExpressionKind::Call { function, args } => {
                let ExpressionKind::Identifier(name) = &function.node else {
                    return Err(analysis_error(
                        expr.span.clone(),
                        "Function-like constant expressions must use a simple name".to_string(),
                    ));
                };

                let mut formatted_args = Vec::with_capacity(args.len());
                for arg in args {
                    formatted_args.push(Self::format_expression_for_constant_eval(arg)?);
                }

                Ok(format!("{}({})", name, formatted_args.join(", ")))
            }
        }
    }

    /// Validate that an argument matches the expected parameter type
    fn validate_argument_type(
        &self,
        arg: &Expression,
        param_type: &ParamType,
        command: &str,
        param_name: &str,
        arg_index: usize,
    ) -> ParseResult<()> {
        match &arg.node {
            ExpressionKind::Number(n) => {
                Self::validate_number_for_type(
                    *n, param_type, &arg.span, command, param_name, arg_index,
                )?;
            }
            ExpressionKind::Label(_) => {
                if !matches!(
                    param_type,
                    ParamType::Label | ParamType::ScriptId | ParamType::MovementId
                ) {
                    return Err(analysis_error(
                        arg.span.clone(),
                        format!(
                            "Argument {} ('{}') for '{}' expects {:?}, got a label reference",
                            arg_index + 1,
                            param_name,
                            command,
                            param_type
                        ),
                    ));
                }
            }
            ExpressionKind::Identifier(name) => {
                if let Some(SymbolType::Constant(value)) = self.resolve_symbol(name) {
                    Self::validate_number_for_type(
                        value, param_type, &arg.span, command, param_name, arg_index,
                    )?;
                }
            }
            _ => {}
        }

        Ok(())
    }

    /// Validate that a number fits within the range for a parameter type
    fn validate_number_for_type(
        n: i32,
        param_type: &ParamType,
        span: &Range<usize>,
        command: &str,
        param_name: &str,
        arg_index: usize,
    ) -> ParseResult<()> {
        let (valid, type_desc) = match param_type {
            ParamType::U8 => {
                let fits_unsigned = u8::try_from(n).is_ok();
                let fits_signed = i8::try_from(n).is_ok();
                (fits_unsigned || fits_signed, "u8 (0-255)")
            }
            ParamType::U16 | ParamType::Var | ParamType::Flag | ParamType::MsgId => {
                let fits_unsigned = u16::try_from(n).is_ok();
                let fits_signed = i16::try_from(n).is_ok();
                (fits_unsigned || fits_signed, "u16 (0-65535)")
            }
            ParamType::U32 | ParamType::Label | ParamType::ScriptId | ParamType::MovementId => {
                (n >= 0, "u32 (non-negative)")
            }
            ParamType::Unknown => (true, "unknown"),
        };

        if !valid {
            return Err(analysis_error(
                span.clone(),
                format!(
                    "Argument {} ('{}') for '{}' is out of range: {} does not fit in {}",
                    arg_index + 1,
                    param_name,
                    command,
                    n,
                    type_desc
                ),
            ));
        }

        Ok(())
    }

    pub fn validate_expression(&self, expr: &Expression) -> ParseResult<()> {
        match &expr.node {
            ExpressionKind::Number(_) | ExpressionKind::Label(_) => {}
            ExpressionKind::Identifier(name) => {
                if self.resolve_symbol(name).is_none() {
                    return Err(analysis_error(
                        expr.span.clone(),
                        format!("Undefined symbol: '{}'", name),
                    ));
                }
            }
            ExpressionKind::Infix { left, right, .. } => {
                self.validate_expression(left)?;
                self.validate_expression(right)?;
            }
            ExpressionKind::Prefix { id, .. } => {
                self.validate_expression(id)?;
            }
            ExpressionKind::Call { function, args } => {
                if self.is_command_call(expr) {
                    self.validate_autovar_call(function, args, &expr.span)?;
                } else {
                    self.resolve_expression_to_int(expr).map(|_| ())?;
                }
            }
        }
        Ok(())
    }

    fn validate_jump_target(&self, expr: &Expression) -> ParseResult<()> {
        let (ExpressionKind::Label(name) | ExpressionKind::Identifier(name)) = &expr.node else {
            return Ok(());
        };

        match self.resolve_symbol(name) {
            Some(_) => Ok(()),
            None => Err(analysis_error(
                expr.span.clone(),
                format!(
                    "Jump target '{}' not found. (Did you forget to define the label?)",
                    name
                ),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::ast::{ExpressionKind, FunctionHeader, StatementKind};
    use crate::database::{CommandType, DatabaseMeta, Variant};
    use std::collections::HashMap;

    fn view_rankings_shape_db() -> DatabaseV2 {
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
        }
    }

    #[test]
    fn test_analyzer_registers_alias() {
        let mut analyzer = Analyzer::new();
        let alias_stmt = Statement {
            node: StatementKind::AliasStatement {
                value: Expression {
                    node: ExpressionKind::Number(1),
                    span: 0..1,
                },
                name: "global_var".to_string(),
            },
            span: 0..10,
        };
        analyzer
            .register_alias(&alias_stmt)
            .expect("Failed to register alias");
        match analyzer.symbols.resolve("global_var") {
            Some(SymbolType::Variable(id)) => assert_eq!(*id, 1),
            _ => panic!("Alias not found in symbol table"),
        }
    }

    #[test]
    fn test_analyzer_registers_alias_from_prior_alias() {
        let source = r"
alias 1 as foo
alias foo as bar

function Test #1:
    End
";
        let lexer = crate::compiler::Lexer::new(source);
        let mut parser = crate::compiler::Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::new();
        analyzer
            .analyze(&script_file)
            .expect("Failed to analyze chained aliases");

        match analyzer.symbols.resolve("bar") {
            Some(SymbolType::Variable(id)) => assert_eq!(*id, 1),
            _ => panic!("Chained alias not found in symbol table"),
        }
    }

    #[test]
    fn test_analyzer_rejects_forward_alias_reference() {
        let source = r"
alias foo as bar
alias 1 as foo

function Test #1:
    End
";
        let lexer = crate::compiler::Lexer::new(source);
        let mut parser = crate::compiler::Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::new();
        let result = analyzer.analyze(&script_file);
        assert!(result.is_err(), "forward alias reference should fail");
    }

    #[test]
    fn test_break_inside_while_is_valid() {
        let db = DatabaseV2::load("src/db/platinum_v2.json").unwrap();
        let constants = ConstantDb::new();
        let mut analyzer = Analyzer::with_database(&constants, &db);

        let source = r"
function TestFunc #1:
    while 0x8000 != 0 do
        break
    endwhile
    End
";
        let lexer = crate::compiler::Lexer::new(source);
        let mut parser = crate::compiler::Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let result = analyzer.analyze(&script_file);
        assert!(result.is_ok(), "break inside while should be valid");
    }

    #[test]
    fn test_break_outside_while_is_error() {
        let db = DatabaseV2::load("src/db/platinum_v2.json").unwrap();
        let constants = ConstantDb::new();
        let mut analyzer = Analyzer::with_database(&constants, &db);

        let source = r"
function TestFunc #1:
    break
    End
";
        let lexer = crate::compiler::Lexer::new(source);
        let mut parser = crate::compiler::Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let result = analyzer.analyze(&script_file);
        assert!(result.is_err(), "break outside while should be an error");
        let err = format!("{:?}", result.unwrap_err());
        assert!(err.contains("break statement can only be used inside a while loop"));
    }

    #[test]
    fn test_break_after_while_is_error() {
        let db = DatabaseV2::load("src/db/platinum_v2.json").unwrap();
        let constants = ConstantDb::new();
        let mut analyzer = Analyzer::with_database(&constants, &db);

        let source = r"
function TestFunc #1:
    while 0x8000 != 0 do
        End
    endwhile
    break
";
        let lexer = crate::compiler::Lexer::new(source);
        let mut parser = crate::compiler::Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let result = analyzer.analyze(&script_file);
        assert!(result.is_err(), "break after while should be an error");
    }

    #[test]
    fn test_break_in_if_inside_while_is_valid() {
        let db = DatabaseV2::load("src/db/platinum_v2.json").unwrap();
        let constants = ConstantDb::new();
        let mut analyzer = Analyzer::with_database(&constants, &db);

        let source = r"
function TestFunc #1:
    while 0x8000 != 0 do
        if 0x8000 == 5 then
            break
        endif
    endwhile
    End
";
        let lexer = crate::compiler::Lexer::new(source);
        let mut parser = crate::compiler::Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let result = analyzer.analyze(&script_file);
        assert!(result.is_ok(), "break in if inside while should be valid");
    }

    #[test]
    fn test_match_with_variable_is_valid() {
        let db = DatabaseV2::load("src/db/platinum_v2.json").unwrap();
        let constants = ConstantDb::new();
        let mut analyzer = Analyzer::with_database(&constants, &db);

        let source = r"
function TestFunc #1:
    match 0x8000 with
        case 0:
            Message 1
        else:
            Message 2
    endmatch
    End
";
        let lexer = crate::compiler::Lexer::new(source);
        let mut parser = crate::compiler::Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let result = analyzer.analyze(&script_file);
        assert!(result.is_ok(), "match with variable should be valid");
    }

    #[test]
    fn test_match_with_literal_value_is_error() {
        let db = DatabaseV2::load("src/db/platinum_v2.json").unwrap();
        let constants = ConstantDb::new();
        let mut analyzer = Analyzer::with_database(&constants, &db);

        let source = r"
function TestFunc #1:
    match 5 with
        case 0:
            Message 1
    endmatch
    End
";
        let lexer = crate::compiler::Lexer::new(source);
        let mut parser = crate::compiler::Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let result = analyzer.analyze(&script_file);
        assert!(
            result.is_err(),
            "match with literal value should be an error"
        );
        let err = format!("{:?}", result.unwrap_err());
        assert!(err.contains("must be a variable"));
    }

    #[test]
    fn test_autovar_call_in_condition_valid() {
        let db = DatabaseV2::load("src/db/platinum_v2.json").unwrap();
        let constants = ConstantDb::new();
        let mut analyzer = Analyzer::with_database(&constants, &db);

        let source = r"
function TestFunc #1:
    if CheckPlayerOnBike() then
        Message 1
    endif
    End
";
        let lexer = crate::compiler::Lexer::new(source);
        let mut parser = crate::compiler::Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let result = analyzer.analyze(&script_file);
        assert!(
            result.is_ok(),
            "autovar command in condition should be valid: {:?}",
            result
        );
    }

    #[test]
    fn test_autovar_call_in_match_valid() {
        let db = DatabaseV2::load("src/db/platinum_v2.json").unwrap();
        let constants = ConstantDb::new();
        let mut analyzer = Analyzer::with_database(&constants, &db);

        let source = r"
function TestFunc #1:
    match ShowYesNoMenu() with
        case 0:
            Message 1
        case 1:
            Message 2
    endmatch
    End
";
        let lexer = crate::compiler::Lexer::new(source);
        let mut parser = crate::compiler::Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let result = analyzer.analyze(&script_file);
        assert!(
            result.is_ok(),
            "autovar command in match should be valid: {:?}",
            result
        );
    }

    #[test]
    fn test_non_autovar_call_in_condition_error() {
        let db = DatabaseV2::load("src/db/platinum_v2.json").unwrap();
        let constants = ConstantDb::new();
        let mut analyzer = Analyzer::with_database(&constants, &db);

        let source = r"
function TestFunc #1:
    if Message() then
        End
    endif
    End
";
        let lexer = crate::compiler::Lexer::new(source);
        let mut parser = crate::compiler::Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let result = analyzer.analyze(&script_file);
        assert!(
            result.is_err(),
            "non-autovar command in condition should be an error"
        );
        let err = format!("{:?}", result.unwrap_err());
        assert!(err.contains("does not return a result"));
    }

    #[test]
    fn test_autovar_call_with_comparison() {
        let db = DatabaseV2::load("src/db/platinum_v2.json").unwrap();
        let constants = ConstantDb::new();
        let mut analyzer = Analyzer::with_database(&constants, &db);

        let source = r"
function TestFunc #1:
    if ShowYesNoMenu() == 1 then
        Message 1
    endif
    End
";
        let lexer = crate::compiler::Lexer::new(source);
        let mut parser = crate::compiler::Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let result = analyzer.analyze(&script_file);
        assert!(
            result.is_ok(),
            "autovar command with comparison should be valid: {:?}",
            result
        );
    }

    #[test]
    fn test_analyzer_detects_undefined_symbol() {
        let analyzer = Analyzer::new();
        let expr = Expression {
            node: ExpressionKind::Identifier("undefined_var".to_string()),
            span: 0..15,
        };
        let result = analyzer.validate_expression(&expr);
        assert!(result.is_err());
    }

    #[test]
    fn test_analyzer_registers_function_name() {
        let mut analyzer = Analyzer::new();
        let func_stmt = Statement {
            node: StatementKind::Function {
                headers: vec![FunctionHeader {
                    name: "my_function".to_string(),
                    id: Some(42),
                    is_public: true,
                }],
                body: vec![],
            },
            span: 0..20,
        };
        analyzer
            .register_function_names(&func_stmt)
            .expect("Failed to register function name");
        match analyzer.symbols.resolve("my_function") {
            Some(SymbolType::Function(id)) => assert_eq!(*id, Some(42)),
            _ => panic!("Function name not found in symbol table"),
        }
    }

    #[test]
    fn test_analyzer_registers_labels_in_block() {
        let mut analyzer = Analyzer::new();
        let block = vec![
            Statement {
                node: StatementKind::Label("global_label".to_string()),
                span: 0..15,
            },
            Statement {
                node: StatementKind::Label(".local_label".to_string()),
                span: 16..30,
            },
        ];
        analyzer
            .register_labels_in_block(&block)
            .expect("Failed to register labels in block");
        match analyzer.symbols.resolve("global_label") {
            Some(SymbolType::Label) => {}
            _ => panic!("Global label not found in symbol table"),
        }
        match analyzer.symbols.resolve(".local_label") {
            Some(SymbolType::Label) => {}
            _ => panic!("Local label not found in symbol table"),
        }
    }

    #[test]
    fn test_analyze_full_script() {
        use crate::compiler::lexer::Lexer;
        use crate::compiler::parser::Parser;

        let source = r"
alias 0x800C as VAR_RESULT

function MainFunc #0:
    SetVar VAR_RESULT, 5
    if VAR_RESULT == 5 then
        Message 1
    endif
    Jump .done
.done:
    End

action TestMovement
    WalkNormalNorth 3
    EndMovement
";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::new();
        let result = analyzer.analyze(&script_file);
        assert!(
            result.is_ok(),
            "Full script analysis should succeed: {:?}",
            result
        );

        // Check that symbols were registered
        assert!(analyzer.symbols.resolve("VAR_RESULT").is_some());
        assert!(analyzer.symbols.resolve("MainFunc").is_some());
        assert!(analyzer.symbols.resolve("TestMovement").is_some());
    }

    #[test]
    fn test_duplicate_alias_rebinds_latest_value() {
        use crate::compiler::lexer::Lexer;
        use crate::compiler::parser::Parser;

        let source = r"
alias 0x8000 as VAR_X
alias 0x8001 as VAR_X

function Dummy #0:
    End
";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::new();
        analyzer
            .analyze(&script_file)
            .expect("duplicate alias should rebind to the latest value");
        match analyzer.symbols.resolve("VAR_X") {
            Some(SymbolType::Variable(id)) => assert_eq!(*id, 0x8001),
            _ => panic!("Expected rebound alias in symbol table"),
        }
    }

    #[test]
    fn test_control_flow_in_action_error() {
        use crate::compiler::lexer::Lexer;
        use crate::compiler::parser::Parser;

        let source = r"
function Dummy #0:
    End

action BadAction
    if 0x8000 == 1 then
        WalkNorth
    endif
    EndMovement
";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::new();
        let result = analyzer.analyze(&script_file);
        assert!(result.is_err(), "Control flow in action should cause error");
    }

    #[test]
    fn test_analyzer_stacked_headers_no_duplicate_error() {
        use crate::compiler::lexer::Lexer;
        use crate::compiler::parser::Parser;
        use crate::database::DatabaseV2;
        use std::path::Path;

        let source = r"
function TestFunc #1:
function TestFunc #2:
    End
";
        let db = DatabaseV2::load(Path::new("src/db/platinum_v2.json")).unwrap();
        let mut constants = ConstantDb::new();
        constants.load_from_db(&db);

        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::with_constants(&constants);
        let result = analyzer.analyze(&script_file);
        assert!(
            result.is_ok(),
            "Analyzer failed to handle stacked headers: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_command_parameter_count_too_few() {
        use crate::compiler::lexer::Lexer;
        use crate::compiler::parser::Parser;
        use crate::database::DatabaseV2;
        use std::path::Path;

        let source = r"
function Test #1:
    LockAll
    End
";
        let db = DatabaseV2::load(Path::new("src/db/platinum_v2.json")).unwrap();
        let mut constants = ConstantDb::new();
        constants.load_from_db(&db);

        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::with_database(&constants, &db);
        let result = analyzer.analyze(&script_file);
        assert!(
            result.is_ok(),
            "LockAll takes 0 params, should pass: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_command_parameter_count_validation() {
        use crate::compiler::lexer::Lexer;
        use crate::compiler::parser::Parser;
        use crate::database::DatabaseV2;
        use std::path::Path;

        let source = r"
function Test #1:
    SetFlag 100
    End
";
        let db = DatabaseV2::load(Path::new("src/db/platinum_v2.json")).unwrap();
        let mut constants = ConstantDb::new();
        constants.load_from_db(&db);

        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::with_database(&constants, &db);
        let result = analyzer.analyze(&script_file);
        assert!(
            result.is_ok(),
            "SetFlag with 1 param should pass: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_command_parameter_u8_range_overflow() {
        use crate::compiler::lexer::Lexer;
        use crate::compiler::parser::Parser;
        use crate::database::DatabaseV2;
        use std::path::Path;

        let source = r"
function Test #1:
    RegValueSet 999, 1
    End
";
        let db = DatabaseV2::load(Path::new("src/db/platinum_v2.json")).unwrap();
        let mut constants = ConstantDb::new();
        constants.load_from_db(&db);

        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::with_database(&constants, &db);
        let result = analyzer.analyze(&script_file);
        assert!(
            result.is_err(),
            "999 should overflow u8 and fail validation"
        );
        let err_msg = format!("{:?}", result.err());
        assert!(
            err_msg.contains("out of range"),
            "Error should mention out of range: {}",
            err_msg
        );
    }

    #[test]
    fn test_analyzer_without_database_skips_command_validation() {
        use crate::compiler::lexer::Lexer;
        use crate::compiler::parser::Parser;

        let source = r"
function Test #1:
    SomeUnknownCommand 1, 2, 3, 4, 5
    End
";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::new();
        let result = analyzer.analyze(&script_file);
        assert!(
            result.is_ok(),
            "Without database, unknown commands should be allowed: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_command_parameter_count_too_many() {
        use crate::compiler::lexer::Lexer;
        use crate::compiler::parser::Parser;
        use crate::database::DatabaseV2;
        use std::path::Path;

        let source = r"
function Test #1:
    SetFlag 100, 200, 300
    End
";
        let db = DatabaseV2::load(Path::new("src/db/platinum_v2.json")).unwrap();
        let mut constants = ConstantDb::new();
        constants.load_from_db(&db);

        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::with_database(&constants, &db);
        let result = analyzer.analyze(&script_file);
        assert!(
            result.is_err(),
            "SetFlag with 3 args should fail (expects 1)"
        );
        let err_msg = format!("{:?}", result.err());
        assert!(
            err_msg.contains("at most 1"),
            "Error should mention max args: {}",
            err_msg
        );
    }

    #[test]
    fn test_command_missing_required_arguments() {
        use crate::compiler::lexer::Lexer;
        use crate::compiler::parser::Parser;
        use crate::database::DatabaseV2;
        use std::path::Path;

        let source = r"
function Test #1:
    SetFlag
    End
";
        let db = DatabaseV2::load(Path::new("src/db/platinum_v2.json")).unwrap();
        let mut constants = ConstantDb::new();
        constants.load_from_db(&db);

        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::with_database(&constants, &db);
        let result = analyzer.analyze(&script_file);
        assert!(
            result.is_err(),
            "SetFlag with 0 args should fail (requires 1)"
        );
        let err_msg = format!("{:?}", result.err());
        assert!(
            err_msg.contains("requires at least 1"),
            "Error should mention required args: {}",
            err_msg
        );
    }

    #[test]
    fn test_command_u16_range_valid() {
        use crate::compiler::lexer::Lexer;
        use crate::compiler::parser::Parser;
        use crate::database::DatabaseV2;
        use std::path::Path;

        let source = r"
function Test #1:
    SetFlag 65535
    End
";
        let db = DatabaseV2::load(Path::new("src/db/platinum_v2.json")).unwrap();
        let mut constants = ConstantDb::new();
        constants.load_from_db(&db);

        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::with_database(&constants, &db);
        let result = analyzer.analyze(&script_file);
        assert!(
            result.is_ok(),
            "SetFlag with 65535 should pass (max u16): {:?}",
            result.err()
        );
    }

    #[test]
    fn test_command_u16_range_overflow() {
        use crate::compiler::lexer::Lexer;
        use crate::compiler::parser::Parser;
        use std::path::Path;

        let source = r"
function TestFunc #1:
    SetFlag 65536
    End
";
        let db = DatabaseV2::load(Path::new("src/db/platinum_v2.json")).unwrap();
        let mut constants = ConstantDb::new();
        constants.load_from_db(&db);

        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::with_database(&constants, &db);
        let result = analyzer.analyze(&script_file);
        assert!(
            result.is_err(),
            "SetFlag with 65536 should fail (overflows u16)"
        );
        let err_msg = format!("{:?}", result.err());
        assert!(
            err_msg.contains("out of range"),
            "Error should mention out of range: {}",
            err_msg
        );
    }

    #[test]
    fn test_macro_param_count_validation() {
        use crate::compiler::lexer::Lexer;
        use crate::compiler::parser::Parser;
        use std::path::Path;

        let source = r"
function Test #1:
    GoToIfNotEnoughMoney 100000
    End
";
        let db = DatabaseV2::load(Path::new("src/db/platinum_v2.json")).unwrap();
        let constants = ConstantDb::new();

        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::with_database(&constants, &db);
        let result = analyzer.analyze(&script_file);

        assert!(
            result.is_err(),
            "Macro with too few arguments should fail validation"
        );
        let err_msg = format!("{:?}", result.err().unwrap());
        assert!(
            err_msg.contains("requires") || err_msg.contains("argument"),
            "Error should mention missing arguments: {}",
            err_msg
        );
    }

    #[test]
    fn test_macro_does_not_validate_param_types() {
        use crate::compiler::lexer::Lexer;
        use crate::compiler::parser::Parser;
        use std::path::Path;

        // GoToIfNotEnoughMoney declares value as u16, but should accept u32 values
        // since it expands to CheckMoney which takes u32
        let source = r"
function Test #1:
    GoToIfNotEnoughMoney 100000, TestLabel
TestLabel:
    End
";
        let db = DatabaseV2::load(Path::new("src/db/platinum_v2.json")).unwrap();
        let constants = ConstantDb::new();

        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::with_database(&constants, &db);
        let result = analyzer.analyze(&script_file);

        assert!(
            result.is_ok(),
            "Macro should not validate param types (value > u16::MAX is OK). Error: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_command_with_default_param_can_be_omitted() {
        use crate::compiler::lexer::Lexer;
        use crate::compiler::parser::Parser;
        use crate::database::DatabaseV2;
        use std::path::Path;

        let source = r"
function Test #1:
    PlayCry 440
    End
";
        let db = DatabaseV2::load(Path::new("src/db/platinum_v2.json")).unwrap();
        let mut constants = ConstantDb::new();
        constants.load_from_db(&db);

        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::with_database(&constants, &db);
        let result = analyzer.analyze(&script_file);
        assert!(
            result.is_ok(),
            "PlayCry with 1 arg should pass (2nd param has default): {:?}",
            result.err()
        );
    }

    #[test]
    fn test_command_with_all_default_params_can_be_omitted() {
        use crate::compiler::lexer::Lexer;
        use crate::compiler::parser::Parser;
        use crate::database::DatabaseV2;
        use std::path::Path;

        let source = r"
function Test #1:
    PokeMartCommon
    End
";
        let db = DatabaseV2::load(Path::new("src/db/platinum_v2.json")).unwrap();
        let mut constants = ConstantDb::new();
        constants.load_from_db(&db);

        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::with_database(&constants, &db);
        let result = analyzer.analyze(&script_file);
        assert!(
            result.is_ok(),
            "PokeMartCommon with 0 args should pass (param has default): {:?}",
            result.err()
        );
    }

    #[test]
    fn test_command_with_variants_accepts_valid_count() {
        use crate::compiler::lexer::Lexer;
        use crate::compiler::parser::Parser;
        use crate::database::DatabaseV2;
        use std::path::Path;

        let source = r"
function Test #1:
    CallTVBroadcast 0, 0x800C
    End
";
        let db = DatabaseV2::load(Path::new("src/db/platinum_v2.json")).unwrap();
        let mut constants = ConstantDb::new();
        constants.load_from_db(&db);

        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::with_database(&constants, &db);
        let result = analyzer.analyze(&script_file);
        assert!(
            result.is_ok(),
            "CallTVBroadcast with 2 args should pass (variant 0 accepts 2): {:?}",
            result.err()
        );
    }

    #[test]
    fn test_command_variant_emit_args_accepts_source_call_shape() {
        use crate::compiler::lexer::Lexer;
        use crate::compiler::parser::Parser;

        let source = r"
function Test #1:
    ViewRankings 1, 2, 0x800C
    End
";
        let db = view_rankings_shape_db();
        let constants = ConstantDb::new();

        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::with_database(&constants, &db);
        let result = analyzer.analyze(&script_file);
        assert!(
            result.is_ok(),
            "ViewRankings 3-arg source shape should be accepted: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_command_variant_emit_args_rejects_invalid_source_count() {
        use crate::compiler::lexer::Lexer;
        use crate::compiler::parser::Parser;

        let source = r"
function Test #1:
    ViewRankings 1, 2, 3, 0x800C
    End
";
        let db = view_rankings_shape_db();
        let constants = ConstantDb::new();

        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::with_database(&constants, &db);
        let result = analyzer.analyze(&script_file);
        assert!(result.is_err(), "4 args should still be rejected");
        assert!(
            format!("{:?}", result.unwrap_err()).contains("accepts at most 2 argument(s), got 4")
        );
    }

    #[test]
    fn test_command_trainer_battle_with_default() {
        use crate::compiler::lexer::Lexer;
        use crate::compiler::parser::Parser;
        use crate::database::DatabaseV2;
        use std::path::Path;

        let source = r"
function Test #1:
    StartTrainerBattle 913
    End
";
        let db = DatabaseV2::load(Path::new("src/db/platinum_v2.json")).unwrap();
        let mut constants = ConstantDb::new();
        constants.load_from_db(&db);

        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::with_database(&constants, &db);
        let result = analyzer.analyze(&script_file);
        assert!(
            result.is_ok(),
            "StartTrainerBattle with 1 arg should pass (2nd param has default): {:?}",
            result.err()
        );
    }

    #[test]
    fn test_nested_labels_in_if_are_registered() {
        use crate::compiler::lexer::Lexer;
        use crate::compiler::parser::Parser;

        let source = r"
function Test #1:
    if 0x8000 == 1 then
        Jump IfBranch
    else
        Jump ElseBranch
    endif
IfBranch:
    End
ElseBranch:
    End
";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::new();
        let result = analyzer.analyze(&script_file);
        assert!(
            result.is_ok(),
            "labels defined for if/else branches should be registered: {:?}",
            result.err()
        );
        assert!(analyzer.symbols.resolve("IfBranch").is_some());
        assert!(analyzer.symbols.resolve("ElseBranch").is_some());
    }

    #[test]
    fn test_nested_labels_in_while_are_registered() {
        use crate::compiler::lexer::Lexer;
        use crate::compiler::parser::Parser;

        let source = r"
function Test #1:
    while 0x8000 != 0 do
        Jump LoopExit
    endwhile
LoopExit:
    End
";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::new();
        let result = analyzer.analyze(&script_file);
        assert!(
            result.is_ok(),
            "labels referenced from while bodies should be registered: {:?}",
            result.err()
        );
        assert!(analyzer.symbols.resolve("LoopExit").is_some());
    }

    #[test]
    fn test_nested_labels_in_match_are_registered() {
        use crate::compiler::lexer::Lexer;
        use crate::compiler::parser::Parser;

        let source = r"
function Test #1:
    match 0x8000 with
        case 0:
            Jump CaseZero
        else:
            Jump CaseDefault
    endmatch
CaseZero:
    End
CaseDefault:
    End
";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::new();
        let result = analyzer.analyze(&script_file);
        assert!(
            result.is_ok(),
            "labels referenced from match branches should be registered: {:?}",
            result.err()
        );
        assert!(analyzer.symbols.resolve("CaseZero").is_some());
        assert!(analyzer.symbols.resolve("CaseDefault").is_some());
    }

    #[test]
    fn test_autovar_call_rejects_explicit_result_argument() {
        let db = DatabaseV2::load("src/db/platinum_v2.json").unwrap();
        let constants = ConstantDb::new();
        let mut analyzer = Analyzer::with_database(&constants, &db);

        let source = r"
function TestFunc #1:
    if ShowYesNoMenu(0x800C) then
        Message 1
    endif
    End
";
        let lexer = crate::compiler::Lexer::new(source);
        let mut parser = crate::compiler::Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let result = analyzer.analyze(&script_file);
        assert!(
            result.is_err(),
            "explicit autovar destination should be rejected in conditions"
        );
        let err = format!("{:?}", result.unwrap_err());
        assert!(err.contains("do not specify the result variable"));
    }

    #[test]
    fn test_match_with_named_variable_alias_is_valid() {
        use crate::compiler::lexer::Lexer;
        use crate::compiler::parser::Parser;
        use crate::database::DatabaseV2;
        use std::path::Path;

        let source = r"
alias 0x8000 as VAR_TEST

function Test #1:
    match VAR_TEST with
        case 0:
            End
    endmatch
";
        let db = DatabaseV2::load(Path::new("src/db/platinum_v2.json")).unwrap();
        let mut constants = ConstantDb::new();
        constants.load_from_db(&db);

        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::with_database(&constants, &db);
        let result = analyzer.analyze(&script_file);
        assert!(
            result.is_ok(),
            "named variable aliases should be accepted as match subjects: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_validate_number_for_type_rejects_negative_u32_like_params() {
        let result =
            Analyzer::validate_number_for_type(-1, &ParamType::Label, &(0..1), "Jump", "target", 0);
        assert!(
            result.is_err(),
            "negative values must not fit u32-like params"
        );
        let err_msg = format!("{:?}", result.err());
        assert!(
            err_msg.contains("out of range"),
            "error should mention out of range: {}",
            err_msg
        );
    }
}
