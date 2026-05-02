use std::fmt::Write as _;

use tower_lsp::lsp_types::{
    Hover, HoverContents, MarkupContent, MarkupKind, Position as LspPosition,
};

use rotom::compiler::{
    ast::{ExpressionKind, Statement, StatementKind},
    sourcemap::{Position as SourcePosition, SourceMap},
};
use rotom::database::{Command, ConstantDb, DatabaseV2};

use crate::util::parse_source;

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
            if cmd.legacy_name.as_deref() == Some(&word) && canonical != &word {
                let mut lines = Vec::new();
                lines.push(format!("**{canonical}**"));
                lines.push(String::new());
                lines.push(format!("Also known as `{word}` (legacy alias)"));

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
        let content = format!("**{word}**\n\nConstant value: `{value}` hex: `0x{value:x}`");
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: content,
            }),
            range: None,
        });
    }

    // Try aliases defined in the source file.
    if let Some((alias_value, alias_name)) = find_alias_value(source, &word) {
        let value_str = match &alias_value.node {
            ExpressionKind::Number(n) => format!("{n}"),
            ExpressionKind::Identifier(id) => id.clone(),
            ExpressionKind::Label(l) => format!(".{l}"),
            ExpressionKind::Prefix { operator, id } => {
                format!("{operator:?} {}", format_expr(id))
            }
            ExpressionKind::Infix {
                left,
                operator,
                right,
            } => {
                format!("{} {operator:?} {}", format_expr(left), format_expr(right))
            }
            ExpressionKind::Call { function, args } => {
                let arg_strs: Vec<String> = args.iter().map(format_expr).collect();
                format!("{}({})", format_expr(function), arg_strs.join(", "))
            }
            ExpressionKind::Error => "<error>".to_string(),
        };
        let value_int: i32 = value_str.parse().ok()?;
        let content = format!(
            "**{alias_name}**\n\nAlias value: `{value_str}` hex: `0x{value_int:x}`"
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

fn format_expr(expr: &rotom::compiler::ast::Expression) -> String {
    match &expr.node {
        ExpressionKind::Number(n) => n.to_string(),
        ExpressionKind::Identifier(id) => id.clone(),
        ExpressionKind::Label(l) => format!(".{l}"),
        ExpressionKind::Prefix { operator, id } => format!("{operator:?} {}", format_expr(id)),
        ExpressionKind::Infix {
            left,
            operator,
            right,
        } => {
            format!("{} {operator:?} {}", format_expr(left), format_expr(right))
        }
        ExpressionKind::Call { function, args } => {
            let arg_strs: Vec<String> = args.iter().map(format_expr).collect();
            format!("{}({})", format_expr(function), arg_strs.join(", "))
        }
        ExpressionKind::Error => "<error>".to_string(),
    }
}

/// Parse the source and look for an alias whose name matches `word`.
/// Returns the alias expression and name if found.
fn find_alias_value(
    source: &str,
    word: &str,
) -> Option<(rotom::compiler::ast::Expression, String)> {
    let ast = parse_source(source)?;
    find_alias_in_items(&ast.items, word)
}

fn find_alias_in_items(
    items: &[Statement],
    word: &str,
) -> Option<(rotom::compiler::ast::Expression, String)> {
    for item in items {
        if let StatementKind::AliasStatement { value, name } = &item.node
            && name == word
        {
            return Some((value.clone(), name.clone()));
        }
        // Recurse into blocks.
        let body = match &item.node {
            StatementKind::Function { body, .. }
            | StatementKind::Action { body, .. }
            | StatementKind::IfStatement { body, .. }
            | StatementKind::WhileStatement { body, .. } => Some(body),
            _ => None,
        };
        if let Some(body) = body
            && let Some(result) = find_alias_in_items(body, word)
        {
            return Some(result);
        }
        if let StatementKind::IfStatement { elseblock, .. } = &item.node
            && let Some(else_b) = elseblock
            && let Some(result) = find_alias_in_items(else_b, word)
        {
            return Some(result);
        }
        if let StatementKind::MatchStatement { cases, default, .. } = &item.node {
            for case in cases {
                if let Some(result) = find_alias_in_items(&case.body, word) {
                    return Some(result);
                }
            }
            if let Some(default) = default
                && let Some(result) = find_alias_in_items(default, word)
            {
                return Some(result);
            }
        }
    }
    None
}

fn format_command_hover(name: &str, cmd: &Command) -> String {
    let mut lines = Vec::new();
    lines.push(format!("**{name}**"));

    if let Some(legacy) = &cmd.legacy_name
        && name != legacy
    {
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
pub fn extract_word(source: &str, byte_offset: usize) -> Option<String> {
    let before = &source[..byte_offset.min(source.len())];

    // Walk backward to find the start of the current identifier.
    let start = before
        .rfind(|c: char| !rotom::compiler::lexer::is_identifier_char(c))
        .map_or(0, |i| i + before[i..].chars().next().map_or(1, char::len_utf8));

    // Walk forward from the cursor to find the end.
    let after = &source[byte_offset.min(source.len())..];
    let end_forward = after
        .find(|c: char| !rotom::compiler::lexer::is_identifier_char(c))
        .unwrap_or(after.len());

    let word = &source[start..byte_offset.min(source.len()) + end_forward];
    if word.is_empty() {
        None
    } else {
        Some(word.to_string())
    }
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
