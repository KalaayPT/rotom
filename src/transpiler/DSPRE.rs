//! DSPRE Script Transpiler
//!
//! Converts DSPRE script format to Rotoscript format.
//!
//! ## DSPRE Format:
//! ```text
//! Script 1:
//!     PlayFanfare 1500
//!     LockAll
//! End
//!
//! Function 2:
//!     CloseMessage
//!     UseScript_#3
//! End
//!
//! Action 1:
//!     LookRight 0x1
//! End
//! ```
//!
//! ## Rotoscript Output:
//! ```text
//! function script_1 #1:
//!     PlayFanfare 1500
//!     LockAll
//! End
//!
//! func_2:
//!     CloseMessage
//!     Jump script_3
//! End
//!
//! action action_1
//!     LookRight 0x1
//! EndMovement
//! ```

use regex::Regex;
use std::sync::LazyLock;

// ============================================================================
// Cached Regexes
// ============================================================================

/// Script header: "Script N:"
static RE_SCRIPT_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^Script\s+(\d+)\s*:").unwrap()
});

/// Function header: "Function N:"
static RE_FUNCTION_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^Function\s+(\d+)\s*:").unwrap()
});

/// Action header: "Action N:"
static RE_ACTION_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^Action\s+(\d+)\s*:").unwrap()
});

/// Script reference in arguments: "Script#N"
static RE_SCRIPT_REF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Script#(\d+)").unwrap()
});

/// Function reference in arguments: "Function#N"
static RE_FUNCTION_REF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Function#(\d+)").unwrap()
});

/// Action reference in arguments: "Action#N"
static RE_ACTION_REF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Action#(\d+)").unwrap()
});

/// UseScript workaround: "UseScript_#N"
static RE_USE_SCRIPT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*UseScript_#(\d+)\s*$").unwrap()
});

/// Descriptor pattern: Word.Value -> Value (e.g., "Overworld.0" -> "0")
static RE_DESCRIPTOR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[A-Za-z_][A-Za-z0-9_]*\.([A-Za-z0-9_]+)").unwrap()
});

/// Transpile a DSPRE script to Rotoscript format
pub fn transpile(input: &str) -> String {
    // First, strip block comments /* ... */
    let input = strip_block_comments(input);
    
    let mut output = String::new();
    let mut in_action = false; // Track if we're inside an Action block

    for line in input.lines() {
        let trimmed = line.trim();

        // Skip empty lines and comments (preserve them)
        if trimmed.is_empty() {
            output.push('\n');
            continue;
        }
        if trimmed.starts_with("//") {
            output.push_str(line);
            output.push('\n');
            continue;
        }

        // Check for Script N: header -> becomes `function script_N #N:`
        if let Some(caps) = RE_SCRIPT_HEADER.captures(trimmed) {
            let id: u32 = caps[1].parse().unwrap();
            output.push_str(&format!("function script_{} #{}:\n", id, id));
            in_action = false;
            continue;
        }

        // Check for Function N: header -> becomes bare label `func_N:`
        if let Some(caps) = RE_FUNCTION_HEADER.captures(trimmed) {
            let id: u32 = caps[1].parse().unwrap();
            output.push_str(&format!("func_{}:\n", id));
            in_action = false;
            continue;
        }

        // Check for Action N: header
        if let Some(caps) = RE_ACTION_HEADER.captures(trimmed) {
            let id: u32 = caps[1].parse().unwrap();
            output.push_str(&format!("action action_{}\n", id));
            in_action = true;
            continue;
        }

        // Check for UseScript_#N (DSPRE workaround for jumping to scripts)
        if let Some(caps) = RE_USE_SCRIPT.captures(&line) {
            let id: u32 = caps[1].parse().unwrap();
            // Preserve leading whitespace
            let leading_ws = &line[..line.len() - line.trim_start().len()];
            output.push_str(leading_ws);
            output.push_str(&format!("Jump script_{}\n", id));
            continue;
        }
        
        // Convert End to EndMovement inside Action blocks
        if in_action && trimmed.eq_ignore_ascii_case("End") {
            output.push_str("EndMovement\n");
            in_action = false;
            continue;
        }

        // For all other lines, replace references in arguments
        let mut processed = line.to_string();

        // Replace Script#N -> script_N
        processed = RE_SCRIPT_REF.replace_all(&processed, "script_$1").to_string();

        // Replace Function#N -> func_N
        processed = RE_FUNCTION_REF.replace_all(&processed, "func_$1").to_string();

        // Replace Action#N -> action_N
        processed = RE_ACTION_REF.replace_all(&processed, "action_$1").to_string();
        
        // Strip descriptors: Overworld.0 -> 0, Move.HM01 -> HM01
        processed = RE_DESCRIPTOR.replace_all(&processed, "$1").to_string();
        
        // Convert DSPRE comparison operators: GREATER/EQUAL -> GREATER_EQUAL, LESS/EQUAL -> LESS_EQUAL
        processed = processed.replace("GREATER/EQUAL", "GREATER_EQUAL");
        processed = processed.replace("LESS/EQUAL", "LESS_EQUAL");
        
        // Convert space-separated arguments to comma-separated
        // Command Arg1 Arg2 Arg3 -> Command Arg1, Arg2, Arg3
        processed = convert_space_to_comma_args(&processed);

        output.push_str(&processed);
        output.push('\n');
    }

    output
}

