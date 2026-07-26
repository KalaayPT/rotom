use dashmap::DashMap;
use tower_lsp::lsp_types::{DocumentSymbol, SymbolKind, TextDocumentContentChangeEvent, Url};

use rotom::compiler::{
    ast::StatementKind,
    sourcemap::{Position, SourceMap},
};

use crate::util::parse_source;

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

    /// Return the URIs of all currently open documents.
    pub fn uris(&self) -> Vec<Url> {
        self.docs.iter().map(|entry| entry.key().clone()).collect()
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
/// Scripts, aliases, labels, and actions are bucketed into collapsible parent nodes
/// so the editor outline shows them grouped by kind.
pub fn compute_document_symbols(source: &str) -> Vec<DocumentSymbol> {
    let Some(file) = parse_source(source) else {
        return Vec::new();
    };

    let mut scripts = Vec::new();
    let mut aliases = Vec::new();
    let mut labels = Vec::new();
    let mut actions = Vec::new();

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
                aliases.push(make_symbol(name, SymbolKind::VARIABLE, &item.span, &map));
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
    if !aliases.is_empty() {
        groups.push(make_group("Aliases", SymbolKind::NAMESPACE, &aliases, &map));
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

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::{Position as LspPosition, Range};

    fn group_names(symbols: &[DocumentSymbol]) -> Vec<&str> {
        symbols.iter().map(|symbol| symbol.name.as_str()).collect()
    }

    #[test]
    fn document_symbols_group_aliases_separately_from_labels() {
        let symbols = compute_document_symbols(
            r#"alias 0x800C as VAR_RESULT

script Main #1:
    End

Helper:
    Return
"#,
        );

        assert_eq!(group_names(&symbols), vec!["Scripts", "Aliases", "Labels"]);

        let aliases = symbols
            .iter()
            .find(|symbol| symbol.name == "Aliases")
            .and_then(|symbol| symbol.children.as_ref())
            .expect("Aliases group should have children");
        assert_eq!(aliases[0].name, "VAR_RESULT");
        assert_eq!(aliases[0].kind, SymbolKind::VARIABLE);

        let labels = symbols
            .iter()
            .find(|symbol| symbol.name == "Labels")
            .and_then(|symbol| symbol.children.as_ref())
            .expect("Labels group should have children");
        assert_eq!(labels[0].name, "Helper");
        assert_eq!(labels[0].kind, SymbolKind::FUNCTION);
    }

    #[test]
    fn apply_changes_replaces_text_after_multibyte_character() {
        let cache = DocumentCache::new();
        let uri = Url::parse("file:///tmp/multibyte.rotom").expect("valid test URI");
        cache.insert(uri.clone(), "script Café #1:\n    End\n".to_string());

        cache.apply_changes(
            &uri,
            vec![TextDocumentContentChangeEvent {
                range: Some(Range {
                    start: LspPosition {
                        line: 0,
                        character: 12,
                    },
                    end: LspPosition {
                        line: 0,
                        character: 14,
                    },
                }),
                range_length: None,
                text: "Main".to_string(),
            }],
        );

        let doc = cache.get(&uri).expect("document should remain cached");
        assert_eq!(doc.text, "script Café Main:\n    End\n");
    }

    #[test]
    fn apply_changes_replaces_text_after_surrogate_pair() {
        let cache = DocumentCache::new();
        let uri = Url::parse("file:///tmp/emoji.rotom").expect("valid test URI");
        cache.insert(
            uri.clone(),
            "// 🔥 marker\nscript Main #1:\n    End\n".to_string(),
        );

        cache.apply_changes(
            &uri,
            vec![TextDocumentContentChangeEvent {
                range: Some(Range {
                    start: LspPosition {
                        line: 0,
                        character: 6,
                    },
                    end: LspPosition {
                        line: 0,
                        character: 12,
                    },
                }),
                range_length: None,
                text: "note".to_string(),
            }],
        );

        let doc = cache.get(&uri).expect("document should remain cached");
        assert_eq!(doc.text, "// 🔥 note\nscript Main #1:\n    End\n");
    }
}
