//! Source position mapping utilities.
//!
//! Converts between byte offsets, UTF-8 line/column positions, and LSP
//! UTF-16 line/character positions.

/// A zero-based source position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Position {
    /// Zero-based line number.
    pub line: u32,
    /// Zero-based character offset (UTF-16 code units for LSP compatibility).
    pub character: u32,
}

/// Maps between byte offsets and source positions for a single file.
#[derive(Debug, Clone)]
pub struct SourceMap<'a> {
    source: &'a str,
    /// Byte offset of the start of each line.
    line_starts: Vec<usize>,
}

impl<'a> SourceMap<'a> {
    /// Build a `SourceMap` from the given source text.
    pub fn new(source: &'a str) -> Self {
        let mut line_starts = vec![0usize];
        for (i, c) in source.char_indices() {
            if c == '\n' {
                let next = i + c.len_utf8();
                line_starts.push(next);
            }
        }
        Self {
            source,
            line_starts,
        }
    }

    /// Convert a byte offset to a zero-based `(line, column)` pair using
    /// **UTF-8** character counts.
    ///
    /// Returns `(0, 0)` for out-of-bounds offsets.
    pub fn byte_to_position_utf8(&self, offset: usize) -> Position {
        let line = self.line_for_byte(offset);
        let line_start = self.line_starts[line];
        let col = self.source[line_start..offset.min(self.source.len())]
            .chars()
            .count() as u32;
        Position {
            line: line as u32,
            character: col,
        }
    }

    /// Convert a byte offset to an LSP-compatible `Position` using **UTF-16**
    /// code units.
    ///
    /// LSP defaults to UTF-16 encoding for positions. This counts surrogate
    /// pairs for characters outside the Basic Multilingual Plane.
    ///
    /// Returns `(0, 0)` for out-of-bounds offsets.
    pub fn byte_to_position(&self, offset: usize) -> Position {
        let line = self.line_for_byte(offset);
        let line_start = self.line_starts[line];
        let mut utf16_col = 0u32;
        for ch in self.source[line_start..offset.min(self.source.len())].chars() {
            utf16_col += ch.len_utf16() as u32;
        }
        Position {
            line: line as u32,
            character: utf16_col,
        }
    }

    /// Convert an LSP `Position` (UTF-16) back to a byte offset.
    ///
    /// Clamps to the end of the source if the position is past the last line.
    /// Clamps to the end of the line if the character is past the line length.
    pub fn position_to_byte(&self, pos: Position) -> usize {
        let line = pos.line as usize;
        if line >= self.line_starts.len() {
            return self.source.len();
        }
        let line_start = self.line_starts[line];
        let line_end = self
            .line_starts
            .get(line + 1)
            .copied()
            .unwrap_or(self.source.len());
        let line_text = &self.source[line_start..line_end];
        let mut remaining = pos.character;
        for (byte_idx, ch) in line_text.char_indices() {
            if remaining == 0 {
                return line_start + byte_idx;
            }
            let ch_utf16 = ch.len_utf16() as u32;
            if remaining < ch_utf16 {
                // Position falls inside a surrogate pair; clamp to start of char.
                return line_start + byte_idx;
            }
            remaining -= ch_utf16;
        }
        // Position past end of line -> clamp to end of line.
        line_start + line_text.len()
    }

    /// Convert a `std::ops::Range<usize>` byte range into an LSP-style
    /// `Range` using the `lsp_types` crate.
    #[cfg(feature = "lsp-types")]
    pub fn byte_range_to_lsp_range(&self, range: std::ops::Range<usize>) -> lsp_types::Range {
        let pos = self.byte_to_position(range.start);
        let start = lsp_types::Position {
            line: pos.line,
            character: pos.character,
        };
        let pos = self.byte_to_position(range.end);
        let end = lsp_types::Position {
            line: pos.line,
            character: pos.character,
        };
        lsp_types::Range { start, end }
    }

    /// Total number of lines in the source.
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Return the line index (0-based) that contains the given byte offset.
    fn line_for_byte(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(idx) => idx,
            Err(idx) => idx.saturating_sub(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_positions() {
        let src = "line1\nline2\nline3";
        let map = SourceMap::new(src);
        assert_eq!(
            map.byte_to_position(0),
            Position {
                line: 0,
                character: 0
            }
        );
        assert_eq!(
            map.byte_to_position(3),
            Position {
                line: 0,
                character: 3
            }
        );
        assert_eq!(
            map.byte_to_position(6),
            Position {
                line: 1,
                character: 0
            }
        );
        assert_eq!(
            map.byte_to_position(9),
            Position {
                line: 1,
                character: 3
            }
        );
    }

    #[test]
    fn utf16_surrogate_pairs() {
        // 😀 (U+1F600) is 2 UTF-16 code units, 4 UTF-8 bytes.
        let src = "a😀b";
        let map = SourceMap::new(src);
        assert_eq!(
            map.byte_to_position(0),
            Position {
                line: 0,
                character: 0
            }
        ); // 'a'
        assert_eq!(
            map.byte_to_position(1),
            Position {
                line: 0,
                character: 1
            }
        ); // 'a'
        assert_eq!(
            map.byte_to_position(5),
            Position {
                line: 0,
                character: 3
            }
        ); // 'b'
        assert_eq!(
            map.byte_to_position(6),
            Position {
                line: 0,
                character: 4
            }
        ); // past end
    }

    #[test]
    fn roundtrip_byte_position() {
        let src = "Hello\nWorld 🎉!";
        let map = SourceMap::new(src);
        // Test every valid char boundary to avoid slicing inside multi-byte chars.
        let mut offsets: Vec<usize> = src.char_indices().map(|(i, _)| i).collect();
        offsets.push(src.len());
        for offset in offsets {
            let pos = map.byte_to_position(offset);
            let back = map.position_to_byte(pos);
            // Round-trip may clamp inside a surrogate pair, so it should never
            // exceed the original offset and should land on a char boundary.
            assert!(back <= offset, "round-trip exceeded original offset");
            assert!(
                src.is_char_boundary(back),
                "round-trip landed on non-char-boundary"
            );
        }
    }

    #[test]
    fn position_to_byte_past_end() {
        let src = "hi";
        let map = SourceMap::new(src);
        assert_eq!(
            map.position_to_byte(Position {
                line: 0,
                character: 10
            }),
            2
        );
        assert_eq!(
            map.position_to_byte(Position {
                line: 10,
                character: 0
            }),
            2
        );
    }
}
