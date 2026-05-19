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
//! script script_1 #1:
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

use crate::database::GameFamily;
use regex::Regex;
use std::borrow::Cow;
use std::fmt::Write;
use std::sync::LazyLock;

// ============================================================================
// Cached Regexes
// ============================================================================

static RE_SCRIPT_HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^Script\s+(\d+)\s*:").expect("static regex is valid"));

static RE_FUNCTION_HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^Function\s+(\d+)\s*:").expect("static regex is valid"));

static RE_ACTION_HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^Action\s+(\d+)\s*:").expect("static regex is valid"));

static RE_SCRIPT_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Script#(\d+)").expect("static regex is valid"));

static RE_FUNCTION_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Function#(\d+)").expect("static regex is valid"));

static RE_ACTION_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Action#(\d+)").expect("static regex is valid"));

static RE_USE_SCRIPT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*UseScript_#(\d+)\s*$").expect("static regex is valid"));

static RE_DESCRIPTOR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[A-Za-z_][A-Za-z0-9_]*\.([A-Za-z0-9_]+)").expect("static regex is valid")
});

/// Transpile a DSPRE script to Rotoscript format
pub fn transpile(input: &str, db: Option<&crate::database::DatabaseV2>) -> String {
    // First, strip block comments /* ... */
    let input = strip_block_comments(input);

    let mut output = String::new();
    let mut in_action = false; // Track if we're inside an Action block
    let mut action_has_end_movement = false; // Track if action already has EndMovement from hex opcode

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

        // Check for Script N: header -> becomes `script script_N #N:`
        if let Some(caps) = RE_SCRIPT_HEADER.captures(trimmed) {
            let id: u32 = caps[1].parse().expect("regex guarantees digits");
            let _ = writeln!(output, "script script_{} #{}:", id, id);
            in_action = false;
            continue;
        }

        // Check for Function N: header -> becomes bare label `func_N:`
        if let Some(caps) = RE_FUNCTION_HEADER.captures(trimmed) {
            let id: u32 = caps[1].parse().expect("regex guarantees digits");
            let _ = writeln!(output, "func_{}:", id);
            in_action = false;
            continue;
        }

        // Check for Action N: header
        if let Some(caps) = RE_ACTION_HEADER.captures(trimmed) {
            let id: u32 = caps[1].parse().expect("regex guarantees digits");
            let _ = writeln!(output, "action action_{}:", id);
            in_action = true;
            action_has_end_movement = false;
            continue;
        }

        // Check for UseScript_#N (DSPRE workaround for jumping to scripts)
        if let Some(caps) = RE_USE_SCRIPT.captures(line) {
            let id: u32 = caps[1].parse().expect("regex guarantees digits");
            // Preserve leading whitespace
            let leading_ws = &line[..line.len() - line.trim_start().len()];
            output.push_str(leading_ws);
            let _ = writeln!(output, "Jump script_{}", id);
            continue;
        }

        // Convert End to EndMovement inside Action blocks
        if in_action && trimmed.eq_ignore_ascii_case("End") {
            if !action_has_end_movement {
                output.push_str("EndMovement\n");
            }
            in_action = false;
            continue;
        }

        // For all other lines, replace references in arguments.
        // Keep a single buffer and only allocate when a replacement is needed.
        let mut processed = line.to_string();

        // Replace Script#N -> script_N
        if RE_SCRIPT_REF.is_match(&processed) {
            processed = RE_SCRIPT_REF
                .replace_all(&processed, "script_$1")
                .into_owned();
        }

        // Replace Function#N -> func_N
        if RE_FUNCTION_REF.is_match(&processed) {
            processed = RE_FUNCTION_REF
                .replace_all(&processed, "func_$1")
                .into_owned();
        }

        // Replace Action#N -> action_N
        if RE_ACTION_REF.is_match(&processed) {
            processed = RE_ACTION_REF
                .replace_all(&processed, "action_$1")
                .into_owned();
        }

        // Strip descriptors: Overworld.0 -> 0, Move.HM01 -> HM01
        if RE_DESCRIPTOR.is_match(&processed) {
            processed = RE_DESCRIPTOR.replace_all(&processed, "$1").into_owned();
        }

        // Convert DSPRE comparison operators: GREATER/EQUAL -> GREATER_EQUAL, LESS/EQUAL -> LESS_EQUAL
        if processed.contains("GREATER/EQUAL") {
            processed = processed.replace("GREATER/EQUAL", "GREATER_EQUAL");
        }
        if processed.contains("LESS/EQUAL") {
            processed = processed.replace("LESS/EQUAL", "LESS_EQUAL");
        }

        // Convert space-separated arguments to comma-separated
        // Command Arg1 Arg2 Arg3 -> Command Arg1, Arg2, Arg3
        let (processed_line, is_end_movement) =
            convert_space_to_comma_args(&processed, db, in_action);

        if is_end_movement {
            action_has_end_movement = true;
        }

        output.push_str(&processed_line);
        output.push('\n');
    }

    output
}

