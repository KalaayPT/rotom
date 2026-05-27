use std::path::{Path, PathBuf};

use rotom::compiler::{
    ast::{Expression, ExpressionKind, Statement, StatementKind},
    sourcemap::SourceMap,
};
use rotom::database::{ConstantDb, DatabaseV2, ParamType};

use crate::util::parse_source;

/// A single script-side reference to a message slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageRef {
    /// Absolute script path used for lens navigation.
    pub script_path: PathBuf,
    /// 0-indexed source line in the script file.
    pub line: u32,
    /// Command that references this message.
    pub command: String,
}

/// Commands whose first argument explicitly selects the text archive.
pub const CROSS_ARCHIVE_COMMANDS: &[&str] = &[
    "MessageFromBankInstant",
    "MessageFromBank",
    "MessageFromArchive",
    "MessageAllFromArchive",
    "MsgBoxExtern",
];

/// Parse one script and collect all message references.
pub fn collect_message_refs(
    source: &str,
    workspace: &uxie::Workspace,
    db: &DatabaseV2,
    script_path: &Path,
    constants: Option<&ConstantDb>,
) -> Vec<((u16, u16), MessageRef)> {
    let Some(ast) = parse_source(source) else {
        return Vec::new();
    };
    let script_path =
        std::fs::canonicalize(script_path).unwrap_or_else(|_| script_path.to_path_buf());
    let map = SourceMap::new(source);
    let mut refs = Vec::new();
    walk_commands(&ast.items, &mut |stmt| {
        let StatementKind::ScriptCommand { command, args } = &stmt.node else {
            return;
        };
        if !db.commands.contains_key(command.as_str()) {
            return;
        }
        for (arg_index, arg) in args.iter().enumerate() {
            if !is_text_slot(command, arg_index, Some(db)) {
                continue;
            }
            let byte_offset = arg.span.start;
            let Some((archive_id, msg_index)) = resolve_message_pair(
                arg,
                command,
                args,
                &ast.items,
                byte_offset,
                workspace,
                script_path.file_stem().and_then(|s| s.to_str()),
                constants,
            ) else {
                continue;
            };
            let line = map.byte_to_position(byte_offset).line;
            refs.push((
                (archive_id, msg_index),
                MessageRef {
                    script_path: script_path.clone(),
                    line,
                    command: command.clone(),
                },
            ));
        }
    });
    refs
}

/// Determine whether a command argument is a message slot.
pub fn is_text_slot(command: &str, arg_index: usize, db: Option<&DatabaseV2>) -> bool {
    // MsgBoxExtern: arg 0 is the archive, arg 1 is the text slot. Decide this
    // explicitly and first — its DB params vary by game (HGSS uses nonstandard
    // ones), so a present-but-untyped arg 1 must not fall through to `false`.
    if command == "MsgBoxExtern" {
        return arg_index == 1;
    }
    if let Some(db) = db
        && let Some(cmd) = db.commands.get(command)
        && let Some(param) = cmd.params.get(arg_index)
    {
        return param.name == "text_slot" || param.param_type == ParamType::MsgId;
    }
    false
}

/// Resolve `(archive_id, msg_index)` for one message-slot argument.
#[allow(clippy::too_many_arguments)] // mirrors call-site context (command, args, scope, workspace).
pub fn resolve_message_pair(
    arg: &Expression,
    command: &str,
    args: &[Expression],
    all_items: &[Statement],
    offset: usize,
    workspace: &uxie::Workspace,
    script_file_name: Option<&str>,
    constants: Option<&ConstantDb>,
) -> Option<(u16, u16)> {
    if let ExpressionKind::Identifier(id) = &arg.node
        && let Some(pair) = workspace.resolve_message_id(id)
    {
        return Some(pair);
    }
    let msg_index = resolve_message_index(arg, workspace)?;
    let archive_id = resolve_archive_id(
        command,
        args,
        all_items,
        offset,
        workspace,
        script_file_name,
        constants,
    )?;
    Some((archive_id, msg_index))
}

