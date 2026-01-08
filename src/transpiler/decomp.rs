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
//! EndMovement
//! ```


/// Maps command names to argument reordering indices.
/// Used when decomp macro format has different argument order than game binary expects.
///
/// Example: [0, 1, 2, 4, 3] means:
/// - arg 0 stays at position 0
/// - arg 1 stays at position 1
/// - arg 2 stays at position 2
/// - arg 4 moves to position 3
/// - arg 3 moves to position 4
const PARAM_REORDER_MAP: &[(&str, &[usize])] = &[
    // InitGlobalTextListMenu: decomp macro has (x, y, cursor, selection, cancel)
    // but game binary expects (x, y, cursor, cancel, selection)
    ("InitGlobalTextListMenu", &[0, 1, 2, 4, 3]),
    // InitLocalTextListMenu: same issue
    ("InitLocalTextListMenu", &[0, 1, 2, 4, 3]),
];

/// Reorder command arguments according to PARAM_REORDER_MAP
fn reorder_args(command: &str, args: &str) -> String {
    // Find if this command has a reordering map
    let reorder_map = PARAM_REORDER_MAP
        .iter()
        .find(|(name, _)| *name == command)
        .map(|(_, map)| *map);

    if let Some(map) = reorder_map {
        // Parse arguments into a Vec
        let arg_vec: Vec<&str> = args.split(',').map(|s| s.trim()).collect();
        let arg_count = arg_vec.len();
        let map_len = map.len();

        if arg_count == map_len {
            // Args match map length - direct reordering
            let mut reordered = Vec::with_capacity(map_len);
            for &idx in map {
                reordered.push(arg_vec[idx]);
            }
            return reordered.join(", ");
        } else if arg_count < map_len {
            // Fewer args than map - reorder available args
            let mut reordered = Vec::with_capacity(map_len);
            for &idx in map {
                if idx < arg_count {
                    reordered.push(arg_vec[idx]);
                } else {
                    // Missing arg - use placeholder (will be caught by compiler)
                    reordered.push("0");
                }
            }
            return reordered.join(", ");
        }
    }
    // No reordering needed
    args.to_string()
}

