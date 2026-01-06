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

use regex::Regex;

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
        
        // Handle commands
        // Decomp format is already comma-separated, so we just need to indent
        output.push_str("    ");
        output.push_str(trimmed);
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
            r"C:\dev\pokeplatinum\res\field\scripts\scripts_battle_tower_battle_salon.s"
        );
        if let Ok(content) = input {
            let output = transpile(&content);
            // Check that it has the expected structure
            assert!(output.contains("function BattleTowerBattleSalon_Attendant #0:"));
            assert!(output.contains("function BattleTowerBattleSalon_Cheryl #1:"));
            assert!(output.contains("action BattleTowerBattleSalon_PlayerEnterBattleSalonMovement"));
            println!("=== Transpiled Output (first 2000 chars) ===\n{}", &output[..output.len().min(2000)]);
        }
    }
}
