use std::collections::HashMap;

use crate::database::ConstantDb;
use crate::decompiler::levelscript::{
    LevelScript, LevelScriptHeaderEntry, LevelScriptVarConditionEntry,
};
use dashmap::DashMap;
use uxie::c_parser::defines::eval_expr_with_parent;

#[derive(Debug, Clone)]
pub struct LevelscriptTranspileResult {
    pub levelscript: LevelScript,
    pub extra_padding: u32,
}

#[derive(Debug)]
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

pub fn transpile_levelscript(
    source: &str,
    constants: Option<&ConstantDb>,
) -> Result<LevelscriptTranspileResult, TranspileError> {
    let mut levelscript = LevelScript::new();
    let mut var_conditions_label: Option<String> = None;
    let mut in_var_conditions = false;
    let mut extra_padding: u32 = 0;

    for (line_no, line) in source.lines().enumerate() {
        let line_num = line_no + 1;
        let trimmed = line.trim();

        if trimmed.is_empty()
            || trimmed.starts_with("//")
            || trimmed.starts_with('@')
            || trimmed.starts_with("#include")
            || trimmed.starts_with(".balign")
            || trimmed.starts_with(".align")
            || matches!(trimmed, "InitScriptEntryEnd" | "InitScriptEnd")
        {
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix(".long") {
            let value_str = rest.trim();
            if parse_u32_value(value_str) == Ok(0) {
                extra_padding += 4;
            }
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix(".short") {
            let value_str = rest.trim();
            if parse_u16_value(value_str, constants) == Ok(0) {
                extra_padding += 2;
            }
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix(".byte") {
            let value_str = rest.trim();
            if parse_u8_value(value_str) == Ok(0) {
                extra_padding += 1;
            }
            continue;
        }

        if trimmed.starts_with('.') {
            continue;
        }

        if trimmed.ends_with(':') {
            let label = trimmed.trim_end_matches(':');
            if var_conditions_label
                .as_ref()
                .is_some_and(|var_conditions_label| label == var_conditions_label)
            {
                in_var_conditions = true;
            }
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("InitScriptEntry_OnTransition") {
            let script_id =
                parse_script_id(rest.trim(), constants).map_err(|e| TranspileError {
                    message: format!("InitScriptEntry_OnTransition: {}", e),
                    line: line_num,
                })?;
            levelscript
                .header_entries
                .push(LevelScriptHeaderEntry::OnTransition { script_id });
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("InitScriptEntry_OnLoad") {
            let script_id =
                parse_script_id(rest.trim(), constants).map_err(|e| TranspileError {
                    message: format!("InitScriptEntry_OnLoad: {}", e),
                    line: line_num,
                })?;
            levelscript
                .header_entries
                .push(LevelScriptHeaderEntry::OnLoad { script_id });
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("InitScriptEntry_OnResume") {
            let script_id =
                parse_script_id(rest.trim(), constants).map_err(|e| TranspileError {
                    message: format!("InitScriptEntry_OnResume: {}", e),
                    line: line_num,
                })?;
            levelscript
                .header_entries
                .push(LevelScriptHeaderEntry::OnResume { script_id });
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("InitScriptEntry_OnFrameTable") {
            let label = rest.trim().to_string();
            if label.is_empty() {
                return Err(TranspileError {
                    message: "InitScriptEntry_OnFrameTable requires a label argument".to_string(),
                    line: line_num,
                });
            }
            var_conditions_label = Some(label);
            levelscript
                .header_entries
                .push(LevelScriptHeaderEntry::OnVarCondition);
            continue;
        }

        if in_var_conditions {
            if let Some(rest) = trimmed.strip_prefix("InitScriptGoToIfEqual") {
                let args = rest.trim();
                let (var, value, script_id) =
                    parse_frame_table_entry(args, constants).map_err(|e| TranspileError {
                        message: format!("InitScriptGoToIfEqual: {}", e),
                        line: line_num,
                    })?;
                levelscript
                    .var_conditions
                    .push(LevelScriptVarConditionEntry {
                        var,
                        value,
                        script_id,
                    });
                continue;
            }

            if trimmed == "InitScriptFrameTableEnd" {
                in_var_conditions = false;
                continue;
            }
        }

        if trimmed == "InitScriptEnd" {
            continue;
        }

        return Err(TranspileError {
            message: format!("unrecognized levelscript line: '{}'", trimmed),
            line: line_num,
        });
    }

    Ok(LevelscriptTranspileResult {
        levelscript,
        extra_padding,
    })
}

fn parse_int_literal<T>(arg: &str, constants: Option<&ConstantDb>) -> Result<T, String>
where
    T: std::str::FromStr + TryFrom<i32>,
    <T as std::str::FromStr>::Err: std::fmt::Display,
{
    let arg = arg.trim();
    if arg.is_empty() {
        return Err("missing value".to_string());
    }

    if let Some(hex) = arg.strip_prefix("0x").or_else(|| arg.strip_prefix("0X")) {
        return i32::from_str_radix(hex, 16)
            .map_err(|e| format!("invalid hex: {}", e))
            .and_then(|v| T::try_from(v).map_err(|_| "value out of range".to_string()));
    }

    if let Ok(num) = arg.parse::<T>() {
        return Ok(num);
    }

    if let Some(value) = constants.and_then(|c| c.get(arg)) {
        return T::try_from(value).map_err(|_| "constant value out of range".to_string());
    }

    if let Some(constants) = constants {
        let exprs: HashMap<String, String> = HashMap::new();
        let resolved: HashMap<String, i64> = HashMap::new();
        let cache: DashMap<String, i64> = DashMap::new();
        let parent_resolver = |name: &str| constants.get(name).map(i64::from);

        if let Some(value) = eval_expr_with_parent(arg, &exprs, &resolved, &cache, &parent_resolver)
        {
            return i32::try_from(value)
                .map_err(|_| "constant value out of range".to_string())
                .and_then(|v| T::try_from(v).map_err(|_| "value out of range".to_string()));
        }
    }

    Err(format!("unknown constant or invalid number: '{}'", arg))
}

fn parse_u32_value(arg: &str) -> Result<u32, String> {
    parse_int_literal::<u32>(arg, None)
}

fn parse_u8_value(arg: &str) -> Result<u8, String> {
    parse_int_literal::<u8>(arg, None)
}

fn parse_u16_value(arg: &str, constants: Option<&ConstantDb>) -> Result<u16, String> {
    parse_int_literal::<u16>(arg, constants)
}

fn parse_script_id(arg: &str, constants: Option<&ConstantDb>) -> Result<u32, String> {
    parse_int_literal::<u32>(arg, constants)
}

fn parse_frame_table_entry(
    args: &str,
    constants: Option<&ConstantDb>,
) -> Result<(u16, u16, u16), String> {
    let parts: Vec<&str> = args.split(',').map(str::trim).collect();

    if parts.len() != 3 {
        return Err(format!(
            "expected 3 arguments (var, value, script_id), got {}",
            parts.len()
        ));
    }

    let var = parse_u16_value(parts[0], constants)?;
    let value = parse_u16_value(parts[1], constants)?;
    let script_id = parse_u16_value(parts[2], constants)?;

    Ok((var, value, script_id))
}

pub fn is_levelscript_source(source: &str) -> bool {
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("InitScriptEntry_") || trimmed == "InitScriptEntryEnd" {
            return true;
        }
        if trimmed.starts_with("ScriptEntry") {
            return false;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_levelscript() {
        let source = r#"
#include "macros/scrcmd.inc"

    InitScriptEntry_OnLoad 2
    InitScriptEntry_OnTransition 1
    InitScriptEntryEnd

    InitScriptEnd
"#;

        let result = transpile_levelscript(source, None).unwrap();
        assert_eq!(result.levelscript.header_entries.len(), 2);
        assert_eq!(result.extra_padding, 0);

        assert!(matches!(
            &result.levelscript.header_entries[0],
            LevelScriptHeaderEntry::OnLoad { script_id: 2 }
        ));
        assert!(matches!(
            &result.levelscript.header_entries[1],
            LevelScriptHeaderEntry::OnTransition { script_id: 1 }
        ));
    }

    #[test]
    fn test_levelscript_with_frame_table() {
        let source = r#"
#include "macros/scrcmd.inc"

    InitScriptEntry_OnTransition 15
    InitScriptEntry_OnFrameTable InitScriptFrameTable
    InitScriptEntryEnd

InitScriptFrameTable:
    InitScriptGoToIfEqual 0x40BF, 1, 4
    InitScriptGoToIfEqual 0x40BF, 2, 2
    InitScriptGoToIfEqual 0x40BF, 3, 5
    InitScriptFrameTableEnd

    InitScriptEnd
"#;

        let result = transpile_levelscript(source, None).unwrap();
        assert_eq!(result.levelscript.header_entries.len(), 2);
        assert_eq!(result.levelscript.var_conditions.len(), 3);

        assert!(matches!(
            &result.levelscript.header_entries[0],
            LevelScriptHeaderEntry::OnTransition { script_id: 15 }
        ));
        assert!(matches!(
            &result.levelscript.header_entries[1],
            LevelScriptHeaderEntry::OnVarCondition
        ));

        assert_eq!(
            result.levelscript.var_conditions[0],
            LevelScriptVarConditionEntry {
                var: 0x40BF,
                value: 1,
                script_id: 4,
            }
        );
        assert_eq!(
            result.levelscript.var_conditions[1],
            LevelScriptVarConditionEntry {
                var: 0x40BF,
                value: 2,
                script_id: 2,
            }
        );
        assert_eq!(
            result.levelscript.var_conditions[2],
            LevelScriptVarConditionEntry {
                var: 0x40BF,
                value: 3,
                script_id: 5,
            }
        );
    }

    #[test]
    fn test_is_levelscript_source() {
        let levelscript = r"
    InitScriptEntry_OnTransition 1
    InitScriptEntryEnd
";
        assert!(is_levelscript_source(levelscript));

        let normal_script = r"
    ScriptEntry Script1
    ScriptEntryEnd
";
        assert!(!is_levelscript_source(normal_script));
    }

    #[test]
    fn test_all_entry_types() {
        let source = r"
    InitScriptEntry_OnTransition 1
    InitScriptEntry_OnLoad 2
    InitScriptEntry_OnResume 3
    InitScriptEntryEnd
    InitScriptEnd
";

        let result = transpile_levelscript(source, None).unwrap();
        assert_eq!(result.levelscript.header_entries.len(), 3);
        assert!(matches!(
            &result.levelscript.header_entries[0],
            LevelScriptHeaderEntry::OnTransition { script_id: 1 }
        ));
        assert!(matches!(
            &result.levelscript.header_entries[1],
            LevelScriptHeaderEntry::OnLoad { script_id: 2 }
        ));
        assert!(matches!(
            &result.levelscript.header_entries[2],
            LevelScriptHeaderEntry::OnResume { script_id: 3 }
        ));
    }

    #[test]
    fn test_hex_values() {
        let source = r"
    InitScriptEntry_OnTransition 0x10
    InitScriptEntryEnd
";

        let result = transpile_levelscript(source, None).unwrap();
        assert!(matches!(
            &result.levelscript.header_entries[0],
            LevelScriptHeaderEntry::OnTransition { script_id: 16 }
        ));
    }

    #[test]
    fn test_roundtrip() {
        let source = r"
    InitScriptEntry_OnTransition 1
    InitScriptEntry_OnLoad 2
    InitScriptEntryEnd
    InitScriptEnd
";

        let result = transpile_levelscript(source, None).unwrap();
        let bytes = result.levelscript.to_bytes();
        let parsed = LevelScript::from_bytes(&bytes).unwrap();

        assert_eq!(result.levelscript.header_entries, parsed.header_entries);
        assert_eq!(result.levelscript.var_conditions, parsed.var_conditions);
    }

    #[test]
    fn test_roundtrip_with_frame_table() {
        let source = r"
    InitScriptEntry_OnTransition 1
    InitScriptEntry_OnFrameTable FrameTable
    InitScriptEntryEnd

FrameTable:
    InitScriptGoToIfEqual 0x4000, 5, 3
    InitScriptGoToIfEqual 0x4001, 10, 7
    InitScriptFrameTableEnd

    InitScriptEnd
";

        let result = transpile_levelscript(source, None).unwrap();
        let bytes = result.levelscript.to_bytes();
        let parsed = LevelScript::from_bytes(&bytes).unwrap();

        assert_eq!(result.levelscript.header_entries, parsed.header_entries);
        assert_eq!(result.levelscript.var_conditions, parsed.var_conditions);
    }

    #[test]
    fn test_canalave_library_2f_binary_format() {
        let source = r#"
#include "macros/scrcmd.inc"

    InitScriptEntry_OnTransition 5
    InitScriptEntry_OnFrameTable InitScriptFrameTable
    InitScriptEntryEnd

InitScriptFrameTable:
    InitScriptGoToIfEqual 0x4056, 2, 6
    InitScriptFrameTableEnd

    InitScriptEnd
"#;

        let result = transpile_levelscript(source, None).unwrap();
        let bytes = result.levelscript.to_bytes();

        let expected: Vec<u8> = vec![
            0x02, 0x05, 0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x56, 0x40, 0x02,
            0x00, 0x06, 0x00, 0x00, 0x00, 0x00,
        ];

        assert_eq!(
            bytes.len(),
            expected.len(),
            "Size mismatch: got {} bytes, expected {} bytes",
            bytes.len(),
            expected.len()
        );

        for (i, (got, exp)) in bytes.iter().zip(expected.iter()).enumerate() {
            assert_eq!(
                got, exp,
                "Byte mismatch at offset {}: got 0x{:02X}, expected 0x{:02X}",
                i, got, exp
            );
        }
    }

    #[test]
    fn test_decomp_to_json_output() {
        let source = r#"
#include "macros/scrcmd.inc"

    InitScriptEntry_OnTransition 1
    InitScriptEntry_OnLoad 2
    InitScriptEntry_OnFrameTable InitScriptFrameTable
    InitScriptEntryEnd

InitScriptFrameTable:
    InitScriptGoToIfEqual 0x4097, 0, 5
    InitScriptFrameTableEnd

    InitScriptEnd
"#;

        let result = transpile_levelscript(source, None).unwrap();
        let json = result.levelscript.to_json().unwrap();

        assert!(json.contains("on_transition"));
        assert!(json.contains("on_load"));
        assert!(json.contains("on_var_condition"));
        assert!(json.contains("\"script_id\": 1"));
        assert!(json.contains("\"script_id\": 2"));
        assert!(json.contains("\"var\": 16535"));
        assert!(json.contains("\"value\": 0"));
        assert!(json.contains("\"script_id\": 5"));
    }

    #[test]
    fn test_extra_padding_long_directive() {
        let source = r#"
#include "macros/scrcmd.inc"

    InitScriptEntry_OnResume 0x264C
    InitScriptEntry_OnTransition 0x264A
    InitScriptEntryEnd

    InitScriptEnd
    .long 0
"#;

        let result = transpile_levelscript(source, None).unwrap();
        assert_eq!(result.levelscript.header_entries.len(), 2);
        assert_eq!(result.extra_padding, 4);
    }

    #[test]
    fn test_rejects_unknown_top_level_line() {
        let source = r"
    InitScriptEntry_OnTransition 1
    InitScriptEntryEnd
    TotallyUnknownDirective 7
    InitScriptEnd
";

        let error = transpile_levelscript(source, None).unwrap_err();
        assert_eq!(
            error.message,
            "unrecognized levelscript line: 'TotallyUnknownDirective 7'"
        );
        assert_eq!(error.line, 4);
    }

    #[test]
    fn test_rejects_unknown_frame_table_line() {
        let source = r"
    InitScriptEntry_OnFrameTable FrameTable
    InitScriptEntryEnd

FrameTable:
    UnknownFrameCommand 1, 2, 3
    InitScriptFrameTableEnd

    InitScriptEnd
";

        let error = transpile_levelscript(source, None).unwrap_err();
        assert_eq!(
            error.message,
            "unrecognized levelscript line: 'UnknownFrameCommand 1, 2, 3'"
        );
        assert_eq!(error.line, 6);
    }

    #[test]
    fn test_parse_script_id_supports_additive_constant_expression() {
        let db =
            crate::database::DatabaseV2::load(std::path::Path::new("src/db/hgss/hgss_v2.json"))
                .expect("failed to load hgss database");
        let mut constants = ConstantDb::new();
        constants.load_from_db(&db);
        constants
            .load_directory("src/db")
            .expect("failed to load constants directory");

        let base = constants.get("TRUE").expect("TRUE should exist");
        let parsed =
            parse_script_id("TRUE + 1", Some(&constants)).expect("expression should parse");

        assert_eq!(parsed, (base + 1) as u32);
    }
}