fn convert_space_to_comma_args(
    line: &str,
    db: Option<&crate::database::DatabaseV2>,
    in_action: bool,
) -> (String, bool) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return (line.to_string(), false);
    }

    let leading_ws = &line[..line.len() - line.trim_start().len()];
    let has_commas = line.contains(',');

    let (mut command, args): (Cow<'_, str>, Vec<&str>) = if has_commas {
        let mut parts = trimmed.split(',').map(str::trim);
        let Some(first_part) = parts.next() else {
            return (line.to_string(), false);
        };
        let mut first_tokens = first_part.split_whitespace();
        let Some(first_command) = first_tokens.next() else {
            return (line.to_string(), false);
        };
        let mut all_args: Vec<&str> = first_tokens.collect();
        all_args.extend(parts);
        (Cow::Borrowed(first_command), all_args)
    } else {
        let mut tokens = trimmed.split_whitespace();
        let Some(first_command) = tokens.next() else {
            return (line.to_string(), false);
        };
        (Cow::Borrowed(first_command), tokens.collect())
    };

    let mut is_end_movement = false;
    if let Some(db) = db {
        if in_action {
            let hex_str = command.strip_prefix("0x").unwrap_or(command.as_ref());
            if let Ok(id) = u16::from_str_radix(hex_str, 16)
                && let Some((name, _)) = db.get_movement_by_id(id)
            {
                command = Cow::Borrowed(name);
                is_end_movement = command == "EndMovement";
            }
        } else if let Some(id_str) = command.strip_prefix("CMD_")
            && let Ok(id) = id_str.parse::<u16>()
            && let Some((name, _)) = db.get_script_cmd_by_id(id)
        {
            command = Cow::Borrowed(name);
        }

        if !in_action {
            command = canonicalize_dspre_command_name(command, db);
        }
    }

    if args.is_empty() {
        return (format!("{}{}", leading_ws, command), is_end_movement);
    }

    let args_str = args.join(", ");
    (
        format!("{}{} {}", leading_ws, command, args_str),
        is_end_movement,
    )
}

