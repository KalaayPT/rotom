use std::sync::Arc;

use tower_lsp::lsp_types::{GotoDefinitionResponse, Location, Position as LspPosition, Range, Url};

use rotom::compiler::{
    ast::{Statement, StatementKind},
    sourcemap::{Position as SourcePosition, SourceMap},
};

use crate::hover::extract_word;
use crate::util::{byte_span_to_location, parse_source};

/// Produce an LSP go-to-definition response for the symbol under the cursor.
///
/// First searches the source file for local definitions (labels, functions,
/// aliases). If not found and a workspace is provided, tries to resolve the
/// word as a message ID and jumps to its entry in the JSON archive.
pub fn compute_goto_definition(
    source: &str,
    position: tower_lsp::lsp_types::Position,
    uri: &Url,
    workspace: Option<&Arc<uxie::Workspace>>,
) -> Option<GotoDefinitionResponse> {
    let map = SourceMap::new(source);
    let byte_offset = map.position_to_byte(SourcePosition {
        line: position.line,
        character: position.character,
    });

    let word = extract_word(source, byte_offset)?;
    let ast = parse_source(source)?;

    if let Some(stmt) = find_definition(&ast.items, &word) {
        return Some(GotoDefinitionResponse::Scalar(byte_span_to_location(
            uri, &stmt.span, &map,
        )));
    }

    // Try message ID lookup via the public Workspace accessor.
    if let Some(ws) = workspace {
        if let Some((archive_id, msg_index)) = ws.resolve_message_id(&word) {
            if let Some(path) = ws.cached_text_archive_path(archive_id) {
                let location = json_message_location(&path, msg_index as usize)
                    .unwrap_or_else(|| file_start_location(&path));
                return Some(GotoDefinitionResponse::Scalar(location));
            }
        }
    }

    None
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

/// Find the LSP Location of the Nth message entry in a JSON archive.
///
/// Scans the file line by line counting `"id":` occurrences; the Nth such
/// line is the start of the target message object. Falls back to `None` if
/// the file cannot be read or `msg_index` is out of range.
fn json_message_location(path: &std::path::Path, msg_index: usize) -> Option<Location> {
    let uri = Url::from_file_path(path).ok()?;
    let content = std::fs::read_to_string(path).ok()?;
    let mut count = 0usize;
    for (line_idx, line) in content.lines().enumerate() {
        if line.contains("\"id\":") {
            if count == msg_index {
                let location = Location {
                    uri,
                    range: Range {
                        start: LspPosition { line: line_idx as u32, character: 0 },
                        end: LspPosition { line: line_idx as u32, character: 0 },
                    },
                };
                return Some(location);
            }
            count += 1;
        }
    }
    None
}

/// Return a Location pointing to the very start of a file.
fn file_start_location(path: &std::path::Path) -> Location {
    Location {
        uri: Url::from_file_path(path).unwrap_or_else(|_| Url::parse("file:///").unwrap()),
        range: Range {
            start: LspPosition { line: 0, character: 0 },
            end: LspPosition { line: 0, character: 0 },
        },
    }
}
