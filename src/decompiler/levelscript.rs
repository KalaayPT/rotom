//! Levelscript (Initscript) types and parsing
//!
//! Levelscripts are declarative scripts that map trigger conditions to script IDs.
//! Unlike normal scripts, they don't contain executable bytecode - they just define
//! when other scripts should run (e.g., "run script 4 when entering the map").
//!
//! This module provides:
//! - Serde-compatible types for JSON serialization
//! - Binary parsing for the levelscript format
//! - Binary serialization that preserves ordered header layout for round-trip fidelity

use serde::{Deserialize, Serialize};

/// Binary command IDs for levelscript header entries
mod cmd_ids {
    pub const ENTRY_END: u8 = 0x00;
    pub const ON_FRAME_TABLE: u8 = 0x01;
    pub const ON_TRANSITION: u8 = 0x02;
    pub const ON_RESUME: u8 = 0x03;
    pub const ON_LOAD: u8 = 0x04;
}

fn parse_script_entry(bytes: &[u8], pc: &mut usize, entry_type: &str) -> Result<u32, String> {
    if *pc + 4 > bytes.len() {
        return Err(format!("Levelscript: {} truncated", entry_type));
    }
    let script_id =
        u32::from_le_bytes([bytes[*pc], bytes[*pc + 1], bytes[*pc + 2], bytes[*pc + 3]]);
    *pc += 4;
    Ok(script_id)
}

/// Ordered header entry in a levelscript binary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LevelScriptHeaderEntry {
    /// Runs upon first entering a map (including new game/load save)
    /// Primary use: modify object events, manage persisted map features
    OnTransition { script_id: u32 },

    /// Runs after the map has been fully loaded and drawn
    /// Can run again without leaving (e.g., after using a field move)
    OnResume { script_id: u32 },

    /// Runs after the map layout is loaded but not drawn yet
    /// Primary use: modify warp events, set/unset flags tied to object events
    OnLoad { script_id: u32 },

    /// Points to the var-condition region, which contains `on_var_equals` checks.
    ///
    /// This is represented explicitly so we preserve original header ordering and
    /// can round-trip empty var-condition binaries like `01 01 00 00 00 00 00 00`.
    OnVarCondition,
}

/// A var-condition entry (`var == value -> script_id`)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LevelScriptVarConditionEntry {
    pub var: u16,
    pub value: u16,
    pub script_id: u16,
}

/// A complete levelscript definition.
///
/// The important bit for round-trip correctness is that header ordering is
/// preserved explicitly via `header_entries`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LevelScript {
    /// Ordered header entries as they appeared in the binary.
    #[serde(default)]
    pub header_entries: Vec<LevelScriptHeaderEntry>,
    /// Var conditions referenced by `OnVarCondition`, if present.
    #[serde(default)]
    pub var_conditions: Vec<LevelScriptVarConditionEntry>,
    /// Additional trailing zero padding beyond normal 4-byte alignment.
    /// Not serialized to JSON — preserved as a [`BinaryQuirk`] in compile state.
    #[serde(skip)]
    pub padding: u8,
}

impl LevelScript {
    /// Create a new empty levelscript
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if this is an empty levelscript (no headers, no var conditions, no padding)
    pub fn is_empty(&self) -> bool {
        self.header_entries.is_empty() && self.var_conditions.is_empty()
    }

    /// Check if this levelscript has an explicit var-condition header.
    pub fn has_var_condition(&self) -> bool {
        self.header_entries
            .iter()
            .any(|e| matches!(e, LevelScriptHeaderEntry::OnVarCondition))
    }

    /// Check if this levelscript has any `var_equals` conditions.
    pub fn has_var_conditions(&self) -> bool {
        !self.var_conditions.is_empty()
    }

    /// Parse a levelscript from binary data
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        // Handle empty levelscript (4 zero bytes)
        if bytes.len() == 4 && bytes.iter().all(|&b| b == 0) {
            return Ok(Self::new());
        }

