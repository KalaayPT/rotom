//! Database module for loading V2 script command databases
//!
//! Supports the normalized V2 JSON schema from scrcmd-database.

#![allow(dead_code)]

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

use crate::compiler::ParseResult;
use crate::compiler::parse_error::{CompileError, database_error};

// ============================================================================
// Hardcoded Enums (fixed across all games)
// ============================================================================

/// Pokemon Gen 4 game families
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum GameFamily {
    DP = 1,      // Diamond, Pearl
    Platinum = 2, // Platinum
    HGSS = 3,    // HeartGold, SoulSilver
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
    /// ```
    pub fn load_python_enum<P: AsRef<Path>>(&mut self, path: P) -> Result<usize, CompileError> {
        use regex::Regex;

        let path = path.as_ref();
        let contents = std::fs::read_to_string(path).map_err(|e| CompileError::Database {
            message: format!(
                "Failed to read Python enum file '{}': {}",
                path.display(),
                e
            ),
        })?;

        // Match lines like: "    CONSTANT_NAME = 123" or "    CONSTANT_NAME = -1"
        let re = Regex::new(r"^\s+(\w+)\s*=\s*(-?\d+)\s*$").unwrap();

        let mut count = 0;
        for (line_num, line) in contents.lines().enumerate() {
            if let Some(caps) = re.captures(line) {
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
        use regex::Regex;

        let path = path.as_ref();
        let contents = std::fs::read_to_string(path).map_err(|e| CompileError::Database {
            message: format!("Failed to read C header file '{}': {}", path.display(), e),
        })?;

        // Match lines like: "#define NAME 123" or "#define NAME -1" or "#define NAME 0xFF"
        let re_numeric =
            Regex::new(r"^\s*#define\s+(\w+)\s+((?:0[xX][0-9a-fA-F]+)|(?:-?\d+))\s*$").unwrap();

        // Match lines like: "#define NAME OTHER_CONST" (simple constant reference)
        let re_const_ref = Regex::new(r"^\s*#define\s+(\w+)\s+([A-Z_][A-Z0-9_]*)\s*$").unwrap();

        let mut count = 0;
        let mut pending_refs: Vec<(String, String)> = Vec::new();

        for line in contents.lines() {
            // First try numeric match
            if let Some(caps) = re_numeric.captures(line) {
                let name = caps.get(1).unwrap().as_str();
                let value_str = caps.get(2).unwrap().as_str();

                // Parse decimal or hex
                let value = if value_str.starts_with("0x") || value_str.starts_with("0X") {
                    i32::from_str_radix(&value_str[2..], 16).ok()
                } else {
                    value_str.parse::<i32>().ok()
                };

                if let Some(v) = value {
                    self.constants.insert(name.to_string(), v);
                    count += 1;
                }
            }
            // Then try constant reference match
            else if let Some(caps) = re_const_ref.captures(line) {
                let name = caps.get(1).unwrap().as_str().to_string();
                let ref_name = caps.get(2).unwrap().as_str().to_string();
                pending_refs.push((name, ref_name));
            }
        }

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

    /// Load all constants from a decomp project (e.g., pokeplatinum)
    ///
    /// Requires the project to have been built at least once.
    ///
    /// Loads from:
    /// - `{root}/build/generated/*.py` - Python IntEnum files
    /// - `{root}/include/constants/*.h` - C header files (simple defines only)
    pub fn load_decomp_project<P: AsRef<Path>>(&mut self, root: P) -> Result<usize, CompileError> {
        let root = root.as_ref();
        let mut total = 0;

        // Check that build/generated exists
        let build_generated = root.join("build").join("generated");
        if !build_generated.exists() || !build_generated.is_dir() {
            return Err(CompileError::Database {
                message: format!(
                    "Decomp project '{}' has not been built yet. \
                     Directory '{}' does not exist. \
                     Please build the project first (e.g., `make`).",
                    root.display(),
                    build_generated.display()
                ),
            });
        }

        // Load all .py files from build/generated/
        let entries = std::fs::read_dir(&build_generated).map_err(|e| CompileError::Database {
            message: format!(
                "Failed to read directory '{}': {}",
                build_generated.display(),
                e
            ),
        })?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "py").unwrap_or(false) {
                match self.load_python_enum(&path) {
                    Ok(count) => total += count,
                    Err(e) => {
                        return Err(CompileError::Database {
                            message: format!(
                                "Failed to load Python enum '{}': {}",
                                path.display(),
                                e
                            ),
                        });
                    }
                }
            }
        }

        // Load .h files from include/constants/ (simple defines only)
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

            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map(|e| e == "h").unwrap_or(false) {
                    // These may have expressions we can't parse, so we use the
                    // lenient load_c_defines which skips unparseable lines
                    if let Ok(count) = self.load_c_defines(&path) {
                        total += count;
                    }
                }
            }
        }

        Ok(total)
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
