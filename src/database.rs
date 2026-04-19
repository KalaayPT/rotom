//! Database module for loading V2 script command databases
//!
//! Supports the normalized V2 JSON schema from scrcmd-database.

#![allow(dead_code)]

use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::compiler::ParseResult;
use crate::compiler::parse_error::{CompileError, database_error};
pub use uxie::GameFamily;
use uxie::SymbolTable;
use uxie::c_parser::defines::eval_expr_with_parent;

pub fn normalize_command_name(name: &str) -> String {
    name.replace('_', "").to_ascii_lowercase()
}

pub fn game_family_from_hint(hint: impl AsRef<str>) -> Option<GameFamily> {
    let hint = hint.as_ref().to_ascii_uppercase();
    if hint.contains("PLATINUM") {
        Some(GameFamily::Platinum)
    } else if hint.contains("HEARTGOLD") || hint.contains("SOULSILVER") || hint.contains("HGSS") {
        Some(GameFamily::HGSS)
    } else if hint.contains("DIAMOND") || hint.contains("PEARL") || hint.contains("DP") {
        Some(GameFamily::DP)
    } else {
        None
    }
}

pub trait GameFamilyExt {
    fn display_name(self) -> &'static str;
    fn config_name(self) -> &'static str;
}

impl GameFamilyExt for GameFamily {
    fn display_name(self) -> &'static str {
        match self {
            GameFamily::DP => "Diamond/Pearl",
            GameFamily::Platinum => "Platinum",
            GameFamily::HGSS => "HeartGold/SoulSilver",
        }
    }

    fn config_name(self) -> &'static str {
        match self {
            GameFamily::DP => "dp",
            GameFamily::Platinum => "platinum",
            GameFamily::HGSS => "hgss",
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

/// The param list the compiler picked for this call.
///
/// The compiler picks a shape, fills in that shape's defaults, then optionally rewrites the
/// result with `emit_args`.
#[derive(Debug, Clone, Copy)]
pub struct ResolvedCommandShape<'a> {
    pub params: &'a [ParamDef],
    pub emit_args: Option<&'a [String]>,
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
            ParamType::U16
            | ParamType::Var
            | ParamType::Flag
            | ParamType::MsgId
            | ParamType::Unknown => 2,
            ParamType::U32 | ParamType::Label | ParamType::ScriptId | ParamType::MovementId => 4,
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
    /// Optional arg rewrite for this shape.
    ///
    /// Each entry is parsed as a Rotom expression after `$param` substitution. This runs after the
    /// shape is chosen and its defaults have been applied.
    #[serde(default)]
    pub emit_args: Option<Vec<String>>,
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

    pub fn game_family(&self) -> Option<GameFamily> {
        game_family_from_hint(&self.meta.version)
    }

    /// Resolves a command by name, first checking the command map directly, then legacy names, and finally script command aliases, such as placeholder names or dummy commands.
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

        if let Some((_, cmd)) = self.get_script_cmd_by_alias(name) {
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
                } else if let Some((_, cmd)) = self.get_script_cmd_by_alias(name) {
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

    pub fn get_script_cmd_by_alias(&self, name: &str) -> Option<(&String, &Command)> {
        if self
            .commands
            .get(name)
            .is_some_and(|cmd| cmd.cmd_type == CommandType::ScriptCmd)
        {
            return None;
        }

        if let Some(suffix) = name.strip_prefix("ScrCmd_")
            && !suffix.is_empty()
        {
            let id = match self.game_family() {
                Some(GameFamily::HGSS) if suffix.chars().all(|c| c.is_ascii_digit()) => {
                    suffix.parse::<u16>().ok()
                }
                Some(GameFamily::HGSS) => None,
                _ => {
                    let hex_suffix = suffix.rsplit('_').next().unwrap_or(suffix);
                    if hex_suffix.is_empty() {
                        None
                    } else {
                        u16::from_str_radix(hex_suffix, 16).ok()
                    }
                }
            };

            if let Some(id) = id {
                return self.get_script_cmd_by_id(id);
            }
        }

        if let Some(suffix) = name.strip_prefix("Dummy") {
            let id = match self.game_family() {
                Some(GameFamily::HGSS) if suffix.chars().all(|c| c.is_ascii_digit()) => {
                    suffix.parse::<u16>().ok()
                }
                Some(GameFamily::HGSS) => None,
                _ if !suffix.is_empty() => u16::from_str_radix(suffix, 16).ok(),
                _ => None,
            };

            if let Some(id) = id {
                return self.get_script_cmd_by_id(id);
            }
        }

        None
    }

    pub fn get_movement_by_id(&self, id: u16) -> Option<(&String, &Command)> {
        self.commands
            .iter()
            .find(|(_, cmd)| cmd.cmd_type == CommandType::Movement && cmd.id == Some(id))
    }
}

impl Command {
    /// Pick which param list this call should use.
    ///
    /// Order:
    /// 1. first-arg `const` variants
    /// 2. conditional variants in DB order, with `else` as the fallback
    /// 3. the base `params`
    ///
    /// The returned params are the ones used to check the call. Defaults run on that shape later,
    /// and `emit_args` may rewrite the result afterward.
    pub fn resolve_source_call_shape(
        &self,
        first_arg_u8: Option<u8>,
        mut eval_condition: impl FnMut(&str, &[ParamDef]) -> bool,
    ) -> ResolvedCommandShape<'_> {
        if let Some(variants) = &self.variants {
            if let Some(mode) = first_arg_u8 {
                for variant in variants {
                    if variant.matches_first_param_const(mode) {
                        return ResolvedCommandShape {
                            params: variant.source_params_or(&self.params),
                            emit_args: variant.emit_args.as_deref(),
                        };
                    }
                }
            }

            for variant in variants {
                let Some(condition) = variant.condition.as_deref() else {
                    continue;
                };

                if condition == "else"
                    || eval_condition(condition, variant.source_params_or(&self.params))
                {
                    return ResolvedCommandShape {
                        params: variant.source_params_or(&self.params),
                        emit_args: variant.emit_args.as_deref(),
                    };
                }
            }
        }

        ResolvedCommandShape {
            params: &self.params,
            emit_args: None,
        }
    }

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

impl Variant {
    fn source_params_or<'a>(&'a self, default: &'a [ParamDef]) -> &'a [ParamDef] {
        if self.params.is_empty() {
            default
        } else {
            &self.params
        }
    }

    fn matches_first_param_const(&self, mode: u8) -> bool {
        self.params
            .first()
            .and_then(|param| param.const_value.as_deref())
            .and_then(|value| value.parse::<u8>().ok())
            == Some(mode)
    }
}

// ============================================================================
// Constants Database
// ============================================================================

/// Central repository for all named constants (built-in, DSPRE, and Decomp)
#[derive(Default, Clone)]
pub struct ConstantDb {
    /// Manual and built-in constants: name -> value
    constants: HashMap<String, i32>,
    /// Decomp project root for include resolution
    uxie_project_root: Option<PathBuf>,
    /// Base Uxie symbol table loaded for the whole decomp project
    uxie_base_symbols: Option<SymbolTable>,
    /// Active Uxie symbol table, optionally extended with file-local constants
    uxie_symbols: Option<SymbolTable>,
}

impl std::fmt::Debug for ConstantDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConstantDb")
            .field("constants_len", &self.constants.len())
            .field("uxie_project_root", &self.uxie_project_root)
            .field(
                "uxie_base_symbol_count",
                &self
                    .uxie_base_symbols
                    .as_ref()
                    .map(|s| s.get_all_defines().len()),
            )
            .field(
                "uxie_symbol_count",
                &self
                    .uxie_symbols
                    .as_ref()
                    .map(|s| s.get_all_defines().len()),
            )
            .finish()
    }
}

