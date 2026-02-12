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
//! function FunctionName #0:
//!     LockAll
//!     Message 0
//!     End
//!
//! action MovementName
//!     WalkNorth
//!     EndMovement
//! ```

use crate::autovar::is_autovar_param;
use crate::database::CommandType;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct TranspileResult {
    pub source: String,
    pub emit_end_marker: bool,
}

#[derive(Debug, Default)]
struct PrepassData {
    function_to_slots: HashMap<String, Vec<usize>>,
    movement_labels: HashSet<String>,
    local_defines: HashMap<String, String>,
}

/// Transpile a decomp script to Rotoscript format
pub fn transpile(input: &str, db: Option<&crate::database::DatabaseV2>) -> TranspileResult {
    let mut output = String::new();
    let mut has_script_entry_end = false;

    let prepass = collect_prepass_data(input, db);

    // Second pass: generate output
    let mut seen_script_entry_end = false;
    let mut seen_first_label_after_entry_end = false; // Track if we've seen a real label after ScriptEntryEnd
    // Track which functions have had their body emitted (to avoid duplicates)
    let mut functions_with_bodies_emitted: HashSet<String> = HashSet::new();

    for line in input.lines() {
        let raw_trimmed = line.trim();

        // Skip empty lines in output but preserve them
        if raw_trimmed.is_empty() {
            if seen_script_entry_end {
                output.push('\n');
            }
            continue;
        }

        // Skip preprocessor directives
        if raw_trimmed.starts_with("#include") || raw_trimmed.starts_with('#') {
            continue;
        }

        // Handle assembly comments (full line)
        if raw_trimmed.starts_with('@') || raw_trimmed.starts_with("//") {
            // Keep regular comments, and convert @ comments to //
            let comment = if raw_trimmed.starts_with('@') {
                raw_trimmed.replacen('@', "//", 1)
            } else {
                raw_trimmed.to_string()
            };
            output.push_str(&comment);
            output.push('\n');
            continue;
        }

        // Split inline comment
        let (trimmed, inline_comment) = if let Some(idx) = raw_trimmed.find('@') {
            (
                raw_trimmed[..idx].trim(),
                Some(raw_trimmed[idx..].replace('@', "//")),
            )
        } else {
            (raw_trimmed, None)
        };

        // Skip ScriptEntry/ScriptEntryEnd
        if trimmed.starts_with("ScriptEntry") {
            if trimmed == "ScriptEntryEnd" {
                seen_script_entry_end = true;
                has_script_entry_end = true;
            }
            continue;
        }

        // Skip .balign directives
        if trimmed.starts_with(".balign") || trimmed.starts_with(".align") {
            continue;
        }

        // Skip other assembler directives
        if trimmed.starts_with('.') {
            continue;
        }

        // Handle labels
        if let Some(label_name) = trimmed.strip_suffix(':') {
            // If we see a label, the jump table is definitely done
            // (handles files without explicit ScriptEntryEnd)
            seen_script_entry_end = true;

            // Check if this is a movement label
            if prepass.movement_labels.contains(label_name) {
                output.push_str(&format!("action {}", label_name));
                if let Some(ref c) = inline_comment {
                    output.push(' ');
                    output.push_str(c);
                }
                output.push('\n');
            } else if let Some(slots) = prepass.function_to_slots.get(label_name) {
                // Public function (in jump table)
                // Only emit if we haven't seen this function before
                if functions_with_bodies_emitted.contains(label_name) {
                    // Duplicate public header entry: keep only headers, not duplicate body.
                } else {
                    // Emit header for EACH slot this function appears in
                    for slot in slots {
                        output.push_str(&format!("function {} #{}", label_name, slot));
                        if let Some(ref c) = inline_comment {
                            output.push(' ');
                            output.push_str(c);
                        }
                        output.push_str(":\n");
                    }
                    functions_with_bodies_emitted.insert(label_name.to_string());
                }
            } else {
                // Private label
                output.push_str(&format!("{}:", label_name));
                if let Some(ref c) = inline_comment {
                    output.push(' ');
                    output.push_str(c);
                }
                output.push('\n');
            }
            seen_first_label_after_entry_end = true;
            continue;
        }

        // Skip lines before we've seen any real content
        if !seen_script_entry_end {
            continue;
        }

        // Handle bare End at top level (before any label has been seen)
        // Some decomp scripts have an End immediately after ScriptEntryEnd with no label
        // We need to wrap it in a synthetic label so the parser can handle it
        if !seen_first_label_after_entry_end && trimmed == "End" {
            output.push_str("_unused_end:\n");
            output.push_str("    End\n");
            seen_first_label_after_entry_end = true;
            continue;
        }

        output.push_str("    ");
        if let Some(cmd_end_idx) = trimmed.find([' ', '\t']) {
            let cmd_name = resolve_script_command_name(&trimmed[..cmd_end_idx], db);
            let args = trimmed[cmd_end_idx..].trim();

            if args.is_empty() {
                output.push_str(cmd_name);
            } else {
                let substituted_args = substitute_defines(args, &prepass.local_defines);
                let reordered_args = if let Some(db) = db {
                    reorder_decomp_args_to_binary(cmd_name, &substituted_args, db)
                } else {
                    substituted_args
                };
                output.push_str(cmd_name);
                if !reordered_args.is_empty() {
                    output.push(' ');
                    output.push_str(&reordered_args);
                }
            }
        } else {
            let cmd_name = resolve_script_command_name(trimmed, db);
            output.push_str(cmd_name);
        }
        if let Some(ref c) = inline_comment {
            output.push(' ');
            output.push_str(c);
        }
        output.push('\n');
    }

    TranspileResult {
        source: output,
        emit_end_marker: has_script_entry_end,
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
        local_defines: collect_local_defines(input),
    }
}

