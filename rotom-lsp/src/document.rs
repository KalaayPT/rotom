use std::sync::Arc;

use dashmap::DashMap;
use tower_lsp::lsp_types::{CompletionItemKind, TextDocumentContentChangeEvent, Url};

use rotom::compiler::{
    ast::StatementKind,
    lexer::Lexer,
    parser::Parser,
    sourcemap::{Position, SourceMap},
};

/// Cached document entry: text + pre-computed local symbols.
pub struct DocumentEntry {
    pub text: String,
    pub local_symbols: Arc<Vec<(String, CompletionItemKind)>>,
}

/// Incrementally-synced document cache with pre-computed local symbols.
pub struct DocumentCache {
    docs: DashMap<Url, DocumentEntry>,
}

impl DocumentCache {
    pub fn new() -> Self {
        Self {
            docs: DashMap::new(),
        }
    }

    pub fn insert(&self, uri: Url, text: String) {
        let local_symbols = Arc::new(compute_local_symbols(&text));
        self.docs.insert(uri, DocumentEntry { text, local_symbols });
    }

    pub fn remove(&self, uri: &Url) {
        self.docs.remove(uri);
    }

    pub fn get(&self, uri: &Url) -> Option<dashmap::mapref::one::Ref<'_, Url, DocumentEntry>> {
        self.docs.get(uri)
    }

    pub fn apply_changes(&self, uri: &Url, changes: Vec<TextDocumentContentChangeEvent>) {
        if let Some(mut doc) = self.docs.get_mut(uri) {
            for change in changes {
                if let Some(range) = change.range {
                    // Incremental update using SourceMap for robust UTF-16 -> byte conversion.
                    let map = SourceMap::new(&doc.text);
                    let start = map.position_to_byte(Position {
                        line: range.start.line,
                        character: range.start.character,
                    });
                    let end = map.position_to_byte(Position {
                        line: range.end.line,
                        character: range.end.character,
                    });
                    doc.text.replace_range(start..end, &change.text);
                } else {
                    // Full document replacement
                    doc.text = change.text;
                }
            }
            // Recompute local symbols after all changes are applied.
            doc.local_symbols = Arc::new(compute_local_symbols(&doc.text));
        }
    }
}

fn compute_local_symbols(source: &str) -> Vec<(String, CompletionItemKind)> {
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

fn collect_from_script_file(
    file: &rotom::compiler::ast::ScriptFile,
    symbols: &mut Vec<(String, CompletionItemKind)>,
) {
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

fn collect_from_block(
    block: &[rotom::compiler::ast::Statement],
    symbols: &mut Vec<(String, CompletionItemKind)>,
) {
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
