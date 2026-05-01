use dashmap::DashMap;
use tower_lsp::lsp_types::{DocumentSymbol, SymbolKind, TextDocumentContentChangeEvent, Url};

use rotom::compiler::{
    ast::StatementKind,
    lexer::Lexer,
    parser::Parser,
    sourcemap::{Position, SourceMap},
};

/// Cached document entry: text + pre-computed outline symbols.
pub struct DocumentEntry {
    pub text: String,
    pub symbols: Vec<DocumentSymbol>,
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
        let symbols = compute_document_symbols(&text);
        self.docs.insert(uri, DocumentEntry { text, symbols });
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
            doc.symbols = compute_document_symbols(&doc.text);
        }
    }
}

/// Build a flat list of document symbols.
///
/// Rotom files don't have a real hierarchy — scripts, labels, and actions
/// appear in whatever order the author wrote them. We emit them as a flat
/// list so that Scintilla bridges and editor outlines show them in source
/// order.
fn compute_document_symbols(source: &str) -> Vec<DocumentSymbol> {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new_fallible(lexer);
    let ast = parser.parse_script_file().ok();

    let mut symbols = Vec::new();
    let Some(file) = ast else {
        return symbols;
    };

    let map = SourceMap::new(source);

    for item in &file.items {
        match &item.node {
            StatementKind::Function { headers, .. } => {
                for header in headers {
                    symbols.push(make_symbol(&header.name, SymbolKind::FUNCTION, &item.span, &map));
                }
            }
            StatementKind::Action { name, .. } => {
                symbols.push(make_symbol(name, SymbolKind::METHOD, &item.span, &map));
            }
            StatementKind::AliasStatement { name, .. } => {
                symbols.push(make_symbol(name, SymbolKind::VARIABLE, &item.span, &map));
            }
            StatementKind::Label(name) => {
                symbols.push(make_symbol(name, SymbolKind::PROPERTY, &item.span, &map));
            }
            _ => {}
        }
    }

    symbols
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
