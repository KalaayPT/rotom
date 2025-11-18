use anyhow::Result;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, io::Write, path::PathBuf};

use crate::{
    Directive,
    helpers::{get_output_dir, get_rom_root},
};
#[derive(Debug, Serialize, Deserialize)]
pub struct Cache {
    pub rom_id: String,
    pub game_version: String,
    pub last_build: DateTime<Local>,
    pub files: HashMap<String, FileCache>,
}
impl Cache {
    pub fn new() -> Cache {
        Cache {
            rom_id: String::new(),
            game_version: String::new(),
            last_build: Local::now(),
            files: HashMap::new(),
        }
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub struct FileCache {
    pub status: BuildStatus,
    pub build_time: DateTime<Local>,
    pub error_message: Option<String>,
}
impl FileCache {
    pub fn new() -> FileCache {
        FileCache {
            status: BuildStatus::Skipped,
            build_time: Local::now(),
            error_message: None,
        }
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub enum BuildStatus {
    Success,
    PartialDisassembly,
    AssembleError,
    Skipped,
    InvalidFile,
}

pub fn write_cache(
    file: &PathBuf,
    version: String,
    cachemap: HashMap<String, FileCache>,
) -> Result<()> {
    println!("writing cache...");
    let mut cache = Cache {
        rom_id: String::new(),
        game_version: version,
        last_build: Local::now(),
        files: cachemap,
    };
    let output_filename = ".rotom-cache.json";
    if file.is_dir() {
        cache.rom_id = file
            .parent()
            .unwrap()
            .to_str()
            .unwrap()
            .trim_end_matches("_DSPRE_contents")
            .to_string();
    } else {
        cache.rom_id = get_rom_root(file)?
            .file_name()
            .unwrap()
            .to_string_lossy()
            .trim_end_matches("_DSPRE_contents")
            .to_string();
    }
    let output_path = get_output_dir(file, Directive::Disassemble)?.join(output_filename);
    let mut f = std::fs::File::create(&output_path)?;
    let j = serde_json::to_string(&cache)?;
    f.write_all(j.as_bytes())?;
    Ok(())
}

pub fn read_cache(json_path: &PathBuf) -> Result<Cache> {
    let corrected_path =
        get_output_dir(json_path, Directive::Disassemble)?.join(".rotom-cache.json");
    println!("{}", corrected_path.display());
    let db_json = std::fs::read_to_string(corrected_path)?;
    let cache: Cache = serde_json::from_str(&db_json)?;
    Ok(cache)
}
