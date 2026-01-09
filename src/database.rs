//! Database module for loading V2 script command databases
//!
//! Supports the normalized V2 JSON schema from scrcmd-database.

#![allow(dead_code)]

use rayon::prelude::*;
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

// ============================================================================
// Cached Regexes (compiled once, reused across all calls)
// ============================================================================

/// Python enum: "    CONSTANT_NAME = 123" or "    CONSTANT_NAME = -1"
static RE_PYTHON_SIMPLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s+(\w+)\s*=\s*(-?\d+)\s*$").unwrap()
});

/// Python enum bitshift: "    CONSTANT_NAME = (1 << 8)"
static RE_PYTHON_BITSHIFT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s+(\w+)\s*=\s*\((\d+)\s*<<\s*(\d+)\)\s*$").unwrap()
});

/// C define numeric: "#define NAME 123" or "#define NAME 0xFF" (with optional trailing comment)
static RE_C_DEFINE_NUMERIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*#define\s+(\w+)\s+((?:0[xX][0-9a-fA-F]+)|(?:-?\d+))(?:\s*//.*)?$").unwrap()
});

/// C define RGB: "#define NAME RGB(31, 31, 31)"
static RE_C_DEFINE_RGB: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*#define\s+(\w+)\s+RGB\s*\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*\)(?:\s*//.*)?$").unwrap()
});

/// C define const ref: "#define NAME OTHER_CONST" (with optional trailing comment)
static RE_C_DEFINE_CONST_REF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*#define\s+(\w+)\s+([A-Z_][A-Z0-9_]*)(?:\s*//.*)?$").unwrap()
});

use crate::compiler::ParseResult;
use crate::compiler::parse_error::{CompileError, database_error};

// ============================================================================
// Hardcoded Enums (fixed across all games)
// ============================================================================

/// Pokemon Gen 4 game families
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameFamily {
    DP,       // Diamond, Pearl
    Platinum, // Platinum
    HGSS,     // HeartGold, SoulSilver
}

impl GameFamily {
    /// Parse game family from string (case-insensitive)
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "DP" | "DIAMOND" | "PEARL" => Some(Self::DP),
            "PLATINUM" | "PT" => Some(Self::Platinum),
            "HGSS" | "HEARTGOLD" | "SOULSILVER" => Some(Self::HGSS),
            _ => None,
        }
    }

    /// Get display name for the game family
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DP => "Diamond/Pearl",
            Self::Platinum => "Platinum",
            Self::HGSS => "HeartGold/SoulSilver",
        }
    }

    /// Infer game family from database version string
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

/// Comparison operators for conditional jumps
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
            "LESS/EQUAL" | "LESSEQUAL" => Some(Self::LessEqual),
            "GREATER/EQUAL" | "GREATEREQUAL" => Some(Self::GreaterEqual),
            "DIFFERENT" => Some(Self::Different),
            _ => None,
        }
    }
}

/// Overworld facing/movement directions
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

/// Root structure for V2 database JSON
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

/// A command entry (script_cmd, movement, levelscript_cmd, or macro)
#[derive(Debug, Deserialize)]
pub struct Command {
    #[serde(rename = "type")]
    pub cmd_type: CommandType,
    /// Opcode ID - only present for non-macro commands
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
    /// For macros: the expansion commands
    #[serde(default)]
    pub expansion: Option<Vec<String>>,
}

/// Command type discriminant
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandType {
    ScriptCmd,
    Movement,
    LevelscriptCmd,
    Macro,
}

/// Parameter definition
#[derive(Debug, Clone, Deserialize)]
pub struct ParamDef {
    pub name: String,
    #[serde(rename = "type")]
    pub param_type: ParamType,
    /// For variant discriminants, the constant value this param must have
    #[serde(rename = "const")]
    pub const_value: Option<String>,
    /// Default value for optional parameters (e.g. "0", "VAR_RESULT")
    #[serde(default)]
    pub default: Option<String>,
}

/// Parameter type - determines byte size and semantic meaning
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
    /// Get the byte size of this parameter type
    pub fn size(&self) -> usize {
        match self {
            ParamType::U8 => 1,
            ParamType::U16 | ParamType::Var | ParamType::Flag => 2,
            ParamType::U32 | ParamType::Label | ParamType::ScriptId | ParamType::MovementId => 4,
            ParamType::MsgId => 2,   // Usually u16
            ParamType::Unknown => 2, // Default to u16
        }
    }
}

/// Variant for commands with mode-dependent parameters
#[derive(Debug, Deserialize)]
pub struct Variant {
    #[serde(default)]
    pub params: Vec<ParamDef>,
    #[serde(default)]
    pub desc: Option<String>,
    // New fields for macro variants
    #[serde(default)]
    pub condition: Option<String>,
    #[serde(default)]
    pub expansion: Option<Vec<String>>,
}