        if bytes.is_empty() {
            return Ok(Self::new());
        }

        let mut levelscript = LevelScript::new();
        let mut pc = 0;
        let mut var_conditions_offset: Option<usize> = None;
        let mut logical_end = 0usize;

        while pc < bytes.len() {
            let cmd = bytes[pc];
            pc += 1;

            match cmd {
                cmd_ids::ENTRY_END => {
                    logical_end = pc;
                    break;
                }
                cmd_ids::ON_FRAME_TABLE => {
                    if pc + 4 > bytes.len() {
                        return Err("Levelscript: ON_FRAME_TABLE truncated".to_string());
                    }
                    let relative_offset = u32::from_le_bytes([
                        bytes[pc],
                        bytes[pc + 1],
                        bytes[pc + 2],
                        bytes[pc + 3],
                    ]) as usize;
                    let absolute_offset = pc + 4 + relative_offset;
                    pc += 4;
                    logical_end = pc;
                    var_conditions_offset = Some(absolute_offset);
                    levelscript
                        .header_entries
                        .push(LevelScriptHeaderEntry::OnVarCondition);
                }
                cmd_ids::ON_TRANSITION => {
                    let script_id = parse_script_entry(bytes, &mut pc, "ON_TRANSITION")?;
                    levelscript
                        .header_entries
                        .push(LevelScriptHeaderEntry::OnTransition { script_id });
                    logical_end = pc;
                }
                cmd_ids::ON_RESUME => {
                    let script_id = parse_script_entry(bytes, &mut pc, "ON_RESUME")?;
                    levelscript
                        .header_entries
                        .push(LevelScriptHeaderEntry::OnResume { script_id });
                    logical_end = pc;
                }
                cmd_ids::ON_LOAD => {
                    let script_id = parse_script_entry(bytes, &mut pc, "ON_LOAD")?;
                    levelscript
                        .header_entries
                        .push(LevelScriptHeaderEntry::OnLoad { script_id });
                    logical_end = pc;
                }
                unknown => {
                    return Err(format!(
                        "Levelscript: Unknown command byte 0x{:02X} at offset {}",
                        unknown,
                        pc - 1
                    ));
                }
            }
        }

        if let Some(offset) = var_conditions_offset
            && offset < bytes.len()
        {
            let mut var_conditions_pc = offset;

            while var_conditions_pc + 2 <= bytes.len() {
                let var =
                    u16::from_le_bytes([bytes[var_conditions_pc], bytes[var_conditions_pc + 1]]);

                if var == 0 {
                    logical_end = var_conditions_pc + 2;
                    break;
                }

                if var_conditions_pc + 6 > bytes.len() {
                    return Err("Levelscript: Var-condition entry truncated".to_string());
                }

                let value = u16::from_le_bytes([
                    bytes[var_conditions_pc + 2],
                    bytes[var_conditions_pc + 3],
                ]);
                let script_id = u16::from_le_bytes([
                    bytes[var_conditions_pc + 4],
                    bytes[var_conditions_pc + 5],
                ]);
                var_conditions_pc += 6;

                levelscript
                    .var_conditions
                    .push(LevelScriptVarConditionEntry {
                        var,
                        value,
                        script_id,
                    });
                logical_end = var_conditions_pc;
            }
        }

        if logical_end == 0 {
            logical_end = bytes.len();
        }

        let aligned_end = (logical_end + 3) & !3;
        if bytes.len() > aligned_end {
            let extra = &bytes[aligned_end..];
            if extra.iter().all(|&b| b == 0) {
                levelscript.padding = (bytes.len() - aligned_end) as u8;
            }
        }

