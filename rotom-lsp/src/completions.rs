use std::sync::Arc;
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, DocumentSymbol, Position as LspPosition, SymbolKind,
};

use rotom::compiler::lexer::is_identifier_char;
use rotom::compiler::sourcemap::{Position as SourcePosition, SourceMap};
use rotom::database::{DatabaseV2, ParamType};

use crate::signature_help::extract_command_context;

/// Produce LSP completion items for the given document position.
///
/// Context-aware completion:
/// - When typing a command name: suggests commands from the database.
/// - When typing a non-label parameter: suggests constants.
/// - When typing a label parameter (e.g. `Jump`, `Call`): suggests local
///   symbols (scripts, labels, actions) only.
pub fn compute_completions(
    source: &str,
    position: LspPosition,
    db: Option<&DatabaseV2>,
    constant_names: Option<Arc<Vec<String>>>,
    local_symbols: Option<&[DocumentSymbol]>,
) -> Vec<CompletionItem> {
    let map = SourceMap::new(source);
    let byte_offset = map.position_to_byte(SourcePosition {
        line: position.line,
        character: position.character,
    });

    // Skip completions inside comments or string literals.
    if is_in_comment(source, byte_offset) || is_in_string(source, byte_offset) {
        return vec![];
    }

    let prefix = extract_prefix(source, byte_offset);

    // Detect if we're typing a command parameter and which one.
    // Returns:
    // - Some((true, _))  = label-type param → show local symbols
    // - Some((false, true)) = MsgId param → show format() + constants
    // - Some((false, false)) = other param → show constants only
    // - None = not in param context → show commands + constants
    let param_context = db.and_then(|db| {
        let (command_name, param_index) = extract_command_context(source, byte_offset)?;
        let cmd = db.get_command(&command_name).ok()?;
        if cmd.params.is_empty() {
            return None;
        }
        let param = cmd.params.get(param_index as usize)?;
        let is_label = param.param_type == ParamType::Label || param.name == "relative_jump";
        let is_msg = param.name == "text_slot";
        Some((is_label, is_msg))
    });

    // Check if we're typing the first word of a statement (start of line).
    let line_start = source[..byte_offset].rfind('\n').map_or(0, |i| i + 1);
    let line_before_prefix = &source[line_start..(byte_offset - prefix.len())];
    let is_first_word = line_before_prefix.trim().is_empty();

    let mut items: Vec<CompletionItem> = Vec::new();

    match param_context {
        // Label parameter context: only suggest local symbols (flattened from groups).
        Some((true, _)) => {
            if let Some(local_symbols) = local_symbols {
                for group in local_symbols {
                    if let Some(children) = &group.children {
                        for sym in children {
                            if matches_prefix(&sym.name, &prefix) {
                                items.push(CompletionItem {
                                    label: sym.name.clone(),
                                    kind: Some(symbol_kind_to_completion_kind(sym.kind)),
                                    ..Default::default()
                                });
                            }
                        }
                    }
                }
            }
        }
        // Non-label parameter: suggest constants, boolean literals, and format() for MsgId params.
        Some((false, is_msg)) => {
            if is_msg && matches_prefix("format", &prefix) {
                items.push(CompletionItem {
                    label: "format".to_string(),
                    kind: Some(CompletionItemKind::FUNCTION),
                    detail: Some("format(string)".to_string()),
                    ..Default::default()
                });
            }
            for &kw in &["true", "false"] {
                if matches_prefix(kw, &prefix) {
                    items.push(CompletionItem {
                        label: kw.to_string(),
                        kind: Some(CompletionItemKind::KEYWORD),
                        ..Default::default()
                    });
                }
            }
            if let Some(constant_names) = constant_names {
                for name in constant_names.iter() {
                    if matches_prefix(name, &prefix) && is_identifier_name(name) {
                        items.push(CompletionItem {
                            label: name.clone(),
                            kind: Some(CompletionItemKind::CONSTANT),
                            ..Default::default()
                        });
                    }
                }
            }
        }
        // Not in parameter context (typing a command name).
        None => {
            if let Some(db) = db {
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
                        && legacy != name
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

            // Only suggest constants and boolean literals when not typing the first word.
            if !is_first_word {
                for &kw in &["true", "false"] {
                    if matches_prefix(kw, &prefix) {
                        items.push(CompletionItem {
                            label: kw.to_string(),
                            kind: Some(CompletionItemKind::KEYWORD),
                            ..Default::default()
                        });
                    }
                }
                if let Some(constant_names) = constant_names {
                    for name in constant_names.iter() {
                        if matches_prefix(name, &prefix) && is_identifier_name(name) {
                            items.push(CompletionItem {
                                label: name.clone(),
                                kind: Some(CompletionItemKind::CONSTANT),
                                ..Default::default()
                            });
                        }
                    }
                }
            }
        }
    }

    items
}

/// True if `name` is a valid rotom identifier (starts with a letter or
/// underscore, contains only identifier characters). Filters out junk from
/// malformed C preprocessor parsing (e.g. function macro names with params).
fn is_identifier_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(is_identifier_char)
}

