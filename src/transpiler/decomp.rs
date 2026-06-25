//! Decomp Script Transpiler
//!
//! Converts decomp assembly script format to Rotoscript format.
//!
//! ## Decomp Format:
//! ```text
//!     ScriptEntry FunctionName
//!     ScriptEntry OtherFunction
//!     ScriptEntryEnd
//!
//! FunctionName:
//!     LockAll
//!     Message 0
//!     End
//!
//!     .balign 4, 0
//! MovementName:
//!     WalkNorth
//!     EndMovement
//! ```
//!
//! ## Rotoscript Output:
//! ```text
//! script FunctionName #1:
//!     LockAll
//!     Message 0
//!     End
//!
//! action MovementName:
//!     WalkNorth
//!     EndMovement
//! ```

use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::path::Path;

use crate::BinaryQuirk;
use crate::autovar::is_autovar_param;
use crate::compiler::{Lexer, Parser};
use crate::database::CommandType;

#[derive(Debug, Clone)]
pub struct TranspileResult {
    pub source: String,
    pub binary_quirks: BinaryQuirk,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranspileError {
    pub message: String,
    pub line: usize,
}

impl std::fmt::Display for TranspileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for TranspileError {}

#[derive(Debug, Default)]
struct PrepassData {
    function_to_slots: HashMap<String, Vec<usize>>,
    movement_labels: HashSet<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum ScriptEntryDirective<'a> {
    Entry(&'a str),
    End,
    Other,
}

#[derive(Debug, PartialEq, Eq)]
enum StructuralLine<'a> {
    ScriptEntry(&'a str),
    ScriptEntryEnd,
    AssemblerDirective,
    Label(&'a str),
    Other,
}

enum BodyLine<'a> {
    Empty,
    SkipPreprocessor,
    PreserveInclude(&'a str),
    TranslateDefine {
        name: &'a str,
        value: &'a str,
    },
    ErrorFunctionMacro(&'a str),
    FullComment(String),
    Content {
        statement: &'a str,
        inline_comment: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SwitchCaseLine {
    value: String,
    target: String,
}

#[derive(Default)]
struct RenderState {
    jump_table_end_marker_count: u8,
    seen_script_entry_end: bool,
    seen_first_label_after_entry_end: bool,
    seen_labels: HashSet<String>,
    last_emitted_endmovement: bool,
    synthetic_unused_end_counter: usize,
}

/// Parses `asm/macros/scrcmd.inc` from `decomp_root` and returns a map from command name to
/// the set of binary param indices that are macro-optional (have `=DEFAULT` in the `.macro`
/// line). The macro body's `\paramName` references are in binary order, so we map optional
/// macro params to their binary position by counting `\` references.
fn parse_macro_optional_param_indices(decomp_root: &Path) -> HashMap<String, HashSet<usize>> {
    let inc_path = decomp_root.join("asm/macros/scrcmd.inc");
    let Ok(text) = std::fs::read_to_string(&inc_path) else {
        return HashMap::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    let mut result: HashMap<String, HashSet<usize>> = HashMap::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(rest) = lines[i].trim().strip_prefix(".macro ") else {
            i += 1;
            continue;
        };
        let (macro_name, params_str) = rest.split_once(' ').unwrap_or((rest, ""));
        let macro_name = macro_name.trim();

        // Names of params that have =DEFAULT in the macro signature.
        let optional_names: HashSet<&str> = params_str
            .split(',')
            .filter_map(|p| {
                let p = p.trim();
                let eq = p.find('=')?;
                Some(p[..eq].trim())
            })
            .filter(|n| !n.is_empty())
            .collect();

        // Walk the macro body: each `\paramName` is one binary param slot.
        let mut binary_idx = 0usize;
        for body_line in &lines[i + 1..] {
            if body_line.trim().starts_with(".endm") {
                break;
            }
            if body_line.contains('\\') {
                let ref_name = body_line
                    .split('\\')
                    .nth(1)
                    .and_then(|s| s.split_whitespace().next())
                    .unwrap_or("");
                if optional_names.contains(ref_name) {
                    result
                        .entry(macro_name.to_string())
                        .or_default()
                        .insert(binary_idx);
                }
                binary_idx += 1;
            }
        }
        i += 1;
    }
    result
}

/// Transpile a decomp script to Rotoscript format
pub fn transpile(
    input: &str,
    db: Option<&crate::database::DatabaseV2>,
    decomp_root: Option<&Path>,
) -> Result<TranspileResult, TranspileError> {
    let expanded;
    let input = if let Some(root) = decomp_root {
        expanded = preexpand_generated_includes(input, root);
        &expanded as &str
    } else {
        input
    };
    let macro_optional = decomp_root
        .map_or_else(HashMap::new, parse_macro_optional_param_indices);
    let prepass = collect_prepass_data(input, db);
    render_transpile_body(input, &prepass, db, &macro_optional)
}

fn render_transpile_body(
    input: &str,
    prepass: &PrepassData,
    db: Option<&crate::database::DatabaseV2>,
    macro_optional: &HashMap<String, HashSet<usize>>,
) -> Result<TranspileResult, TranspileError> {
    let mut output = String::new();
    let mut state = RenderState::default();
    let lines: Vec<&str> = input.lines().collect();
    let mut line_idx = 0usize;

    while line_idx < lines.len() {
        let raw_trimmed = lines[line_idx].trim();

        match preprocess_body_line(raw_trimmed) {
            BodyLine::Empty => {
                if state.seen_script_entry_end {
                    output.push('\n');
                }
            }
            BodyLine::SkipPreprocessor => {}
            BodyLine::PreserveInclude(include_line) => {
                output.push_str(include_line);
                output.push('\n');
            }
            BodyLine::TranslateDefine { name, value } => {
                let _ = writeln!(output, "alias {} as {}", value, name);
            }
            BodyLine::ErrorFunctionMacro(name) => {
                return Err(TranspileError {
                    message: format!(
                        "Function-like macro '{}' cannot be converted to rotom alias syntax. Remove it manually or refactor to a simple #define.",
                        name
                    ),
                    line: line_idx + 1,
                });
            }
            BodyLine::FullComment(comment) => {
                output.push_str(&comment);
                output.push('\n');
            }
            BodyLine::Content {
                statement,
                inline_comment,
            } => {
                process_content_line(
                    statement,
                    inline_comment.as_deref(),
                    line_idx + 1,
                    prepass,
                    db,
                    macro_optional,
                    &mut state,
                    &mut output,
                )?;
            }
        }

        line_idx += 1;
    }

    Ok(TranspileResult {
        source: output,
        binary_quirks: BinaryQuirk {
            jump_table_end_marker_count: if state.jump_table_end_marker_count == 1 {
                None
            } else {
                Some(state.jump_table_end_marker_count)
            },
            ..Default::default()
        },
    })
}

fn render_label_line(
    label_name: &str,
    inline_comment: Option<&str>,
    prepass: &PrepassData,
    state: &mut RenderState,
    line_number: usize,
    output: &mut String,
) -> Result<(), TranspileError> {
    if !state.seen_labels.insert(label_name.to_string()) {
        return Err(TranspileError {
            message: format!("duplicate label definition '{}'", label_name),
            line: line_number,
        });
    }

    // Check if this is a movement label
    if prepass.movement_labels.contains(label_name) {
        let _ = write!(output, "action {}:", label_name);
        append_inline_comment(output, inline_comment);
        output.push('\n');
        return Ok(());
    }

    if let Some(slots) = prepass.function_to_slots.get(label_name) {
        // Public script (in jump table): emit header for each slot.
        // Jump table indices are 0-based; source slot IDs are 1-based.
        for slot in slots {
            let _ = write!(output, "script {} #{}", label_name, slot + 1);
            append_inline_comment(output, inline_comment);
            output.push_str(":\n");
        }
        return Ok(());
    }

    // Private label
    let _ = write!(output, "{}:", label_name);
    append_inline_comment(output, inline_comment);
    output.push('\n');
    state.last_emitted_endmovement = false;
    Ok(())
}

/// Render one parsed decomp statement into Rotoscript output.
#[allow(clippy::too_many_arguments)]
fn process_content_line(
    statement: &str,
    inline_comment: Option<&str>,
    line_number: usize,
    prepass: &PrepassData,
    db: Option<&crate::database::DatabaseV2>,
    macro_optional: &HashMap<String, HashSet<usize>>,
    state: &mut RenderState,
    output: &mut String,
) -> Result<(), TranspileError> {
    if skip_or_handle_structural_line(
        statement,
        inline_comment,
        line_number,
        prepass,
        state,
        output,
    )? {
        return Ok(());
    }

    if !state.seen_script_entry_end {
        return Ok(());
    }

    if should_emit_synthetic_unused_end(statement, state) {
        output.push_str("_unused_end:\n");
        output.push_str("    End\n");
        state.seen_first_label_after_entry_end = true;
        state.last_emitted_endmovement = false;
        return Ok(());
    }

    if should_emit_synthetic_post_movement_end(statement, state) {
        render_synthetic_unused_end_label(state, output);
        output.push_str("    End");
        append_inline_comment(output, inline_comment);
        output.push('\n');
        state.last_emitted_endmovement = false;
        return Ok(());
    }

    if let Some(subject) = parse_switch_line(statement) {
        render_switch_line(subject, inline_comment, output);
        state.last_emitted_endmovement = false;
        return Ok(());
    }

    if let Some(case_line) = parse_case_line(statement) {
        render_case_line(&case_line, inline_comment, output);
        state.last_emitted_endmovement = false;
        return Ok(());
    }

    render_command_line(statement, inline_comment, db, macro_optional, output);
    state.last_emitted_endmovement = split_command_and_args(statement)
        .0
        .eq_ignore_ascii_case("EndMovement");
    Ok(())
}

fn preprocess_body_line(raw_trimmed: &str) -> BodyLine<'_> {
    if raw_trimmed.is_empty() {
        return BodyLine::Empty;
    }

    if raw_trimmed.starts_with("#include") {
        return BodyLine::PreserveInclude(raw_trimmed);
    }

    if raw_trimmed.starts_with("#define") {
        if let Some((name, value)) = parse_local_define_line(raw_trimmed) {
            if name.contains('(') {
                return BodyLine::ErrorFunctionMacro(name);
            }
            if can_parse_rotom_alias_value(value) {
                return BodyLine::TranslateDefine { name, value };
            }
            return BodyLine::FullComment(format!("// {}", raw_trimmed));
        }
        return BodyLine::SkipPreprocessor;
    }

    if raw_trimmed.starts_with('#') {
        return BodyLine::SkipPreprocessor;
    }

    if raw_trimmed.starts_with('@') || raw_trimmed.starts_with("//") || raw_trimmed.starts_with(';')
    {
        // Keep regular comments, converting decomp comment markers to //.
        let comment = if raw_trimmed.starts_with('@') || raw_trimmed.starts_with(';') {
            raw_trimmed.replacen(raw_trimmed.chars().next().unwrap(), "//", 1)
        } else {
            raw_trimmed.to_string()
        };
        return BodyLine::FullComment(comment);
    }

    let comment_start = [raw_trimmed.find('@'), raw_trimmed.find(';')]
        .into_iter()
        .flatten()
        .min();

    let (trimmed, inline_comment) = if let Some(idx) = comment_start {
        let comment_marker = raw_trimmed[idx..].chars().next().unwrap();
        (
            raw_trimmed[..idx].trim(),
            Some(raw_trimmed[idx..].replacen(comment_marker, "//", 1)),
        )
    } else {
        (raw_trimmed, None)
    };

    BodyLine::Content {
        statement: trimmed,
        inline_comment,
    }
}

fn parse_switch_line(statement: &str) -> Option<&str> {
    let (cmd_name, args) = split_command_and_args(statement);
    if !cmd_name.eq_ignore_ascii_case("switch") {
        return None;
    }

    let subject = args?.trim();
    if subject.is_empty() {
        None
    } else {
        Some(subject)
    }
}

fn render_switch_line(subject: &str, inline_comment: Option<&str>, output: &mut String) {
    output.push_str("    CopyVar 0x8008, ");
    output.push_str(subject);
    append_inline_comment(output, inline_comment);
    output.push('\n');
}

fn render_command_line(
    statement: &str,
    inline_comment: Option<&str>,
    db: Option<&crate::database::DatabaseV2>,
    macro_optional: &HashMap<String, HashSet<usize>>,
    output: &mut String,
) {
    let (raw_cmd_name, args) = split_command_and_args(statement);
    let cmd_name = resolve_script_command_name(raw_cmd_name, db);

    output.push_str("    ");
    output.push_str(cmd_name.as_ref());
    if let Some(args) = args {
        let normalized_args = normalize_command_args(cmd_name.as_ref(), args, db, macro_optional);
        if !normalized_args.is_empty() {
            output.push(' ');
            output.push_str(&normalized_args);
        }
    }
    append_inline_comment(output, inline_comment);
    output.push('\n');
}

fn skip_or_handle_structural_line(
    statement: &str,
    inline_comment: Option<&str>,
    line_number: usize,
    prepass: &PrepassData,
    state: &mut RenderState,
    output: &mut String,
) -> Result<bool, TranspileError> {
    match classify_structural_line(statement) {
        StructuralLine::ScriptEntry(_) | StructuralLine::AssemblerDirective => Ok(true),
        StructuralLine::ScriptEntryEnd => {
            state.seen_script_entry_end = true;
            state.jump_table_end_marker_count = state.jump_table_end_marker_count.saturating_add(1);
            Ok(true)
        }
        StructuralLine::Label(label_name) => {
            state.seen_script_entry_end = true;
            render_label_line(
                label_name,
                inline_comment,
                prepass,
                state,
                line_number,
                output,
            )?;
            state.seen_first_label_after_entry_end = true;
            Ok(true)
        }
        StructuralLine::Other => Ok(false),
    }
}

fn should_emit_synthetic_unused_end(statement: &str, state: &RenderState) -> bool {
    !state.seen_first_label_after_entry_end && statement == "End"
}

fn parse_case_line(statement: &str) -> Option<SwitchCaseLine> {
    let (cmd_name, rest) = split_command_and_args(statement);
    if !cmd_name.eq_ignore_ascii_case("case") {
        return None;
    }

    let rest = rest?.trim();
    let (value, target) = rest.split_once(',')?;
    let value = value.trim();
    let target = target.trim();

    if value.is_empty() || target.is_empty() {
        return None;
    }

    Some(SwitchCaseLine {
        value: value.to_string(),
        target: target.to_string(),
    })
}

fn render_case_line(case_line: &SwitchCaseLine, inline_comment: Option<&str>, output: &mut String) {
    output.push_str("    CompareVarValue 0x8008, ");
    output.push_str(&case_line.value);
    append_inline_comment(output, inline_comment);
    output.push('\n');
    let _ = writeln!(output, "    JumpIf EQUAL, {}", case_line.target);
}

fn append_inline_comment(output: &mut String, inline_comment: Option<&str>) {
    if let Some(comment) = inline_comment {
        output.push(' ');
        output.push_str(comment);
    }
}

fn should_emit_synthetic_post_movement_end(statement: &str, state: &RenderState) -> bool {
    state.last_emitted_endmovement && statement.eq_ignore_ascii_case("End")
}

fn render_synthetic_unused_end_label(state: &mut RenderState, output: &mut String) {
    loop {
        let label_name = format!("_unused_end_{}", state.synthetic_unused_end_counter);
        state.synthetic_unused_end_counter += 1;
        if state.seen_labels.insert(label_name.clone()) {
            output.push_str(&label_name);
            output.push_str(":\n");
            return;
        }
    }
}

fn normalize_command_args(
    cmd_name: &str,
    args: &str,
    db: Option<&crate::database::DatabaseV2>,
    macro_optional: &HashMap<String, HashSet<usize>>,
) -> String {
    if args.is_empty() {
        return String::new();
    }

    if let Some(db) = db {
        reorder_decomp_args_to_binary(cmd_name, args, db, macro_optional)
    } else {
        args.to_owned()
    }
}

fn split_command_and_args(trimmed: &str) -> (&str, Option<&str>) {
    if let Some(cmd_end_idx) = trimmed.find([' ', '\t']) {
        let cmd_name = &trimmed[..cmd_end_idx];
        let args = trimmed[cmd_end_idx..].trim();
        (cmd_name, Some(args))
    } else {
        (trimmed, None)
    }
}

fn collect_prepass_data(input: &str, db: Option<&crate::database::DatabaseV2>) -> PrepassData {
    let lines: Vec<&str> = input.lines().collect();
    let movement_commands = movement_commands_from_db(db);
    let (jump_table, movement_labels) =
        collect_jump_table_and_movement_labels(&lines, &movement_commands);

    PrepassData {
        function_to_slots: build_function_slot_map(&jump_table),
        movement_labels,
    }
}

fn movement_commands_from_db(db: Option<&crate::database::DatabaseV2>) -> HashSet<&str> {
    db.map(|db| {
        db.commands
            .iter()
            .filter(|(_, c)| c.cmd_type == CommandType::Movement)
            .map(|(name, _)| name.as_str())
            .collect()
    })
    .unwrap_or_default()
}

fn collect_jump_table_and_movement_labels(
    lines: &[&str],
    movement_commands: &HashSet<&str>,
) -> (Vec<String>, HashSet<String>) {
    let mut jump_table: Vec<String> = Vec::new();
    let mut movement_labels: HashSet<String> = HashSet::new();
    let mut current_label: Option<String> = None;
    let mut saw_balign = false;

    for (line_idx, line) in lines.iter().enumerate() {
        let statement = match preprocess_body_line(line.trim()) {
            BodyLine::Empty
            | BodyLine::SkipPreprocessor
            | BodyLine::PreserveInclude(_)
            | BodyLine::TranslateDefine { .. }
            | BodyLine::ErrorFunctionMacro(_)
            | BodyLine::FullComment(_) => continue,
            BodyLine::Content { statement, .. } => statement,
        };

        match classify_structural_line(statement) {
            StructuralLine::ScriptEntry(name) => {
                jump_table.push(name.to_string());
                saw_balign = false;
                continue;
            }
            StructuralLine::ScriptEntryEnd => {
                saw_balign = false;
                continue;
            }
            StructuralLine::AssemblerDirective => {
                // .balign before a label is a strong signal for movement data
                if statement.trim().starts_with(".balign") {
                    saw_balign = true;
                }
                continue;
            }
            StructuralLine::Label(label_name) => {
                // If a label follows .balign, treat it as a movement label immediately.
                let name = label_name.to_string();
                if saw_balign {
                    movement_labels.insert(name);
                } else {
                    current_label = Some(name);
                }
                saw_balign = false;
                continue;
            }
            StructuralLine::Other => {}
        }

        if let Some(label) = current_label.take() {
            let cmd_name = split_command_and_args(statement).0;

            let is_movement = if movement_commands.is_empty() {
                lookahead_for_end_movement(lines, line_idx)
            } else {
                movement_commands.contains(cmd_name)
            };

            if is_movement {
                movement_labels.insert(label);
            }
        }
    }

    (jump_table, movement_labels)
}

fn build_function_slot_map(jump_table: &[String]) -> HashMap<String, Vec<usize>> {
    // A script can appear multiple times in ScriptEntry; preserve all slots.
    let mut function_to_slots: HashMap<String, Vec<usize>> = HashMap::new();
    for (slot_idx, name) in jump_table.iter().enumerate() {
        function_to_slots
            .entry(name.clone())
            .or_default()
            .push(slot_idx);
    }
    function_to_slots
}

fn parse_local_define_line(trimmed: &str) -> Option<(&str, &str)> {
    let rest = trimmed.strip_prefix("#define")?;

    let mut chars = rest.chars();
    let first_char = chars.next()?;
    if !first_char.is_whitespace() {
        return None;
    }

    let rest = rest.trim_start();
    let split_idx = rest.find(char::is_whitespace)?;
    let (name, value) = rest.split_at(split_idx);
    let value = value.trim();

    if name.is_empty() || value.is_empty() {
        return None;
    }

    Some((name, value))
}

fn can_parse_rotom_alias_value(value: &str) -> bool {
    let source = format!("alias {} as TEST_ALIAS\n", value);
    let lexer = Lexer::new(&source);
    let mut parser = Parser::new(lexer);

    parser.parse_alias().is_ok()
}

fn parse_script_entry_directive(trimmed: &str) -> ScriptEntryDirective<'_> {
    if trimmed == "ScriptEntryEnd" || trimmed == "ScrDefEnd" {
        return ScriptEntryDirective::End;
    }

    let Some(rest) = trimmed
        .strip_prefix("ScriptEntry")
        .or_else(|| trimmed.strip_prefix("ScrDef"))
    else {
        return ScriptEntryDirective::Other;
    };

    let mut chars = rest.chars();
    let Some(first_char) = chars.next() else {
        return ScriptEntryDirective::Other;
    };
    if !first_char.is_whitespace() {
        return ScriptEntryDirective::Other;
    }

    let rest = rest.trim();
    let name = rest.split('@').next().unwrap_or(rest).trim();
    let name = name.split("//").next().unwrap_or(name).trim();
    if name.is_empty() {
        ScriptEntryDirective::Other
    } else {
        ScriptEntryDirective::Entry(name)
    }
}

fn classify_structural_line(trimmed: &str) -> StructuralLine<'_> {
    match parse_script_entry_directive(trimmed) {
        ScriptEntryDirective::Entry(name) => return StructuralLine::ScriptEntry(name),
        ScriptEntryDirective::End => return StructuralLine::ScriptEntryEnd,
        ScriptEntryDirective::Other => {}
    }

    if trimmed.starts_with('.') {
        return StructuralLine::AssemblerDirective;
    }

    if let Some(label_name) = trimmed.strip_suffix(':') {
        return StructuralLine::Label(label_name);
    }

    StructuralLine::Other
}

fn resolve_script_command_name<'a>(
    cmd_name: &'a str,
    db: Option<&'a crate::database::DatabaseV2>,
) -> std::borrow::Cow<'a, str> {
    let Some(db) = db else {
        return std::borrow::Cow::Borrowed(cmd_name);
    };

