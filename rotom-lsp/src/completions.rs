use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, Position};

use rotom::compiler::{
    ast::{ScriptFile, StatementKind},
    lexer::Lexer,
    parser::Parser,
};
use rotom::database::{ConstantDb, DatabaseV2};

/// Produce LSP completion items for the given document position.
///
/// Suggests commands from the database, constants, and local symbols
/// (labels, aliases, scripts, actions) scoped to the current file.
pub fn compute_completions(
    source: &str,
    position: Position,
    db: Option<&DatabaseV2>,
    constants: Option<&ConstantDb>,
) -> Vec<CompletionItem> {
    let prefix = extract_prefix(source, position);

    let mut items: Vec<CompletionItem> = Vec::new();

    // Parse the file to collect local symbols.
    let locals = collect_local_symbols(source);

    // Commands from the database.
    if let Some(db) = db {
        for name in db.commands.keys() {
            if matches_prefix(name, &prefix) {
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::FUNCTION),
                    detail: command_detail(db, name),
                    ..Default::default()
                });
            }
        }
    }

    // Constants.
    if let Some(constants) = constants {
        for name in constants.constant_names() {
            if matches_prefix(&name, &prefix) {
                items.push(CompletionItem {
                    label: name,
                    kind: Some(CompletionItemKind::CONSTANT),
                    ..Default::default()
                });
            }
        }
    }

    // Local symbols.
    for (name, kind) in locals {
        if matches_prefix(&name, &prefix) {
            items.push(CompletionItem {
                label: name,
                kind: Some(kind),
                ..Default::default()
            });
        }
    }

    items
}

/// Extract the word prefix at the given UTF-16 position.
fn extract_prefix(source: &str, position: Position) -> String {
    let line = source
        .lines()
        .nth(position.line as usize)
        .unwrap_or("");

    // Convert UTF-16 character offset to byte offset for the prefix.
    let byte_col = utf16_to_byte_offset(line, position.character);
    let before = &line[..byte_col.min(line.len())];

    // Walk back to the start of the current identifier.
    let start = before
        .rfind(|c: char| !is_identifier_char(c))
        .map(|i| i + before[i..].chars().next().map_or(1, char::len_utf8))
        .unwrap_or(0);

    before[start..].to_string()
}

fn utf16_to_byte_offset(line: &str, utf16_col: u32) -> usize {
    let mut utf16_seen = 0u32;
    for (byte_idx, ch) in line.char_indices() {
        if utf16_seen >= utf16_col {
            return byte_idx;
        }
        utf16_seen += ch.len_utf16() as u32;
    }
    line.len()
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

fn command_detail(db: &DatabaseV2, name: &str) -> Option<String> {
    db.get_command(name).ok().map(|cmd| {
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
        format!("{}({})", name, params.join(", "))
    })
}

/// Collect local symbols from the source file (labels, aliases, scripts, actions).
fn collect_local_symbols(source: &str) -> Vec<(String, CompletionItemKind)> {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new_fallible(lexer);
    let ast = parser.parse_script_file().ok();

    let mut symbols = Vec::new();
    let Some(file) = ast else {
        return symbols;
    };

    collect_from_script_file(&file, &mut symbols);
    symbols
}

fn collect_from_script_file(file: &ScriptFile, symbols: &mut Vec<(String, CompletionItemKind)>) {
    for item in &file.items {
        match &item.node {
            StatementKind::Function { headers, body, .. } => {
                for header in headers {
                    symbols.push((header.name.clone(), CompletionItemKind::FUNCTION));
                }
                collect_from_block(body, symbols);
            }
            StatementKind::Action { name, body, .. } => {
                symbols.push((name.clone(), CompletionItemKind::FUNCTION));
                collect_from_block(body, symbols);
            }
            StatementKind::AliasStatement { name, .. } => {
                symbols.push((name.clone(), CompletionItemKind::VARIABLE));
            }
            StatementKind::Label(name) => {
                symbols.push((name.clone(), CompletionItemKind::REFERENCE));
            }
            StatementKind::IfStatement { body, elseblock, .. } => {
                collect_from_block(body, symbols);
                if let Some(else_b) = elseblock {
                    collect_from_block(else_b, symbols);
                }
            }
            StatementKind::WhileStatement { body, .. } => {
                collect_from_block(body, symbols);
            }
            StatementKind::MatchStatement { cases, default, .. } => {
                for case in cases {
                    collect_from_block(&case.body, symbols);
                }
                if let Some(default) = default {
                    collect_from_block(default, symbols);
                }
            }
            _ => {}
        }
    }
}

fn collect_from_block(block: &[rotom::compiler::ast::Statement], symbols: &mut Vec<(String, CompletionItemKind)>) {
    for stmt in block {
        match &stmt.node {
            StatementKind::Label(name) => {
                symbols.push((name.clone(), CompletionItemKind::REFERENCE));
            }
            StatementKind::AliasStatement { name, .. } => {
                symbols.push((name.clone(), CompletionItemKind::VARIABLE));
            }
            StatementKind::IfStatement { body, elseblock, .. } => {
                collect_from_block(body, symbols);
                if let Some(else_b) = elseblock {
                    collect_from_block(else_b, symbols);
                }
            }
            StatementKind::WhileStatement { body, .. } => {
                collect_from_block(body, symbols);
            }
            StatementKind::MatchStatement { cases, default, .. } => {
                for case in cases {
                    collect_from_block(&case.body, symbols);
                }
                if let Some(default) = default {
                    collect_from_block(default, symbols);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_prefix_basic() {
        let line = "    Mess";
        let pos = Position { line: 0, character: 8 };
        assert_eq!(extract_prefix(line, pos), "Mess");
    }

    #[test]
    fn test_extract_prefix_empty() {
        let line = "    ";
        let pos = Position { line: 0, character: 4 };
        assert_eq!(extract_prefix(line, pos), "");
    }

    #[test]
    fn test_matches_prefix_case_insensitive() {
        assert!(matches_prefix("Message", "mess"));
        assert!(matches_prefix("message", "Mess"));
        assert!(!matches_prefix("ApplyMovement", "mess"));
    }
}
