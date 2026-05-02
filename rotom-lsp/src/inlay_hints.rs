use tower_lsp::lsp_types::{InlayHint, InlayHintKind, InlayHintLabel, Position as LspPosition};

use rotom::compiler::{
    ast::StatementKind,
    sourcemap::SourceMap,
};
use rotom::database::DatabaseV2;

use crate::util::parse_source;

/// Produce inlay hints showing command parameter names before each argument.
///
/// Renders as ghost text inline:
/// - Call-style: `GiveItem(/*item:*/5, /*quantity:*/1)`
/// - Space-separated: `GiveItem /*item:*/5 /*quantity:*/1`
pub fn compute_inlay_hints(source: &str, db: Option<&DatabaseV2>) -> Vec<InlayHint> {
    let Some(db) = db else {
        return Vec::new();
    };
    let Some(ast) = parse_source(source) else {
        return Vec::new();
    };

    let map = SourceMap::new(source);
    let mut hints = Vec::new();

    for item in &ast.items {
        walk_statement(&item.node, db, &map, &mut hints);
    }

    hints
}

fn walk_statement(
    stmt: &rotom::compiler::ast::StatementKind,
    db: &DatabaseV2,
    map: &SourceMap,
    hints: &mut Vec<InlayHint>,
) {
    match stmt {
        StatementKind::ScriptCommand { command, args } => {
            if let Ok(cmd) = db.get_command(command) {
                for (i, arg) in args.iter().enumerate() {
                    if let Some(param) = cmd.params.get(i) {
                        let pos = map.byte_to_position(arg.span.start);
                        hints.push(InlayHint {
                            position: LspPosition {
                                line: pos.line,
                                character: pos.character,
                            },
                            label: InlayHintLabel::String(
                                if param.optional {
                                    format!("[{}]: ", param.name)
                                } else {
                                    format!("{}: ", param.name)
                                }
                            ),
                            kind: Some(InlayHintKind::PARAMETER),
                            text_edits: None,
                            tooltip: None,
                            padding_left: Some(false),
                            padding_right: Some(false),
                            data: None,
                        });
                    }
                }
            }
        }
        StatementKind::IfStatement { condition, body, elseblock, .. } => {
            walk_expression(condition, db, map, hints);
            for stmt in body {
                walk_statement(&stmt.node, db, map, hints);
            }
            if let Some(else_b) = elseblock {
                for stmt in else_b {
                    walk_statement(&stmt.node, db, map, hints);
                }
            }
        }
        StatementKind::WhileStatement { condition, body, .. } => {
            walk_expression(condition, db, map, hints);
            for stmt in body {
                walk_statement(&stmt.node, db, map, hints);
            }
        }
        StatementKind::MatchStatement { subject, cases, default, .. } => {
            walk_expression(subject, db, map, hints);
            for case in cases {
                for val in &case.values {
                    walk_expression(val, db, map, hints);
                }
                for stmt in &case.body {
                    walk_statement(&stmt.node, db, map, hints);
                }
            }
            if let Some(default) = default {
                for stmt in default {
                    walk_statement(&stmt.node, db, map, hints);
                }
            }
        }
        StatementKind::Function { body, .. } | StatementKind::Action { body, .. } => {
            for stmt in body {
                walk_statement(&stmt.node, db, map, hints);
            }
        }
        StatementKind::AliasStatement { value, .. } => {
            walk_expression(value, db, map, hints);
        }
        _ => {}
    }
}

#[allow(clippy::only_used_in_recursion)]
fn walk_expression(
    expr: &rotom::compiler::ast::Expression,
    db: &DatabaseV2,
    map: &SourceMap,
    hints: &mut Vec<InlayHint>,
) {
    match &expr.node {
        rotom::compiler::ast::ExpressionKind::Prefix { id, .. } => {
            walk_expression(id, db, map, hints);
        }
        rotom::compiler::ast::ExpressionKind::Infix { left, right, .. } => {
            walk_expression(left, db, map, hints);
            walk_expression(right, db, map, hints);
        }
        rotom::compiler::ast::ExpressionKind::Call { function, args } => {
            walk_expression(function, db, map, hints);
            for arg in args {
                walk_expression(arg, db, map, hints);
            }
        }
        _ => {}
    }
}
