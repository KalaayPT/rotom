//! Levelscript (Initscript) types and parsing
//!
//! Levelscripts are declarative scripts that map trigger conditions to script IDs.
//! Unlike normal scripts, they don't contain executable bytecode - they just define
//! when other scripts should run (e.g., "run script 4 when entering the map").
//!
//! This module provides:
//! - Serde-compatible types for JSON serialization
//! - Binary parsing for the levelscript format

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

/// The type of trigger that causes a script to run
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LevelScriptEntry {
    /// Runs upon first entering a map (including new game/load save)
    /// Primary use: modify object events, manage persisted map features
    OnTransition { script_id: u32 },

    /// Runs after the map has been fully loaded and drawn
    /// Can run again without leaving (e.g., after using a field move)
    OnResume { script_id: u32 },

    /// Runs after the map layout is loaded but not drawn yet
    /// Primary use: modify warp events, set/unset flags tied to object events
    OnLoad { script_id: u32 },

    /// Runs when a variable equals a specific value (checked every frame)
    /// This is the flattened representation of frame table entries
    OnVarEquals {
        var: u16,
        value: u16,
        script_id: u16,
    },
}

/// A complete levelscript definition
///
/// Levelscripts are purely declarative - they define trigger conditions
/// that cause other scripts to run. This structure can be serialized to
/// JSON for easy editing or to binary for the game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LevelScript {
    /// The trigger entries (on_transition, on_load, on_resume, on_var_equals)
    pub entries: Vec<LevelScriptEntry>,
}

