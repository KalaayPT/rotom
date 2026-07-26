use tower_lsp::lsp_types::{GotoDefinitionResponse, Location, Position as LspPosition, Range, Url};

use rotom::compiler::{
    ast::{ExpressionKind, Statement, StatementKind},
    sourcemap::{Position as SourcePosition, SourceMap},
};

use crate::hover::extract_word;
use crate::message_refs::{find_command_at_offset, is_text_slot, resolve_archive_id};
use crate::util::{byte_span_to_location, parse_source};

/// Produce an LSP go-to-definition response for the symbol under the cursor.
///
/// Resolves cross-file global script references first, then local definitions
/// and message archive entries.
pub fn compute_goto_definition(
    source: &str,
    position: tower_lsp::lsp_types::Position,
    uri: &Url,
    project: Option<&rotom::ProjectContext>,
    script_file_name: Option<&str>,
) -> Option<GotoDefinitionResponse> {
    let map = SourceMap::new(source);
    let byte_offset = map.position_to_byte(SourcePosition {
        line: position.line,
        character: position.character,
    });

    let ast = parse_source(source)?;

    if let Some(project) = project
        && let Some((command, args, 0)) = find_command_at_offset(&ast.items, byte_offset)
        && project.db().is_global_script_call(command)
        && let Some(ExpressionKind::ModuleRef { module, label }) = args.first().map(|arg| &arg.node)
        && let Ok(resolved) = project.resolve_global_script_ref(module, label)
        && let Ok(uri) = Url::from_file_path(&resolved.path)
    {
        let target_map = SourceMap::new(&resolved.source);
        return Some(GotoDefinitionResponse::Scalar(byte_span_to_location(
            &uri,
            &resolved.symbol.span,
            &target_map,
        )));
    }

    let workspace = project.and_then(|project| project.workspace());
    let db = project.map(rotom::ProjectContext::db);

    if let Some(word) = extract_word(source, byte_offset) {
        if let Some(stmt) = find_definition(&ast.items, &word) {
            return Some(GotoDefinitionResponse::Scalar(byte_span_to_location(
                uri, &stmt.span, &map,
            )));
        }

        // Try message ID lookup via the public Workspace accessor.
        if let Some(ws) = workspace
            && let Some((archive_id, msg_index)) = ws.resolve_message_id(&word)
            && let Some(path) = ws.cached_text_archive_path(archive_id)
        {
            let location = json_message_location(&path, msg_index as usize)
                .unwrap_or_else(|| file_start_location(&path));
            return Some(GotoDefinitionResponse::Scalar(location));
        }
    }

    // Numeric literal in a text slot: jump directly to that message index.
    if let Some(ws) = workspace
        && let Some(msg_index) = extract_numeric_literal_at_offset(source, byte_offset)
            .and_then(|n| u16::try_from(n).ok())
        && let Some((command, args, arg_index)) = find_command_at_offset(&ast.items, byte_offset)
        && is_text_slot(command, arg_index, db)
        && let Some(archive_id) = resolve_archive_id(
            command,
            args,
            &ast.items,
            byte_offset,
            ws,
            script_file_name,
            None,
        )
        && let Some(path) = ws.cached_text_archive_path(archive_id)
    {
        let location = json_message_location(&path, msg_index as usize)
            .unwrap_or_else(|| file_start_location(&path));
        return Some(GotoDefinitionResponse::Scalar(location));
    }

    None
}

/// Extract a decimal or hexadecimal literal around the byte offset.
fn extract_numeric_literal_at_offset(source: &str, byte_offset: usize) -> Option<i32> {
    let bytes = source.as_bytes();
    if bytes.is_empty() {
        return None;
    }

    let mut start = byte_offset.min(bytes.len());
    while start > 0 && is_numeric_token_char(bytes[start - 1] as char) {
        start -= 1;
    }

    let mut end = byte_offset.min(bytes.len());
    while end < bytes.len() && is_numeric_token_char(bytes[end] as char) {
        end += 1;
    }

    if start >= end {
        return None;
    }

    let token = &source[start..end];
    if let Some(hex) = token
        .strip_prefix("0x")
        .or_else(|| token.strip_prefix("0X"))
    {
        i32::from_str_radix(hex, 16).ok()
    } else {
        token.parse::<i32>().ok()
    }
}

fn is_numeric_token_char(c: char) -> bool {
    c.is_ascii_hexdigit() || c == 'x' || c == 'X'
}

