use anyhow::Result;
use serde::Deserialize;
use std::{collections::HashMap, path::PathBuf};

#[derive(Debug, Deserialize)]
pub struct ScriptDatabase {
    pub game_version: String,
    pub movements: HashMap<String, Movements>,
    pub comparisonOperators: HashMap<String, String>,
    pub specialOverworlds: HashMap<String, String>,
    pub overworldDirections: HashMap<String, String>,
    pub scrcmd: HashMap<String, ScrCmd>,
    pub lvlscrcmd: HashMap<String, LvlScrCmd>,
    pub sounds: HashMap<String, Sounds>,
}
#[derive(Debug, Deserialize)]
pub struct Movements {
    pub name: String,
    pub decomp_name: String,
    pub description: String,
}
#[derive(Debug, Deserialize)]
pub struct ScrCmd {
    pub name: String,
    pub decomp_name: String,
    pub parameters: Vec<u8>,
    pub parameter_types: Vec<ScriptParameter>,
    pub parameter_values: Vec<String>,
    pub description: String,
}
#[derive(Debug, Deserialize)]
pub struct LvlScrCmd {
    pub length: Option<String>,
    pub value: Option<u8>,
    pub parameters: Vec<u8>,
    pub parameter_types: Vec<String>,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub enum ScriptParameter {
    Integer,
    Variable,
    Flex,
    Overworld,
    OwMovementType,
    OwMovementDirection,
    ComparisonOperator,
    Function,
    Action,
    CMDNumber,
    Pokemon,
    Item,
    Move,
    Sound,
    Trainer,
}
#[derive(Debug, Deserialize)]
pub struct Sounds {
    pub name: String,
    pub used_in: String,
}
#[derive(Debug, Deserialize)]
pub struct Enums {
    pub items: HashMap<String, String>,
    pub moves: HashMap<String, String>,
    pub pokemon: HashMap<String, String>,
    pub trainers: HashMap<String, String>,
}
impl Enums {
    pub fn new() -> Enums {
        Enums {
            items: HashMap::new(),
            moves: HashMap::new(),
            pokemon: HashMap::new(),
            trainers: HashMap::new(),
        }
    }
}

pub fn read_jsons(json_path: &PathBuf) -> Result<(ScriptDatabase, Enums)> {
    // println!("{}", format!("{}\\scrcmd_database.json",json_path));
    let db_json = std::fs::read_to_string(json_path.join("scrcmd_database.json"))?;
    let items_json = std::fs::read_to_string(json_path.join("items.json"))?;
    let pokemon_json = std::fs::read_to_string(json_path.join("pokemon.json"))?;
    let trainers_json = std::fs::read_to_string(json_path.join("trainers.json"))?;
    let moves_json = std::fs::read_to_string(json_path.join("moves.json"))?;
    let db: ScriptDatabase = serde_json::from_str(&db_json)?;
    let mut enums = Enums::new();
    enums.trainers = serde_json::from_str(&trainers_json)?;
    enums.items = serde_json::from_str(&items_json)?;
    enums.pokemon = serde_json::from_str(&pokemon_json)?;
    enums.moves = serde_json::from_str(&moves_json)?;
    Ok((db, enums))
}
