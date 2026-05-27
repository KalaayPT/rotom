use std::collections::HashMap;
use std::path::Path;

use tower_lsp::lsp_types::{CodeLens, Command, Location, Position, Range, Url};

use rotom::compiler::{
    ast::{ExpressionKind, Statement, StatementKind},
    sourcemap::SourceMap,
};
use rotom::database::{DatabaseV2, ParamType};

use crate::message_refs::MessageRef;
use crate::util::{byte_span_to_location, byte_span_to_range, parse_source};

/// True when the URI points to a supported text-archive JSON.
pub fn is_message_archive_uri(uri: &Url) -> bool {
    let Some(path) = uri.to_file_path().ok() else {
        return false;
    };
    is_message_archive_path(&path)
}

/// True when the path points to a supported text-archive JSON.
pub fn is_message_archive_path(path: &Path) -> bool {
    if path.extension().and_then(|s| s.to_str()) != Some("json") {
        return false;
    }
    // Match on path components, not a substring of the rendered path: `res/text`
    // would never match backslash-separated paths on Windows (the primary
    // platform). DSPRE archives live in `.../textArchives/<id>.json`; Platinum
    // decomp in `.../res/text/<name>.json`.
    let mut rev = path.components().rev();
    rev.next(); // file name
    let parent = rev.next().and_then(|c| c.as_os_str().to_str());
    let grandparent = rev.next().and_then(|c| c.as_os_str().to_str());
    parent == Some("textArchives") || (parent == Some("text") && grandparent == Some("res"))
}

/// Produce `CodeLens` hints for a Rotom source file.
///
/// Shows reference counts above scripts, labels, aliases, and actions.
pub fn compute_script_code_lens(
    source: &str,
    uri: &Url,
    db: Option<&DatabaseV2>,
) -> Vec<CodeLens> {
    let Some(ast) = parse_source(source) else {
        return Vec::new();
    };

    let map = SourceMap::new(source);
    let mut refs: HashMap<String, Vec<Location>> = HashMap::new();

    count_refs(&ast.items, uri, &map, &mut refs, db);

    let mut lenses = Vec::new();
    emit_lenses(&ast.items, uri, &map, &refs, &mut lenses);
    lenses
}

/// Build message-reference `CodeLens` entries for one archive JSON.
pub fn build_message_code_lens(
    archive_uri: &Url,
    entries: Vec<(usize, Vec<MessageRef>)>,
) -> Vec<CodeLens> {
    let mut lenses = Vec::new();
    for (line, refs) in entries {
        let references: Vec<Location> = refs
            .iter()
            .filter_map(|r| {
                let uri = Url::from_file_path(&r.script_path).ok()?;
                Some(Location {
                    uri,
                    range: Range {
                        start: Position {
                            line: r.line,
                            character: 0,
                        },
                        end: Position {
                            line: r.line,
                            character: 0,
                        },
                    },
                })
            })
            .collect();
        let line_u32 = u32::try_from(line).unwrap_or(u32::MAX);
        let position = Position {
            line: line_u32,
            character: 0,
        };
        lenses.push(CodeLens {
            range: Range {
                start: position,
                end: position,
            },
            command: Some(Command {
                title: format!(
                    "{} reference{}",
                    references.len(),
                    if references.len() == 1 { "" } else { "s" }
                ),
                command: "editor.action.showReferences".to_string(),
                arguments: Some(vec![
                    serde_json::json!(archive_uri.as_str()),
                    serde_json::json!(position),
                    serde_json::json!(references),
                ]),
            }),
            data: None,
        });
    }
    lenses
}

fn count_refs(
    items: &[Statement],
    uri: &Url,
    map: &SourceMap,
    refs: &mut HashMap<String, Vec<Location>>,
    db: Option<&DatabaseV2>,
) {
    for item in items {
        match &item.node {
            StatementKind::Jump(expr) => {
                if let Some(name) = expr_name(expr) {
                    refs.entry(name.to_string())
                        .or_default()
                        .push(byte_span_to_location(uri, &expr.span, map));
                }
            }
            StatementKind::ScriptCommand { command, args } => {
                if let Some(db) = db
                    && let Ok(cmd) = db.get_command(command)
                {
                    for (i, arg) in args.iter().enumerate() {
                        if let Some(param) = cmd.params.get(i)
                            && (param.param_type == ParamType::Label
                                || param.name == "relative_jump")
                            && let Some(name) = expr_name(arg)
                        {
                            refs.entry(name.to_string())
                                .or_default()
                                .push(byte_span_to_location(uri, &arg.span, map));
                        }
                    }
                }
            }
            StatementKind::Function { body, .. }
            | StatementKind::Action { body, .. }
            | StatementKind::WhileStatement { body, .. } => {
                count_refs(body, uri, map, refs, db);
            }
            StatementKind::IfStatement {
                body, elseblock, ..
            } => {
                count_refs(body, uri, map, refs, db);
                if let Some(else_b) = elseblock {
                    count_refs(else_b, uri, map, refs, db);
                }
            }
            StatementKind::MatchStatement { cases, default, .. } => {
                for case in cases {
                    count_refs(&case.body, uri, map, refs, db);
                }
                if let Some(default) = default {
                    count_refs(default, uri, map, refs, db);
                }
            }
            _ => {}
        }
    }
}

