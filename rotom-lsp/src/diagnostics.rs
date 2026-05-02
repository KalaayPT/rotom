use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity};

use rotom::compiler::{
    diagnostic::{CompileError, CompileWarning},
    lexer::Lexer,
    parser::Parser,
    sourcemap::SourceMap,
    Analyzer,
};
use rotom::database::{ConstantDb, DatabaseV2};

use crate::util::byte_span_to_range;

/// Produce LSP diagnostics for a Rotom source document.
///
/// Uses the error-tolerant parser so incomplete code still yields partial
/// diagnostics rather than a single fatal error. When a database is provided,
/// also runs semantic analysis for unknown commands, undefined symbols, etc.
pub fn compute_diagnostics(
    source: &str,
    db: Option<&DatabaseV2>,
    constants: Option<&ConstantDb>,
) -> Vec<Diagnostic> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        compute_diagnostics_inner(source, db, constants)
    }));

    if let Ok(diagnostics) = result {
        diagnostics
    } else {
        eprintln!("[rotom-lsp] compute_diagnostics panicked, returning empty diagnostics");
        vec![]
    }
}

fn compute_diagnostics_inner(
    source: &str,
    db: Option<&DatabaseV2>,
    constants: Option<&ConstantDb>,
) -> Vec<Diagnostic> {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new_fallible(lexer);

    let ast = parser.parse_script_file().ok();
    let mut errors = std::mem::take(&mut parser.errors);

    // Run semantic analysis if we have a parsed AST and a database.
    let mut warnings = Vec::new();
    if let (Some(ast), Some(db)) = (ast.as_ref(), db) {
        let mut analyzer = if let Some(constants) = constants {
            Analyzer::with_database(constants, db)
        } else {
            Analyzer::new()
        };
        if let Err(e) = analyzer.analyze(ast) {
            errors.push(e);
        }
        warnings.clone_from(&analyzer.warnings);
    }

    let map = SourceMap::new(source);
    let mut diagnostics: Vec<Diagnostic> = errors
        .into_iter()
        .filter_map(|err| compile_error_to_diagnostic(&err, &map))
        .collect();

    for w in &warnings {
        diagnostics.push(compile_warning_to_diagnostic(w, &map));
    }

    diagnostics
}

fn compile_error_to_diagnostic(error: &CompileError, map: &SourceMap) -> Option<Diagnostic> {
    let (span, message, severity) = match error {
        CompileError::Parse { span, message } | CompileError::Analysis { span, message } => {
            (span.clone(), message.clone(), DiagnosticSeverity::ERROR)
        }
        // Other error kinds don't have source spans, so skip them for now.
        _ => return None,
    };

    Some(Diagnostic {
        range: byte_span_to_range(map, &span),
        severity: Some(severity),
        code: None,
        code_description: None,
        source: Some("rotom".to_string()),
        message,
        related_information: None,
        tags: None,
        data: None,
    })
}

fn compile_warning_to_diagnostic(warning: &CompileWarning, map: &SourceMap) -> Diagnostic {
    Diagnostic {
        range: byte_span_to_range(map, &warning.span()),
        severity: Some(DiagnosticSeverity::WARNING),
        code: None,
        code_description: None,
        source: Some("rotom".to_string()),
        message: warning.message(),
        related_information: None,
        tags: None,
        data: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_parse_error() {
        let source = "script\n"; // missing name and colon
        let diagnostics = compute_diagnostics(source, None, None);
        assert!(
            !diagnostics.is_empty(),
            "expected at least one diagnostic for malformed script header"
        );
        assert_eq!(diagnostics[0].severity, Some(DiagnosticSeverity::ERROR));
        assert!(
            diagnostics[0].message.contains("expected"),
            "diagnostic should mention what was expected: {}",
            diagnostics[0].message
        );
    }

    #[test]
    fn test_valid_source_has_no_diagnostics() {
        let source = r#"script Main #1:
    End
"#;
        let diagnostics = compute_diagnostics(source, None, None);
        assert!(diagnostics.is_empty(), "valid source should have no diagnostics");
    }

    #[test]
    fn test_multiple_errors_recovery() {
        let source = r#"script
    End
script
    End
"#;
        let diagnostics = compute_diagnostics(source, None, None);
        // Both malformed script headers should produce diagnostics
        assert!(
            diagnostics.len() >= 2,
            "expected at least two diagnostics, got {}",
            diagnostics.len()
        );
    }
}