/// Find the command/arg under a byte offset.
pub fn find_command_at_offset(
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

/// Walk every script command statement in depth-first order.
pub fn walk_commands<F>(items: &[Statement], f: &mut F)
where
    F: FnMut(&Statement),
{
    for item in items {
        if matches!(&item.node, StatementKind::ScriptCommand { .. }) {
            f(item);
        }
        let body = match &item.node {
            StatementKind::Function { body, .. }
            | StatementKind::Action { body, .. }
            | StatementKind::IfStatement { body, .. }
            | StatementKind::WhileStatement { body, .. } => Some(body.as_slice()),
            _ => None,
        };
        if let Some(body) = body {
            walk_commands(body, f);
        }
        if let StatementKind::IfStatement { elseblock, .. } = &item.node
            && let Some(elsebody) = elseblock
        {
            walk_commands(elsebody, f);
        }
        if let StatementKind::MatchStatement { cases, default, .. } = &item.node {
            for case in cases {
                walk_commands(&case.body, f);
            }
            if let Some(default_body) = default {
                walk_commands(default_body, f);
            }
        }
    }
}

/// Resolve archive id for a command argument position.
pub fn resolve_archive_id(
    command: &str,
    args: &[Expression],
    all_items: &[Statement],
    offset: usize,
    workspace: &uxie::Workspace,
    script_file_name: Option<&str>,
    constants: Option<&ConstantDb>,
) -> Option<u16> {
    if CROSS_ARCHIVE_COMMANDS.contains(&command) {
        return resolve_archive_from_first_arg(args, all_items, offset, workspace);
    }
    if let Some(id) = uxie::Workspace::menu_entry_id(command, workspace.family) {
        return Some(id);
    }
    if let Some(stem) = constants.and_then(|c| c.text_bank_stem()) {
        return workspace.text_archive_id(stem);
    }
    workspace.text_archive_for_script_file(script_file_name?)
}

/// Resolve the first argument of a cross-archive command to an archive id.
pub fn resolve_archive_from_first_arg(
    args: &[Expression],
    all_items: &[Statement],
    offset: usize,
    workspace: &uxie::Workspace,
) -> Option<u16> {
    let expr = args.first()?;
    match &expr.node {
        ExpressionKind::Number(n) if *n >= 0x8000 => {
            resolve_text_archive_by_get_std_msg_naix(all_items, offset, expr)
        }
        ExpressionKind::Number(n) => u16::try_from(*n).ok(),
        ExpressionKind::Identifier(id) if id.starts_with("VAR_") => {
            resolve_text_archive_by_get_std_msg_naix(all_items, offset, expr)
        }
        ExpressionKind::Identifier(id) => workspace.symbols.resolve_constant(id)?.try_into().ok(),
        _ => None,
    }
}

/// Resolve `GetStdMsgNaix <bank>, <var>` pattern for cross-archive commands.
pub fn resolve_text_archive_by_get_std_msg_naix(
    items: &[Statement],
    offset: usize,
    var_expr: &Expression,
) -> Option<u16> {
    for item in items {
        if !item.span.contains(&offset) {
            continue;
        }
        if let StatementKind::Function { body, .. } = &item.node {
            if let Some(pos) = body.iter().position(|s| s.span.contains(&offset))
                && let Some(prev) = pos.checked_sub(1).and_then(|i| body.get(i))
                && let StatementKind::ScriptCommand { command, args } = &prev.node
                && command == "GetStdMsgNaix"
                && args.len() >= 2
                && expr_matches(&args[1], var_expr)
            {
                return resolve_explicit_u16(&args[0]);
            }
            continue;
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

/// Check if two expressions refer to the same identifier or number.
pub fn expr_matches(a: &Expression, b: &Expression) -> bool {
    match (&a.node, &b.node) {
        (ExpressionKind::Identifier(a_id), ExpressionKind::Identifier(b_id)) => a_id == b_id,
        (ExpressionKind::Number(a_n), ExpressionKind::Number(b_n)) => a_n == b_n,
        _ => false,
    }
}

/// Resolve an expression to a `u16` when it is a numeric literal.
pub fn resolve_explicit_u16(expr: &Expression) -> Option<u16> {
    match &expr.node {
        ExpressionKind::Number(n) => u16::try_from(*n).ok(),
        _ => None,
    }
}

fn resolve_message_index(expr: &Expression, workspace: &uxie::Workspace) -> Option<u16> {
    match &expr.node {
        ExpressionKind::Number(n) => u16::try_from(*n).ok(),
        ExpressionKind::Identifier(id) => {
            if let Some((_, idx)) = workspace.resolve_message_id(id) {
                Some(idx)
            } else {
                workspace.symbols.resolve_constant(id)?.try_into().ok()
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::collect_message_refs;
    use rotom::database::DatabaseV2;

    fn test_db() -> DatabaseV2 {
        let json = r#"{
  "meta": { "version": "test" },
  "commands": {
    "Message": {
      "type": "script_cmd",
      "id": 1,
      "description": "",
      "params": [{ "name": "text_slot", "type": "msg_id" }]
    },
    "MessageFromBank": {
      "type": "script_cmd",
      "id": 2,
      "description": "",
      "params": [
        { "name": "bank", "type": "u16" },
        { "name": "text_slot", "type": "msg_id" }
      ]
    },
    "GetStdMsgNaix": {
      "type": "script_cmd",
      "id": 3,
      "description": "",
      "params": [
        { "name": "archive", "type": "u16" },
        { "name": "var", "type": "var" }
      ]
    },
    "MsgBoxExtern": {
      "type": "script_cmd",
      "id": 4,
      "description": "",
      "params": [
        { "name": "bank", "type": "u16" },
        { "name": "value", "type": "u16" }
      ]
    }
  },
  "movements": {}
}"#;
        let dir = tempfile::tempdir().expect("tmpdir");
        let db_path = dir.path().join("commands.json");
        std::fs::write(&db_path, json).expect("write db");
        DatabaseV2::load(&db_path).expect("test db")
    }

    #[test]
    fn collect_message_refs_unit() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = dir.path();
        std::fs::create_dir_all(root.join("scripts")).expect("scripts dir");
        std::fs::create_dir_all(root.join("expanded/textArchives")).expect("text dir");
        std::fs::write(
            root.join("expanded/textArchives/0199.json"),
            r#"{"messages":[{"id":"msg_0199_00000","en_US":"A"},{"id":"msg_0199_00001","en_US":"B"}]}"#,
        )
        .expect("archive");
        let workspace = uxie::Workspace::new(root.to_path_buf(), uxie::game::Game::Platinum);
        workspace.ensure_archive_loaded(199).expect("load");
        let db = test_db();
        let refs = collect_message_refs(
            "script test # 0:\n    MessageFromBank 199, 0\n",
            &workspace,
            &db,
            &root.join("scripts/test.rotom"),
            None,
        );
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].0, (199, 0));
    }

    #[test]
    fn collect_message_refs_msgboxextern_uses_second_arg() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = dir.path();
        std::fs::create_dir_all(root.join("scripts")).expect("scripts dir");
        std::fs::create_dir_all(root.join("expanded/textArchives")).expect("text dir");
        std::fs::write(
            root.join("expanded/textArchives/0199.json"),
            r#"{"messages":[{"id":"msg_0199_00000","en_US":"A"},{"id":"msg_0199_00001","en_US":"B"}]}"#,
        )
        .expect("archive");
        let workspace = uxie::Workspace::new(root.to_path_buf(), uxie::game::Game::Platinum);
        let db = test_db();
        // MsgBoxExtern: arg 0 is the archive, arg 1 is the text slot. The test DB
        // deliberately does not tag arg 1 as text_slot, so this also exercises
        // the `is_text_slot` fallback for MsgBoxExtern.
        let refs = collect_message_refs(
            "script test # 0:\n    MsgBoxExtern 199, 1\n",
            &workspace,
            &db,
            &root.join("scripts/test.rotom"),
            None,
        );
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].0, (199, 1));
    }
}
