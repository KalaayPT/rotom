use tower_lsp::lsp_types::{GotoDefinitionResponse, Url};

use rotom::compiler::{
    ast::{Statement, StatementKind},
    sourcemap::{Position as SourcePosition, SourceMap},
};

use crate::hover::extract_word;
use crate::util::{byte_span_to_location, parse_source};

/// Produce an LSP go-to-definition response for the symbol under the cursor.
///
/// Searches recursively through all statement blocks so that inline labels
/// and aliases inside functions, if-branches, while-loops, and match cases
/// are found as well as top-level definitions.
pub fn compute_goto_definition(
    source: &str,
    position: tower_lsp::lsp_types::Position,
    uri: &Url,
) -> Option<GotoDefinitionResponse> {
    let map = SourceMap::new(source);
    let byte_offset = map.position_to_byte(SourcePosition {
        line: position.line,
        character: position.character,
    });

    let word = extract_word(source, byte_offset)?;
    let ast = parse_source(source)?;

    let stmt = find_definition(&ast.items, &word)?;
    Some(GotoDefinitionResponse::Scalar(byte_span_to_location(
        uri, &stmt.span, &map,
    )))
}

/// Recursively walk statements looking for a definition whose name matches `word`.
fn find_definition<'a>(items: &'a [Statement], word: &str) -> Option<&'a Statement> {
    for item in items {
        let found = match &item.node {
            StatementKind::Function { headers, .. } => {
                headers.iter().any(|h| h.name == word)
            }
            StatementKind::Action { name, .. }
            | StatementKind::AliasStatement { name, .. }
            | StatementKind::Label(name) => *name == word,
            _ => false,
        };
        if found {
            return Some(item);
        }
        // Recurse into blocks.
        match &item.node {
            StatementKind::Function { body, .. }
            | StatementKind::Action { body, .. }
            | StatementKind::WhileStatement { body, .. } => {
                if let Some(stmt) = find_definition(body, word) {
                    return Some(stmt);
                }
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
