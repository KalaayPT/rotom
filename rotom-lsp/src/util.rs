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
