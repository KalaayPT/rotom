use std::{collections::HashMap, ops::Range};

use super::{
    ast::{Expression, ExpressionKind, ScriptFile, Statement, StatementKind},
    parse_error::{analysis_error, CompileError, ParseResult},
};

pub enum SymbolType {
    Function(Option<i32>),
    Action,
    Label,
    Variable(i32),
}

pub struct SymbolTable {
    scopes: Vec<HashMap<String, (SymbolType, Range<usize>)>>,
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
        if let Some((_, original_span)) = self.scopes[0].get(&name) {
            return Err(analysis_error(
                span.clone(),
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
        let current_scope = self.scopes.last_mut().unwrap();

        if let Some((_, original_span)) = current_scope.get(&name) {
            return Err(analysis_error(
                span.clone(),
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

pub struct Analyzer {
    pub symbols: SymbolTable,
}

impl Analyzer {
    pub fn new() -> Analyzer {
        Analyzer {
            symbols: SymbolTable::new(),
        }
    }
    pub fn analyze(&mut self, file: &ScriptFile) -> ParseResult<()> {
        for alias in &file.aliases {
            self.register_global_alias(alias)?;
        }
        for func in &file.functions {
            self.register_function_names(func)?;
        }
        for act in &file.actions {
            self.register_action_name(act)?;
        }
        for func in &file.functions {
            self.validate_function_body(func)?;
        }
        for act in &file.actions {
            self.validate_action_body(act)?;
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
            for header in headers {
                // Define EACH alias in the Global Scope
                self.symbols.define_global(
                    header.name.clone(),
                    SymbolType::Function(header.id),
                    func.span.clone(),
                )?;
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
                    StatementKind::End => {}
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
            StatementKind::AliasStatement {
                name,
                id,
                is_global,
            } => {
                if !*is_global {
                    self.symbols.define_scoped(
                        name.clone(),
                        SymbolType::Variable(*id),
                        stmt.span.clone(),
                    )?;
                } else {
                    return Err(analysis_error(
                        stmt.span.clone(),
                        "Global aliases have to be defined at the global level",
                    ));
                }
            }
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
                self.validate_block(body)?;
            }
            StatementKind::ScriptCommand { args, .. } => {
                for arg in args {
                    self.validate_expression(arg)?;
                }
            }
            // labels already gathered, skip
            StatementKind::Label(_) => {}
            StatementKind::Return | StatementKind::End => {}
            _ => {
                return Err(analysis_error(
                    stmt.span.clone(),
                    format!("unexpected statement: {:?}", stmt.node),
                ));
            }
        }
        Ok(())
    }
    fn validate_expression(&self, expr: &Expression) -> ParseResult<()> {
        match &expr.node {
            ExpressionKind::Identifier(name) => {
                if self.symbols.resolve(name).is_none() {
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
                self.validate_expression(function)?;
                for arg in args {
                    self.validate_expression(arg)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn validate_jump_target(&self, expr: &Expression) -> ParseResult<()> {
        let name = match &expr.node {
            ExpressionKind::Label(n) => n,      // Jump .local
            ExpressionKind::Identifier(n) => n, // Jump Global
            _ => return Ok(()), // Jump 100 is also valid in case anyone ever needs it
        };
        match self.symbols.resolve(name) {
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
