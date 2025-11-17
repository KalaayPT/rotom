use anyhow::{Result, anyhow};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, io::Write, path::PathBuf};
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
    // pub hash: String,
    pub error_message: Option<String>,
}
impl FileCache {
    pub fn new() -> FileCache {
        FileCache {
            status: BuildStatus::Error,
            build_time: Local::now(),
            // hash: String::new(),
            error_message: None,
        }
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub enum BuildStatus {
    Success,
    Partial,
    Error,
    Skipped,
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
    let mut output_path = PathBuf::new();
    if file.is_dir() {
        cache.rom_id = file
            .parent()
            .unwrap()
            .to_str()
            .unwrap()
            .trim_end_matches("_DSPRE_contents")
            .to_string();
        output_path = file
            .ancestors()
            .nth(2)
            .ok_or(anyhow!("couldnt find ROM root"))?
            .join("expanded")
            .join("scripts")
            .join(output_filename);
    } else {
        cache.rom_id = file
            .ancestors()
            .nth(3)
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .trim_end_matches("_DSPRE_contents")
            .to_string();
        output_path = file
            .ancestors()
            .nth(3)
            .ok_or(anyhow!("couldnt find ROM root"))?
            .join("expanded")
            .join("scripts")
            .join(output_filename);
    }
    let mut f = std::fs::File::create(&output_path)?;
    let j = serde_json::to_string(&cache)?;
    f.write_all(j.as_bytes())?;
    Ok(())
}

pub fn read_cache(json_path: &PathBuf) -> Result<Cache> {
    let mut corrected_path = PathBuf::new();
    if json_path.is_dir() {
        corrected_path = json_path
            .ancestors()
            .nth(2)
            .ok_or(anyhow!("couldnt find ROM root"))?
            .join("expanded")
            .join("scripts")
            .join(".rotom-cache.json");
    } else {
        corrected_path = json_path
            .ancestors()
            .nth(3)
            .ok_or(anyhow!("couldnt find ROM root"))?
            .join("expanded")
            .join("scripts")
            .join(".rotom-cache.json");
    }
    println!("{}", json_path.display());
    let db_json = std::fs::read_to_string(corrected_path)?;
    let cache: Cache = serde_json::from_str(&db_json)?;
    Ok(cache)
}