/// Sound entry
#[derive(Debug, Deserialize)]
pub struct Sound {
    pub name: String,
    #[serde(default)]
    pub used_in: Option<String>,
}

// ============================================================================
// Database Loading
// ============================================================================

impl DatabaseV2 {
    /// Load a V2 database from a JSON file
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

    /// Look up a command by name
    pub fn get_command(&self, name: &str) -> ParseResult<&Command> {
        // Try direct lookup first
        if let Some(cmd) = self.commands.get(name) {
            return Ok(cmd);
        }

        // Try legacy_name lookup
        if let Some((_, cmd)) = self
            .commands
            .iter()
            .find(|(_, cmd)| cmd.legacy_name == Some(name.to_string()))
        {
            return Ok(cmd);
        }

        // Try ID-based lookup for "ScrCmd_N" format (N is hex)
        if let Some(id_str) = name.strip_prefix("ScrCmd_") {
            // Parse as hex (e.g., "131" -> 0x131 = 305)
            if let Ok(id) = i32::from_str_radix(id_str, 16) {
                if let Some((_, cmd)) = self
                    .commands
                    .iter()
                    .find(|(_, cmd)| cmd.id == Some(id as u16))
                {
                    return Ok(cmd);
                }
            }
        }

        Err(database_error(format!(
            "Command '{}' not found in database",
            name
        )))
    }

    /// Look up a script command by name (returns database error if no command with that name as
    /// legacy name or normal name is found)
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
                    .find(|(_, cmd)| cmd.legacy_name == Some(name.to_string()))
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

    /// Look up a movement by name (checks both primary name and legacy_name)
    pub fn get_movement(&self, name: &str) -> Result<&Command, CompileError> {
        match self
            .commands
            .get(name)
            .filter(|cmd| cmd.cmd_type == CommandType::Movement)
        {
            Some(cmd) => Ok(cmd),
            None => {
                // Check legacy_name
                if let Some((_, cmd)) = self
                    .commands
                    .iter()
                    .find(|(_, cmd)| cmd.legacy_name == Some(name.to_string()))
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

    /// Get all script commands
    pub fn script_commands(&self) -> impl Iterator<Item = (&String, &Command)> {
        self.commands
            .iter()
            .filter(|(_, cmd)| cmd.cmd_type == CommandType::ScriptCmd)
    }

    /// Get all movements
    pub fn movements(&self) -> impl Iterator<Item = (&String, &Command)> {
        self.commands
            .iter()
            .filter(|(_, cmd)| cmd.cmd_type == CommandType::Movement)
    }
}

impl Command {
    /// Check if this command is a macro (requires expansion)
    pub fn is_macro(&self) -> bool {
        self.cmd_type == CommandType::Macro
    }

    /// Get parameters for a specific variant mode
    /// Returns the base params if no variants or mode not found
    pub fn get_variant_params(&self, mode: u8) -> &[ParamDef] {
        if let Some(variants) = &self.variants {
            for variant in variants {
                // Check if this variant matches the mode
                if let Some(first_param) = variant.params.first() {
                    if let Some(const_val) = &first_param.const_value {
                        if const_val.parse::<u8>().ok() == Some(mode) {
                            return &variant.params;
                        }
                    }
                }
            }
        }
        &self.params
    }

    /// Check if this command has variants (mode-dependent parameters)
    pub fn has_variants(&self) -> bool {
        self.variants.is_some()
    }

    /// Calculate total parameter byte size for this command
    pub fn params_size(&self) -> usize {
        self.params.iter().map(|p| p.param_type.size()).sum()
    }
}

// ============================================================================
// Constants Database
// ============================================================================

/// Database for named constants (species, items, trainers, etc.)
#[derive(Debug, Default)]
pub struct ConstantDb {
    /// All constants: name -> value
    constants: HashMap<String, i32>,
}

impl ConstantDb {
    /// Create a new empty ConstantDb
    pub fn new() -> Self {
        ConstantDb {
            constants: HashMap::new(),
        }
    }

    /// Load built-in constants from DatabaseV2 (comparison_operators, directions, special_overworlds, sounds)
    pub fn load_from_db(&mut self, db: &DatabaseV2) -> usize {
        let mut count = 0;

        // Load fundamental C/SDK constants (TRUE, FALSE)
        self.constants.insert("TRUE".to_string(), 1);
        self.constants.insert("FALSE".to_string(), 0);
        count += 2;

        // Load comparison operators
        for (id_str, name) in &db.comparison_operators {
            if let Ok(id) = id_str.parse::<i32>() {
                // Normalize name: "LESS/EQUAL" -> "LESS_EQUAL"
                let normalized = name.replace("/", "_");
                self.constants.insert(normalized, id);
                count += 1;
            }
        }

        // Load overworld directions
        for (id_str, name) in &db.overworld_directions {
            if let Ok(id) = id_str.parse::<i32>() {
                self.constants.insert(name.clone(), id);
                count += 1;
            }
        }

        // Load special overworlds (Player, Camera, etc.)
        for (id_str, name) in &db.special_overworlds {
            if let Ok(id) = id_str.parse::<i32>() {
                self.constants.insert(name.clone(), id);
                count += 1;
            }
        }

        // Load sounds
        for (id_str, sound) in &db.sounds {
            if let Ok(id) = id_str.parse::<i32>() {
                self.constants.insert(sound.name.clone(), id);
                count += 1;
            }
        }

        count
    }

    /// Load constants from a JSON file with format { "id": "NAME", ... }
    /// The JSON has numeric string keys and name values, we invert to name -> id
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

    /// Load all JSON files from a directory
    pub fn load_directory<P: AsRef<Path>>(&mut self, dir: P) -> Result<usize, CompileError> {
        let dir = dir.as_ref();
        let mut total = 0;

        if !dir.exists() || !dir.is_dir() {
            return Ok(0); // Directory doesn't exist, that's fine
        }

        let entries = std::fs::read_dir(dir).map_err(|e| CompileError::Database {
            message: format!("Failed to read directory '{}': {}", dir.display(), e),
        })?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                // Skip the main command database if it's in the same directory
                let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if filename.contains("_v2") || filename == "commands.json" {
                    continue;
                }
                match self.load_json(&path) {
                    Ok(count) => total += count,
                    Err(e) => eprintln!("Warning: Failed to load '{}': {}", path.display(), e),
                }
            }
        }

        Ok(total)
    }

    // ========================================================================
    // Decomp Project Support (pokeplatinum only)
    // ========================================================================

    /// Load constants from a Python IntEnum file (pokeplatinum build/generated format)
    ///
    /// Parses lines like:
    /// ```python
    /// class VarFlag(enum.IntEnum):
    ///     VAR_JUBILIFE_STATE = 16500
    ///     GENDER_MALE = 0
    ///     PLAYER_TRANSITION_HEALING = (1 << 8)
    /// ```
    pub fn load_python_enum<P: AsRef<Path>>(&mut self, path: P) -> Result<usize, CompileError> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path).map_err(|e| CompileError::Database {
            message: format!(
                "Failed to read Python enum file '{}': {}",
                path.display(),
                e
            ),
        })?;

