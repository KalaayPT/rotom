use dashmap::DashMap;
use tower_lsp::lsp_types::{TextDocumentContentChangeEvent, Url};

use rotom::compiler::sourcemap::{Position, SourceMap};

/// Incrementally-synced document cache.
pub struct DocumentCache {
    docs: DashMap<Url, String>,
}

impl DocumentCache {
    pub fn new() -> Self {
        Self {
            docs: DashMap::new(),
        }
    }

    pub fn insert(&self, uri: Url, text: String) {
        self.docs.insert(uri, text);
    }

    pub fn remove(&self, uri: &Url) {
        self.docs.remove(uri);
    }

    pub fn get(&self, uri: &Url) -> Option<dashmap::mapref::one::Ref<'_, Url, String>> {
        self.docs.get(uri)
    }

    pub fn apply_changes(&self, uri: Url, changes: Vec<TextDocumentContentChangeEvent>) {
        if let Some(mut doc) = self.docs.get_mut(&uri) {
            for change in changes {
                if let Some(range) = change.range {
                    // Incremental update using SourceMap for robust UTF-16 -> byte conversion.
                    let map = SourceMap::new(&*doc);
                    let start = map.position_to_byte(Position {
                        line: range.start.line,
                        character: range.start.character,
                    });
                    let end = map.position_to_byte(Position {
                        line: range.end.line,
                        character: range.end.character,
                    });
                    doc.replace_range(start..end, &change.text);
                } else {
                    // Full document replacement
                    *doc = change.text;
                }
            }
        }
    }
}
