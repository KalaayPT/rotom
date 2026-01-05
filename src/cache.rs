use anyhow::Result;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, io::Write, path::PathBuf};

use crate::helpers::{get_output_dir, get_rom_root, Directive, PathExt};
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
    pub fn needs_rebuild(&self, file: &PathBuf, file_cache: &mut FileCache) -> Result<bool> {
        let file_name = file.name_to_str()?;

        let cached_file = self.files.get(&file_name);
        let mut last_modified = Local::now();
        if let Ok(file) = std::fs::metadata(format!(
            "{}.script",
            get_output_dir(file, Directive::Disassemble)?
                .join(&file_name)
                .display()
        )) {
            last_modified = file.modified()?.into();
        }
        // println!("{last_modified:?}");
        if let Some(cached_file) = cached_file {
            if last_modified <= cached_file.build_time {
                file_cache.status = BuildStatus::Skipped;
                // println!("build skipped: {}", file_name);
                return Ok(true);
            }
        }
        Ok(false)
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
            status: BuildStatus::InvalidFile,
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
    cache.rom_id = get_rom_root(file)?
        .name_to_str()?
        .trim_end_matches("_DSPRE_contents")
        .to_string();
    let output_path = get_output_dir(file, Directive::Disassemble)?.join(output_filename);
    let mut f = std::fs::File::create(&output_path)?;
    let j = serde_json::to_string(&cache)?;
    f.write_all(j.as_bytes())?;
    Ok(())
}

pub fn read_cache(json_path: &PathBuf) -> Result<Cache> {
    let corrected_path =
        get_output_dir(json_path, Directive::Disassemble)?.join(".rotom-cache.json");
    let db_json = std::fs::read_to_string(corrected_path)?;
    let cache: Cache = serde_json::from_str(&db_json)?;
    Ok(cache)
}
