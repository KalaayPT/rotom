use dashmap::DashMap;
use tower_lsp::lsp_types::{DocumentSymbol, SymbolKind, TextDocumentContentChangeEvent, Url};

use rotom::compiler::{
    ast::StatementKind,
    lexer::Lexer,
    parser::Parser,
    sourcemap::{Position, SourceMap},
};

/// Cached document entry.
pub struct DocumentEntry {
    pub text: String,
}

/// Incrementally-synced document cache with pre-computed outline symbols.
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
        self.docs.insert(uri, DocumentEntry { text });
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
                    doc.text = change.text;
                }
            }
        }
    }
}

/// Build grouped document symbols.
///
/// Scripts, labels, and actions are bucketed into collapsible parent nodes
/// so the editor outline shows them grouped by kind.
pub fn compute_document_symbols(source: &str) -> Vec<DocumentSymbol> {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new_fallible(lexer);
    let ast = parser.parse_script_file().ok();

    let mut scripts = Vec::new();
    let mut labels = Vec::new();
    let mut actions = Vec::new();

    let Some(file) = ast else {
        return Vec::new();
    };

    let map = SourceMap::new(source);

    for item in &file.items {
        match &item.node {
            StatementKind::Function { headers, .. } => {
                for header in headers {
                    if header.is_public {
                        scripts.push(make_symbol(
                            &header.name,
                            SymbolKind::FUNCTION,
                            &item.span,
                            &map,
                        ));
                    } else {
                        labels.push(make_symbol(
                            &header.name,
                            SymbolKind::FUNCTION,
                            &item.span,
                            &map,
                        ));
                    }
                }
            }
            StatementKind::Action { name, .. } => {
                actions.push(make_symbol(name, SymbolKind::METHOD, &item.span, &map));
            }
            StatementKind::AliasStatement { name, .. } => {
                labels.push(make_symbol(name, SymbolKind::VARIABLE, &item.span, &map));
            }
            StatementKind::Label(name) => {
                labels.push(make_symbol(name, SymbolKind::PROPERTY, &item.span, &map));
            }
            _ => {}
        }
    }

    let mut groups = Vec::new();
    if !scripts.is_empty() {
        groups.push(make_group("Scripts", SymbolKind::NAMESPACE, &scripts, &map));
    }
    if !labels.is_empty() {
        groups.push(make_group("Labels", SymbolKind::NAMESPACE, &labels, &map));
    }
    if !actions.is_empty() {
        groups.push(make_group("Actions", SymbolKind::NAMESPACE, &actions, &map));
    }
    groups
}

fn make_group(
    name: &str,
    kind: SymbolKind,
    children: &[DocumentSymbol],
    _map: &SourceMap,
) -> DocumentSymbol {
    // Use the first child's range as the group's range, or a zero range if empty.
    let (range, selection_range) = children.first().map_or_else(
        || {
            let zero = tower_lsp::lsp_types::Range {
                start: tower_lsp::lsp_types::Position {
                    line: 0,
                    character: 0,
                },
                end: tower_lsp::lsp_types::Position {
                    line: 0,
                    character: 0,
                },
            };
            (zero, zero)
        },
        |c| (c.range, c.selection_range),
    );

    #[allow(deprecated)]
    DocumentSymbol {
        name: name.to_string(),
        detail: None,
        kind,
        tags: None,
        deprecated: None,
        range,
        selection_range,
        children: Some(children.to_vec()),
    }
}

fn make_symbol(
    name: &str,
    kind: SymbolKind,
    span: &std::ops::Range<usize>,
    map: &SourceMap,
) -> DocumentSymbol {
    let start = map.byte_to_position(span.start);
    let end = map.byte_to_position(span.end);
    let range = tower_lsp::lsp_types::Range {
        start: tower_lsp::lsp_types::Position {
            line: start.line,
            character: start.character,
        },
        end: tower_lsp::lsp_types::Position {
            line: end.line,
            character: end.character,
        },
    };

    #[allow(deprecated)]
    DocumentSymbol {
        name: name.to_string(),
        detail: None,
        kind,
        tags: None,
        deprecated: None,
        range,
        selection_range: range,
        children: None,
    }
}