    resolve_opcode_alias(db, cmd_name).map_or_else(
        || std::borrow::Cow::Borrowed(cmd_name),
        std::borrow::Cow::Owned,
    )
}

fn resolve_opcode_alias(db: &crate::database::DatabaseV2, cmd_name: &str) -> Option<String> {
    db.get_script_cmd_by_alias(cmd_name)
        .map(|(name, _)| name.clone())
}

/// Rewrites shortened decomp macro calls into binary parameter order.
///
/// Decomp `.macro` definitions can either keep args in binary order or move optional args
/// (those with `=DEFAULT` syntax) to the end. A param having a `default` in the v2 DB does not
/// mean the macro allows omitting it — some defaults are binary-level only. This function
/// uses `macro_optional_indices` (parsed from `scrcmd.inc`) to identify which binary param
/// positions are actually omittable in the macro.
fn reorder_decomp_args_to_binary(
    cmd_name: &str,
    args_str: &str,
    db: &crate::database::DatabaseV2,
    macro_optional_indices: &HashMap<String, HashSet<usize>>,
) -> String {
    let Ok(cmd) = db.get_command(cmd_name) else {
        return args_str.to_owned();
    };

    let params = &cmd.params;
    if params.is_empty() {
        return args_str.to_owned();
    }

    let cmd_optional = macro_optional_indices.get(cmd_name);
    let mut required_indices: Vec<usize> = Vec::new();
    let mut optional_indices: Vec<usize> = Vec::new();
    for (i, p) in params.iter().enumerate() {
        let is_optional = p.default.is_some()
            && !is_autovar_param(p)
            && cmd_optional.is_some_and(|s| s.contains(&i));
        if is_optional {
            optional_indices.push(i);
        } else {
            required_indices.push(i);
        }
    }

    if optional_indices.is_empty() {
        return args_str.to_owned();
    }

    let all_optional_at_end = optional_indices
        .iter()
        .all(|&i| i >= required_indices.len());
    if all_optional_at_end {
        return args_str.to_owned();
    }

    let args: Vec<&str> = args_str.split(',').map(str::trim).collect();

    let req_count = required_indices.len();
    let total_params = params.len();

    if args.len() < req_count || args.len() > total_params {
        return args_str.to_owned();
    }

    let provided_optional_count = args.len() - req_count;

    let mut result: Vec<Option<&str>> = vec![None; total_params];

    for (decomp_idx, &binary_idx) in required_indices.iter().enumerate() {
        if decomp_idx < args.len() {
            result[binary_idx] = Some(args[decomp_idx]);
        }
    }

    for (opt_num, &binary_idx) in optional_indices.iter().enumerate() {
        if opt_num < provided_optional_count {
            let decomp_idx = req_count + opt_num;
            if decomp_idx < args.len() {
                result[binary_idx] = Some(args[decomp_idx]);
            }
        } else {
            result[binary_idx] = Some(params[binary_idx].default.as_deref().unwrap_or_default());
        }
    }

    let final_args: Vec<&str> = result.into_iter().flatten().collect();
    final_args.join(", ")
}

/// Pre-expand build-generated `#include` directives before the main transpile
/// pass so that the resulting `ScriptEntry` calls go through the normal
/// prepass and get converted to rotom jump-table format.
///
/// Two includes are handled, mirroring the C generator tools exactly:
///
/// `res/trainers/trainer_scripts.h` (`trainerproc`):
///   N × `ScriptEntry Battles_Trainer` (one per trainer JSON),
///   then `ScriptEntry Battles_ApproachingTrainer`, then `ScriptEntryEnd`.
///
/// `res/items/hidden_item_scripts.h` (`itemproc`):
///   (maxScriptID + 1) × `ScriptEntry HiddenItems_Item` derived from the
///   `script` field of `gHiddenItems` in `include/data/field/hidden_items.h`,
///   then `ScriptEntryEnd`.
fn preexpand_generated_includes(input: &str, decomp_root: &Path) -> String {
    if !input.contains("res/trainers/trainer_scripts.h")
        && !input.contains("res/items/hidden_item_scripts.h")
    {
        return input.to_owned();
    }

    let mut out = String::with_capacity(input.len() + 4096);
    for line in input.lines() {
        let trimmed = line.trim();
        if let Some(replacement) = expand_one_generated_include(trimmed, decomp_root) {
            out.push_str(&replacement);
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn expand_one_generated_include(include_line: &str, decomp_root: &Path) -> Option<String> {
    let path = include_line
        .strip_prefix("#include \"")?
        .strip_suffix('"')?;

    match path {
        "res/trainers/trainer_scripts.h" => {
            let trainer_count = std::fs::read_dir(decomp_root.join("res/trainers/data"))
                .ok()?
                .filter_map(std::result::Result::ok)
                .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("json"))
                .count();
            if trainer_count == 0 {
                return None;
            }
            let mut out = String::new();
            for _ in 0..trainer_count {
                out.push_str("    ScriptEntry Battles_Trainer\n");
            }
            out.push_str("    ScriptEntry Battles_ApproachingTrainer\n");
            out.push_str("    ScriptEntryEnd\n");
            Some(out)
        }
        "res/items/hidden_item_scripts.h" => {
            let header =
                std::fs::read_to_string(decomp_root.join("include/data/field/hidden_items.h"))
                    .ok()?;
            let max_id = header
                .lines()
                .filter_map(|line| {
                    let line = line.trim();
                    if !line.starts_with("HIDDEN_ITEM_ENTRY(") {
                        return None;
                    }
                    // Format: HIDDEN_ITEM_ENTRY(item, qty, range, script),
                    // rfind ')' then rfind ',' before it to get the script field.
                    let close = line.rfind(')')?;
                    let before_close = &line[..close];
                    let last_comma = before_close.rfind(',')?;
                    before_close[last_comma + 1..].trim().parse::<usize>().ok()
                })
                .max()?;
            let mut out = String::new();
            for _ in 0..=max_id {
                out.push_str("    ScriptEntry HiddenItems_Item\n");
            }
            out.push_str("    ScriptEntryEnd\n");
            Some(out)
        }
        _ => None,
    }
}

fn lookahead_for_end_movement(lines: &[&str], start_idx: usize) -> bool {
    const MAX_LOOKAHEAD: usize = 32;

    for line in lines
        .iter()
        .take(std::cmp::min(start_idx + MAX_LOOKAHEAD, lines.len()))
        .skip(start_idx)
    {
        let trimmed = line.trim();

        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with('@') {
            continue;
        }

        match classify_structural_line(trimmed) {
            StructuralLine::ScriptEntry(_)
            | StructuralLine::ScriptEntryEnd
            | StructuralLine::AssemblerDirective => continue,
            StructuralLine::Label(_) => return false,
            StructuralLine::Other => {}
        }

        let cmd_name = split_command_and_args(trimmed).0;

        if cmd_name == "EndMovement" {
            return true;
        }

        if cmd_name == "End" || cmd_name == "Return" {
            return false;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_prepass_data_keeps_duplicate_function_slots() {
        let input = r"
    ScriptEntry Main
    ScriptEntry Main
    ScriptEntryEnd

Main:
    End
";

        let prepass = collect_prepass_data(input, None);
        assert_eq!(prepass.function_to_slots.get("Main"), Some(&vec![0, 1]));
    }

    #[test]
    fn test_parse_script_entry_directive_distinguishes_end_and_entries() {
        assert_eq!(
            parse_script_entry_directive("ScriptEntry Main"),
            ScriptEntryDirective::Entry("Main")
        );
        assert_eq!(
            parse_script_entry_directive("ScriptEntry Main // trailing"),
            ScriptEntryDirective::Entry("Main")
        );
        assert_eq!(
            parse_script_entry_directive("ScriptEntry Main @ trailing"),
            ScriptEntryDirective::Entry("Main")
        );
        assert_eq!(
            parse_script_entry_directive("ScriptEntryEnd"),
            ScriptEntryDirective::End
        );
        assert_eq!(
            parse_script_entry_directive("ScriptEntryMain"),
            ScriptEntryDirective::Other
        );
    }

    #[test]
    fn test_classify_structural_line_variants() {
        assert_eq!(
            classify_structural_line("ScriptEntry Main"),
            StructuralLine::ScriptEntry("Main")
        );
        assert_eq!(
            classify_structural_line("ScriptEntryEnd"),
            StructuralLine::ScriptEntryEnd
        );
        assert_eq!(
            classify_structural_line(".balign 4, 0"),
            StructuralLine::AssemblerDirective
        );
        assert_eq!(
            classify_structural_line("Main:"),
            StructuralLine::Label("Main")
        );
        assert_eq!(classify_structural_line("Message 1"), StructuralLine::Other);
    }

    #[test]
    fn test_parse_local_define_line_accepts_whitespace_variants() {
        assert_eq!(
            parse_local_define_line("#define FOO BAR"),
            Some(("FOO", "BAR"))
        );
        assert_eq!(
            parse_local_define_line("#define\tFOO\tBAR"),
            Some(("FOO", "BAR"))
        );
    }

    #[test]
    fn test_parse_local_define_line_rejects_invalid_forms() {
        assert_eq!(parse_local_define_line("#define"), None);
        assert_eq!(parse_local_define_line("#defineFOO BAR"), None);
        assert_eq!(parse_local_define_line("#define FOO"), None);
        assert_eq!(parse_local_define_line("#define  FOO   "), None);
    }

    #[test]
    fn test_split_command_and_args_variants() {
        assert_eq!(split_command_and_args("End"), ("End", None));
        assert_eq!(split_command_and_args("Message 1"), ("Message", Some("1")));
        assert_eq!(
            split_command_and_args("ApplyMovement\t0, Move"),
            ("ApplyMovement", Some("0, Move"))
        );
        assert_eq!(split_command_and_args("LockAll   "), ("LockAll", Some("")));
    }

    #[test]
    fn test_lookahead_for_end_movement_respects_structural_lines() {
        let lines = vec![
            ".align 4",
            "ScriptEntry Main",
            "EndMovement",
            "LaterLabel:",
            "EndMovement",
        ];
        assert!(lookahead_for_end_movement(&lines, 0));
        assert!(!lookahead_for_end_movement(&lines, 3));
    }

    #[test]
    fn test_collect_jump_table_and_movement_labels_scopes_pending_label() {
        let lines = vec![
            "ScriptEntry Main",
            "ScriptEntryEnd",
            "Main:",
            "LockAll",
            "Helper:",
            "WalkNorth",
            "EndMovement",
        ];
        let movement_commands: HashSet<&str> = HashSet::new();

        let (_, movement_labels) =
            collect_jump_table_and_movement_labels(&lines, &movement_commands);
        assert!(!movement_labels.contains("Main"));
        assert!(movement_labels.contains("Helper"));
    }

    #[test]
    fn test_append_inline_comment_only_appends_when_present() {
        let mut output = String::from("base");
        append_inline_comment(&mut output, Some("// note"));
        assert_eq!(output, "base // note");

        append_inline_comment(&mut output, None);
        assert_eq!(output, "base // note");
    }

    #[test]
    fn test_preprocess_body_line_splits_inline_at_comment() {
        match preprocess_body_line("Message 1 @ note") {
            BodyLine::Content {
                statement,
                inline_comment,
            } => {
                assert_eq!(statement, "Message 1");
                assert_eq!(inline_comment.as_deref(), Some("// note"));
            }
            _ => panic!("expected content line classification"),
        }
    }

    #[test]
    fn test_preprocess_body_line_recognizes_at_comments() {
        match preprocess_body_line("@ note") {
            BodyLine::FullComment(comment) => assert_eq!(comment, "// note"),
            _ => panic!("expected full comment classification"),
        }
    }

    #[test]
    fn test_duplicate_label_definitions_error() {
        let input = r"
    ScriptEntry Main
    ScriptEntry Main
    ScriptEntryEnd

Main:
    Message 1
    End

Main:
    Message 2
    End

Helper:
    Return
";

        let err = transpile(input, None, None).expect_err("duplicate labels should error");
        assert!(
            err.message.contains("duplicate label definition 'Main'"),
            "unexpected error message: {}",
            err.message
        );
    }

    #[test]
    fn test_resolve_script_command_name_maps_scrcmd_opcode() {
        let db = crate::database::DatabaseV2::test_platinum();
        let end_id = db
            .get_command("End")
            .expect("End command should exist")
            .id
            .expect("End command should have opcode");
        let opcode_name = format!("ScrCmd_{:04X}", end_id);

        assert_eq!(
            resolve_script_command_name(&opcode_name, Some(db)).as_ref(),
            "End"
        );
        assert_eq!(
            resolve_script_command_name("NotAnOpcodeAlias", Some(db)).as_ref(),
            "NotAnOpcodeAlias"
        );
    }

    #[test]
    fn test_resolve_script_command_name_maps_hgss_decimal_scrcmd_opcode() {
        let db = crate::database::DatabaseV2::test_hgss();
        let command = db
            .get_script_cmd_by_id(28)
            .expect("database should contain GoToIf at opcode 28");
        let opcode_name = format!("ScrCmd_{}", command.1.id.expect("opcode should exist"));

        assert_eq!(
            resolve_script_command_name(&opcode_name, Some(db)).as_ref(),
            command.0.as_str()
        );
    }

    #[test]
    fn test_resolve_script_command_name_ignores_noncanonical_hgss_scrcmd_spelling() {
        let db = crate::database::DatabaseV2::test_hgss();

        assert_eq!(
            resolve_script_command_name("scrcmd_28", Some(db)).as_ref(),
            "scrcmd_28"
        );
        assert_eq!(
            resolve_script_command_name("ScrCmd_001C", Some(db)).as_ref(),
            "ScrCmd_001C"
        );
    }

    #[test]
    fn test_resolve_script_command_name_keeps_exact_scrcmd_key() {
        let db = crate::database::DatabaseV2::test_hgss();

        assert_eq!(
            resolve_script_command_name("ScrCmd_055", Some(db)).as_ref(),
            "ScrCmd_055"
        );
    }

    #[test]
    fn test_collect_jump_table_and_movement_labels_handles_label_semicolon_comments() {
        let lines = vec![
            "ScriptEntry Main",
            "ScriptEntryEnd",
            "",
            ".balign 4, 0",
            "MoveLabel: ; unreferenced",
            "WalkNorth",
            "EndMovement",
        ];
        let movement_commands = HashSet::from(["WalkNorth"]);

        let (_, movement_labels) =
            collect_jump_table_and_movement_labels(&lines, &movement_commands);
        assert!(movement_labels.contains("MoveLabel"));
    }

    #[test]
    fn test_transpile_inserts_synthetic_label_before_end_after_endmovement() {
        let input = r"
    ScriptEntry Main
    ScriptEntryEnd

Main:
    End

    .balign 4, 0
MoveLabel:
    WalkNorth
    EndMovement
    End
";

        let output = transpile(input, None, None).expect("transpile should succeed");
        assert!(output.source.contains("action MoveLabel"));
        assert!(
            output
                .source
                .contains("    EndMovement\n_unused_end_0:\n    End\n")
        );
    }

    #[test]
    fn test_jump_table_parsing() {
        let input = r"
    ScriptEntry Function1
    ScriptEntry Function2
    ScriptEntryEnd

Function1:
    End

Function2:
    End
";
        let output = transpile(input, None, None).expect("transpile should succeed");
        assert!(output.source.contains("script Function1 #1:"));
        assert!(output.source.contains("script Function2 #2:"));
    }

    #[test]
    fn test_private_label() {
        let input = r"
    ScriptEntry MainFunc
    ScriptEntryEnd

MainFunc:
    GoTo HelperLabel
    End

HelperLabel:
    Return
";
        let output = transpile(input, None, None).expect("transpile should succeed");
        assert!(output.source.contains("script MainFunc #1:"));
        assert!(output.source.contains("HelperLabel:"));
        assert!(!output.source.contains("script HelperLabel"));
    }

    #[test]
    fn test_movement_detection() {
        let input = r"
    ScriptEntry MainFunc
    ScriptEntryEnd

MainFunc:
    ApplyMovement 0, TestMovement
    End

    .balign 4, 0
TestMovement:
    WalkNorth
    EndMovement
";
        let output = transpile(input, None, None).expect("transpile should succeed");
        assert!(output.source.contains("script MainFunc #1:"));
        assert!(output.source.contains("action TestMovement"));
        assert!(output.source.contains("    WalkNorth"));
        assert!(output.source.contains("    EndMovement"));
    }

    #[test]
    fn test_movement_without_balign() {
        let input = r"
    ScriptEntry MainFunc
    ScriptEntryEnd

MainFunc:
    ApplyMovement 0, TestMovement
    End

TestMovement:
    WalkNorth
    EndMovement
";
        let output = transpile(input, None, None).expect("transpile should succeed");
        assert!(output.source.contains("script MainFunc #1:"));
        assert!(output.source.contains("action TestMovement"));
        assert!(output.source.contains("    WalkNorth"));
        assert!(output.source.contains("    EndMovement"));
    }

    #[test]
    fn test_preserves_includes() {
        let input = r#"#include "macros/scrcmd.inc"
#include "constants/map.h"

    ScriptEntry Test
    ScriptEntryEnd

Test:
    End
"#;
        let output = transpile(input, None, None).expect("transpile should succeed");
        assert!(output.source.contains("#include \"macros/scrcmd.inc\""));
        assert!(output.source.contains("#include \"constants/map.h\""));
        assert!(output.source.contains("script Test #1:"));
    }

    #[test]
    fn test_translates_numeric_defines_to_aliases() {
        let input = r"#define TEST_VALUE 7
#define TEST_HEX 0x2A
    ScriptEntry Test
    ScriptEntryEnd

Test:
    End
";

        let output = transpile(input, None, None).expect("transpile should succeed");
        assert!(output.source.contains("alias 7 as TEST_VALUE"));
        assert!(output.source.contains("alias 0x2A as TEST_HEX"));
    }

    #[test]
    fn test_symbolic_defines_are_translated_to_aliases() {
        let input = r"#define TEST_VALUE ITEM_POKE_BALL
    ScriptEntry Test
    ScriptEntryEnd

Test:
    End
";

        let output = transpile(input, None, None).expect("transpile should succeed");
        assert!(output.source.contains("alias ITEM_POKE_BALL as TEST_VALUE"));
    }

    #[test]
    fn test_unsupported_define_expressions_are_left_as_comments() {
        let input = r"#define TEST_VALUE (1 << 2)
    ScriptEntry Test
    ScriptEntryEnd

Test:
    End
";

        let output = transpile(input, None, None).expect("transpile should succeed");
        assert!(output.source.contains("// #define TEST_VALUE (1 << 2)"));
        assert!(!output.source.contains("alias (1 << 2) as TEST_VALUE"));
    }

    #[test]
    fn test_function_like_macros_error() {
        let input = r"#define TEST_VALUE(x) x
    ScriptEntry Test
    ScriptEntryEnd

Test:
    End
";

        let err = transpile(input, None, None).expect_err("function-like macro should fail");
        assert!(err.message.contains("Function-like macro 'TEST_VALUE(x)'"));
        assert_eq!(err.line, 1);
    }

    #[test]
    fn test_other_preprocessor_directives_are_skipped() {
        let input = r"#pragma once
    ScriptEntry Test
    ScriptEntryEnd

Test:
    End
";

        let output = transpile(input, None, None).expect("transpile should succeed");
        assert!(!output.source.contains("#pragma"));
        assert!(output.source.contains("script Test #1:"));
    }

    #[test]
    fn test_can_parse_rotom_alias_value_rejects_invalid_syntax() {
        assert!(!can_parse_rotom_alias_value("0x"));
        assert!(!can_parse_rotom_alias_value("0xGG"));
        assert!(can_parse_rotom_alias_value("0x2A"));
        assert!(can_parse_rotom_alias_value("ITEM_POKE_BALL"));
        assert!(!can_parse_rotom_alias_value("1 << 2"));
    }

    #[test]
    fn test_commands_preserved() {
        let input = r"
    ScriptEntry Test
    ScriptEntryEnd

Test:
    SetVar VAR_RESULT, 5
    Message 0
    ShowYesNoMenu VAR_RESULT
    End
";
        let output = transpile(input, None, None).expect("transpile should succeed");
        assert!(output.source.contains("    SetVar VAR_RESULT, 5"));
        assert!(output.source.contains("    Message 0"));
        assert!(output.source.contains("    ShowYesNoMenu VAR_RESULT"));
    }

    #[test]
    fn test_parse_switch_line_is_case_insensitive() {
        assert_eq!(
            parse_switch_line("switch VAR_UNK_412D"),
            Some("VAR_UNK_412D")
        );
        assert_eq!(
            parse_switch_line("Switch VAR_UNK_412D"),
            Some("VAR_UNK_412D")
        );
        assert!(parse_switch_line("switchfoo VAR_UNK_412D").is_none());
    }

    #[test]
    fn test_transpile_switch_case_macros_to_commands() {
        let input = r"
    ScriptEntry Test
    ScriptEntryEnd

Test:
    Switch VAR_UNK_412D
    Case 0, _01C8
    Case 1, _01E4
_01C8:
    Return
_01E4:
    Return
";
        let output = transpile(input, None, None).expect("transpile should succeed");
        assert!(output.source.contains("    CopyVar 0x8008, VAR_UNK_412D"));
        assert!(output.source.contains("    CompareVarValue 0x8008, 0"));
        assert!(output.source.contains("    JumpIf EQUAL, _01C8"));
        assert!(output.source.contains("    CompareVarValue 0x8008, 1"));
        assert!(output.source.contains("    JumpIf EQUAL, _01E4"));
    }

    #[test]
    fn test_parse_case_line() {
        let case_line = parse_case_line("case 5, _03AC").expect("case line should parse");
        assert_eq!(
            case_line,
            SwitchCaseLine {
                value: "5".to_string(),
                target: "_03AC".to_string(),
            }
        );
        let pascal_case_line = parse_case_line("Case 7, _0400").expect("case line should parse");
        assert_eq!(
            pascal_case_line,
            SwitchCaseLine {
                value: "7".to_string(),
                target: "_0400".to_string(),
            }
        );
    }

    #[test]
    fn test_transpile_switch_case_with_intervening_commands() {
        let input = r"
    ScriptEntry Test
    ScriptEntryEnd

Test:
    Switch VAR_SPECIAL_RESULT
    Case 5, _0071
    npc_msg msg_0139_D49R0102_00004
    Case 0, _0166
    goto _024E
    end
";
        let output = transpile(input, None, None).expect("transpile should succeed");
        assert!(
            output
                .source
                .contains("    CopyVar 0x8008, VAR_SPECIAL_RESULT")
        );
        assert!(output.source.contains("    CompareVarValue 0x8008, 5"));
        assert!(output.source.contains("    JumpIf EQUAL, _0071"));
        assert!(output.source.contains("    CompareVarValue 0x8008, 0"));
        assert!(output.source.contains("    JumpIf EQUAL, _0166"));
        assert!(output.source.contains("    goto _024E"));
        assert!(output.source.contains("    end"));
    }

    #[test]
    fn test_transpile_semicolon_comments() {
        let input = r"
; file comment
    ScriptEntry Test
    ScriptEntryEnd

Test:
    SetVar VAR_RESULT, 1 ; inline comment
    ; block comment
    End
";

        let output = transpile(input, None, None).expect("transpile should succeed");
        assert!(output.source.contains("// file comment"));
        assert!(
            output
                .source
                .contains("    SetVar VAR_RESULT, 1 // inline comment")
        );
        assert!(output.source.contains("// block comment"));
    }

    #[test]
    fn test_multiple_movements() {
        let input = r"
    ScriptEntry Main
    ScriptEntryEnd

Main:
    End

    .balign 4, 0
Move1:
    WalkNorth
    EndMovement

    .balign 4, 0
Move2:
    WalkSouth
    EndMovement
";
        let output = transpile(input, None, None).expect("transpile should succeed");
        assert!(output.source.contains("action Move1"));
        assert!(output.source.contains("action Move2"));
    }

    #[test]
    fn test_transpile_acuity_lakefront_fixture() {
        let content = r"
    ScriptEntry _004E
    ScriptEntry _0012
    ScriptEntryEnd

_004E:
    GoTo AcuityLakefront_SetWarpsLakeAcuityNormal
    End

_0012:
    ApplyMovement 0, _00E8
    End

    .balign 4, 0
_00E8:
    WalkNorth
    EndMovement

AcuityLakefront_SetWarpsLakeAcuityNormal:
    Return
";

        let output = transpile(content, None, None).expect("transpile should succeed");
        assert!(output.source.contains("script _0012 #2:"));
        assert!(output.source.contains("script _004E #1:"));
        assert!(output.source.contains("action _00E8"));
        assert!(
            output
                .source
                .contains("AcuityLakefront_SetWarpsLakeAcuityNormal:"),
            "helper label should be preserved"
        );
    }

    #[test]
    fn test_custom_message_word_preserves_explicit_result_var_order() {
        let db = crate::database::DatabaseV2::test_platinum();

        let input = r"
    ScriptEntry Test
    ScriptEntryEnd

Test:
    ChooseCustomMessageWord 0, VAR_RESULT, VAR_0x8004
    End
";
        let output = transpile(input, Some(db), None).expect("transpile should succeed");
        assert!(
            output
                .source
                .contains("    ChooseCustomMessageWord 0, VAR_RESULT, VAR_0x8004"),
            "custom message word argument order should be preserved"
        );
    }

    #[test]
    fn test_two_custom_message_words_preserve_explicit_result_var_order() {
        let db = crate::database::DatabaseV2::test_platinum();

        let input = r"
    ScriptEntry Test
    ScriptEntryEnd

Test:
    ChooseTwoCustomMessageWords 0, VAR_RESULT, VAR_0x8000, VAR_0x8001
    End
";
        let output = transpile(input, Some(db), None).expect("transpile should succeed");
        assert!(
            output
                .source
                .contains("    ChooseTwoCustomMessageWords 0, VAR_RESULT, VAR_0x8000, VAR_0x8001"),
            "two custom message words argument order should be preserved"
        );
    }
}
