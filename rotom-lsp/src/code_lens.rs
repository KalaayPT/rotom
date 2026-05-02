use std::collections::HashMap;

use tower_lsp::lsp_types::{CodeLens, Command, Location, Position as LspPosition, Range, Url};

use rotom::compiler::{
    ast::{ExpressionKind, Statement, StatementKind},
    lexer::Lexer,
    parser::Parser,
    sourcemap::SourceMap,
};

/// Produce `CodeLens` hints for a Rotom source file.
///
/// Shows reference counts above scripts, labels, aliases, and actions.
pub fn compute_code_lens(source: &str, uri: &Url) -> Vec<CodeLens> {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new_fallible(lexer);
    let Some(ast) = parser.parse_script_file().ok() else {
        return Vec::new();
    };

    let map = SourceMap::new(source);
    let mut refs: HashMap<String, Vec<Location>> = HashMap::new();

    // First pass: count every reference.
    count_refs(&ast.items, uri, &map, &mut refs);

    // Second pass: emit a lens for every definition.
    let mut lenses = Vec::new();
    emit_lenses(&ast.items, uri, &map, &refs, &mut lenses);
    lenses
}

fn count_refs(
    items: &[Statement],
    uri: &Url,
    map: &SourceMap,
    refs: &mut HashMap<String, Vec<Location>>,
) {
    for item in items {
        match &item.node {
            StatementKind::Jump(expr) => {
                if let Some(name) = expr_name(expr) {
                    refs.entry(name.to_string())
                        .or_default()
                        .push(make_location(uri, &expr.span, map));
                }
            }
            StatementKind::ScriptCommand { command, args } => {
                let lower = command.to_lowercase();
                if lower.contains("jump") || lower.contains("call") || lower.contains("goto") {
                    for arg in args {
                        if let Some(name) = expr_name(arg) {
                            refs.entry(name.to_string())
                                .or_default()
                                .push(make_location(uri, &arg.span, map));
                        }
                    }
                }
            }
            StatementKind::Function { body, .. }
            | StatementKind::Action { body, .. }
            | StatementKind::WhileStatement { body, .. } => {
                count_refs(body, uri, map, refs);
            }
            StatementKind::IfStatement { body, elseblock, .. } => {
                count_refs(body, uri, map, refs);
                if let Some(else_b) = elseblock {
                    count_refs(else_b, uri, map, refs);
                }
            }
            StatementKind::MatchStatement { cases, default, .. } => {
                for case in cases {
                    count_refs(&case.body, uri, map, refs);
                }
                if let Some(default) = default {
                    count_refs(default, uri, map, refs);
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
            StatementKind::IfStatement { body, elseblock, .. } => {
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
    let range = make_range(span, map);
    CodeLens {
        range,
        command: Some(Command {
            title: format!(
                "{} reference{}",
                locations.len(),
                if locations.len() == 1 { "" } else { "s" }
            ),
            command: "rotom.showReferences".to_string(),
            arguments: Some(vec![
                serde_json::json!(uri.as_str()),
                serde_json::json!(range.start),
                serde_json::json!(locations),
            ]),
        }),
        data: None,
    }
}

fn make_location(uri: &Url, span: &std::ops::Range<usize>, map: &SourceMap) -> Location {
    Location {
        uri: uri.clone(),
        range: make_range(span, map),
    }
}

fn make_range(span: &std::ops::Range<usize>, map: &SourceMap) -> Range {
    let start = map.byte_to_position(span.start);
    let end = map.byte_to_position(span.end);
    Range {
        start: LspPosition {
            line: start.line,
            character: start.character,
        },
        end: LspPosition {
            line: end.line,
            character: end.character,
        },
    }
}

fn expr_name(expr: &rotom::compiler::ast::Expression) -> Option<&str> {
    match &expr.node {
        ExpressionKind::Identifier(name) | ExpressionKind::Label(name) => Some(name),
        _ => None,
    }
}