fn movement_commands_from_db<'a>(db: Option<&'a crate::database::DatabaseV2>) -> HashSet<&'a str> {
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

    for (line_idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        // Skip empty lines and comments
        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with('@') {
            continue;
        }

        // Parse ScriptEntry
        if let Some(rest) = trimmed.strip_prefix("ScriptEntry") {
            let rest = rest.trim();
            let name = rest.split('@').next().unwrap_or(rest).trim();
            let name = name.split("//").next().unwrap_or(name).trim();
            if !name.is_empty() {
                jump_table.push(name.to_string());
            }
            continue;
        }

        // Skip assembler directives
        if trimmed.starts_with('.') {
            continue;
        }

        // Track labels
        if let Some(label_name) = trimmed.strip_suffix(':') {
            current_label = Some(label_name.to_string());
            continue;
        }

        if let Some(ref label) = current_label {
            let cmd_name = trimmed.split([' ', '\t']).next().unwrap_or("");

            let is_movement = if movement_commands.is_empty() {
                lookahead_for_end_movement(lines, line_idx)
            } else {
                movement_commands.contains(cmd_name)
            };

            if is_movement {
                movement_labels.insert(label.clone());
            }
            current_label = None;
        }
    }

    (jump_table, movement_labels)
}

fn build_function_slot_map(jump_table: &[String]) -> HashMap<String, Vec<usize>> {
    // A function can appear multiple times in ScriptEntry; preserve all slots.
    let mut function_to_slots: HashMap<String, Vec<usize>> = HashMap::new();
    for (slot_idx, name) in jump_table.iter().enumerate() {
        function_to_slots
            .entry(name.clone())
            .or_default()
            .push(slot_idx);
    }
    function_to_slots
}

fn collect_local_defines(input: &str) -> HashMap<String, String> {
    let mut local_defines: HashMap<String, String> = HashMap::new();

    for line in input.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("#define ") {
            // Parse: NAME VALUE
            let parts: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
            if parts.len() == 2 {
                let name = parts[0].trim();
                let value = parts[1].trim();
                // Only store simple identifier-to-identifier mappings
                if !name.is_empty() && !value.is_empty() {
                    local_defines.insert(name.to_string(), value.to_string());
                }
            }
        }
    }

    local_defines
}

fn resolve_script_command_name<'a>(
    cmd_name: &'a str,
    db: Option<&'a crate::database::DatabaseV2>,
) -> &'a str {
    let Some(id_str) = cmd_name.strip_prefix("ScrCmd_") else {
        return cmd_name;
    };

    let Some(db) = db else {
        return cmd_name;
    };

    let Ok(id) = u16::from_str_radix(id_str, 16) else {
        return cmd_name;
    };

    db.commands
        .iter()
        .find(|(_, c)| c.id == Some(id) && c.cmd_type == CommandType::ScriptCmd)
        .map_or(cmd_name, |(name, _)| name.as_str())
}