        let mut count = 0;
        for (line_num, line) in contents.lines().enumerate() {
            // Try simple integer match first
            if let Some(caps) = RE_PYTHON_SIMPLE.captures(line) {
                let name = caps.get(1).unwrap().as_str();
                let value_str = caps.get(2).unwrap().as_str();

                let value: i32 = value_str.parse().map_err(|_| CompileError::Database {
                    message: format!(
                        "Invalid integer value '{}' at {}:{}",
                        value_str,
                        path.display(),
                        line_num + 1
                    ),
                })?;

                self.constants.insert(name.to_string(), value);
                count += 1;
            }
            // Try bitshift match
            else if let Some(caps) = RE_PYTHON_BITSHIFT.captures(line) {
                let name = caps.get(1).unwrap().as_str();
                let base: i32 = caps.get(2).unwrap().as_str().parse().unwrap_or(0);
                let shift: u32 = caps.get(3).unwrap().as_str().parse().unwrap_or(0);
                
                let value = base << shift;
                self.constants.insert(name.to_string(), value);
                count += 1;
            }
        }

        Ok(count)
    }

    /// Load constants from a C header file with simple #define statements
    ///
    /// Parses lines like:
    /// ```c
    /// #define JUBILIFE_CITY_COUNTERPART 7
    /// #define DIR_NONE -1
    /// #define LOCALID_PLAYER 0xFF
    /// #define NO_EXIT_ON_B FALSE
    /// ```
    ///
    /// Skips lines with expressions like `(1 << 5)` or complex expressions.
    pub fn load_c_defines<P: AsRef<Path>>(&mut self, path: P) -> Result<usize, CompileError> {
        let (constants, pending_refs) = Self::parse_c_defines_file(&path)?;
        
        let mut count = constants.len();
        self.constants.extend(constants);
        
        // Resolve constant references
        for (name, ref_name) in pending_refs {
            if let Some(&value) = self.constants.get(&ref_name) {
                self.constants.insert(name, value);
                count += 1;
            }
            // If reference not found, silently skip (it might be a complex macro)
        }

        Ok(count)
    }
    
    /// Parse a C header file and return constants and pending references
    /// 
    /// Returns (constants, pending_references) for parallel processing.
    /// Pending references need to be resolved after all files are parsed.
    fn parse_c_defines_file<P: AsRef<Path>>(path: P) -> Result<(HashMap<String, i32>, Vec<(String, String)>), CompileError> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path).map_err(|e| CompileError::Database {
            message: format!("Failed to read C header file '{}': {}", path.display(), e),
        })?;

        let mut constants = HashMap::new();
        let mut pending_refs: Vec<(String, String)> = Vec::new();

        for line in contents.lines() {
            // First try numeric match
            if let Some(caps) = RE_C_DEFINE_NUMERIC.captures(line) {
                let name = caps.get(1).unwrap().as_str();
                let value_str = caps.get(2).unwrap().as_str();

                // Parse decimal or hex
                let value = if value_str.starts_with("0x") || value_str.starts_with("0X") {
                    i32::from_str_radix(&value_str[2..], 16).ok()
                } else {
                    value_str.parse::<i32>().ok()
                };

                if let Some(v) = value {
                    constants.insert(name.to_string(), v);
                }
            }
            // Try RGB match
            else if let Some(caps) = RE_C_DEFINE_RGB.captures(line) {
                let name = caps.get(1).unwrap().as_str();
                let r: i32 = caps.get(2).unwrap().as_str().parse().unwrap_or(0);
                let g: i32 = caps.get(3).unwrap().as_str().parse().unwrap_or(0);
                let b: i32 = caps.get(4).unwrap().as_str().parse().unwrap_or(0);

                // 15-bit color: (b << 10) | (g << 5) | r
                let value = (b << 10) | (g << 5) | r;
                constants.insert(name.to_string(), value);
            }
            // Then try constant reference match
            else if let Some(caps) = RE_C_DEFINE_CONST_REF.captures(line) {
                let name = caps.get(1).unwrap().as_str().to_string();
                let ref_name = caps.get(2).unwrap().as_str().to_string();
                pending_refs.push((name, ref_name));
            }
        }

        Ok((constants, pending_refs))
    }

    /// List of generated constant files from pokeplatinum
    /// Format: (base_filename, metang_type, tag_name, extra_args)
    /// The base_filename is used to find both .py files in build/generated/
    /// and .txt files in generated/
    const GENERATED_CONSTANT_FILES: &'static [(&'static str, &'static str, &'static str, &'static [&'static str])] = &[
        ("abilities", "enum", "Ability", &[]),
        ("accessories", "enum", "Accessory", &[]),
        ("ai_action_choices", "enum", "AIActionChoice", &[]),
        ("ai_flags", "mask", "AIFlag", &["--no-auto"]),
        ("ai_load_type_targets", "enum", "AILoadTypeTarget", &[]),
        ("ai_weather_types", "enum", "AIWeatherType", &[]),
        ("backdrops", "enum", "Backdrop", &[]),
        ("badges", "enum", "Badge", &[]),
        ("battle_actions", "enum", "BattleAction", &[]),
        ("battle_backgrounds", "enum", "BattleBackground", &[]),
        ("battle_boot_states", "enum", "BattleBootState", &[]),
        ("battle_context_params", "enum", "BattleContextParam", &[]),
        ("battle_message_tags", "enum", "BattleMessageTag", &[]),
        ("battle_mon_params", "enum", "BattleMonParam", &[]),
        ("battle_move_effects", "enum", "BattleMoveEffect", &[]),
        ("battle_move_subscript_ptrs", "enum", "BattleMoveSubscriptPtr", &[]),
        ("battle_script_battlers", "enum", "Battler", &[]),
        ("battle_script_check_side_condition_ops", "enum", "BattleScriptCheckSideConditionOp", &[]),
        ("battle_script_opcodes", "enum", "BattleScriptOpCode", &[]),
        ("battle_script_side_conditions", "enum", "BattleScriptSideCondition", &[]),
        ("battle_script_turn_flags", "enum", "BattleScriptTurnFlag", &[]),
        ("battle_script_vars", "enum", "BattleScriptVars", &[]),
        ("battle_side_effect_types", "enum", "BattleSideEffectType", &[]),
        ("battle_stats", "enum", "BattleStat", &[]),
        ("battle_sub_animations", "enum", "BattleSubAnimation", &[]),
        ("battle_subscripts", "enum", "BattleSubscript", &[]),
        ("battle_terrains", "enum", "BattleTerrain", &[]),
        ("battle_tower_functions", "enum", "BattleTowerFunction", &[]),
        ("battle_tower_modes", "enum", "BattleTowerMode", &[]),
        ("berry_growth_stages", "enum", "BerryGrowthStage", &[]),
        ("bg_event_dirs", "enum", "BgEventDir", &[]),
        ("bg_event_types", "enum", "BgEventType", &[]),
        ("catching_show_points_category", "enum", "CatchingShowPointsCategory", &[]),
        ("contest_effects", "enum", "ContestEffects", &[]),
        ("comm_club_ret_codes", "enum", "CommClubRetCode", &[]),
        ("days_of_week", "enum", "DayOfWeek", &[]),
        ("distribution_events", "enum", "DistributionEvent", &[]),
        ("egg_groups", "enum", "EggGroup", &[]),
        ("evolution_methods", "enum", "EvolutionMethod", &[]),
        ("exp_rates", "enum", "ExpRate", &[]),
        ("game_records", "enum", "GameRecord", &[]),
        ("fade_types", "enum", "FadeType", &[]),
        ("first_arrival_to_zones", "enum", "FirstArrivalToZone", &[]),
        ("footprint_sizes", "enum", "FootprintSize", &[]),
        ("frontier_trainers", "enum", "FrontierTrainerID", &[]),
        ("gender_ratios", "enum", "GenderRatio", &[]),
        ("genders", "enum", "Gender", &[]),
        ("giratina_shadow_animations", "enum", "GiratinaShadowAnimation", &[]),
        ("hidden_locations", "enum", "HiddenLocation", &[]),
        ("item_ai_categories", "enum", "ItemAICategory", &[]),
        ("item_battle_categories", "enum", "ItemBattleCategory", &[]),
        ("item_hold_effects", "enum", "ItemHoldEffect", &[]),
        ("items", "enum", "Item", &[]),
        ("journal_location_events", "enum", "JournalLocationEventType", &[]),
        ("journal_online_events", "enum", "JournalOnlineEventType", &[]),
        ("map_headers", "enum", "MapHeader", &[]),
        ("maps", "enum", "MapID", &[]),
        ("move_attributes", "enum", "MoveAttribute", &[]),
        ("move_classes", "enum", "MoveClass", &[]),
        ("move_flags", "mask", "MoveFlag", &[]),
        ("move_ranges", "mask", "MoveRange", &["--no-auto"]),
        ("movement_actions", "enum", "MovementAction", &[]),
        ("movement_types", "enum", "MovementType", &[]),
        ("moves", "enum", "Move", &[]),
        ("natures", "enum", "Nature", &[]),
        ("npc_trades", "enum", "NpcTradeID", &[]),
        ("object_events", "enum", "ObjectEventGfx", &[]),
        ("pal_park_land_area", "enum", "PalParkLandArea", &[]),
        ("pal_park_water_area", "enum", "PalParkWaterArea", &[]),
        ("player_transitions", "mask", "PlayerTransition", &[]),
        ("pokemon_anim_constants", "enum", "PokemonAnimConstants", &[]),
        ("pokemon_body_shapes", "enum", "PokemonBodyShape", &[]),
        ("pokemon_colors", "enum", "PokemonColor", &[]),
        ("pokemon_contest_ranks", "enum", "PokemonContestRank", &[]),
        ("pokemon_contest_types", "enum", "PokemonContestType", &[]),
        ("pokemon_data_params", "enum", "PokemonDataParam", &[]),
        ("pokemon_stats", "enum", "PokemonStat", &[]),
        ("pokemon_types", "enum", "PokemonType", &[]),
        ("poketch_apps", "enum", "PoketchAppID", &[]),
        ("ribbons", "enum", "RibbonID", &[]),
        ("roaming_slots", "enum", "RoamingSlot", &[]),
        ("save_types", "enum", "SaveType", &[]),
        ("sdat", "enum", "SDATID", &[]),
        ("seals", "enum", "Seal", &[]),
        ("signpost_commands", "enum", "SignpostCommand", &[]),
        ("signpost_types", "enum", "SignpostType", &[]),
        ("size_contest_results", "enum", "SizeContestResult", &[]),
        ("shadow_sizes", "enum", "ShadowSize", &[]),
        ("species", "enum", "Species", &[]),
        ("species_data_params", "enum", "SpeciesDataParam", &[]),
        ("string_padding_mode", "enum", "PaddingMode", &[]),
        ("text_banks", "enum", "TextBank", &[]),
        ("time_of_day", "enum", "TimeOfDay", &[]),
        ("town_map_description_flag_types", "enum", "TownMapDescriptionFlagType", &[]),
        ("trainers", "enum", "TrainerID", &[]),
        ("trainer_classes", "enum", "TrainerClass", &[]),
        ("trainer_message_types", "enum", "TrainerMessageType", &[]),
        ("trainer_score_events", "enum", "TrainerScoreEvent", &[]),
        ("trainer_types", "enum", "TrainerType", &[]),
        ("tutor_locations", "enum", "TutorLocation", &[]),
        ("vars_flags", "enum", "VarFlag", &[]),
        ("versions", "enum", "Version", &[]),
        ("villa_furnitures", "enum", "VillaFurniture", &[]),
        ("mart_specialties_id", "enum", "MartSpecialtiesID", &[]),
        ("mart_decor_id", "enum", "MartDecorID", &[]),
        ("mart_seal_id", "enum", "MartSealID", &[]),
        ("mart_frontier_id", "enum", "MartFrontierId", &[]),
        ("mystery_gift_delivery_stages", "enum", "MysteryGiftDeliveryStage", &[]),
    ];

    /// Load all constants from a decomp project (e.g., pokeplatinum)
    ///
    /// Prioritizes pre-built files from `build/generated/*.py` (fast path).
    /// Falls back to invoking metang on `generated/*.txt` if the project
    /// hasn't been built yet (slow path, with warning).
    ///
    /// Loads from:
    /// - `{root}/build/generated/*.py` - Python IntEnum files (preferred, fast)
    /// - `{root}/generated/*.txt` via metang - if build/generated/ doesn't exist
    /// - `{root}/include/constants/*.h` - C header files (simple defines only)
    /// - `{root}/res/text/*.json` - Text bank message IDs
    pub fn load_decomp_project<P: AsRef<Path>>(&mut self, root: P) -> Result<usize, CompileError> {
        let root = root.as_ref();
        let mut total = 0;

        // Load .h files from include/constants/ (simple defines only) - PARALLEL
        let include_constants = root.join("include").join("constants");
        if include_constants.exists() && include_constants.is_dir() {
            let entries =
                std::fs::read_dir(&include_constants).map_err(|e| CompileError::Database {
                    message: format!(
                        "Failed to read directory '{}': {}",
                        include_constants.display(),
                        e
                    ),
                })?;

            // Collect header file paths
            let header_paths: Vec<_> = entries
                .flatten()
                .filter_map(|entry| {
                    let path = entry.path();
                    if path.extension().map(|e| e == "h").unwrap_or(false) {
                        Some(path)
                    } else {
                        None
                    }
                })
                .collect();
            
            // Parse files in parallel
            let results: Vec<_> = header_paths
                .par_iter()
                .filter_map(|path| Self::parse_c_defines_file(path).ok())
                .collect();
            
            // Merge results and collect pending refs
            let mut all_pending_refs = Vec::new();
            for (constants, pending_refs) in results {
                total += constants.len();
                self.constants.extend(constants);
                all_pending_refs.extend(pending_refs);
            }
            
            // Resolve constant references (must be done after all constants are loaded)
            for (name, ref_name) in all_pending_refs {
                if let Some(&value) = self.constants.get(&ref_name) {
                    self.constants.insert(name, value);
                    total += 1;
                }
            }
        }

        // Load text bank constants from res/text/*.json
        let text_json = root.join("res").join("text");
        if text_json.exists() && text_json.is_dir() {
            if let Ok(count) = self.load_text_bank_json_dir(&text_json) {
                total += count;
            }
        }

        // Try the fast path: load pre-built .py files from build/generated/
        let build_generated = root.join("build").join("generated");
        if build_generated.exists() && build_generated.is_dir() {
            let count = self.load_build_generated_py(&build_generated)?;
            total += count;
        } else {
            // Slow path: use metang to process generated/*.txt
            eprintln!(
                "Warning: '{}' not found. \
                 Please build pokeplatinum before running rotom for faster constant loading. \
                 Falling back to metang (slower)...",
                build_generated.display()
            );
            
            let generated_txt = root.join("generated");
            if generated_txt.exists() && generated_txt.is_dir() {
                if let Ok(count) = self.load_generated_via_metang(&generated_txt) {
                    total += count;
                } else {
                    // Fallback to simple loader if metang fails or is not available
                    let entries =
                        std::fs::read_dir(&generated_txt).map_err(|e| CompileError::Database {
                            message: format!(
                                "Failed to read directory '{}': {}",
                                generated_txt.display(),
                                e
                            ),
                        })?;

                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().map(|e| e == "txt").unwrap_or(false) {
                            if let Ok(count) = self.load_enum_txt(&path) {
                                total += count;
                            }
                        }
                    }
                }
            }
        }

        Ok(total)
    }

    /// Load pre-built Python enum files from build/generated/
    /// 
    /// This is the fast path - these files are already processed by metang
    /// during the decomp build, so we just parse the simple Python IntEnum format.
    /// Uses rayon for parallel file loading.
    fn load_build_generated_py<P: AsRef<Path>>(&mut self, dir: P) -> Result<usize, CompileError> {
        let dir = dir.as_ref();
        
        // Collect file paths to process
        let file_paths: Vec<_> = Self::GENERATED_CONSTANT_FILES
            .iter()
            .filter_map(|(filename, _, _, _)| {
                let py_path = dir.join(format!("{}.py", filename));
                if py_path.exists() {
                    Some(py_path)
                } else {
                    None
                }
            })
            .collect();
        
        // Parse files in parallel, collecting results
        let results: Vec<_> = file_paths
            .par_iter()
            .filter_map(|py_path| {
                match Self::parse_python_enum_file(py_path) {
                    Ok(constants) => Some(constants),
                    Err(e) => {
                        eprintln!("Warning: Failed to load '{}': {}", py_path.display(), e);
                        None
                    }
                }
            })
            .collect();
        
        // Merge results into self.constants
        let mut total = 0;
        for constants in results {
            total += constants.len();
            self.constants.extend(constants);
        }

        Ok(total)
    }
    
    /// Parse a Python IntEnum file and return the constants as a HashMap
    /// 
    /// This is a standalone function for parallel processing.
    fn parse_python_enum_file<P: AsRef<Path>>(path: P) -> Result<HashMap<String, i32>, CompileError> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path).map_err(|e| CompileError::Database {
            message: format!(
                "Failed to read Python enum file '{}': {}",
                path.display(),
                e
            ),
        })?;

        let mut constants = HashMap::new();
        
        for (line_num, line) in contents.lines().enumerate() {
            // Try simple integer match first
            if let Some(caps) = RE_PYTHON_SIMPLE.captures(line) {
                let name = caps.get(1).unwrap().as_str();
                let value_str = caps.get(2).unwrap().as_str();

                let value: i32 = value_str.parse().map_err(|_| CompileError::Database {
                    message: format!(
                        "Invalid integer value '{}' at {}:{}",
                        value_str,
                        path.display(),
                        line_num + 1
                    ),
                })?;

                constants.insert(name.to_string(), value);
            }
            // Try bitshift match
            else if let Some(caps) = RE_PYTHON_BITSHIFT.captures(line) {
                let name = caps.get(1).unwrap().as_str();
                let base: i32 = caps.get(2).unwrap().as_str().parse().unwrap_or(0);
                let shift: u32 = caps.get(3).unwrap().as_str().parse().unwrap_or(0);
                
                let value = base << shift;
                constants.insert(name.to_string(), value);
            }
        }

        Ok(constants)
    }

    /// Load constants from generated/*.txt using the local metang tool
    fn load_generated_via_metang<P: AsRef<Path>>(&mut self, dir: P) -> Result<usize, CompileError> {
        use std::process::Command;

        let dir = dir.as_ref();
        let metang_script = Path::new("C:/dev/metang/metang.py");
        if !metang_script.exists() {
            return Err(CompileError::Database {
                message: "metang.py not found at C:/dev/metang/metang.py".to_string(),
            });
        }

        let mut total_loaded = 0;
        for (filename, cmd_type, tag, extra_args) in Self::GENERATED_CONSTANT_FILES {
            let file_path = dir.join(format!("{}.txt", filename));
            if !file_path.exists() {
                continue;
            }

            let mut cmd = Command::new("python");
            cmd.arg(metang_script)
                .arg(cmd_type)
                .arg("--lang")
                .arg("json")
                .arg("--tag-name")
                .arg(tag)
                .env("PYTHONPATH", "C:/dev/metang");

            for arg in *extra_args {
                cmd.arg(arg);
            }

            cmd.arg(&file_path);

            let output = cmd.output().map_err(|e| {
                eprintln!("Failed to execute metang for {}: {}", filename, e);
                CompileError::Database {
                    message: format!("Failed to run metang for {}: {}", filename, e),
                }
            })?;

            if !output.status.success() {
                eprintln!(
                    "metang failed for {} with status {}. Stderr: {}",
                    filename,
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                );
                continue; // Skip failed ones
            }

            let json: std::collections::HashMap<String, serde_json::Value> =
                serde_json::from_slice(&output.stdout).map_err(|e| {
                    eprintln!(
                        "Failed to parse metang JSON for {}: {}. Output: {}",
                        filename,
                        e,
                        String::from_utf8_lossy(&output.stdout)
                    );
                    CompileError::Database {
                        message: format!("Failed to parse metang JSON for {}: {}", filename, e),
                    }
                })?;

            for (name, value) in json {
                if let Some(val) = value.as_i64() {
                    self.constants.insert(name, val as i32);
                    total_loaded += 1;
                } else if let Some(val) = value.as_u64() {
                    self.constants.insert(name, val as i32);
                    total_loaded += 1;
                }
            }
        }

        if total_loaded == 0 {
            return Err(CompileError::Database {
                message: "No constants were loaded via metang".to_string(),
            });
        }

        Ok(total_loaded)
    }

    /// Load per-map object event constants for a specific script
    ///
    /// Extracts the map name from the script filename and loads the corresponding
    /// event header from `build/res/field/events/`.
    ///
    /// For example:
    /// - `scripts_jubilife_city.s` → loads `events_jubilife_city.h`
    /// - `scripts_route_201.s` → loads `events_route_201.h`
    ///
    /// Returns 0 if no matching header is found (not an error).
    pub fn load_map_events<P: AsRef<Path>>(
        &mut self,
        decomp_root: P,
        script_path: P,
    ) -> Result<usize, CompileError> {
        let decomp_root = decomp_root.as_ref();
        let script_path = script_path.as_ref();

        // Extract the script filename
        let script_name = match script_path.file_stem().and_then(|s| s.to_str()) {
            Some(name) => name,
            None => return Ok(0),
        };

        // Convert "scripts_jubilife_city" to "events_jubilife_city"
        let map_name = if let Some(stripped) = script_name.strip_prefix("scripts_") {
            stripped
        } else {
            // Not a standard script filename, skip
            return Ok(0);
        };

        // Construct path to the events header
        let events_header = decomp_root
            .join("build")
            .join("res")
            .join("field")
            .join("events")
            .join(format!("events_{}.h", map_name));

        if !events_header.exists() {
            // No events header for this map, that's fine
            return Ok(0);
        }

        self.load_c_defines(&events_header)
    }

    /// Load enum constants from a generated/*.txt file
    ///
    /// Each line in the file is a constant name, and its value is
    /// the 0-based line index (matching how metang generates Python enums).
    fn load_enum_txt<P: AsRef<Path>>(&mut self, path: P) -> Result<usize, CompileError> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path).map_err(|e| CompileError::Database {
            message: format!("Failed to read enum file '{}': {}", path.display(), e),
        })?;

        let mut count = 0;
        for (index, line) in contents.lines().enumerate() {
            let name = line.trim();
            // Skip empty lines and comments
            if name.is_empty() || name.starts_with('#') || name.starts_with("//") {
                continue;
            }
            // Only accept valid identifier names
            if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                self.constants.insert(name.to_string(), index as i32);
                count += 1;
            }
        }

        Ok(count)
    }

    /// Load text bank constants from JSON files in res/text/
    ///
    /// Each JSON file has a `"messages"` array where each entry contains:
    /// - `"id"`: The constant name (e.g., "CommonStrings_Text_PokecenterGreeting_Day")
    /// - The constant value is the index in the array (0, 1, 2, ...)
    ///
    /// This allows loading text message references without requiring
    /// the decomp project to be built first.
    pub fn load_text_bank_json_dir<P: AsRef<Path>>(
        &mut self,
        dir: P,
    ) -> Result<usize, CompileError> {
        let dir = dir.as_ref();
        let mut total = 0;

        let entries = std::fs::read_dir(dir).map_err(|e| CompileError::Database {
            message: format!(
                "Failed to read text bank directory '{}': {}",
                dir.display(),
                e
            ),
        })?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                match self.load_text_bank_json(&path) {
                    Ok(count) => total += count,
                    Err(_) => {
                        // Silently skip files that don't match the expected format
                    }
                }
            }
        }

        Ok(total)
    }

    /// Load text bank constants from a single JSON file
    fn load_text_bank_json<P: AsRef<Path>>(&mut self, path: P) -> Result<usize, CompileError> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path).map_err(|e| CompileError::Database {
            message: format!("Failed to read text bank file '{}': {}", path.display(), e),
        })?;

        let json: serde_json::Value =
            serde_json::from_str(&contents).map_err(|e| CompileError::Database {
                message: format!("Failed to parse JSON '{}': {}", path.display(), e),
            })?;

        let messages = json
            .get("messages")
            .and_then(|v| v.as_array())
            .ok_or_else(|| CompileError::Database {
                message: format!("No 'messages' array in '{}'", path.display()),
            })?;

        let mut count = 0;
        for (index, msg) in messages.iter().enumerate() {
            if let Some(id) = msg.get("id").and_then(|v| v.as_str()) {
                self.constants.insert(id.to_string(), index as i32);
                count += 1;
            }
        }

        Ok(count)
    }

    /// Look up a constant by name
    pub fn get(&self, name: &str) -> Option<i32> {
        self.constants.get(name).copied()
    }

    /// Get the number of constants loaded
    pub fn len(&self) -> usize {
        self.constants.len()
    }

    /// Check if database is empty
    pub fn is_empty(&self) -> bool {
        self.constants.is_empty()
    }

    /// Iterate over all constants
    pub fn iter(&self) -> impl Iterator<Item = (&String, &i32)> {
        self.constants.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
