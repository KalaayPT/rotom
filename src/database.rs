//! Database module for loading V2 script command databases
//!
//! Supports the normalized V2 JSON schema from scrcmd-database.

#![allow(dead_code)]

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

use crate::compiler::ParseResult;
use crate::compiler::parse_error::{CompileError, database_error};
use uxie::SymbolTable;

pub fn normalize_command_name(name: &str) -> String {
    name.replace('_', "").to_ascii_lowercase()
}

// ============================================================================
// Hardcoded Enums (fixed across all games)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameFamily {
    DP,       // Diamond, Pearl
    Platinum, // Platinum
    HGSS,     // HeartGold, SoulSilver
}

impl GameFamily {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "DP" | "DIAMOND" | "PEARL" => Some(Self::DP),
            "PLATINUM" | "PT" => Some(Self::Platinum),
            "HGSS" | "HEARTGOLD" | "SOULSILVER" => Some(Self::HGSS),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DP => "Diamond/Pearl",
            Self::Platinum => "Platinum",
            Self::HGSS => "HeartGold/SoulSilver",
        }
    }

    pub fn from_db_version(version: &str) -> Option<Self> {
        let v = version.to_uppercase();
        if v.contains("PLATINUM") {
            Some(Self::Platinum)
        } else if v.contains("HEARTGOLD") || v.contains("SOULSILVER") || v.contains("HGSS") {
            Some(Self::HGSS)
        } else if v.contains("DIAMOND") || v.contains("PEARL") || v.contains("DP") {
            Some(Self::DP)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ComparisonOperator {
    Less = 0,
    Equal = 1,
    Greater = 2,
    LessEqual = 3,
    GreaterEqual = 4,
    Different = 5,
}

impl ComparisonOperator {
    pub fn from_id(id: u8) -> Option<Self> {
        match id {
            0 => Some(Self::Less),
            1 => Some(Self::Equal),
            2 => Some(Self::Greater),
            3 => Some(Self::LessEqual),
            4 => Some(Self::GreaterEqual),
            5 => Some(Self::Different),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Less => "LESS",
            Self::Equal => "EQUAL",
            Self::Greater => "GREATER",
            Self::LessEqual => "LESS/EQUAL",
            Self::GreaterEqual => "GREATER/EQUAL",
            Self::Different => "DIFFERENT",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "LESS" => Some(Self::Less),
            "EQUAL" => Some(Self::Equal),
            "GREATER" => Some(Self::Greater),
            "LESS/EQUAL" | "LESSEQUAL" | "LESS_EQUAL" => Some(Self::LessEqual),
            "GREATER/EQUAL" | "GREATEREQUAL" | "GREATER_EQUAL" => Some(Self::GreaterEqual),
            "DIFFERENT" => Some(Self::Different),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Direction {
    Up = 0,
    Down = 1,
    Left = 2,
    Right = 3,
}

impl Direction {
    pub fn from_id(id: u8) -> Option<Self> {
        match id {
            0 => Some(Self::Up),
            1 => Some(Self::Down),
            2 => Some(Self::Left),
            3 => Some(Self::Right),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Up => "Up",
            Self::Down => "Down",
            Self::Left => "Left",
            Self::Right => "Right",
        }
    }
}

// ============================================================================
// V2 Database Schema
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct DatabaseV2 {
    pub meta: DatabaseMeta,
    pub commands: HashMap<String, Command>,
    #[serde(default)]
    pub sounds: HashMap<String, Sound>,
    #[serde(default)]
    pub comparison_operators: HashMap<String, String>,
    #[serde(default)]
    pub overworld_directions: HashMap<String, String>,
    #[serde(default)]
    pub special_overworlds: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct DatabaseMeta {
    pub version: String,
    #[serde(default)]
    pub generated_at: Option<String>,
    #[serde(default)]
    pub generated_from: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Command {
    #[serde(rename = "type")]
    pub cmd_type: CommandType,
    #[serde(default)]
    pub id: Option<u16>,
    #[serde(default)]
    pub legacy_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub params: Vec<ParamDef>,
    #[serde(default)]
    pub variants: Option<Vec<Variant>>,
    #[serde(default)]
    pub expansion: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandType {
    ScriptCmd,
    Movement,
    LevelscriptCmd,
    Macro,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ParamDef {
    pub name: String,
    #[serde(rename = "type")]
    pub param_type: ParamType,
    #[serde(rename = "const")]
    pub const_value: Option<String>,
    #[serde(default)]
    pub default: Option<String>,
    /// If true, this parameter can be omitted entirely (used for macros with arg-count variants)
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamType {
    U8,
    U16,
    U32,
    Var,
    Flag,
    Label,
    #[serde(alias = "msg_id")]
    MsgId,
    #[serde(alias = "script_id")]
    ScriptId,
    #[serde(alias = "movement_id")]
    MovementId,
    #[serde(other)]
    Unknown,
}

impl ParamType {
    pub fn size(&self) -> usize {
        match self {
            ParamType::U8 => 1,
            ParamType::U16 | ParamType::Var | ParamType::Flag => 2,
            ParamType::U32 | ParamType::Label | ParamType::ScriptId | ParamType::MovementId => 4,
            ParamType::MsgId => 2,
            ParamType::Unknown => 2,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Variant {
    #[serde(default)]
    pub params: Vec<ParamDef>,
    #[serde(default)]
    pub desc: Option<String>,
    #[serde(default)]
    pub condition: Option<String>,
    #[serde(default)]
    pub expansion: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct Sound {
    pub name: String,
    #[serde(default)]
    pub used_in: Option<String>,
}

impl DatabaseV2 {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, CompileError> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path).map_err(|e| CompileError::Database {
            message: format!("Failed to read database file '{}': {}", path.display(), e),
        })?;

        let db: DatabaseV2 =
            serde_json::from_str(&contents).map_err(|e| CompileError::Database {
                message: format!("Failed to parse database JSON '{}': {}", path.display(), e),
            })?;

        Ok(db)
    }

    pub fn get_command(&self, name: &str) -> ParseResult<&Command> {
        if let Some(cmd) = self.commands.get(name) {
            return Ok(cmd);
        }

        if let Some((_, cmd)) = self
            .commands
            .iter()
            .find(|(_, cmd)| cmd.legacy_name.as_deref() == Some(name))
        {
            return Ok(cmd);
        }

        if let Some(id_str) = name.strip_prefix("ScrCmd_")
            && let Ok(id) = i32::from_str_radix(id_str, 16)
            && let Some((_, cmd)) = self.commands.iter().find(|(_, cmd)| {
                cmd.id == Some(id as u16) && cmd.cmd_type == CommandType::ScriptCmd
            })
        {
            return Ok(cmd);
        }

        if let Some(id_str) = name.strip_prefix("Dummy")
            && let Ok(id) = i32::from_str_radix(id_str, 16)
            && let Some((_, cmd)) = self.commands.iter().find(|(_, cmd)| {
                cmd.id == Some(id as u16) && cmd.cmd_type == CommandType::ScriptCmd
            })
        {
            return Ok(cmd);
        }

        Err(database_error(format!(
            "Command '{}' not found in database",
            name
        )))
    }

    pub fn get_script_cmd(&self, name: &str) -> ParseResult<&Command> {
        match self
            .commands
            .get(name)
            .filter(|cmd| cmd.cmd_type == CommandType::ScriptCmd)
        {
            Some(cmd) => Ok(cmd),
            None => {
                if let Some((_, cmd)) = self
                    .commands
                    .iter()
                    .find(|(_, cmd)| cmd.legacy_name.as_deref() == Some(name))
                    .filter(|(_, cmd)| cmd.cmd_type == CommandType::ScriptCmd)
                {
                    Ok(cmd)
                } else {
                    Err(database_error(format!(
                        "Command '{}' not found in database",
                        name
                    )))
                }
            }
        }
    }

    pub fn get_movement(&self, name: &str) -> Result<&Command, CompileError> {
        match self
            .commands
            .get(name)
            .filter(|cmd| cmd.cmd_type == CommandType::Movement)
        {
            Some(cmd) => Ok(cmd),
            None => {
                if let Some((_, cmd)) = self
                    .commands
                    .iter()
                    .find(|(_, cmd)| cmd.legacy_name.as_deref() == Some(name))
                    .filter(|(_, cmd)| cmd.cmd_type == CommandType::Movement)
                {
                    Ok(cmd)
                } else {
                    Err(database_error(format!(
                        "Movement '{}' not found in database",
                        name
                    )))
                }
            }
        }
    }

    pub fn script_commands(&self) -> impl Iterator<Item = (&String, &Command)> {
        self.commands
            .iter()
            .filter(|(_, cmd)| cmd.cmd_type == CommandType::ScriptCmd)
    }

    pub fn movements(&self) -> impl Iterator<Item = (&String, &Command)> {
        self.commands
            .iter()
            .filter(|(_, cmd)| cmd.cmd_type == CommandType::Movement)
    }

    pub fn get_script_cmd_by_id(&self, id: u16) -> Option<(&String, &Command)> {
        self.commands
            .iter()
            .find(|(_, cmd)| cmd.cmd_type == CommandType::ScriptCmd && cmd.id == Some(id))
    }

    pub fn get_movement_by_id(&self, id: u16) -> Option<(&String, &Command)> {
        self.commands
            .iter()
            .find(|(_, cmd)| cmd.cmd_type == CommandType::Movement && cmd.id == Some(id))
    }
}

impl Command {
    pub fn is_macro(&self) -> bool {
        self.cmd_type == CommandType::Macro
    }

    pub fn get_variant_params(&self, mode: u8) -> &[ParamDef] {
        if let Some(variants) = &self.variants {
            for variant in variants {
                if let Some(first_param) = variant.params.first()
                    && let Some(const_val) = &first_param.const_value
                    && const_val.parse::<u8>().ok() == Some(mode)
                {
                    return &variant.params;
                }
            }
        }
        &self.params
    }

    pub fn has_variants(&self) -> bool {
        self.variants.is_some()
    }

    pub fn params_size(&self) -> usize {
        self.params.iter().map(|p| p.param_type.size()).sum()
    }
}

// ============================================================================
// Constants Database
// ============================================================================

/// Central repository for all named constants (built-in, DSPRE, and Decomp)
#[derive(Debug, Default, Clone)]
pub struct ConstantDb {
    /// Manual and built-in constants: name -> value
    constants: HashMap<String, i32>,
    /// Uxie-powered symbol table for decomp projects
    uxie_symbols: Option<SymbolTable>,
}

impl ConstantDb {
    pub fn new() -> Self {
        ConstantDb {
            constants: HashMap::new(),
            uxie_symbols: None,
        }
    }

    pub fn load_from_db(&mut self, db: &DatabaseV2) -> usize {
        let mut count = 0;

        self.constants.insert("TRUE".to_string(), 1);
        self.constants.insert("FALSE".to_string(), 0);
        count += 2;

        self.constants.insert("VARS_START".to_string(), 0x4000);
        count += 1;

        for (id_str, name) in &db.comparison_operators {
            if let Ok(id) = id_str.parse::<i32>() {
                let normalized = name.replace('/', "_");
                self.constants.insert(normalized, id);
                count += 1;
            }
        }

        for (id_str, name) in &db.overworld_directions {
            if let Ok(id) = id_str.parse::<i32>() {
                self.constants.insert(name.clone(), id);
                count += 1;
            }
        }

        for (id_str, name) in &db.special_overworlds {
            if let Ok(id) = id_str.parse::<i32>() {
                self.constants.insert(name.clone(), id);
                count += 1;
            }
        }

        for (id_str, sound) in &db.sounds {
            if let Ok(id) = id_str.parse::<i32>() {
                self.constants.insert(sound.name.clone(), id);
                count += 1;
            }
        }

        count
    }

    pub fn load_json<P: AsRef<Path>>(&mut self, path: P) -> Result<usize, CompileError> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path).map_err(|e| CompileError::Database {
            message: format!("Failed to read constants file '{}': {}", path.display(), e),
        })?;

        let raw: HashMap<String, String> =
            serde_json::from_str(&contents).map_err(|e| CompileError::Database {
                message: format!("Failed to parse constants JSON '{}': {}", path.display(), e),
            })?;

        let mut count = 0;
        for (id_str, name) in raw {
            if let Ok(id) = id_str.parse::<i32>() {
                self.constants.insert(name, id);
                count += 1;
            }
        }

        Ok(count)
    }

    pub fn load_directory<P: AsRef<Path>>(&mut self, dir: P) -> Result<usize, CompileError> {
        let dir = dir.as_ref();
        let mut total = 0;
        let mut errors: Vec<String> = Vec::new();

        if !dir.exists() || !dir.is_dir() {
            return Ok(0);
        }

        let entries = std::fs::read_dir(dir).map_err(|e| CompileError::Database {
            message: format!("Failed to read directory '{}': {}", dir.display(), e),
        })?;

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    errors.push(format!(
                        "failed to read directory entry in '{}': {}",
                        dir.display(),
                        e
                    ));
                    continue;
                }
            };
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if filename.contains("_v2") || filename == "commands.json" {
                    continue;
                }
                match self.load_json(&path) {
                    Ok(count) => {
                        total += count;
                    }
                    Err(e) => {
                        errors.push(format!("{}: {}", path.display(), e));
                    }
                }
            }
        }

        if errors.is_empty() {
            Ok(total)
        } else {
            const MAX_ERRORS_IN_MESSAGE: usize = 5;
            let shown_errors: Vec<&str> = errors
                .iter()
                .take(MAX_ERRORS_IN_MESSAGE)
                .map(String::as_str)
                .collect();
            let mut message = format!(
                "Failed to load one or more constants JSON files from '{}'. Loaded {} constants before failure(s): {}",
                dir.display(),
                total,
                shown_errors.join("; "),
            );
            if errors.len() > MAX_ERRORS_IN_MESSAGE {
                use std::fmt::Write;
                let _ = write!(
                    message,
                    "; and {} additional error(s)",
                    errors.len() - MAX_ERRORS_IN_MESSAGE
                );
            }
            Err(CompileError::Database { message })
        }
    }

    pub fn load_decomp_project<P: AsRef<Path>>(&mut self, root: P) -> Result<usize, CompileError> {
        let ws = uxie::Workspace::open_decomp(root).map_err(|e| CompileError::Database {
            message: format!("Failed to open decomp project via Uxie: {}", e),
        })?;

        let symbols = (*ws.symbols).clone();
        let count = symbols.get_all_defines().len();
        self.uxie_symbols = Some(symbols);
        Ok(count)
    }

    pub fn load_map_events<P: AsRef<Path>, Q: AsRef<Path>>(
        &mut self,
        decomp_root: P,
        script_path: Q,
    ) -> Result<usize, CompileError> {
        let script_name = match script_path.as_ref().file_stem().and_then(|s| s.to_str()) {
            Some(name) => name,
            None => return Ok(0),
        };

        let map_name = if let Some(stripped) = script_name.strip_prefix("scripts_") {
            stripped
        } else {
            return Ok(0);
        };

        let events_json = decomp_root
            .as_ref()
            .join("res")
            .join("field")
            .join("events")
            .join(format!("events_{}.json", map_name));

        if !events_json.exists() {
            return Ok(0);
        }

        if let Some(symbols) = &mut self.uxie_symbols {
            let start_count = symbols.get_all_defines().len();
            symbols
                .load_events_json(&events_json)
                .map_err(|e| CompileError::Database {
                    message: format!("Failed to load map events JSON: {}", e),
                })?;
            Ok(symbols.get_all_defines().len() - start_count)
        } else {
            Ok(0)
        }
    }

    pub fn get(&self, name: &str) -> Option<i32> {
        if let Some(val) = self.constants.get(name) {
            return Some(*val);
        }

        if let Some(symbols) = &self.uxie_symbols
            && let Some(val) = symbols.resolve_constant(name)
        {
            return Some(val as i32);
        }

        None
    }

    pub fn len(&self) -> usize {
        let uxie_count = self
            .uxie_symbols
            .as_ref()
            .map_or(0, |s| s.get_all_defines().len());
        self.constants.len() + uxie_count
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_command(cmd_type: CommandType, id: u16, legacy_name: Option<&str>) -> Command {
        Command {
            cmd_type,
            id: Some(id),
            legacy_name: legacy_name.map(std::string::ToString::to_string),
            description: None,
            params: Vec::new(),
            variants: None,
            expansion: None,
        }
    }

    fn test_db_for_legacy_lookup() -> DatabaseV2 {
        let mut commands = HashMap::new();
        commands.insert(
            "Message".to_string(),
            test_command(CommandType::ScriptCmd, 1, Some("MessageLegacy")),
        );
        commands.insert(
            "WalkUp".to_string(),
            test_command(CommandType::Movement, 2, Some("WalkUpLegacy")),
        );

        DatabaseV2 {
            meta: DatabaseMeta {
                version: "test".to_string(),
                generated_at: None,
                generated_from: None,
            },
            commands,
            sounds: HashMap::new(),
            comparison_operators: HashMap::new(),
            overworld_directions: HashMap::new(),
            special_overworlds: HashMap::new(),
        }
    }

    #[test]
    fn test_comparison_operators() {
        assert_eq!(
            ComparisonOperator::from_id(0),
            Some(ComparisonOperator::Less)
        );
        assert_eq!(
            ComparisonOperator::from_id(1),
            Some(ComparisonOperator::Equal)
        );
        assert_eq!(
            ComparisonOperator::from_id(5),
            Some(ComparisonOperator::Different)
        );
        assert_eq!(ComparisonOperator::from_id(6), None);
    }

    #[test]
    fn test_direction() {
        assert_eq!(Direction::from_id(0), Some(Direction::Up));
        assert_eq!(Direction::from_id(3), Some(Direction::Right));
        assert_eq!(Direction::from_id(4), None);
    }

    #[test]
    fn test_param_size() {
        assert_eq!(ParamType::U8.size(), 1);
        assert_eq!(ParamType::U16.size(), 2);
        assert_eq!(ParamType::U32.size(), 4);
        assert_eq!(ParamType::Label.size(), 4);
        assert_eq!(ParamType::Var.size(), 2);
    }

    #[test]
    fn test_normalize_command_name() {
        assert_eq!(normalize_command_name("GoToIf"), "gotoif");
        assert_eq!(normalize_command_name("goto_if"), "gotoif");
        assert_eq!(normalize_command_name("APPLY_MOVEMENT"), "applymovement");
    }

    #[test]
    fn test_get_command_resolves_legacy_name() {
        let db = test_db_for_legacy_lookup();
        let cmd = db
            .get_command("MessageLegacy")
            .expect("legacy lookup failed");
        assert_eq!(cmd.id, Some(1));
        assert_eq!(cmd.cmd_type, CommandType::ScriptCmd);
    }

    #[test]
    fn test_get_script_cmd_resolves_legacy_name() {
        let db = test_db_for_legacy_lookup();
        let cmd = db
            .get_script_cmd("MessageLegacy")
            .expect("legacy script command lookup failed");
        assert_eq!(cmd.id, Some(1));
        assert_eq!(cmd.cmd_type, CommandType::ScriptCmd);
    }

    #[test]
    fn test_get_movement_resolves_legacy_name() {
        let db = test_db_for_legacy_lookup();
        let cmd = db
            .get_movement("WalkUpLegacy")
            .expect("legacy movement lookup failed");
        assert_eq!(cmd.id, Some(2));
        assert_eq!(cmd.cmd_type, CommandType::Movement);
    }

    fn unique_temp_dir(name: &str) -> std::path::PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before UNIX_EPOCH")
            .as_nanos();
        std::env::temp_dir().join(format!("rotom_{}_{}_{}", name, std::process::id(), now))
    }

    #[test]
    fn test_load_directory_reports_invalid_json() {
        let temp_dir = unique_temp_dir("constants_invalid_json");
        fs::create_dir_all(&temp_dir).expect("failed to create temp dir");
        let invalid_json = temp_dir.join("broken.json");
        fs::write(&invalid_json, "{ invalid json").expect("failed to write invalid json");

        let mut constants = ConstantDb::new();
        let result = constants.load_directory(&temp_dir);
        fs::remove_dir_all(&temp_dir).ok();

        let err = result.expect_err("invalid constants JSON should return an error");
        match err {
            CompileError::Database { message } => {
                assert!(
                    message.contains("broken.json"),
                    "error should include failing filename, got: {}",
                    message
                );
            }
            _ => panic!("expected database error"),
        }
    }

    #[test]
    fn test_load_directory_keeps_successes_but_surfaces_failures() {
        let temp_dir = unique_temp_dir("constants_mixed_json");
        fs::create_dir_all(&temp_dir).expect("failed to create temp dir");
        let valid_json = temp_dir.join("ok.json");
        let invalid_json = temp_dir.join("broken.json");
        fs::write(&valid_json, "{\"1\":\"CONST_ONE\"}").expect("failed to write valid json");
        fs::write(&invalid_json, "{ nope").expect("failed to write invalid json");

        let mut constants = ConstantDb::new();
        let result = constants.load_directory(&temp_dir);
        let loaded_value = constants.get("CONST_ONE");
        fs::remove_dir_all(&temp_dir).ok();

        assert_eq!(
            loaded_value,
            Some(1),
            "valid constants should still be loaded before reporting failures"
        );
        assert!(
            result.is_err(),
            "mixed valid/invalid constants should surface an error"
        );
    }
}
