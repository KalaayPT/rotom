use std::{collections::HashMap, ops::Range};

use crate::database::ConstantDb;

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

pub struct Analyzer<'a> {
    pub symbols: SymbolTable,
    constants: Option<&'a ConstantDb>,
}

impl<'a> Analyzer<'a> {
    pub fn new() -> Analyzer<'a> {
        Analyzer {
            symbols: SymbolTable::new(),
            constants: None,
        }
    }
    
    /// Create an analyzer with a constants database
    pub fn with_constants(constants: &'a ConstantDb) -> Analyzer<'a> {
        Analyzer {
            symbols: SymbolTable::new(),
            constants: Some(constants),
        }
    }
    
    pub fn analyze(&mut self, file: &ScriptFile) -> ParseResult<()> {
        // First, register all constants from the database into the symbol table
        if let Some(const_db) = self.constants {
            for (name, value) in const_db.iter() {
                let _ = self.symbols.define_global(
                    name.clone(),
                    SymbolType::Constant(*value),
                    0..0,
                );
            }
        }
        
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

#[cfg(test)]
mod tests {
    use std::fmt::Display;

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
}