/// Convert DSPRE space-separated arguments to comma-separated
/// e.g., "    WaitTime 8 0x800C" -> "    WaitTime 8, 0x800C"
/// Only converts if there are no commas already present (to avoid double-comma issues)
fn convert_space_to_comma_args(line: &str) -> String {
    let trimmed = line.trim();
    
    // Skip if empty or doesn't look like a command line
    if trimmed.is_empty() {
        return line.to_string();
    }
    
    // If line already contains commas, assume it's already formatted correctly
    if line.contains(',') {
        return line.to_string();
    }
    
    // Preserve leading whitespace
    let leading_ws = &line[..line.len() - line.trim_start().len()];
    
    // Split into tokens by whitespace
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    
    if tokens.is_empty() {
        return line.to_string();
    }
    
    // First token is the command name, rest are arguments
    let command = tokens[0];
    let args = &tokens[1..];
    
    if args.is_empty() {
        // No arguments, return as-is
        return line.to_string();
    }
    
    // Join arguments with ", "
    let args_str = args.join(", ");
    
    format!("{}{} {}", leading_ws, command, args_str)
}

/// Strip block comments /* ... */ from the input, handling nested and multi-line comments
fn strip_block_comments(input: &str) -> String {
    let mut output = String::new();
    let mut chars = input.chars().peekable();
    
    while let Some(c) = chars.next() {
        if c == '/' && chars.peek() == Some(&'*') {
            // Start of block comment, consume the '*'
            chars.next();
            // Skip until we find */
            loop {
                match chars.next() {
                    Some('*') if chars.peek() == Some(&'/') => {
                        chars.next(); // consume the '/'
                        break;
                    }
                    Some(_) => continue,
                    None => break, // Unterminated comment, stop
                }
            }
        } else {
            output.push(c);
        }
    }
    
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_script_header() {
        let input = "Script 1:\n    LockAll\nEnd";
        let output = transpile(input);
        assert!(output.contains("function script_1 #1:"));
    }

    #[test]
    fn test_function_header() {
        let input = "Function 2:\n    CloseMessage\nEnd";
        let output = transpile(input);
        // DSPRE "Function" becomes a bare label in rotoscript
        assert!(output.contains("func_2:"));
        assert!(!output.contains("function func_2"));
    }

    #[test]
    fn test_action_header() {
        let input = "Action 1:\n    LookRight 0x1\nEnd";
        let output = transpile(input);
        assert!(output.contains("action action_1"));
        // End inside action should become EndMovement
        assert!(output.contains("EndMovement"));
        assert!(!output.contains("\nEnd\n"));
    }

    #[test]
    fn test_script_reference() {
        let input = "    JumpIf EQUAL Script#3";
        let output = transpile(input);
        // Space-separated args become comma-separated
        assert!(output.contains("JumpIf EQUAL, script_3"));
    }

    #[test]
    fn test_comparison_operators() {
        let input = "    CallIf GREATER/EQUAL func_2";
        let output = transpile(input);
        assert!(output.contains("GREATER_EQUAL"));
        assert!(!output.contains("GREATER/EQUAL"));
        
        let input2 = "    JumpIf LESS/EQUAL Script#1";
        let output2 = transpile(input2);
        assert!(output2.contains("LESS_EQUAL"));
        assert!(!output2.contains("LESS/EQUAL"));
    }

    #[test]
    fn test_function_reference() {
        let input = "    Jump Function#2";
        let output = transpile(input);
        assert!(output.contains("Jump func_2"));
    }

    #[test]
    fn test_action_reference() {
        let input = "    ApplyMovement 0xFF, Action#1";
        let output = transpile(input);
        assert!(output.contains("ApplyMovement 0xFF, action_1"));
    }

    #[test]
    fn test_use_script_workaround() {
        let input = "    UseScript_#5";
        let output = transpile(input);
        assert!(output.contains("Jump script_5"));
    }

    #[test]
    fn test_full_dspre_script() {
        let input = r#"Script 1:
    PlayFanfare 1500
    LockAll
    FacePlayer
    Message 0
    WaitButton
    CloseMessage
    ReleaseAll
End

Script 2:
    LockAll
    Message 9
    YesNoBox 0x800C
    CompareVarValue 0x800C, 0
    JumpIf EQUAL Script#3
    CompareVarValue 0x800C, 1
    JumpIf EQUAL Function#2
End

Script 3:
    CMD_162
    Message 12
    CMD_309 94
    CloseMessage
    CMD_699
    ExitBattleRoom
    ReleaseAll
End

Function 2:
    CloseMessage
    ReleaseAll
End

Action 1:
    LookRight 0x1
End
"#;
        let output = transpile(input);

        assert!(output.contains("function script_1 #1:"));
        assert!(output.contains("function script_2 #2:"));
        assert!(output.contains("function script_3 #3:"));
        assert!(output.contains("func_2:"));
        assert!(!output.contains("function func_2"));
        assert!(output.contains("action action_1"));
        // End inside action should become EndMovement
        assert!(output.contains("EndMovement"));
        // Space-separated args become comma-separated
        assert!(output.contains("JumpIf EQUAL, script_3"));
        assert!(output.contains("JumpIf EQUAL, func_2"));
    }

    #[test]
    fn test_preserves_comments() {
        let input = "// This is a comment\nScript 1:\nEnd";
        let output = transpile(input);
        assert!(output.contains("// This is a comment"));
    }

    #[test]
    fn test_strip_block_comments() {
        let input = "Script 1:\n/* this is a comment */\n    LockAll\nEnd";
        let output = transpile(input);
        assert!(output.contains("LockAll"));
        assert!(!output.contains("this is a comment"));
    }

    #[test]
    fn test_strip_multiline_block_comments() {
        let input = "Script 1:\n/*\n  multi\n  line\n  comment\n*/\n    LockAll\nEnd";
        let output = transpile(input);
        assert!(output.contains("LockAll"));
        assert!(!output.contains("multi"));
    }

    #[test]
    fn test_strip_descriptor_overworld() {
        let input = "    ApplyMovement Overworld.0, Action#1";
        let output = transpile(input);
        assert!(output.contains("ApplyMovement 0, action_1"));
    }

    #[test]
    fn test_strip_descriptor_move() {
        let input = "    GiveItem Move.HM01, Pokemon.5";
        let output = transpile(input);
        assert!(output.contains("GiveItem HM01, 5"));
    }

    #[test]
    fn test_strip_descriptor_multiple() {
        let input = "    SomeCommand Type.Fire Pokemon.CHARIZARD Item.POTION";
        let output = transpile(input);
        assert!(output.contains("SomeCommand Fire, CHARIZARD, POTION"));
    }

    #[test]
    fn test_space_to_comma_args() {
        let input = "    WaitTime 8 0x800C";
        let output = transpile(input);
        assert!(output.contains("WaitTime 8, 0x800C"));
    }

    #[test]
    fn test_space_to_comma_single_arg() {
        let input = "    Message 5";
        let output = transpile(input);
        assert!(output.contains("Message 5"));
    }

    #[test]
    fn test_space_to_comma_no_args() {
        let input = "    LockAll";
        let output = transpile(input);
        assert!(output.contains("LockAll"));
    }

    #[test]
    fn test_space_to_comma_many_args() {
        let input = "    SomeCmd 1 2 3 4 5";
        let output = transpile(input);
        assert!(output.contains("SomeCmd 1, 2, 3, 4, 5"));
    }
}