fn emit_lenses(
    items: &[Statement],
    uri: &Url,
    map: &SourceMap,
    refs: &HashMap<String, Vec<Location>>,
    lenses: &mut Vec<CodeLens>,
) {
    for item in items {
        match &item.node {
            StatementKind::Function { headers, body, .. } => {
                for header in headers {
                    let locations = refs.get(&header.name).map_or(&[] as &[_], |v| v.as_slice());
                    lenses.push(make_ref_lens(&item.span, map, uri, locations));
                }
                emit_lenses(body, uri, map, refs, lenses);
            }
            StatementKind::Action { name, body, .. } => {
                let locations = refs.get(name).map_or(&[] as &[_], |v| v.as_slice());
                lenses.push(make_ref_lens(&item.span, map, uri, locations));
                emit_lenses(body, uri, map, refs, lenses);
            }
            StatementKind::AliasStatement { name, .. } | StatementKind::Label(name) => {
                let locations = refs.get(name).map_or(&[] as &[_], |v| v.as_slice());
                lenses.push(make_ref_lens(&item.span, map, uri, locations));
            }
            StatementKind::IfStatement {
                body, elseblock, ..
            } => {
                emit_lenses(body, uri, map, refs, lenses);
                if let Some(else_b) = elseblock {
                    emit_lenses(else_b, uri, map, refs, lenses);
                }
            }
            StatementKind::WhileStatement { body, .. } => {
                emit_lenses(body, uri, map, refs, lenses);
            }
            StatementKind::MatchStatement { cases, default, .. } => {
                for case in cases {
                    emit_lenses(&case.body, uri, map, refs, lenses);
                }
                if let Some(default) = default {
                    emit_lenses(default, uri, map, refs, lenses);
                }
            }
            _ => {}
        }
    }
}

fn make_ref_lens(
    span: &std::ops::Range<usize>,
    map: &SourceMap,
    uri: &Url,
    locations: &[Location],
) -> CodeLens {
    let range = byte_span_to_range(map, span);
    CodeLens {
        range,
        command: Some(Command {
            title: format!(
                "{} reference{}",
                locations.len(),
                if locations.len() == 1 { "" } else { "s" }
            ),
            command: "editor.action.showReferences".to_string(),
            arguments: Some(vec![
                serde_json::json!(uri.as_str()),
                serde_json::json!(range.start),
                serde_json::json!(locations),
            ]),
        }),
        data: None,
    }
}

fn expr_name(expr: &rotom::compiler::ast::Expression) -> Option<&str> {
    match &expr.node {
        ExpressionKind::Identifier(name) | ExpressionKind::Label(name) => Some(name),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{build_message_code_lens, compute_script_code_lens, is_message_archive_uri};
    use crate::message_refs::MessageRef;
    use std::path::PathBuf;
    use tower_lsp::lsp_types::Url;

    #[test]
    fn archive_uri_matches_known_layouts() {
        let dspre =
            Url::from_file_path("/tmp/project/expanded/textArchives/0199.json").expect("dspre uri");
        let decomp = Url::from_file_path("/tmp/project/res/text/0001.json").expect("decomp uri");
        let no = Url::from_file_path("/tmp/project/scripts/test.rotom").expect("script uri");
        assert!(is_message_archive_uri(&dspre));
        assert!(is_message_archive_uri(&decomp));
        assert!(!is_message_archive_uri(&no));
    }

    #[test]
    fn script_code_lens_counts_label_references() {
        let uri = Url::from_file_path("/tmp/test.rotom").expect("uri");
        let source = "script Main # 0:\n    Jump Helper\nHelper:\n    End\n";
        let lenses = compute_script_code_lens(source, &uri, None);
        assert!(!lenses.is_empty());
        assert!(lenses.iter().any(|l| {
            l.command
                .as_ref()
                .is_some_and(|c| c.title.contains("1 reference"))
        }));
    }

    #[test]
    fn builds_show_references_lenses() {
        let archive_uri =
            Url::from_file_path("/tmp/project/textArchives/0199.json").expect("archive uri");
        let lenses = build_message_code_lens(
            &archive_uri,
            vec![(
                12,
                vec![
                    MessageRef {
                        script_path: PathBuf::from("/tmp/project/scripts/a.rotom"),
                        line: 4,
                        command: "Message".to_string(),
                    },
                    MessageRef {
                        script_path: PathBuf::from("/tmp/project/scripts/b.rotom"),
                        line: 8,
                        command: "Message".to_string(),
                    },
                ],
            )],
        );
        assert_eq!(lenses.len(), 1);
        let command = lenses[0].command.as_ref().expect("command");
        assert_eq!(command.command, "editor.action.showReferences");
        assert_eq!(command.title, "2 references");
    }
}