impl ConstantDb {
    pub fn new() -> Self {
        ConstantDb {
            constants: HashMap::new(),
            uxie_project_root: None,
            uxie_base_symbols: None,
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
        let root = root.as_ref();
        let ws = uxie::Workspace::open_decomp(root).map_err(|e| CompileError::Database {
            message: format!("Failed to open decomp project via Uxie: {}", e),
        })?;

        Ok(self.load_decomp_symbols(root, (*ws.symbols).clone()))
    }

    pub fn load_decomp_symbols<P: AsRef<Path>>(&mut self, root: P, symbols: SymbolTable) -> usize {
        let count = symbols.get_all_defines().len();
        self.uxie_project_root = Some(root.as_ref().to_path_buf());
        self.uxie_base_symbols = Some(symbols.clone());
        self.uxie_symbols = Some(symbols);
        count
    }

    /// Load file-local constants for a specific source file using Uxie's include handling.
    ///
    /// When a workspace is loaded, this follows the file's `#include`s and lets Uxie handle
    /// special cases like per-map event headers. The resulting symbol table replaces the current
    /// Uxie symbols for this `ConstantDb` instance.
    pub fn load_script_constants<Q: AsRef<Path>>(
        &mut self,
        script_path: Q,
    ) -> Result<usize, CompileError> {
        let script_path = script_path.as_ref();
        if !script_path.exists() || !script_path.is_file() {
            return Ok(0);
        }

        let Some(project_root) = &self.uxie_project_root else {
            return Ok(0);
        };
        let Some(base_symbols) = &self.uxie_base_symbols else {
            return Ok(0);
        };

        let include_dirs = Self::decomp_include_dirs(project_root);
        let mut collected = SymbolTable::with_parent(Arc::new(base_symbols.clone()));
        let mut unresolved_include_handler = |table: &mut SymbolTable,
                                              parent_dir: &Path,
                                              include_dirs: &[PathBuf],
                                              include_path: &str|
         -> std::io::Result<bool> {
            if Self::try_load_decomp_events_include_json(
                table,
                parent_dir,
                include_dirs,
                include_path,
            )? {
                return Ok(true);
            }

            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "Unresolved include '{}' (searched from {})",
                    include_path,
                    parent_dir.display()
                ),
            ))
        };

        collected
            .load_recursive_with_handler(
                script_path,
                &include_dirs,
                Some(&mut unresolved_include_handler),
            )
            .map_err(|e| CompileError::Database {
                message: format!(
                    "Failed to collect file-local constants via Uxie for '{}': {}",
                    script_path.display(),
                    e
                ),
            })?;

        let base_count = base_symbols.get_all_defines().len();
        let collected_count = collected.get_all_defines().len();
        self.uxie_symbols = Some(collected);
        Ok(collected_count.saturating_sub(base_count))
    }

    /// Clone the constant database and apply file-local constants for one source file.
    pub fn clone_for_script<Q: AsRef<Path>>(&self, script_path: Q) -> Result<Self, CompileError> {
        let mut cloned = self.clone();
        cloned.load_script_constants(script_path)?;
        Ok(cloned)
    }

    pub fn loaded_script_file_paths(&self) -> Vec<PathBuf> {
        self.uxie_symbols
            .as_ref()
            .map(SymbolTable::loaded_file_paths)
            .unwrap_or_default()
    }

    pub fn load_map_events<P: AsRef<Path>, Q: AsRef<Path>>(
        &mut self,
        decomp_root: P,
        script_path: Q,
    ) -> Result<usize, CompileError> {
        if self.uxie_project_root.is_some() {
            return self.load_script_constants(script_path);
        }

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

    fn decomp_include_dirs(project_root: &Path) -> Vec<PathBuf> {
        vec![
            project_root.to_path_buf(),
            project_root.join("include"),
            project_root.join("res/field/scripts"),
        ]
    }

    fn try_load_decomp_events_include_json(
        table: &mut SymbolTable,
        parent_dir: &Path,
        include_dirs: &[PathBuf],
        include_path: &str,
    ) -> std::io::Result<bool> {
        if !include_path.contains("res/field/events/") || !include_path.ends_with(".h") {
            return Ok(false);
        }

        let json_path_str = include_path.replace(".h", ".json");
        let json_rel = parent_dir.join(&json_path_str);
        if json_rel.exists() {
            table.load_events_json(&json_rel)?;
            return Ok(true);
        }

        for dir in include_dirs {
            let json_path = dir.join(&json_path_str);
            if json_path.exists() {
                table.load_events_json(&json_path)?;
                return Ok(true);
            }
        }

        Ok(false)
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

    pub fn evaluate_expression(&self, expr: &str) -> Option<i32> {
        let expr = expr.trim();
        if expr.is_empty() {
            return None;
        }

        if let Some(symbols) = &self.uxie_symbols
            && let Some(val) = symbols.evaluate_expression(expr)
            && let Ok(val) = i32::try_from(val)
        {
            return Some(val);
        }

        let exprs: HashMap<String, String> = HashMap::new();
        let resolved: HashMap<String, i64> = HashMap::new();
        let cache: dashmap::DashMap<String, i64> = dashmap::DashMap::new();
        let parent_resolver = |name: &str| self.get(name).map(i64::from);

        eval_expr_with_parent(expr, &exprs, &resolved, &cache, &parent_resolver)
            .and_then(|val| i32::try_from(val).ok())
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

    fn test_db_for_legacy_lookup_with_version(version: &str) -> DatabaseV2 {
        let mut commands = HashMap::new();
        commands.insert(
            "Message".to_string(),
            test_command(CommandType::ScriptCmd, 1, Some("MessageLegacy")),
        );
        commands.insert(
            "WalkUp".to_string(),
            test_command(CommandType::Movement, 2, Some("WalkUpLegacy")),
        );
        commands.insert(
            "RadixTest".to_string(),
            test_command(CommandType::ScriptCmd, 16, Some("RadixTestLegacy")),
        );

        DatabaseV2 {
            meta: DatabaseMeta {
                version: version.to_string(),
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

    fn test_db_for_legacy_lookup() -> DatabaseV2 {
        test_db_for_legacy_lookup_with_version("test")
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
    fn test_get_command_resolves_platinum_scrcmd_hex_alias() {
        let db = test_db_for_legacy_lookup_with_version("platinum");
        let cmd = db
            .get_command("ScrCmd_0010")
            .expect("hex scrcmd lookup failed");
        assert_eq!(cmd.id, Some(16));
        assert_eq!(cmd.cmd_type, CommandType::ScriptCmd);
    }

    #[test]
    fn test_get_command_resolves_hgss_scrcmd_decimal_alias() {
        let db = test_db_for_legacy_lookup_with_version("hgss");
        let cmd = db
            .get_command("ScrCmd_16")
            .expect("decimal scrcmd lookup failed");
        assert_eq!(cmd.id, Some(16));
        assert_eq!(cmd.cmd_type, CommandType::ScriptCmd);
    }

    #[test]
    fn test_get_command_rejects_wrong_scrcmd_radix_for_family() {
        let platinum = test_db_for_legacy_lookup_with_version("platinum");
        let hgss = test_db_for_legacy_lookup_with_version("hgss");

        assert!(
            platinum.get_command("ScrCmd_10").is_ok(),
            "platinum accepts hex-compatible ScrCmd aliases"
        );
        assert!(
            hgss.get_command("ScrCmd_000A").is_err(),
            "hgss ScrCmd aliases must be decimal"
        );
        assert!(
            hgss.get_command("ScrCmd_0001").is_ok(),
            "hgss decimal aliases may contain leading zeroes"
        );
    }

    #[test]
    fn test_get_command_resolves_scrcmd_key_style_with_suffix_text() {
        let db = test_db_for_legacy_lookup_with_version("platinum");
        let cmd = db
            .get_command("ScrCmd_Unused_001")
            .expect("ScrCmd_*_<hex> alias lookup failed");
        assert_eq!(cmd.id, Some(1));
        assert_eq!(cmd.cmd_type, CommandType::ScriptCmd);
    }

    #[test]
    fn test_get_command_resolves_dummy_alias() {
        let db = test_db_for_legacy_lookup();
        let cmd = db
            .get_command("Dummy0001")
            .expect("Dummy alias lookup failed");
        assert_eq!(cmd.id, Some(1));
        assert_eq!(cmd.cmd_type, CommandType::ScriptCmd);
    }

    #[test]
    fn test_get_command_resolves_hgss_decimal_dummy_alias() {
        let db = test_db_for_legacy_lookup_with_version("hgss");
        let cmd = db
            .get_command("Dummy16")
            .expect("hgss decimal dummy alias lookup failed");
        assert_eq!(cmd.id, Some(16));
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
    fn test_get_script_cmd_resolves_aliases() {
        let db = test_db_for_legacy_lookup_with_version("hgss");
        let cmd = db
            .get_script_cmd("ScrCmd_1")
            .expect("script command alias lookup failed");
        assert_eq!(cmd.id, Some(1));
        assert_eq!(cmd.cmd_type, CommandType::ScriptCmd);
    }

    #[test]
    fn test_get_script_cmd_by_alias_does_not_override_exact_script_cmd_name() {
        let mut db = test_db_for_legacy_lookup_with_version("hgss");
        db.commands.insert(
            "ScrCmd_055".to_string(),
            test_command(CommandType::ScriptCmd, 56, None),
        );
        db.commands.insert(
            "DirectionSignpost".to_string(),
            test_command(CommandType::ScriptCmd, 55, None),
        );

        assert!(
            db.get_script_cmd_by_alias("ScrCmd_055").is_none(),
            "exact script command keys must not be reinterpreted as aliases"
        );
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

    #[test]
    fn test_load_script_constants_ignores_directory_inputs() {
        let temp_dir = unique_temp_dir("script_constants_directory");
        fs::create_dir_all(&temp_dir).expect("failed to create temp dir");

        let mut constants = ConstantDb::new();
        constants.load_decomp_symbols(&temp_dir, SymbolTable::new());

        let loaded = constants
            .load_script_constants(&temp_dir)
            .expect("directory inputs should be ignored");

        fs::remove_dir_all(&temp_dir).ok();

        assert_eq!(loaded, 0);
    }

    #[test]
    fn test_try_load_decomp_events_include_json_loads_event_symbols() {
        let temp_dir = unique_temp_dir("events_include_json");
        let parent_dir = temp_dir.join("res/field/scripts");
        let events_dir = temp_dir.join("res/field/events");
        fs::create_dir_all(&parent_dir).expect("failed to create scripts dir");
        fs::create_dir_all(&events_dir).expect("failed to create events dir");
        fs::write(
            events_dir.join("events_test_map.json"),
            concat!(
                "{\n",
                "  \"object_events\": [\n",
                "    { \"id\": \"LOCALID_HIKER\" },\n",
                "    { \"id\": \"LOCALID_TWIN\" }\n",
                "  ]\n",
                "}\n"
            ),
        )
        .expect("failed to write events json");

        let mut table = SymbolTable::new();
        let loaded = ConstantDb::try_load_decomp_events_include_json(
            &mut table,
            &parent_dir,
            std::slice::from_ref(&temp_dir),
            "res/field/events/events_test_map.h",
        )
        .expect("event include fallback should succeed");

        fs::remove_dir_all(&temp_dir).ok();

        assert!(loaded);
        assert_eq!(table.resolve_constant("LOCALID_HIKER"), Some(0));
        assert_eq!(table.resolve_constant("LOCALID_TWIN"), Some(1));
    }

    #[test]
    fn test_try_load_decomp_events_include_json_ignores_non_event_headers() {
        let temp_dir = unique_temp_dir("non_event_include_json");
        fs::create_dir_all(&temp_dir).expect("failed to create temp dir");
        fs::write(
            temp_dir.join("not_events.json"),
            "{ \"object_events\": [ { \"id\": \"LOCALID_HIKER\" } ] }",
        )
        .expect("failed to write json fixture");

        let mut table = SymbolTable::new();
        let loaded = ConstantDb::try_load_decomp_events_include_json(
            &mut table,
            &temp_dir,
            &[],
            "not_events.h",
        )
        .expect("non-event includes should be ignored");

        fs::remove_dir_all(&temp_dir).ok();

        assert!(!loaded);
        assert_eq!(table.resolve_constant("LOCALID_HIKER"), None);
    }
}
