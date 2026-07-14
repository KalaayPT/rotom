use std::sync::Arc;

use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity};

use rotom::compiler::{
    Analyzer,
    ast::ScriptFile,
    diagnostic::{CompileError, CompileWarning},
    lexer::Lexer,
    parser::Parser,
    sourcemap::SourceMap,
};
use rotom::database::{ConstantDb, DatabaseV2};

use crate::util::byte_span_to_range;

/// Produce LSP diagnostics for a Rotom source document.
///
/// Uses the error-tolerant parser so incomplete code still yields partial
/// diagnostics rather than a single fatal error. When a database is provided,
/// also runs semantic analysis for unknown commands, undefined symbols, etc.
///
/// When `reuse_directive_parse` is `Some`, uses that AST and recoverable parse errors from an
/// earlier `.rotom` `#include` / `#define` pass instead of parsing again.
pub fn compute_diagnostics(
    source: &str,
    db: Option<&DatabaseV2>,
    constants: Option<&ConstantDb>,
    reuse_directive_parse: Option<(Arc<ScriptFile>, Vec<CompileError>)>,
) -> Vec<Diagnostic> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        compute_diagnostics_inner(source, db, constants, reuse_directive_parse)
    }));

    if let Ok(diagnostics) = result {
        diagnostics
    } else {
        eprintln!("[rotom-lsp] compute_diagnostics panicked");
        vec![internal_error_diagnostic("diagnostic computation panicked")]
    }
}

fn internal_error_diagnostic(reason: &str) -> Diagnostic {
    Diagnostic {
        range: tower_lsp::lsp_types::Range {
            start: tower_lsp::lsp_types::Position::new(0, 0),
            end: tower_lsp::lsp_types::Position::new(0, 0),
        },
        severity: Some(DiagnosticSeverity::ERROR),
        code: None,
        code_description: None,
        source: Some("rotom".to_string()),
        message: format!("rotom-lsp internal error: {reason}"),
        related_information: None,
        tags: None,
        data: None,
    }
}

fn compute_diagnostics_inner(
    source: &str,
    db: Option<&DatabaseV2>,
    constants: Option<&ConstantDb>,
    reuse_directive_parse: Option<(Arc<ScriptFile>, Vec<CompileError>)>,
) -> Vec<Diagnostic> {
    let (ast_arc, mut errors) = if let Some((arc, errs)) = reuse_directive_parse {
        (Some(arc), errs)
    } else {
        let lexer = Lexer::new(source);
        let mut parser = Parser::new_fallible(lexer);
        let script_opt = parser.parse_script_file().ok();
        let errs = std::mem::take(&mut parser.errors);
        (script_opt.map(Arc::new), errs)
    };

    let ast = ast_arc.as_deref();

    // Run semantic analysis if we have a parsed AST and a database.
    let mut warnings = Vec::new();
    if let (Some(ast), Some(db)) = (ast, db) {
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
        CompileError::Lowering {
            span: Some(span),
            message,
        } => (span.clone(), message.clone(), DiagnosticSeverity::ERROR),
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
        let diagnostics = compute_diagnostics(source, None, None, None);
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
        let diagnostics = compute_diagnostics(source, None, None, None);
        assert!(
            diagnostics.is_empty(),
            "valid source should have no diagnostics"
        );
    }

    #[test]
    fn internal_error_diagnostic_is_visible_to_editor() {
        let diagnostic = internal_error_diagnostic("diagnostic computation panicked");

        assert_eq!(diagnostic.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(diagnostic.source.as_deref(), Some("rotom"));
        assert!(diagnostic.message.contains("internal error"));
        assert_eq!(diagnostic.range.start.line, 0);
        assert_eq!(diagnostic.range.start.character, 0);
    }

    #[test]
    fn test_multiple_errors_recovery() {
        let source = r#"script
    End
script
    End
"#;
        let diagnostics = compute_diagnostics(source, None, None, None);
        // Both malformed script headers should produce diagnostics
        assert!(
            diagnostics.len() >= 2,
            "expected at least two diagnostics, got {}",
            diagnostics.len()
        );
    }

    #[test]
    fn semantic_analysis_reports_errors_and_warnings() {
        let db = DatabaseV2::test_platinum();
        let constants = ConstantDb::new();

        let warning_diagnostics = compute_diagnostics(
            "alias 1 as UNUSED\nscript Test #1:\n    Message 0\n    End\n",
            Some(db),
            Some(&constants),
            None,
        );
        assert!(
            warning_diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == Some(DiagnosticSeverity::WARNING))
        );

        let error_diagnostics = compute_diagnostics(
            "script Test #1:\n    SetFlag\n    End\n",
            Some(db),
            Some(&constants),
            None,
        );
        assert!(
            error_diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == Some(DiagnosticSeverity::ERROR))
        );
    }

    #[test]
    fn menu_builder_is_understood_by_diagnostics() {
        let source = r#"script Main #1:
    Menu(
        "Option" -> Handle,
    )
    .cancel("Cancel" -> Cancel)
    End

Handle:
    End
Cancel:
    End
"#;
        let diagnostics = compute_diagnostics(
            source,
            Some(DatabaseV2::test_platinum()),
            Some(&ConstantDb::new()),
            None,
        );
        assert!(
            diagnostics.is_empty(),
            "valid menu builder should not produce diagnostics: {diagnostics:?}"
        );
    }

    #[test]
    fn menu_builder_layout_warnings_are_reported() {
        let source = r#"script Main #1:
    Menu((10, 11) -> Handle).scrollable(false)
    Menu(
        0 -> Handle,
        1 -> Handle,
        2 -> Handle,
        3 -> Handle,
        4 -> Handle,
        5 -> Handle,
        6 -> Handle,
        7 -> Handle,
        8 -> Handle,
    ).prompt(1)
    End

Handle:
    End
"#;
        let diagnostics = compute_diagnostics(
            source,
            Some(DatabaseV2::test_platinum()),
            Some(&ConstantDb::new()),
            None,
        );
        let warnings: Vec<_> = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Some(DiagnosticSeverity::WARNING))
            .map(|diagnostic| diagnostic.message.as_str())
            .collect();

        assert_eq!(warnings.len(), 2, "{diagnostics:?}");
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("scrollable(false)"))
        );
        assert!(warnings.iter().any(|warning| warning.contains("at most 8")));
    }

    #[test]
    fn menu_builder_diagnostics_follow_game_family() {
        let valid_hgss = r#"script Main #1:
    MenuGlobal(
        (10, 20) -> Handle,
    )
    End

Handle:
    End
"#;
        let constants = ConstantDb::new();
        let diagnostics = compute_diagnostics(
            valid_hgss,
            Some(DatabaseV2::test_hgss()),
            Some(&constants),
            None,
        );
        assert!(diagnostics.is_empty(), "{diagnostics:?}");

        let invalid_hgss = valid_hgss.replace(")\n    End", ").width(8)\n    End");
        let diagnostics = compute_diagnostics(
            &invalid_hgss,
            Some(DatabaseV2::test_hgss()),
            Some(&constants),
            None,
        );
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("only supported on Platinum"))
        );
    }
}
