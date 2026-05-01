use std::sync::Arc;
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, Position as LspPosition};

use rotom::compiler::sourcemap::{Position as SourcePosition, SourceMap};
use rotom::database::DatabaseV2;

/// Produce LSP completion items for the given document position.
///
/// Suggests commands from the database, constants, and local symbols
/// (labels, aliases, scripts, actions) scoped to the current file.
///
/// `constant_names` and `local_symbols` are pre-computed by the caller
/// so this function does not re-parse or re-allocate on every keystroke.
pub fn compute_completions(
    source: &str,
    position: LspPosition,
    db: Option<&DatabaseV2>,
    constant_names: Option<Arc<Vec<String>>>,
    local_symbols: Option<Arc<Vec<(String, CompletionItemKind)>>>,
) -> Vec<CompletionItem> {
    let map = SourceMap::new(source);
    let byte_offset = map.position_to_byte(SourcePosition {
        line: position.line,
        character: position.character,
    });

    // Skip completions inside comments.
    if is_in_comment(source, byte_offset) {
        return vec![];
    }

    let prefix = extract_prefix(source, byte_offset);
    let in_command_params = is_typing_command_params(source, byte_offset);

    let mut items: Vec<CompletionItem> = Vec::new();

    // Commands from the database (canonical + legacy names).
    // Suppressed when we're clearly typing command parameters.
    if !in_command_params && let Some(db) = db {
        for (name, cmd) in &db.commands {
            if matches_prefix(name, &prefix) {
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::FUNCTION),
                    detail: Some(command_detail(name, cmd)),
                    ..Default::default()
                });
            }
            if let Some(legacy) = &cmd.legacy_name
                && matches_prefix(legacy, &prefix)
            {
                items.push(CompletionItem {
                    label: legacy.clone(),
                    kind: Some(CompletionItemKind::FUNCTION),
                    detail: Some(format!("legacy alias for {name}")),
                    ..Default::default()
                });
            }
        }
    }

    // Constants.
    if let Some(constant_names) = constant_names {
        for name in constant_names.iter() {
            if matches_prefix(name, &prefix) {
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::CONSTANT),
                    ..Default::default()
                });
            }
        }
    }

    // Local symbols.
    if let Some(local_symbols) = local_symbols {
        for (name, kind) in local_symbols.iter() {
            if matches_prefix(name, &prefix) {
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(*kind),
                    ..Default::default()
                });
            }
        }
    }

    items
}

/// Return true if the cursor is inside a line comment or block comment.
fn is_in_comment(source: &str, byte_offset: usize) -> bool {
    // Check for line comment on the current line.
    let line_start = source[..byte_offset].rfind('\n').map_or(0, |i| i + 1);
    let line_text = &source[line_start..byte_offset];
    if let Some(idx) = line_text.find("//") {
        // Cursor is after the // on this line.
        if line_start + idx < byte_offset {
            return true;
        }
    }

    // Check for block comment by scanning from the start.
    let mut in_block = false;
    for (i, _) in source[..byte_offset].char_indices() {
        if in_block {
            if source[i..].starts_with("*/") {
                in_block = false;
            }
        } else if source[i..].starts_with("/*") {
            in_block = true;
        }
    }

    in_block
}

/// Return true if the cursor is inside a command's argument list.
///
/// Detects both space-separated (`Message 1`) and call-style
/// (`GiveItem(1)`) notation.
fn is_typing_command_params(source: &str, byte_offset: usize) -> bool {
    let before_cursor = &source[..byte_offset.min(source.len())];
    let last_line_start = before_cursor.rfind('\n').map_or(0, |i| i + 1);
    let before_cursor_on_line = &before_cursor[last_line_start..];

    // Space-separated: `CommandName arg` — command name followed by whitespace.
    let trimmed = before_cursor_on_line.trim_start();
    let has_command = !trimmed.is_empty()
        && trimmed
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_');
    let ends_with_space = before_cursor_on_line.ends_with(' ');

    if has_command && ends_with_space {
        return true;
    }

    // Call-style: `CommandName(` — count unclosed parentheses.
    let open = before_cursor_on_line.matches('(').count();
    let close = before_cursor_on_line.matches(')').count();
    open > close
}