/// Return true if the cursor is inside a string literal.
fn is_in_string(source: &str, byte_offset: usize) -> bool {
    let before = &source[..byte_offset];
    let mut in_string = false;
    let mut chars = before.chars().peekable();
    while let Some(c) = chars.next() {
        if in_string {
            if c == '\\' {
                chars.next(); // skip escaped character
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
        }
    }
    in_string
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

/// Extract the identifier prefix at the given byte offset.
fn extract_prefix(source: &str, byte_offset: usize) -> String {
    let before = &source[..byte_offset.min(source.len())];
    let start = before
        .rfind(|c: char| !rotom::compiler::lexer::is_identifier_char(c))
        .map_or(0, |i| {
            i + before[i..].chars().next().map_or(1, char::len_utf8)
        });
    before[start..].to_string()
}

fn matches_prefix(name: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }

    name.get(..prefix.len())
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix))
}

fn symbol_kind_to_completion_kind(kind: SymbolKind) -> CompletionItemKind {
    match kind {
        SymbolKind::FUNCTION | SymbolKind::METHOD => CompletionItemKind::FUNCTION,
        SymbolKind::VARIABLE => CompletionItemKind::VARIABLE,
        SymbolKind::PROPERTY | SymbolKind::KEY => CompletionItemKind::REFERENCE,
        _ => CompletionItemKind::TEXT,
    }
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
    use std::sync::Arc;

    fn test_db() -> &'static DatabaseV2 {
        DatabaseV2::test_platinum()
    }

    fn pos_at(source: &str, needle: &str) -> LspPosition {
        let offset = source.find(needle).expect("needle not found") + needle.len();
        let pos = SourceMap::new(source).byte_to_position(offset);
        LspPosition {
            line: pos.line,
            character: pos.character,
        }
    }

    #[test]
    fn test_extract_prefix_basic() {
        let source = "    Mess";
        let pos = LspPosition {
            line: 0,
            character: 8,
        };
        let map = SourceMap::new(source);
        let byte = map.position_to_byte(SourcePosition {
            line: pos.line,
            character: pos.character,
        });
        assert_eq!(extract_prefix(source, byte), "Mess");
    }

    #[test]
    fn test_extract_prefix_empty() {
        let source = "    ";
        let pos = LspPosition {
            line: 0,
            character: 4,
        };
        let map = SourceMap::new(source);
        let byte = map.position_to_byte(SourcePosition {
            line: pos.line,
            character: pos.character,
        });
        assert_eq!(extract_prefix(source, byte), "");
    }

    #[test]
    fn test_matches_prefix_case_insensitive() {
        assert!(matches_prefix("Message", "mess"));
        assert!(matches_prefix("message", "Mess"));
        assert!(!matches_prefix("ApplyMovement", "mess"));
    }

    #[test]
    fn matches_prefix_is_case_insensitive() {
        assert!(matches_prefix("Message", "mes"));
        assert!(matches_prefix("CheckPlayerOnBike", "checkplayer"));
        assert!(matches_prefix("VAR_RESULT", "var_"));
    }

    #[test]
    fn matches_prefix_rejects_longer_or_different_prefixes() {
        assert!(!matches_prefix("Msg", "Message"));
        assert!(!matches_prefix("Message", "Wait"));
    }

    #[test]
    fn matches_prefix_accepts_empty_prefix() {
        assert!(matches_prefix("Message", ""));
    }

    #[test]
    fn test_is_in_comment_line_comment() {
        let source = "script Test #1:\n    // Message 1\n    End\n";
        let map = SourceMap::new(source);
        let byte = map.position_to_byte(SourcePosition {
            line: 1,
            character: 10,
        });
        assert!(is_in_comment(source, byte));
        let byte = map.position_to_byte(SourcePosition {
            line: 1,
            character: 3,
        });
        assert!(!is_in_comment(source, byte));
    }

    #[test]
    fn test_is_in_comment_block_comment() {
        let source = "script Test #1:\n    /* block\n    comment */ Message 1\n    End\n";
        let map = SourceMap::new(source);
        let byte = map.position_to_byte(SourcePosition {
            line: 1,
            character: 10,
        });
        assert!(is_in_comment(source, byte));
        let byte = map.position_to_byte(SourcePosition {
            line: 2,
            character: 5,
        });
        assert!(is_in_comment(source, byte));
        let byte = map.position_to_byte(SourcePosition {
            line: 2,
            character: 18,
        });
        assert!(!is_in_comment(source, byte));
    }

    #[test]
    fn compute_completions_suggests_command_names() {
        let source = "    Mes";

        let items = compute_completions(source, pos_at(source, "Mes"), Some(test_db()), None, None);

        assert!(items.iter().any(|item| item.label == "Message"));
        assert!(
            items
                .iter()
                .all(|item| item.kind == Some(CompletionItemKind::FUNCTION))
        );
    }

    #[test]
    fn compute_completions_suggests_constants_for_parameters() {
        let source = "script Test #1:\n    Message MSG";
        let constants = Arc::new(vec![
            "MSG_HELLO".to_string(),
            "1INVALID".to_string(),
            "OTHER".to_string(),
        ]);
        let position = LspPosition {
            line: 1,
            character: 15,
        };

        let items = compute_completions(source, position, Some(test_db()), Some(constants), None);

        assert!(items.iter().any(|item| item.label == "MSG_HELLO"));
        assert!(!items.iter().any(|item| item.label == "1INVALID"));
        assert!(!items.iter().any(|item| item.label == "OTHER"));
    }

    #[test]
    fn compute_completions_suppresses_strings_and_comments() {
        let constants = Arc::new(vec!["MSG_HELLO".to_string()]);

        assert!(
            compute_completions(
                "script Test #1:\n    Message \"MSG",
                LspPosition {
                    line: 1,
                    character: 16,
                },
                Some(test_db()),
                Some(constants.clone()),
                None,
            )
            .is_empty()
        );
        assert!(
            compute_completions(
                "script Test #1:\n    // Mes",
                LspPosition {
                    line: 1,
                    character: 10,
                },
                Some(test_db()),
                Some(constants),
                None,
            )
            .is_empty()
        );
    }

    #[test]
    fn compute_completions_suggests_local_symbols_for_label_params() {
        let source = "script Test #1:\n.start:\n    Jump .st";
        let symbols = crate::document::compute_document_symbols(source);

        let items = compute_completions(
            source,
            LspPosition {
                line: 2,
                character: 12,
            },
            Some(test_db()),
            None,
            Some(&symbols),
        );

        assert!(items.iter().any(|item| item.label == ".start"));
    }
}