/// Recursively walk statements looking for a definition whose name matches `word`.
fn find_definition<'a>(items: &'a [Statement], word: &str) -> Option<&'a Statement> {
    for item in items {
        let found = match &item.node {
            StatementKind::Function { headers, .. } => headers.iter().any(|h| h.name == word),
            StatementKind::Action { name, .. }
            | StatementKind::AliasStatement { name, .. }
            | StatementKind::Label(name) => *name == word,
            _ => false,
        };
        if found {
            return Some(item);
        }
        // Recurse into blocks.
        match &item.node {
            StatementKind::Function { body, .. }
            | StatementKind::Action { body, .. }
            | StatementKind::WhileStatement { body, .. } => {
                if let Some(stmt) = find_definition(body, word) {
                    return Some(stmt);
                }
            }
            StatementKind::IfStatement {
                body, elseblock, ..
            } => {
                if let Some(stmt) = find_definition(body, word) {
                    return Some(stmt);
                }
                if let Some(else_b) = elseblock
                    && let Some(stmt) = find_definition(else_b, word)
                {
                    return Some(stmt);
                }
            }
            StatementKind::MatchStatement { cases, default, .. } => {
                for case in cases {
                    if let Some(stmt) = find_definition(&case.body, word) {
                        return Some(stmt);
                    }
                }
                if let Some(default) = default
                    && let Some(stmt) = find_definition(default, word)
                {
                    return Some(stmt);
                }
            }
            _ => {}
        }
    }
    None
}

/// Find the LSP Location of the Nth message entry in a JSON archive.
///
/// Scans the file line by line counting `"id":` occurrences; the Nth such
/// line is the start of the target message object. Falls back to `None` if
/// the file cannot be read or `msg_index` is out of range.
pub(crate) fn json_message_location(path: &std::path::Path, msg_index: usize) -> Option<Location> {
    let uri = Url::from_file_path(path).ok()?;
    let content = std::fs::read_to_string(path).ok()?;
    let mut count = 0usize;
    for (line_idx, line) in content.lines().enumerate() {
        if line.contains("\"id\":") {
            if count == msg_index {
                let location = Location {
                    uri,
                    range: Range {
                        start: LspPosition {
                            line: line_idx as u32,
                            character: 0,
                        },
                        end: LspPosition {
                            line: line_idx as u32,
                            character: 0,
                        },
                    },
                };
                return Some(location);
            }
            count += 1;
        }
    }
    None
}

/// Return the source line of every message entry in a JSON archive, in order
/// (the Nth `"id":` line is message index N). Single pass over the file — use
/// this instead of calling `json_message_location` once per index, which
/// re-reads and re-scans the whole file each time.
pub(crate) fn json_message_lines(path: &std::path::Path) -> Vec<u32> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    content
        .lines()
        .enumerate()
        .filter_map(|(i, line)| line.contains("\"id\":").then_some(i as u32))
        .collect()
}

