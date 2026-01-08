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
        if let Some(rest) = trimmed.strip_prefix("ScriptEntry") {
            // Get the name after "ScriptEntry"
            let rest = rest.trim();
            // Strip any comments (e.g., "Name @ 0x123" -> "Name")
            let name = rest.split('@').next().unwrap_or(rest).trim();
            let name = name.split("//").next().unwrap_or(name).trim();
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

    // Create a map from function name to ALL slot numbers it appears in
    // (A function can appear multiple times in the jump table!)
    let mut function_to_slots: std::collections::HashMap<String, Vec<usize>> = std::collections::HashMap::new();
    for (slot_idx, name) in jump_table.iter().enumerate() {
        function_to_slots
            .entry(name.clone())
            .or_insert_with(Vec::new)
            .push(slot_idx);
    }
    
    // Collect local #define macros for substitution
    let mut local_defines: std::collections::HashMap<String, String> = std::collections::HashMap::new();
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

    // Second pass: generate output
    let mut skip_until_label = false; // Skip lines until we hit the first real label (after jump table)
    let mut seen_script_entry_end = false;
    // Track which functions have had their body emitted (to avoid duplicates)
    let mut functions_with_bodies_emitted: std::collections::HashSet<String> = std::collections::HashSet::new();

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
            // Check if this is a movement label
            if movement_labels.contains(label_name) {
                output.push_str(&format!("action {}\n", label_name));
            } else if let Some(slots) = function_to_slots.get(label_name) {
                // Public function (in jump table)
                // Only emit if we haven't seen this function before
                if functions_with_bodies_emitted.contains(label_name) {
                    // Skip this entire function (headers + body already emitted)
                    // Set skip_until_label to skip all commands until next label
                    skip_until_label = true;
                } else {
                    // Emit header for EACH slot this function appears in
                    for slot in slots {
                        output.push_str(&format!("function {} #{}:\n", label_name, slot));
                    }
                    functions_with_bodies_emitted.insert(label_name.to_string());
                }
            } else {
                // Private label
                output.push_str(&format!("{}:\n", label_name));
            }
            skip_until_label = false;
            continue;
        }

        // Skip lines before we've seen any real content
        if !seen_script_entry_end {
            continue;
        }

        // Handle commands
        // Decomp format: CommandName arg1, arg2, arg3
        output.push_str("    ");
        if let Some(cmd_end_idx) = trimmed.find(|c| c == ' ' || c == '\t') {
            let cmd_name = &trimmed[..cmd_end_idx];
            let args = trimmed[cmd_end_idx..].trim();

            if args.is_empty() {
                // Command with no arguments
                output.push_str(cmd_name);
            } else {
                // Apply local #define substitutions to arguments
                let substituted_args = substitute_defines(args, &local_defines);
                output.push_str(cmd_name);
                if !substituted_args.is_empty() {
                    output.push(' ');
                    output.push_str(&substituted_args);
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

/// Substitute local #define macros in argument string
fn substitute_defines(args: &str, defines: &std::collections::HashMap<String, String>) -> String {
    if defines.is_empty() {
        return args.to_string();
    }

    // Split by comma to handle each argument separately
    let parts: Vec<&str> = args.split(',').collect();
    let substituted: Vec<String> = parts
        .iter()
        .map(|part| {
            let trimmed = part.trim();
            // Check if this is an identifier that needs substitution
            if let Some(replacement) = defines.get(trimmed) {
                replacement.clone()
            } else {
                trimmed.to_string()
            }
        })
        .collect();

    substituted.join(", ")
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
    fn test_transpile_acuity_lakefront_fixture() {
        let fixture_path = "tests/fixtures/scripts/scripts_acuity_lakefront.s";
        let input = std::fs::read_to_string(fixture_path);
        
        if let Ok(content) = input {
            let output = transpile(&content);
            assert!(output.contains("function _0012 #1:"));
            assert!(output.contains("function _004E #0:"));
            assert!(output.contains("action _00E8"));
            assert!(output.contains("AcuityLakefront_SetWarpsLakeAcuityNormal:"));
        }
    }
}
