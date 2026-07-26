use tower_lsp::lsp_types::{Location, Position as LspPosition, Range, Url};

use rotom::compiler::{ast::ScriptFile, lexer::Lexer, parser::Parser, sourcemap::SourceMap};

/// Parse `source` with the error-tolerant parser.
///
/// Returns `None` only if parsing panics or produces no AST at all.
pub fn parse_source(source: &str) -> Option<ScriptFile> {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new_fallible(lexer);
    parser.parse_script_file().ok()
}

/// Convert a byte span to an LSP `Range`.
pub fn byte_span_to_range(map: &SourceMap, span: &std::ops::Range<usize>) -> Range {
    let start = map.byte_to_position(span.start);
    let end = map.byte_to_position(span.end);
    Range {
        start: LspPosition {
            line: start.line,
            character: start.character,
        },
        end: LspPosition {
            line: end.line,
            character: end.character,
        },
    }
}

/// Convert a byte span to an LSP `Location`.
pub fn byte_span_to_location(
    uri: &Url,
    span: &std::ops::Range<usize>,
    map: &SourceMap,
) -> Location {
    Location {
        uri: uri.clone(),
        range: byte_span_to_range(map, span),
    }
}

#[cfg(test)]
pub(crate) fn test_project_context(
    workspace: uxie::Workspace,
    db_path: &std::path::Path,
    family: rotom::GameFamily,
    constants: rotom::ConstantDb,
) -> rotom::ProjectContext {
    use std::sync::Arc;

    use rotom::project::config::{
        DatabaseConfig, PathsConfig, ProjectMetadata, ProjectTypeConfig, RotomConfig,
        WorkspaceConfig,
    };

    let config = RotomConfig {
        format_version: 1,
        project: ProjectMetadata {
            name: "test".to_string(),
        },
        workspace: WorkspaceConfig {
            project_type: ProjectTypeConfig::Dspre,
            game_family: Some(family),
        },
        paths: PathsConfig {
            database_dir: ".rotom/command_database".to_string(),
            cache_dir: ".rotom/cache".to_string(),
            status_dir: ".rotom/status".to_string(),
            source_roots: Vec::new(),
            include_roots: Vec::new(),
            binary_roots: Vec::new(),
        },
        database: Some(DatabaseConfig {
            default_file: db_path.display().to_string(),
        }),
    };
    rotom::ProjectContext::from_parts(
        workspace.project_path.clone(),
        config,
        Arc::new(rotom::DatabaseV2::load(db_path).expect("failed to load test database")),
        constants,
        Some(Arc::new(workspace)),
    )
}