/// Extract the identifier prefix at the given byte offset.
fn extract_prefix(source: &str, byte_offset: usize) -> String {
    let before = &source[..byte_offset.min(source.len())];
    let start = before
        .rfind(|c: char| !is_identifier_char(c))
        .map_or(0, |i| i + before[i..].chars().next().map_or(1, char::len_utf8));
    before[start..].to_string()
}

fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.' || c == '?'
}

fn matches_prefix(name: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    name.to_lowercase().starts_with(&prefix.to_lowercase())
}

fn command_detail(name: &str, cmd: &rotom::database::Command) -> String {
    let params: Vec<String> = cmd
        .params
        .iter()
        .map(|p| {
            if p.optional {
                format!("[{}]", p.name)
            } else {
                p.name.clone()
            }
        })
        .collect();
    format!("{name}({})", params.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_prefix_basic() {
        let source = "    Mess";
        let pos = LspPosition { line: 0, character: 8 };
        let map = SourceMap::new(source);
        let byte = map.position_to_byte(SourcePosition { line: pos.line, character: pos.character });
        assert_eq!(extract_prefix(source, byte), "Mess");
    }

    #[test]
    fn test_extract_prefix_empty() {
        let source = "    ";
        let pos = LspPosition { line: 0, character: 4 };
        let map = SourceMap::new(source);
        let byte = map.position_to_byte(SourcePosition { line: pos.line, character: pos.character });
        assert_eq!(extract_prefix(source, byte), "");
    }

    #[test]
    fn test_matches_prefix_case_insensitive() {
        assert!(matches_prefix("Message", "mess"));
        assert!(matches_prefix("message", "Mess"));
        assert!(!matches_prefix("ApplyMovement", "mess"));
    }

    #[test]
    fn test_is_in_comment_line_comment() {
        let source = "script Test #1:\n    // Message 1\n    End\n";
        let map = SourceMap::new(source);
        let byte = map.position_to_byte(SourcePosition { line: 1, character: 10 });
        assert!(is_in_comment(source, byte));
        let byte = map.position_to_byte(SourcePosition { line: 1, character: 3 });
        assert!(!is_in_comment(source, byte));
    }

    #[test]
    fn test_is_in_comment_block_comment() {
        let source = "script Test #1:\n    /* block\n    comment */ Message 1\n    End\n";
        let map = SourceMap::new(source);
        let byte = map.position_to_byte(SourcePosition { line: 1, character: 10 });
        assert!(is_in_comment(source, byte));
        let byte = map.position_to_byte(SourcePosition { line: 2, character: 5 });
        assert!(is_in_comment(source, byte));
        let byte = map.position_to_byte(SourcePosition { line: 2, character: 18 });
        assert!(!is_in_comment(source, byte));
    }

    #[test]
    fn test_is_typing_command_params_detects_param_context() {
        let source = "script Test #1:\n    GiveItem \n";
        let map = SourceMap::new(source);
        // Cursor right after the space following the command name.
        let byte = map.position_to_byte(SourcePosition { line: 1, character: 13 });
        assert!(is_typing_command_params(source, byte));
        // Cursor still inside the command name.
        let byte = map.position_to_byte(SourcePosition { line: 1, character: 10 });
        assert!(!is_typing_command_params(source, byte));
    }

    #[test]
    fn test_is_typing_command_params_detects_call_style() {
        let source = "script Test #1:\n    GiveItem(\n";
        let map = SourceMap::new(source);
        let byte = map.position_to_byte(SourcePosition { line: 1, character: 13 });
        assert!(is_typing_command_params(source, byte));
    }
}
