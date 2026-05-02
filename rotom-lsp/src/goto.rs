use tower_lsp::lsp_types::{
    GotoDefinitionResponse, Location, Position as LspPosition, Range, Url,
};

use rotom::compiler::{
    ast::{Statement, StatementKind},
    lexer::Lexer,
    parser::Parser,
    sourcemap::{Position as SourcePosition, SourceMap},
};

use crate::hover::extract_word;

/// Produce an LSP go-to-definition response for the symbol under the cursor.
///
/// Searches recursively through all statement blocks so that inline labels
/// and aliases inside functions, if-branches, while-loops, and match cases
/// are found as well as top-level definitions.
pub fn compute_goto_definition(
    source: &str,
    position: LspPosition,
    uri: &Url,
) -> Option<GotoDefinitionResponse> {
    let map = SourceMap::new(source);
    let byte_offset = map.position_to_byte(SourcePosition {
        line: position.line,
        character: position.character,
    });

    let word = extract_word(source, byte_offset)?;

    let lexer = Lexer::new(source);
    let mut parser = Parser::new_fallible(lexer);
    let ast = parser.parse_script_file().ok()?;

    let stmt = find_definition(&ast.items, &word)?;
    Some(GotoDefinitionResponse::Scalar(make_location(
        uri, &stmt.span, &map,
    )))
}

/// Recursively walk statements looking for a definition whose name matches `word`.
fn find_definition<'a>(items: &'a [Statement], word: &str) -> Option<&'a Statement> {
    for item in items {
        match &item.node {
            StatementKind::Function { headers, body, .. } => {
                if headers.iter().any(|h| h.name == word) {
                    return Some(item);
                }
                if let Some(stmt) = find_definition(body, word) {
                    return Some(stmt);
                }
            }
            StatementKind::Action { name, body, .. } => {
                if *name == word {
                    return Some(item);
                }
                if let Some(stmt) = find_definition(body, word) {
                    return Some(stmt);
                }
            }
            StatementKind::AliasStatement { name, .. } | StatementKind::Label(name)
                if *name == word =>
            {
                return Some(item);
            }
            StatementKind::IfStatement { body, elseblock, .. } => {
                if let Some(stmt) = find_definition(body, word) {
                    return Some(stmt);
                }
                if let Some(else_b) = elseblock
                    && let Some(stmt) = find_definition(else_b, word)
                {
                    return Some(stmt);
                }
            }
            StatementKind::WhileStatement { body, .. } => {
                if let Some(stmt) = find_definition(body, word) {
                    return Some(stmt);
                }
            }
            StatementKind::MatchStatement { cases, default, .. } => {
                for case in cases {
                    if let Some(stmt) = find_definition(&case.body, word) {
                        return Some(stmt);
                    }
                }
                if let Some(default) = default
                    && let Some(stmt) = find_definition(default, word)
                {
                    return Some(stmt);
                }
            }
            _ => {}
        }
    }
    None
}

fn make_location(uri: &Url, span: &std::ops::Range<usize>, map: &SourceMap) -> Location {
    let start = map.byte_to_position(span.start);
    let end = map.byte_to_position(span.end);
    Location {
        uri: uri.clone(),
        range: Range {
            start: LspPosition {
                line: start.line,
                character: start.character,
            },
            end: LspPosition {
                line: end.line,
                character: end.character,
            },
        },
    }
}