impl LevelScript {
    /// Create a new empty levelscript
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if this is an empty levelscript (no entries)
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Check if this levelscript has any var_equals conditions
    /// (which require a frame table in binary format)
    pub fn has_var_conditions(&self) -> bool {
        self.entries
            .iter()
            .any(|e| matches!(e, LevelScriptEntry::OnVarEquals { .. }))
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
        let mut frame_table_offset: Option<usize> = None;

        while pc < bytes.len() {
            let cmd = bytes[pc];
            pc += 1;

            match cmd {
                cmd_ids::ENTRY_END => break,
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
                    frame_table_offset = Some(absolute_offset);
                }
                cmd_ids::ON_TRANSITION => {
                    let script_id = parse_script_entry(bytes, &mut pc, "ON_TRANSITION")?;
                    levelscript
                        .entries
                        .push(LevelScriptEntry::OnTransition { script_id });
                }
                cmd_ids::ON_RESUME => {
                    let script_id = parse_script_entry(bytes, &mut pc, "ON_RESUME")?;
                    levelscript
                        .entries
                        .push(LevelScriptEntry::OnResume { script_id });
                }
                cmd_ids::ON_LOAD => {
                    let script_id = parse_script_entry(bytes, &mut pc, "ON_LOAD")?;
                    levelscript
                        .entries
                        .push(LevelScriptEntry::OnLoad { script_id });
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

        if let Some(offset) = frame_table_offset
            && offset < bytes.len()
        {
            let mut ft_pc = offset;

            while ft_pc + 2 <= bytes.len() {
                let var = u16::from_le_bytes([bytes[ft_pc], bytes[ft_pc + 1]]);

                if var == 0 {
                    break;
                }

                if ft_pc + 6 > bytes.len() {
                    return Err("Levelscript: Frame table entry truncated".to_string());
                }

                let value = u16::from_le_bytes([bytes[ft_pc + 2], bytes[ft_pc + 3]]);
                let script_id = u16::from_le_bytes([bytes[ft_pc + 4], bytes[ft_pc + 5]]);
                ft_pc += 6;

                levelscript.entries.push(LevelScriptEntry::OnVarEquals {
                    var,
                    value,
                    script_id,
                });
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

        let header_entries: Vec<_> = self
            .entries
            .iter()
            .filter(|e| !matches!(e, LevelScriptEntry::OnVarEquals { .. }))
            .collect();

        let var_conditions: Vec<_> = self
            .entries
            .iter()
            .filter_map(|e| {
                if let LevelScriptEntry::OnVarEquals {
                    var,
                    value,
                    script_id,
                } = e
                {
                    Some((*var, *value, *script_id))
                } else {
                    None
                }
            })
            .collect();

        let has_var_conditions = !var_conditions.is_empty();

        for entry in &header_entries {
            let (cmd, script_id) = match entry {
                LevelScriptEntry::OnTransition { script_id } => (cmd_ids::ON_TRANSITION, script_id),
                LevelScriptEntry::OnResume { script_id } => (cmd_ids::ON_RESUME, script_id),
                LevelScriptEntry::OnLoad { script_id } => (cmd_ids::ON_LOAD, script_id),
                LevelScriptEntry::OnVarEquals { .. } => unreachable!(),
            };

            bytes.push(cmd);
            bytes.extend_from_slice(&script_id.to_le_bytes());
        }

        if has_var_conditions {
            bytes.push(cmd_ids::ON_FRAME_TABLE);
            let offset_pos = bytes.len();
            bytes.extend_from_slice(&[0, 0, 0, 0]);

            bytes.push(cmd_ids::ENTRY_END);

            let frame_table_start = bytes.len();

            // NOTE: relative offset formula matches the decomp assembler: label - current_pos - 4
            let relative_offset = (frame_table_start - offset_pos - 4) as u32;

            let offset_bytes = relative_offset.to_le_bytes();
            bytes[offset_pos] = offset_bytes[0];
            bytes[offset_pos + 1] = offset_bytes[1];
            bytes[offset_pos + 2] = offset_bytes[2];
            bytes[offset_pos + 3] = offset_bytes[3];

            for (var, value, script_id) in var_conditions {
                bytes.extend_from_slice(&var.to_le_bytes());
                bytes.extend_from_slice(&value.to_le_bytes());
                bytes.extend_from_slice(&script_id.to_le_bytes());
            }

            bytes.extend_from_slice(&[0, 0]);
        } else {
            bytes.push(cmd_ids::ENTRY_END);
        }

        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }

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
        ls.entries
            .push(LevelScriptEntry::OnTransition { script_id: 1 });
        ls.entries.push(LevelScriptEntry::OnLoad { script_id: 2 });

        let bytes = ls.to_bytes();
        let parsed = LevelScript::from_bytes(&bytes).unwrap();

        assert_eq!(parsed.entries.len(), 2);
        assert!(matches!(
            &parsed.entries[0],
            LevelScriptEntry::OnTransition { script_id: 1 }
        ));
        assert!(matches!(
            &parsed.entries[1],
            LevelScriptEntry::OnLoad { script_id: 2 }
        ));
    }

    #[test]
    fn test_json_roundtrip() {
        let mut ls = LevelScript::new();
        ls.entries
            .push(LevelScriptEntry::OnTransition { script_id: 1 });
        ls.entries.push(LevelScriptEntry::OnVarEquals {
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
        ls.entries
            .push(LevelScriptEntry::OnTransition { script_id: 1 });
        ls.entries.push(LevelScriptEntry::OnVarEquals {
            var: 0x411A,
            value: 1,
            script_id: 29,
        });
        ls.entries.push(LevelScriptEntry::OnVarEquals {
            var: 0x4000,
            value: 5,
            script_id: 3,
        });

        let bytes = ls.to_bytes();
        let parsed = LevelScript::from_bytes(&bytes).unwrap();

        assert_eq!(parsed.entries.len(), 3);
        assert!(parsed.has_var_conditions());
    }

    #[test]
    fn test_all_entry_types() {
        let mut ls = LevelScript::new();
        ls.entries
            .push(LevelScriptEntry::OnTransition { script_id: 1 });
        ls.entries.push(LevelScriptEntry::OnLoad { script_id: 2 });
        ls.entries.push(LevelScriptEntry::OnResume { script_id: 3 });

        let bytes = ls.to_bytes();
        let parsed = LevelScript::from_bytes(&bytes).unwrap();

        assert_eq!(parsed.entries.len(), 3);
    }

    #[test]
    fn test_binary_alignment() {
        let mut ls = LevelScript::new();
        ls.entries
            .push(LevelScriptEntry::OnTransition { script_id: 1 });

        let bytes = ls.to_bytes();
        assert_eq!(bytes.len() % 4, 0);

        ls.entries.push(LevelScriptEntry::OnVarEquals {
            var: 0x4000,
            value: 1,
            script_id: 5,
        });

        let bytes = ls.to_bytes();
        assert_eq!(bytes.len() % 4, 0);
    }
}