fn canonicalize_dspre_command_name<'a>(
    command: Cow<'a, str>,
    db: &crate::database::DatabaseV2,
) -> Cow<'a, str> {
    match (db.game_family(), command.as_ref()) {
        (_, "PlayFanfare") => Cow::Borrowed("PlaySE"),
        (_, "StopFanfare") => Cow::Borrowed("StopSE"),
        (_, "WaitFanfare") => Cow::Borrowed("WaitSE"),
        (_, "PlaySound") => Cow::Borrowed("PlayFanfare"),
        (_, "WaitSound") => Cow::Borrowed("WaitFanfare"),
        (Some(GameFamily::DP | GameFamily::Platinum), "WildBattle") => {
            Cow::Borrowed("StartWildBattle")
        }
        (Some(GameFamily::HGSS), "WildBattle") => Cow::Borrowed("RocketTrapBattle"),
        (Some(GameFamily::HGSS), "WildBattleSp") => Cow::Borrowed("WildBattle"),
        _ => command,
    }
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
                    Some(_) => {}
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
        let output = transpile(input, None);
        assert!(output.contains("script script_1 #1:"));
    }

    #[test]
    fn test_function_header() {
        let input = "Function 2:\n    CloseMessage\nEnd";
        let output = transpile(input, None);
        // DSPRE "Function" becomes a bare label in rotoscript
        assert!(output.contains("func_2:"));
        assert!(!output.contains("script func_2"));
    }

    #[test]
    fn test_action_header() {
        let input = "Action 1:\n    LookRight 0x1\nEnd";
        let output = transpile(input, None);
        assert!(output.contains("action action_1"));
        // End inside action should become EndMovement
        assert!(output.contains("EndMovement"));
        assert!(!output.contains("\nEnd\n"));
    }

    #[test]
    fn test_script_reference() {
        let input = "    JumpIf EQUAL Script#3";
        let output = transpile(input, None);
        // Space-separated args become comma-separated
        assert!(output.contains("JumpIf EQUAL, script_3"));
    }

    #[test]
    fn test_comparison_operators() {
        let input = "    CallIf GREATER/EQUAL func_2";
        let output = transpile(input, None);
        assert!(output.contains("GREATER_EQUAL"));
        assert!(!output.contains("GREATER/EQUAL"));

        let input2 = "    JumpIf LESS/EQUAL Script#1";
        let output2 = transpile(input2, None);
        assert!(output2.contains("LESS_EQUAL"));
        assert!(!output2.contains("LESS/EQUAL"));
    }

    #[test]
    fn test_function_reference() {
        let input = "    Jump Function#2";
        let output = transpile(input, None);
        assert!(output.contains("Jump func_2"));
    }

    #[test]
    fn test_action_reference() {
        let input = "    ApplyMovement 0xFF, Action#1";
        let output = transpile(input, None);
        assert!(output.contains("ApplyMovement 0xFF, action_1"));
    }

    #[test]
    fn test_use_script_workaround() {
        let input = "    UseScript_#5";
        let output = transpile(input, None);
        assert!(output.contains("Jump script_5"));
    }

    #[test]
    fn test_full_dspre_script() {
        let input = r"Script 1:
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
";
        let output = transpile(input, None);

        assert!(output.contains("script script_1 #1:"));
        assert!(output.contains("script script_2 #2:"));
        assert!(output.contains("script script_3 #3:"));
        assert!(output.contains("func_2:"));
        assert!(!output.contains("script func_2"));
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
        let output = transpile(input, None);
        assert!(output.contains("// This is a comment"));
    }

    #[test]
    fn test_strip_block_comments() {
        let input = "Script 1:\n/* this is a comment */\n    LockAll\nEnd";
        let output = transpile(input, None);
        assert!(output.contains("LockAll"));
        assert!(!output.contains("this is a comment"));
    }

    #[test]
    fn test_strip_multiline_block_comments() {
        let input = "Script 1:\n/*\n  multi\n  line\n  comment\n*/\n    LockAll\nEnd";
        let output = transpile(input, None);
        assert!(output.contains("LockAll"));
        assert!(!output.contains("multi"));
    }

    #[test]
    fn test_strip_descriptor_overworld() {
        let input = "    ApplyMovement Overworld.0, Action#1";
        let output = transpile(input, None);
        assert!(output.contains("ApplyMovement 0, action_1"));
    }

    #[test]
    fn test_strip_descriptor_move() {
        let input = "    GiveItem Move.HM01, Pokemon.5";
        let output = transpile(input, None);
        assert!(output.contains("GiveItem HM01, 5"));
    }

    #[test]
    fn test_strip_descriptor_multiple() {
        let input = "    SomeCommand Type.Fire Pokemon.CHARIZARD Item.POTION";
        let output = transpile(input, None);
        assert!(output.contains("SomeCommand Fire, CHARIZARD, POTION"));
    }

    #[test]
    fn test_space_to_comma_args() {
        let input = "    WaitTime 8 0x800C";
        let output = transpile(input, None);
        assert!(output.contains("WaitTime 8, 0x800C"));
    }

    #[test]
    fn test_space_to_comma_single_arg() {
        let input = "    Message 5";
        let output = transpile(input, None);
        assert!(output.contains("Message 5"));
    }

    #[test]
    fn test_space_to_comma_no_args() {
        let input = "    LockAll";
        let output = transpile(input, None);
        assert!(output.contains("LockAll"));
    }

    #[test]
    fn test_space_to_comma_many_args() {
        let input = "    SomeCmd 1 2 3 4 5";
        let output = transpile(input, None);
        assert!(output.contains("SomeCmd 1, 2, 3, 4, 5"));
    }

    #[test]
    fn test_dspre_transpile_applies_explicit_legacy_sound_overrides() {
        let db = crate::database::DatabaseV2::test_platinum();
        let input = r"Script 1:
    PlayFanfare 1500
    StopFanfare 1500
    WaitFanfare 1500
    PlaySound 1501
    WaitSound 1501
End";
        let output = transpile(input, Some(db));

        assert!(output.contains("PlaySE 1500"));
        assert!(output.contains("StopSE 1500"));
        assert!(output.contains("WaitSE 1500"));
        assert!(output.contains("PlayFanfare 1501"));
        assert!(output.contains("WaitFanfare 1501"));
    }

    #[test]
    fn test_dspre_transpile_applies_family_specific_wild_battle_overrides() {
        let platinum = crate::database::DatabaseV2::test_platinum();
        let hgss = crate::database::DatabaseV2::test_hgss();

        let input = "Script 1:\n    WildBattle 1 2 3\nEnd";

        assert!(transpile(input, Some(platinum)).contains("StartWildBattle 1, 2, 3"));
        assert!(transpile(input, Some(hgss)).contains("RocketTrapBattle 1, 2, 3"));
    }

    #[test]
    fn test_dspre_transpile_maps_hgss_wild_battle_sp_to_canonical_name() {
        let hgss = crate::database::DatabaseV2::test_hgss();
        let input = "Script 1:\n    WildBattleSp 1 2 3 4\nEnd";

        assert!(transpile(input, Some(hgss)).contains("WildBattle 1, 2, 3, 4"));
    }
}