/// Return a Location pointing to the very start of a file.
fn file_start_location(path: &std::path::Path) -> Location {
    Location {
        uri: Url::from_file_path(path).unwrap_or_else(|_| Url::parse("file:///").unwrap()),
        range: Range {
            start: LspPosition {
                line: 0,
                character: 0,
            },
            end: LspPosition {
                line: 0,
                character: 0,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{compute_goto_definition, json_message_lines, json_message_location};
    use crate::util::test_project_context;
    use rotom::compiler::sourcemap::SourceMap;
    use rotom::database::{ConstantDb, DatabaseV2};
    use std::path::PathBuf;
    use tower_lsp::lsp_types::{GotoDefinitionResponse, Position, Url};
    use uxie::game::Game;

    fn test_db(path: &std::path::Path) -> PathBuf {
        let json = r#"{
  "meta": { "version": "test" },
  "commands": {
    "MessageFromBank": {
      "type": "script_cmd",
      "id": 2,
      "description": "",
      "params": [
        { "name": "bank", "type": "u16" },
        { "name": "text_slot", "type": "msg_id" }
      ]
    }
  },
  "movements": {}
}"#;
        let db_path = path.join("commands.json");
        std::fs::write(&db_path, json).expect("db write");
        db_path
    }

    #[test]
    fn goto_numeric_text_slot_jumps_to_archive_message() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = dir.path();
        std::fs::create_dir_all(root.join("expanded/textArchives")).expect("archives dir");
        std::fs::write(
            root.join("expanded/textArchives/0199.json"),
            r#"{"messages":[{"id":"msg_0199_00000","en_US":"A"},{"id":"msg_0199_00001","en_US":"B"}]}"#,
        )
        .expect("archive");

        let workspace = uxie::Workspace::new(root.to_path_buf(), Game::Platinum);
        workspace.ensure_archive_loaded(199).expect("load archive");
        let db_path = test_db(root);
        let project = test_project_context(
            workspace,
            &db_path,
            rotom::GameFamily::Platinum,
            ConstantDb::new(),
        );

        let source = "script Test # 0:\n    MessageFromBank 199, 1\n";
        let line_start = source.find("    MessageFromBank").expect("line start");
        let offset = source.find(", 1").expect("message index") + 2;
        let position = Position {
            line: 1,
            character: (offset - line_start) as u32,
        };

        let script_uri = tower_lsp::lsp_types::Url::from_file_path(root.join("scripts/test.rotom"))
            .expect("script uri");
        let result =
            compute_goto_definition(source, position, &script_uri, Some(&project), Some("test"));
        let Some(tower_lsp::lsp_types::GotoDefinitionResponse::Scalar(location)) = result else {
            panic!("expected scalar goto location");
        };
        assert!(location.uri.as_str().contains("0199.json"));
    }

    #[test]
    fn goto_local_label_returns_definition_location() {
        let source = "alias 42 as VALUE\nscript Main #1:\n    Jump .target\n.target:\n    End\n";
        let offset = source.find(".target").expect("label reference") + 2;
        let source_position = SourceMap::new(source).byte_to_position(offset);
        let position = Position {
            line: source_position.line,
            character: source_position.character,
        };
        let uri = tower_lsp::lsp_types::Url::parse("file:///tmp/main.rotom").expect("uri");

        let result = compute_goto_definition(source, position, &uri, None, None);
        let Some(tower_lsp::lsp_types::GotoDefinitionResponse::Scalar(location)) = result else {
            panic!("expected local label definition");
        };
        assert_eq!(location.range.start.line, 3);
    }

    #[test]
    fn goto_global_module_ref_returns_target_script_location() {
        let temp = tempfile::tempdir().expect("tmp");
        let scripts = temp.path().join("scripts");
        std::fs::create_dir(&scripts).unwrap();
        let target = scripts.join("0211.rotom");
        std::fs::write(&target, "script NewGame #1:\n    End\n").unwrap();
        let mut workspace =
            uxie::Workspace::new(temp.path().to_path_buf(), uxie::game::Game::Platinum);
        workspace.scripts.load_dspre_script_dir(&scripts).unwrap();
        workspace.global_script_table = uxie::script_file::GlobalScriptTable::from_entries(vec![
            uxie::script_file::GlobalScriptEntry::new(2000, 211, 213, "Common Scripts"),
        ]);
        let project = test_project_context(
            workspace,
            &DatabaseV2::test_platinum_path(),
            rotom::GameFamily::Platinum,
            ConstantDb::new(),
        );
        let source = "script Main #1:\n    CallCommonScript CommonScripts::NewGame\n    End\n";
        let offset = source.find("NewGame").unwrap();
        let position = SourceMap::new(source).byte_to_position(offset);
        let position = Position::new(position.line, position.character);
        let uri = tower_lsp::lsp_types::Url::parse("file:///tmp/main.rotom").unwrap();

        let result = compute_goto_definition(source, position, &uri, Some(&project), Some("main"));

        let Some(GotoDefinitionResponse::Scalar(location)) = result else {
            panic!("expected global script definition");
        };
        assert_eq!(location.uri, Url::from_file_path(target).unwrap());
        assert_eq!(location.range.start.line, 0);
    }

    #[test]
    fn archive_location_helpers_handle_matches_and_missing_entries() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("archive.json");
        std::fs::write(
            &path,
            "{\"id\":\"first\"}\n{\"text\":\"middle\"}\n{\"id\":\"second\"}\n",
        )
        .expect("archive");

        let location = json_message_location(&path, 1).expect("second message");
        assert_eq!(location.range.start.line, 2);
        assert_eq!(json_message_lines(&path), vec![0, 2]);
        assert!(json_message_location(&path, 2).is_none());
        assert!(json_message_lines(&dir.path().join("missing.json")).is_empty());
    }
}
