use std::{collections::HashMap, ops::Range};

use crate::database::{Command, ComparisonOperator, ConstantDb, DatabaseV2, ParamType};

use super::{
    ast::{Expression, ExpressionKind, ScriptFile, Statement, StatementKind},
    parse_error::{CompileError, ParseResult, analysis_error},
};

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
        for alias in &file.aliases {
            self.register_global_alias(alias)?;
        }
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
    fn register_global_alias(&mut self, stmt: &Statement) -> ParseResult<()> {
        if let StatementKind::AliasStatement { name, id, .. } = &stmt.node {
            self.symbols.define_global(
                name.clone(),
                SymbolType::Variable(*id),
                stmt.span.clone(),
            )?;
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
    fn validate_action_body(&mut self, action: &Statement) -> ParseResult<()> {
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
        self.database?.commands.get(name)
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

        let actual_count = args.len();
        let is_macro = cmd.is_macro();

        if let Some(variants) = &cmd.variants {
            let valid_counts: Vec<usize> = variants.iter().map(|v| v.params.len()).collect();
            if !valid_counts.contains(&actual_count) && !cmd.params.is_empty() {
                let params = &cmd.params;
                let required_count = params
                    .iter()
                    .filter(|p| !p.optional && p.default.is_none())
                    .count();
                let max_count = params.len();

                if (actual_count < required_count || actual_count > max_count)
                    && !valid_counts.iter().any(|&vc| actual_count <= vc)
                {
                    return Ok(());
                }
            }
            return Ok(());
        }

        let params = &cmd.params;
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
                self.validate_autovar_call(function, args, &expr.span)?;
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

    #[test]
    fn test_analyzer_registers_global_alias() {
        let mut analyzer = Analyzer::new();
        let alias_stmt = Statement {
            node: StatementKind::AliasStatement {
                is_global: true,
                id: 1,
                name: "global_var".to_string(),
            },
            span: 0..10,
        };
        analyzer
            .register_global_alias(&alias_stmt)
            .expect("Failed to register global alias");
        match analyzer.symbols.resolve("global_var") {
            Some(SymbolType::Variable(id)) => assert_eq!(*id, 1),
            _ => panic!("Global alias not found in symbol table"),
        }
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
    fn test_duplicate_symbol_error() {
        use crate::compiler::lexer::Lexer;
        use crate::compiler::parser::Parser;

        let source = r"
alias 0x8000 as VAR_X
alias 0x8000 as VAR_X

function Dummy #0:
    End
";
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let script_file = parser.parse_script_file().unwrap();

        let mut analyzer = Analyzer::new();
        let result = analyzer.analyze(&script_file);
        assert!(result.is_err(), "Duplicate alias should cause error");
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
}
