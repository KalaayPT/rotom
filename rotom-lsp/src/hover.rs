use std::fmt::Write as _;

use tower_lsp::lsp_types::{
    Hover, HoverContents, MarkupContent, MarkupKind, Position as LspPosition,
};

use rotom::compiler::{
    ast::{Expression, ExpressionKind, ScriptFile, Statement, StatementKind},
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
    workspace: Option<&uxie::Workspace>,
    script_file_name: Option<&str>,
) -> Option<Hover> {
    let map = SourceMap::new(source);
    let byte_offset = map.position_to_byte(SourcePosition {
        line: position.line,
        character: position.character,
    });

    let word = extract_word(source, byte_offset)?;

    // Parse the source once — shared by alias lookup and message text resolution.
    let ast = parse_source(source);

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

    // Try constants (may be augmented with message text below).
    let constant_value = constants.as_ref().and_then(|c| c.get(&word));
    if let Some(value) = constant_value {
        let mut content = format!("**{word}**\n\nConstant value: `{value}` hex: `0x{value:x}`");
        append_message_text(
            &mut content,
            byte_offset,
            value,
            ast.as_ref(),
            workspace,
            script_file_name,
            db,
            constants,
        );
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: content,
            }),
            range: None,
        });
    }

    // Try aliases defined in the source file.
    if let Some(ref ast) = ast
        && let Some((alias_value, alias_name)) = find_alias_in_items(&ast.items, &word)
    {
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
        let mut content =
            format!("**{alias_name}**\n\nAlias value: `{value_str}` hex: `0x{value_int:x}`");
        append_message_text(
            &mut content,
            byte_offset,
            value_int,
            Some(ast),
            workspace,
            script_file_name,
            db,
            constants,
        );
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: content,
            }),
            range: None,
        });
    }

    // Raw number literal — show hex/decimal conversion.
    // Optionally augmented with message text when inside a text_slot command.
    if let Ok(num) = word
        .strip_prefix("0x")
        .or_else(|| word.strip_prefix("0X"))
        .map_or_else(|| word.parse::<i32>(), |hex| i32::from_str_radix(hex, 16))
    {
        let mut content = if word.starts_with("0x") || word.starts_with("0X") {
            format!("**{word}**\n\nDecimal: `{num}`")
        } else {
            format!("**{word}**\n\nHex: `0x{num:x}`")
        };
        append_message_text(
            &mut content,
            byte_offset,
            num,
            ast.as_ref(),
            workspace,
            script_file_name,
            db,
            constants,
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

/// If the cursor is inside a command with a `text_slot` parameter, append
/// the message text from the game's text archive to the hover content.
#[allow(clippy::too_many_arguments)]
fn append_message_text(
    content: &mut String,
    byte_offset: usize,
    msg_index: i32,
    ast: Option<&ScriptFile>,
    workspace: Option<&uxie::Workspace>,
    script_file_name: Option<&str>,
    db: Option<&DatabaseV2>,
    constants: Option<&ConstantDb>,
) {
    let Some(ast) = ast else { return };
    let Some(workspace) = workspace else { return };
    let Some(text) =
        resolve_message_text(byte_offset, msg_index, ast, workspace, script_file_name, db, constants)
    else {
        return;
    };
    content.push_str("\n\n> ");
    content.push('\u{AB}');
    for line in text.replace('\r', "\n").lines() {
        content.push_str("\n> ");
        content.push_str(line);
    }
    content.push('\u{BB}');
}

/// Commands that explicitly specify which text archive to read from as their
/// first argument, overriding the file-level include association.
const CROSS_ARCHIVE_COMMANDS: &[&str] = &[
    "MessageFromBankInstant",
    "MessageFromBank",
    "MessageFromArchive",
    "MessageAllFromArchive",
    "MsgBoxExtern",
];

/// Determine the archive ID and fetch the message text for the hovered
/// argument, or return `None` if this position does not correspond to a
/// message index.
fn resolve_message_text(
    byte_offset: usize,
    msg_index: i32,
    ast: &ScriptFile,
    workspace: &uxie::Workspace,
    script_file_name: Option<&str>,
    db: Option<&DatabaseV2>,
    constants: Option<&ConstantDb>,
) -> Option<String> {
    let (command, args, arg_index) = find_command_at_offset(&ast.items, byte_offset)?;

    // Guard: cursor must be on a text_slot param (or MsgBoxExtern which is not
    // in the regular DB but always has a text arg).
    let is_text_slot = command == "MsgBoxExtern"
        || db
            .and_then(|db| db.commands.get(command))
            .and_then(|cmd| cmd.params.get(arg_index))
            .is_some_and(|p| p.name == "text_slot");
    if !is_text_slot {
        return None;
    }

    let archive_id = if CROSS_ARCHIVE_COMMANDS.contains(&command) {
        // Archive ID comes from the first argument, not from the file include.
        resolve_archive_from_first_arg(args, &ast.items, byte_offset, workspace)?
    } else if let Some(id) = uxie::Workspace::menu_entry_id(command, workspace.family) {
        // Menu entry commands always use a game-specific fixed archive.
        id
    } else {
        // Normal: the file's text bank include tells us which archive to use.
        // The stem was cached by apply_directives — no source re-scan needed.
        if let Some(stem) = constants.and_then(|c| c.text_bank_stem()) {
            workspace.text_archive_id(stem)?
        } else {
            // No text bank include in this file; fall back to the map-level
            // association (workspace maps script file name → archive ID).
            workspace.text_archive_for_script_file(script_file_name?)?
        }
    };

    workspace.read_message(archive_id, msg_index as u16)
}

/// Resolve the first argument of a cross-archive command to a numeric archive ID.
fn resolve_archive_from_first_arg(
    args: &[Expression],
    all_items: &[Statement],
    offset: usize,
    workspace: &uxie::Workspace,
) -> Option<u16> {
    let expr = args.first()?;
    match &expr.node {
        ExpressionKind::Number(n) if *n >= 0x8000 => {
            // Variable register as a raw number — look for a preceding GetStdMsgNaix.
            resolve_text_archive_by_get_std_msg_naix(all_items, offset, expr)
        }
        ExpressionKind::Number(n) => u16::try_from(*n).ok(),
        ExpressionKind::Identifier(id) if id.starts_with("VAR_") => {
            // Named variable — look for a preceding GetStdMsgNaix assignment.
            resolve_text_archive_by_get_std_msg_naix(all_items, offset, expr)
        }
        ExpressionKind::Identifier(id) => {
            workspace.symbols.resolve_constant(id)?.try_into().ok()
        }
        _ => None,
    }
}

fn resolve_text_archive_by_get_std_msg_naix(
    items: &[Statement],
    offset: usize,
    var_expr: &Expression,
) -> Option<u16> {
    for item in items {
        if !item.span.contains(&offset) {
            continue;
        }
        if let StatementKind::Function { body, .. } = &item.node {
            // Find the current statement's position in the body
            if let Some(pos) = body.iter().position(|s| s.span.contains(&offset))
                && let Some(prev) = pos.checked_sub(1).and_then(|i| body.get(i))
                && let StatementKind::ScriptCommand { command, args } = &prev.node
                && command == "GetStdMsgNaix"
                && args.len() >= 2
                && expr_matches(&args[1], var_expr)
            {
                // First arg of GetStdMsgNaix is the bank ID
                return resolve_explicit_u16(&args[0]);
            } else {
                continue;
            }
        }
        let body = match &item.node {
            StatementKind::Action { body, .. }
            | StatementKind::IfStatement { body, .. }
            | StatementKind::WhileStatement { body, .. } => Some(body),
            _ => None,
        };
        if let Some(body) = body
            && let Some(id) = resolve_text_archive_by_get_std_msg_naix(body, offset, var_expr)
        {
            return Some(id);
        }
        if let StatementKind::IfStatement { elseblock, .. } = &item.node
            && let Some(elsebody) = elseblock
            && let Some(id) = resolve_text_archive_by_get_std_msg_naix(elsebody, offset, var_expr)
        {
            return Some(id);
        }
        if let StatementKind::MatchStatement { cases, default, .. } = &item.node {
            for case in cases {
                if let Some(id) =
                    resolve_text_archive_by_get_std_msg_naix(&case.body, offset, var_expr)
                {
                    return Some(id);
                }
            }
            if let Some(default_body) = default
                && let Some(id) =
                    resolve_text_archive_by_get_std_msg_naix(default_body, offset, var_expr)
            {
                return Some(id);
            }
        }
    }
    None
}

/// Check if two expressions refer to the same thing (both are identifiers
/// with the same name, or both are numbers with the same value).
fn expr_matches(a: &Expression, b: &Expression) -> bool {
    match (&a.node, &b.node) {
        (ExpressionKind::Identifier(a_id), ExpressionKind::Identifier(b_id)) => a_id == b_id,
        (ExpressionKind::Number(a_n), ExpressionKind::Number(b_n)) => a_n == b_n,
        _ => false,
    }
}

/// Resolve an expression directly to u16 (literal numbers only).
fn resolve_explicit_u16(expr: &Expression) -> Option<u16> {
    match &expr.node {
        ExpressionKind::Number(n) => u16::try_from(*n).ok(),
        _ => None,
    }
}

/// Find the innermost `ScriptCommand` whose span contains `offset` and whose
/// argument list contains `offset` inside one of the arg spans.
///
/// Returns `(command_name, all_args, arg_index)`.
fn find_command_at_offset(
    items: &[Statement],
    offset: usize,
) -> Option<(&str, &[Expression], usize)> {
    for item in items {
        if !item.span.contains(&offset) {
            continue;
        }
        if let StatementKind::ScriptCommand { command, args } = &item.node {
            let arg_index = args.iter().position(|arg| arg.span.contains(&offset))?;
            return Some((command.as_str(), args.as_slice(), arg_index));
        }
        let body = match &item.node {
            StatementKind::Function { body, .. }
            | StatementKind::Action { body, .. }
            | StatementKind::IfStatement { body, .. }
            | StatementKind::WhileStatement { body, .. } => Some(body.as_slice()),
            _ => None,
        };
        if let Some(body) = body
            && let Some(result) = find_command_at_offset(body, offset)
        {
            return Some(result);
        }
        if let StatementKind::IfStatement { elseblock, .. } = &item.node
            && let Some(elsebody) = elseblock
            && let Some(result) = find_command_at_offset(elsebody, offset)
        {
            return Some(result);
        }
        if let StatementKind::MatchStatement { cases, default, .. } = &item.node {
            for case in cases {
                if let Some(result) = find_command_at_offset(&case.body, offset) {
                    return Some(result);
                }
            }
            if let Some(default_body) = default
                && let Some(result) = find_command_at_offset(default_body, offset)
            {
                return Some(result);
            }
        }
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
            && let Some(elsebody) = elseblock
            && let Some(result) = find_alias_in_items(elsebody, word)
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
    lines.push(format!("# {name}"));

    let type_label = match cmd.cmd_type {
        rotom::database::CommandType::Macro => "(macro)",
        rotom::database::CommandType::Movement => "(movement)",
        rotom::database::CommandType::ScriptCmd => "(command)",
        rotom::database::CommandType::LevelscriptCmd => "(levelscript command)",
    };
    lines.push(type_label.to_string());

    if let Some(legacy) = &cmd.legacy_name
        && name != legacy
    {
        lines.push(String::new());
        lines.push(format!("legacy name: `{legacy}`"));
    }

    if let Some(desc) = &cmd.description {
        lines.push(String::new());
        lines.push(desc.clone());
    }

    if !cmd.params.is_empty() {
        lines.push(String::new());
        lines.push("## Parameters:".to_string());
        for p in &cmd.params {
            lines.push(format_param_desc(p));
        }
    }

    if let Some(expansion) = &cmd.expansion {
        lines.push(String::new());
        lines.push("## Expansion:".to_string());
        lines.push(String::from("```rotom"));
        for stmt in expansion {
            lines.push(stmt.to_string());
        }
        lines.push(String::from("```"));
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
        .map_or(0, |i| {
            i + before[i..].chars().next().map_or(1, char::len_utf8)
        });

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

    // ── extract_word ──

    #[test]
    fn test_extract_word_basic() {
        let source = "    Message 1";
        assert_eq!(extract_word(source, 8), Some("Message".to_string()));
    }

    #[test]
    fn test_extract_word_with_dot() {
        let source = "Jump .start";
        assert_eq!(extract_word(source, 8), Some(".start".to_string()));
    }

    #[test]
    fn test_extract_word_empty() {
        let source = "    ";
        assert_eq!(extract_word(source, 2), None);
    }

    // ── resolve_text_archive_by_get_std_msg_naix ──

    #[test]
    fn test_get_std_msg_naix_pattern() {
        // "script T #1:\n    " = 0-17
        // "GetStdMsgNaix 0, VAR_SPECIAL_RESULT\n" at 17; "0" is at 31, "VAR_SPECIAL_RESULT" at 34
        // "    MsgBoxExtern VAR_SPECIAL_RESULT, 51\n" follows
        let source =
            "script T #1:\n    GetStdMsgNaix 0, VAR_SPECIAL_RESULT\n    MsgBoxExtern VAR_SPECIAL_RESULT, 51\n";
        let Some(ast) = parse_source(source) else {
            panic!("parse failed")
        };
        // Locate the offset of "51" to simulate hovering on it.
        let offset_51 = source.find(", 51").unwrap() + 2;
        let result = find_command_at_offset(&ast.items, offset_51);
        assert!(result.is_some(), "find_command_at_offset should find MsgBoxExtern");
        let (cmd, args, idx) = result.unwrap();
        assert_eq!(cmd, "MsgBoxExtern");
        assert_eq!(idx, 1);
        // First arg should be VAR_SPECIAL_RESULT (an Identifier)
        assert!(
            matches!(&args[0].node, ExpressionKind::Identifier(id) if id == "VAR_SPECIAL_RESULT")
        );
        // Now check that resolve_text_archive_by_get_std_msg_naix returns Some(0)
        let var_expr = &args[0];
        let archive_id = resolve_text_archive_by_get_std_msg_naix(&ast.items, offset_51, var_expr);
        assert_eq!(archive_id, Some(0), "should resolve archive 0 from GetStdMsgNaix");
    }

    // ── find_command_at_offset ──

    #[test]
    fn test_find_message_arg_index_first() {
        let source = "script Test #1:\n    Message 0\n";
        let Some(ast) = parse_source(source) else {
            panic!("parse failed")
        };
        // "script Test #1:\n    " = 0-19, "M"=20, "Message "=20-27, "0"=28
        assert_eq!(
            find_command_at_offset(&ast.items, 28).map(|(cmd, _, idx)| (idx, cmd)),
            Some((0, "Message"))
        );
    }

    #[test]
    fn test_find_multi_arg_index() {
        let source = "script Test #1:\n    CallScript 1, 2\n";
        let Some(ast) = parse_source(source) else {
            panic!("parse failed")
        };
        // "script Test #1:\n    " = 0-19, "CallScript "=20-30, "1"=31, "2"=34
        assert_eq!(
            find_command_at_offset(&ast.items, 31).map(|(cmd, _, idx)| (idx, cmd)),
            Some((0, "CallScript"))
        );
        assert_eq!(
            find_command_at_offset(&ast.items, 34).map(|(cmd, _, idx)| (idx, cmd)),
            Some((1, "CallScript"))
        );
    }

    #[test]
    fn test_find_arg_index_on_command_name_only() {
        let source = "script Test #1:\n    Message 0\n";
        let Some(ast) = parse_source(source) else {
            panic!("parse failed")
        };
        // "Message" starts at byte 20; cursor is on command name, not an arg
        assert!(find_command_at_offset(&ast.items, 20).is_none());
    }
}