fn reorder_decomp_args_to_binary(
    cmd_name: &str,
    args_str: &str,
    db: &crate::database::DatabaseV2,
) -> String {
    let cmd = match db.get_command(cmd_name) {
        Ok(c) => c,
        Err(_) => return args_str.to_owned(),
    };

    let params = &cmd.params;
    if params.is_empty() {
        return args_str.to_owned();
    }

    let mut required_indices: Vec<usize> = Vec::new();
    let mut optional_indices: Vec<usize> = Vec::new();
    for (i, p) in params.iter().enumerate() {
        // Autovar params (destVar with VAR_RESULT default) are always provided
        // explicitly in decomp sources, so treat them as required for reordering
        if p.default.is_none() || is_autovar_param(p) {
            required_indices.push(i);
        } else {
            optional_indices.push(i);
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

/// Substitute local #define macros in argument string
fn substitute_defines(args: &str, defines: &std::collections::HashMap<String, String>) -> String {
    if defines.is_empty() {
        return args.to_owned();
    }

    let mut out = String::with_capacity(args.len());
    let mut first = true;
    for part in args.split(',') {
        if !first {
            out.push_str(", ");
        }
        first = false;
        let trimmed = part.trim();
        if let Some(replacement) = defines.get(trimmed) {
            out.push_str(replacement);
        } else {
            out.push_str(trimmed);
        }
    }
    out
}

fn lookahead_for_end_movement(lines: &[&str], start_idx: usize) -> bool {
    const MAX_LOOKAHEAD: usize = 32;

    for i in start_idx..std::cmp::min(start_idx + MAX_LOOKAHEAD, lines.len()) {
        let trimmed = lines[i].trim();

        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with('@') {
            continue;
        }

        if trimmed.starts_with('.') {
            continue;
        }

        if trimmed.ends_with(':') {
            return false;
        }

        let cmd_name = trimmed.split([' ', '\t']).next().unwrap_or("");

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
    use std::path::Path;

    #[test]
    fn test_collect_prepass_data_keeps_duplicate_function_slots() {
        let input = r#"
    ScriptEntry Main
    ScriptEntry Main
    ScriptEntryEnd

Main:
    End
"#;

        let prepass = collect_prepass_data(input, None);
        assert_eq!(prepass.function_to_slots.get("Main"), Some(&vec![0, 1]));
    }

    #[test]
    fn test_resolve_script_command_name_maps_scrcmd_opcode() {
        let db = crate::database::DatabaseV2::load(Path::new("src/db/platinum_v2.json"))
            .expect("test database should load");
        let end_id = db
            .get_command("End")
            .expect("End command should exist")
            .id
            .expect("End command should have opcode");
        let opcode_name = format!("ScrCmd_{:04X}", end_id);

        assert_eq!(resolve_script_command_name(&opcode_name, Some(&db)), "End");
        assert_eq!(
            resolve_script_command_name("NotAnOpcodeAlias", Some(&db)),
            "NotAnOpcodeAlias"
        );
    }

    #[test]
    fn test_jump_table_parsing() {
        let input = r#"
    ScriptEntry Function1
    ScriptEntry Function2
    ScriptEntryEnd

Function1:
    End

Function2:
    End
"#;
        let output = transpile(input, None);
        assert!(output.source.contains("function Function1 #0:"));
        assert!(output.source.contains("function Function2 #1:"));
    }

    #[test]
    fn test_private_label() {
        let input = r#"
    ScriptEntry MainFunc
    ScriptEntryEnd

MainFunc:
    GoTo HelperLabel
    End

HelperLabel:
    Return
"#;
        let output = transpile(input, None);
        assert!(output.source.contains("function MainFunc #0:"));
        assert!(output.source.contains("HelperLabel:"));
        assert!(!output.source.contains("function HelperLabel"));
    }

    #[test]
    fn test_movement_detection() {
        let input = r#"
    ScriptEntry MainFunc
    ScriptEntryEnd

MainFunc:
    ApplyMovement 0, TestMovement
    End

    .balign 4, 0
TestMovement:
    WalkNorth
    EndMovement
"#;
        let output = transpile(input, None);
        assert!(output.source.contains("function MainFunc #0:"));
        assert!(output.source.contains("action TestMovement"));
        assert!(output.source.contains("    WalkNorth"));
        assert!(output.source.contains("    EndMovement"));
    }

    #[test]
    fn test_movement_without_balign() {
        let input = r#"
    ScriptEntry MainFunc
    ScriptEntryEnd

MainFunc:
    ApplyMovement 0, TestMovement
    End

TestMovement:
    WalkNorth
    EndMovement
"#;
        let output = transpile(input, None);
        assert!(output.source.contains("function MainFunc #0:"));
        assert!(output.source.contains("action TestMovement"));
        assert!(output.source.contains("    WalkNorth"));
        assert!(output.source.contains("    EndMovement"));
    }

    #[test]
    fn test_skip_includes() {
        let input = r#"#include "macros/scrcmd.inc"
#include "constants/map.h"

    ScriptEntry Test
    ScriptEntryEnd

Test:
    End
"#;
        let output = transpile(input, None);
        assert!(!output.source.contains("#include"));
        assert!(output.source.contains("function Test #0:"));
    }

    #[test]
    fn test_commands_preserved() {
        let input = r#"
    ScriptEntry Test
    ScriptEntryEnd

Test:
    SetVar VAR_RESULT, 5
    Message 0
    ShowYesNoMenu VAR_RESULT
    End
"#;
        let output = transpile(input, None);
        assert!(output.source.contains("    SetVar VAR_RESULT, 5"));
        assert!(output.source.contains("    Message 0"));
        assert!(output.source.contains("    ShowYesNoMenu VAR_RESULT"));
    }

    #[test]
    fn test_multiple_movements() {
        let input = r#"
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
"#;
        let output = transpile(input, None);
        assert!(output.source.contains("action Move1"));
        assert!(output.source.contains("action Move2"));
    }

    #[test]
    fn test_transpile_acuity_lakefront_fixture() {
        let content = r#"
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
"#;

        let output = transpile(content, None);
        assert!(output.source.contains("function _0012 #1:"));
        assert!(output.source.contains("function _004E #0:"));
        assert!(output.source.contains("action _00E8"));
        assert!(
            output
                .source
                .contains("AcuityLakefront_SetWarpsLakeAcuityNormal:")
        );
    }
}
