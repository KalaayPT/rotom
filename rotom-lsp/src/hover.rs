use std::fmt::Write as _;

use tower_lsp::lsp_types::{
    Hover, HoverContents, MarkupContent, MarkupKind, Position as LspPosition,
};

use rotom::compiler::sourcemap::{Position as SourcePosition, SourceMap};
use rotom::database::{Command, ConstantDb, DatabaseV2};

/// Produce an LSP hover response for the symbol under the cursor.
pub fn compute_hover(
    source: &str,
    position: LspPosition,
    db: Option<&DatabaseV2>,
    constants: Option<&ConstantDb>,
) -> Option<Hover> {
    let map = SourceMap::new(source);
    let byte_offset = map.position_to_byte(SourcePosition {
        line: position.line,
        character: position.character,
    });

    let word = extract_word(source, byte_offset)?;

    // Try commands first.
    if let Some(db) = db {
        if let Ok(cmd) = db.get_command(&word) {
            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: format_command_hover(&word, cmd),
                }),
                range: None,
            });
        }

        // Check legacy names — show the canonical name prominently.
        for (canonical, cmd) in &db.commands {
            if cmd.legacy_name.as_deref() == Some(&word) {
                let mut lines = Vec::new();
                lines.push(format!("**{canonical}**"));
                lines.push(String::new());
                lines.push(format!(
                    "Also known as `{word}` (legacy alias)"
                ));

                if let Some(desc) = &cmd.description {
                    lines.push(String::new());
                    lines.push(desc.clone());
                }

                if !cmd.params.is_empty() {
                    lines.push(String::new());
                    lines.push("**Parameters:**".to_string());
                    for p in &cmd.params {
                        lines.push(format_param_desc(p));
                    }
                }

                return Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: lines.join("\n"),
                    }),
                    range: None,
                });
            }
        }
    }

    // Try constants.
    if let Some(constants) = constants
        && let Some(value) = constants.get(&word)
    {
        let content = format!(
            "**{word}**\n\nConstant value: `{value}` hex: `0x{value:x}`"
        );
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: content,
            }),
            range: None,
        });
    }

    None
}

fn format_command_hover(name: &str, cmd: &Command) -> String {
    let mut lines = Vec::new();
    lines.push(format!("**{name}**"));

    if let Some(legacy) = &cmd.legacy_name {
        lines.push(format!("(legacy name: `{legacy}`)"));
    }

    if let Some(desc) = &cmd.description {
        lines.push(String::new());
        lines.push(desc.clone());
    }

    if !cmd.params.is_empty() {
        lines.push(String::new());
        lines.push("**Parameters:**".to_string());
        for p in &cmd.params {
            lines.push(format_param_desc(p));
        }
    }

    lines.join("\n")
}

fn format_param_desc(p: &rotom::database::ParamDef) -> String {
    let mut desc = format!("- `{}`", p.name);
    let _ = write!(desc, " ({:?})", p.param_type);
    if p.optional {
        desc.push_str(" — *optional*");
    }
    if let Some(default) = &p.default {
        let _ = write!(desc, ", default: `{default}`");
    }
    if let Some(const_val) = &p.const_value {
        let _ = write!(desc, ", const: `{const_val}`");
    }
    desc
}

/// Extract the complete identifier word surrounding the given byte offset.
fn extract_word(source: &str, byte_offset: usize) -> Option<String> {
    let before = &source[..byte_offset.min(source.len())];

    // Walk backward to find the start of the current identifier.
    let start = before
        .rfind(|c: char| !is_identifier_char(c))
        .map_or(0, |i| i + before[i..].chars().next().map_or(1, char::len_utf8));

    // Walk forward from the cursor to find the end.
    let after = &source[byte_offset.min(source.len())..];
    let end_forward = after
        .find(|c: char| !is_identifier_char(c))
        .unwrap_or(after.len());

    let word = &source[start..byte_offset.min(source.len()) + end_forward];
    if word.is_empty() {
        None
    } else {
        Some(word.to_string())
    }
}

fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.' || c == '?'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_word_basic() {
        let source = "    Message 1";
        // Cursor inside "Message"
        assert_eq!(extract_word(source, 8), Some("Message".to_string()));
    }

    #[test]
    fn test_extract_word_with_dot() {
        let source = "Jump .start";
        // Cursor inside ".start"
        assert_eq!(extract_word(source, 8), Some(".start".to_string()));
    }

    #[test]
    fn test_extract_word_empty() {
        let source = "    ";
        assert_eq!(extract_word(source, 2), None);
    }
}