        Ok(levelscript)
    }

    /// Serialize the levelscript to binary format
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();

        if self.is_empty() {
            return vec![0, 0, 0, 0];
        }

        let has_var_condition = self.has_var_condition();

        for entry in &self.header_entries {
            match entry {
                LevelScriptHeaderEntry::OnTransition { script_id } => {
                    bytes.push(cmd_ids::ON_TRANSITION);
                    bytes.extend_from_slice(&script_id.to_le_bytes());
                }
                LevelScriptHeaderEntry::OnResume { script_id } => {
                    bytes.push(cmd_ids::ON_RESUME);
                    bytes.extend_from_slice(&script_id.to_le_bytes());
                }
                LevelScriptHeaderEntry::OnLoad { script_id } => {
                    bytes.push(cmd_ids::ON_LOAD);
                    bytes.extend_from_slice(&script_id.to_le_bytes());
                }
                LevelScriptHeaderEntry::OnVarCondition => {
                    bytes.push(cmd_ids::ON_FRAME_TABLE);
                    // Patched after header serialization once var-condition start is known.
                    bytes.extend_from_slice(&[0, 0, 0, 0]);
                }
            }
        }

        bytes.push(cmd_ids::ENTRY_END);

        if has_var_condition {
            let mut cursor = 0usize;
            let mut var_conditions_pointer_pos: Option<usize> = None;

            for entry in &self.header_entries {
                match entry {
                    LevelScriptHeaderEntry::OnTransition { .. }
                    | LevelScriptHeaderEntry::OnResume { .. }
                    | LevelScriptHeaderEntry::OnLoad { .. } => {
                        cursor += 5;
                    }
                    LevelScriptHeaderEntry::OnVarCondition => {
                        var_conditions_pointer_pos = Some(cursor + 1);
                        cursor += 5;
                    }
                }
            }

            let var_conditions_start = bytes.len();

            if let Some(offset_pos) = var_conditions_pointer_pos {
                let relative_offset = (var_conditions_start - offset_pos - 4) as u32;
                let offset_bytes = relative_offset.to_le_bytes();
                bytes[offset_pos] = offset_bytes[0];
                bytes[offset_pos + 1] = offset_bytes[1];
                bytes[offset_pos + 2] = offset_bytes[2];
                bytes[offset_pos + 3] = offset_bytes[3];
            }

            for entry in &self.var_conditions {
                bytes.extend_from_slice(&entry.var.to_le_bytes());
                bytes.extend_from_slice(&entry.value.to_le_bytes());
                bytes.extend_from_slice(&entry.script_id.to_le_bytes());
            }

            bytes.extend_from_slice(&[0, 0]);
        }

        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }

        bytes.extend(std::iter::repeat_n(0, self.padding as usize));

        bytes
    }

    /// Convert to pretty-printed JSON string
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Parse from JSON string
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_levelscript_roundtrip() {
        let ls = LevelScript::new();
        let bytes = ls.to_bytes();
        assert_eq!(bytes, vec![0, 0, 0, 0]);

        let parsed = LevelScript::from_bytes(&bytes).unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn test_simple_levelscript_roundtrip() {
        let mut ls = LevelScript::new();
        ls.header_entries
            .push(LevelScriptHeaderEntry::OnTransition { script_id: 1 });
        ls.header_entries
            .push(LevelScriptHeaderEntry::OnLoad { script_id: 2 });

        let bytes = ls.to_bytes();
        let parsed = LevelScript::from_bytes(&bytes).unwrap();

        assert_eq!(parsed.header_entries.len(), 2);
        assert!(matches!(
            &parsed.header_entries[0],
            LevelScriptHeaderEntry::OnTransition { script_id: 1 }
        ));
        assert!(matches!(
            &parsed.header_entries[1],
            LevelScriptHeaderEntry::OnLoad { script_id: 2 }
        ));
    }

    #[test]
    fn test_json_roundtrip() {
        let mut ls = LevelScript::new();
        ls.header_entries
            .push(LevelScriptHeaderEntry::OnTransition { script_id: 1 });
        ls.header_entries
            .push(LevelScriptHeaderEntry::OnVarCondition);
        ls.var_conditions.push(LevelScriptVarConditionEntry {
            var: 0x411A,
            value: 1,
            script_id: 29,
        });

        let json = ls.to_json().unwrap();
        let parsed = LevelScript::from_json(&json).unwrap();

        assert_eq!(parsed, ls);
    }

    #[test]
    fn test_var_equals_roundtrip() {
        let mut ls = LevelScript::new();
        ls.header_entries
            .push(LevelScriptHeaderEntry::OnTransition { script_id: 1 });
        ls.header_entries
            .push(LevelScriptHeaderEntry::OnVarCondition);
        ls.var_conditions.push(LevelScriptVarConditionEntry {
            var: 0x411A,
            value: 1,
            script_id: 29,
        });
        ls.var_conditions.push(LevelScriptVarConditionEntry {
            var: 0x4000,
            value: 5,
            script_id: 3,
        });

        let bytes = ls.to_bytes();
        let parsed = LevelScript::from_bytes(&bytes).unwrap();

        assert_eq!(parsed.var_conditions.len(), 2);
        assert!(parsed.has_var_conditions());
    }

    #[test]
    fn test_all_entry_types() {
        let mut ls = LevelScript::new();
        ls.header_entries
            .push(LevelScriptHeaderEntry::OnTransition { script_id: 1 });
        ls.header_entries
            .push(LevelScriptHeaderEntry::OnLoad { script_id: 2 });
        ls.header_entries
            .push(LevelScriptHeaderEntry::OnResume { script_id: 3 });

        let bytes = ls.to_bytes();
        let parsed = LevelScript::from_bytes(&bytes).unwrap();

        assert_eq!(parsed.header_entries.len(), 3);
    }

    #[test]
    fn test_binary_alignment() {
        let mut ls = LevelScript::new();
        ls.header_entries
            .push(LevelScriptHeaderEntry::OnTransition { script_id: 1 });

        let bytes = ls.to_bytes();
        assert_eq!(bytes.len() % 4, 0);

        ls.header_entries
            .push(LevelScriptHeaderEntry::OnVarCondition);
        ls.var_conditions.push(LevelScriptVarConditionEntry {
            var: 0x4000,
            value: 1,
            script_id: 5,
        });

        let bytes = ls.to_bytes();
        assert_eq!(bytes.len() % 4, 0);
    }

    #[test]
    fn test_preserves_padding_beyond_normal_alignment() {
        let bytes = vec![
            cmd_ids::ON_RESUME,
            0x4C,
            0x26,
            0x00,
            0x00,
            cmd_ids::ON_TRANSITION,
            0x4A,
            0x26,
            0x00,
            0x00,
            cmd_ids::ENTRY_END,
            0x00,
            0x00,
            0x00,
            0x00,
            0x00,
        ];

        let parsed = LevelScript::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.padding, 4);
        assert_eq!(parsed.to_bytes(), bytes);
    }

    #[test]
    fn test_preserves_frame_table_header_order_before_other_entries() {
        let bytes = vec![
            cmd_ids::ON_FRAME_TABLE,
            0x0B,
            0x00,
            0x00,
            0x00,
            cmd_ids::ON_RESUME,
            0x0C,
            0x00,
            0x00,
            0x00,
            cmd_ids::ON_TRANSITION,
            0x1A,
            0x00,
            0x00,
            0x00,
            cmd_ids::ENTRY_END,
            0xF7,
            0x40,
            0x01,
            0x00,
            0xA7,
            0x28,
            0x00,
            0x00,
        ];

        let parsed = LevelScript::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.to_bytes(), bytes);
        assert!(matches!(
            parsed.header_entries.first(),
            Some(LevelScriptHeaderEntry::OnVarCondition)
        ));
    }

    #[test]
    fn test_preserves_empty_var_conditions_binary() {
        let bytes = vec![0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];

        let parsed = LevelScript::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.header_entries.len(), 1);
        assert!(matches!(
            parsed.header_entries[0],
            LevelScriptHeaderEntry::OnVarCondition
        ));
        assert!(parsed.var_conditions.is_empty());
        assert_eq!(parsed.to_bytes(), bytes);
    }
}