/// Transpile a decomp script to Rotoscript format
pub fn transpile(input: &str) -> String {
    let mut output = String::new();

    // Track jump table entries: name -> slot number
    let mut jump_table: Vec<String> = Vec::new();

    // Track which labels are movements (preceded by .balign 4, 0)
    let mut movement_labels: std::collections::HashSet<String> = std::collections::HashSet::new();

    // First pass: collect jump table and identify movement labels
    let mut next_is_movement = false;
    for line in input.lines() {
        let trimmed = line.trim();

        // Skip empty lines and comments
        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with("@") {
            continue;
        }

        // Parse ScriptEntry
        if let Some(name) = trimmed.strip_prefix("ScriptEntry ") {
            let name = name.trim();
            if !name.is_empty() {
                jump_table.push(name.to_string());
            }
            continue;
        }

        // Track .balign 4, 0 - next label is a movement
        if trimmed.starts_with(".balign 4") {
            next_is_movement = true;
            continue;
        }

        // If we see a label after .balign, mark it as movement
        if next_is_movement {
            if let Some(label_name) = trimmed.strip_suffix(':') {
                movement_labels.insert(label_name.to_string());
            }
            next_is_movement = false;
        }

        // Also check if label name contains "Movement" - these are always movement labels
        if let Some(label_name) = trimmed.strip_suffix(':') {
            if label_name.contains("Movement") {
                movement_labels.insert(label_name.to_string());
            }
        }
    }

    // Create a map from name to slot number for quick lookup
    let slot_map: std::collections::HashMap<String, usize> = jump_table
        .iter()
        .enumerate()
        .map(|(i, name)| (name.clone(), i))
        .collect();

    // Second pass: generate output
    let mut in_movement = false;
    let mut skip_until_label = false; // Skip lines until we hit the first real label (after jump table)
    let mut seen_script_entry_end = false;

    for line in input.lines() {
        let trimmed = line.trim();

        // Skip empty lines in output but preserve them
        if trimmed.is_empty() {
            if seen_script_entry_end && !skip_until_label {
                output.push('\n');
            }
            continue;
        }

        // Skip preprocessor directives
        if trimmed.starts_with("#include") || trimmed.starts_with("#") {
            continue;
        }

        // Skip assembly comments
        if trimmed.starts_with("@") || trimmed.starts_with("//") {
            // Keep regular comments
            if trimmed.starts_with("//") {
                output.push_str(trimmed);
                output.push('\n');
            }
            continue;
        }

        // Skip ScriptEntry/ScriptEntryEnd
        if trimmed.starts_with("ScriptEntry") {
            if trimmed == "ScriptEntryEnd" {
                seen_script_entry_end = true;
            }
            continue;
        }

        // Skip .balign directives
        if trimmed.starts_with(".balign") || trimmed.starts_with(".align") {
            continue;
        }

        // Skip other assembler directives
        if trimmed.starts_with(".") {
            continue;
        }

        // Handle labels
        if let Some(label_name) = trimmed.strip_suffix(':') {
            // Close previous movement if any
            // (EndMovement should have been emitted by the command itself)

            // Check if this is a movement label
            if movement_labels.contains(label_name) {
                output.push_str(&format!("action {}\n", label_name));
                in_movement = true;
            } else if let Some(&slot) = slot_map.get(label_name) {
                // Public function (in jump table)
                output.push_str(&format!("function {} #{}:\n", label_name, slot));
                in_movement = false;
            } else {
                // Private label
                output.push_str(&format!("{}:\n", label_name));
                in_movement = false;
            }
            skip_until_label = false;
            continue;
        }

        // Skip lines before we've seen any real content
        if !seen_script_entry_end {
            continue;
        }

        // Handle commands - parse and optionally reorder arguments
        // Decomp format: CommandName arg1, arg2, arg3
        output.push_str("    ");
        if let Some(cmd_end_idx) = trimmed.find(|c| c == ' ' || c == '\t') {
            let cmd_name = &trimmed[..cmd_end_idx];
            let args = trimmed[cmd_end_idx..].trim();

            if args.is_empty() {
                // Command with no arguments
                output.push_str(cmd_name);
            } else {
                // Command with arguments - check if reordering is needed
                let reordered_args = reorder_args(cmd_name, args);
                output.push_str(cmd_name);
                if !reordered_args.is_empty() {
                    output.push(' ');
                    output.push_str(&reordered_args);
                }
            }
        } else {
            // No arguments, just the command name
            output.push_str(trimmed);
        }
        output.push('\n');
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let output = transpile(input);
        assert!(output.contains("function Function1 #0:"));
        assert!(output.contains("function Function2 #1:"));
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
        let output = transpile(input);
        assert!(output.contains("function MainFunc #0:"));
        assert!(output.contains("HelperLabel:"));
        assert!(!output.contains("function HelperLabel"));
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
        let output = transpile(input);
        assert!(output.contains("function MainFunc #0:"));
        assert!(output.contains("action TestMovement"));
        assert!(output.contains("    WalkNorth"));
        assert!(output.contains("    EndMovement"));
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
        let output = transpile(input);
        assert!(output.contains("function MainFunc #0:"));
        assert!(output.contains("action TestMovement"));
        assert!(output.contains("    WalkNorth"));
        assert!(output.contains("    EndMovement"));
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
        let output = transpile(input);
        assert!(!output.contains("#include"));
        assert!(output.contains("function Test #0:"));
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
        let output = transpile(input);
        assert!(output.contains("    SetVar VAR_RESULT, 5"));
        assert!(output.contains("    Message 0"));
        assert!(output.contains("    ShowYesNoMenu VAR_RESULT"));
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
        let output = transpile(input);
        assert!(output.contains("action Move1"));
        assert!(output.contains("action Move2"));
    }

    #[test]
    fn test_real_decomp_file() {
        let input = std::fs::read_to_string(
            r"C:\dev\pokeplatinum\res\field\scripts\scripts_battle_tower_battle_salon.s",
        );
        if let Ok(content) = input {
            let output = transpile(&content);
            // Check that it has the expected structure
            assert!(output.contains("function BattleTowerBattleSalon_Attendant #0:"));
            assert!(output.contains("function BattleTowerBattleSalon_Cheryl #1:"));
            assert!(
                output.contains("action BattleTowerBattleSalon_PlayerEnterBattleSalonMovement")
            );
            println!(
                "=== Transpiled Output (first 2000 chars) ===\n{}",
                &output[..output.len().min(2000)]
            );
        }
    }
}
