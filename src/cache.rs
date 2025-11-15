use crate::parser::ParseError;
use chrono::{DateTime, Local};
use std::{collections::HashMap, path::PathBuf};

struct Cache {
    rom_id: String,
    game_version: String,
    last_build: DateTime<Local>,
    files: HashMap<String, FileCache>,
}

pub struct FileCache {
    status: BuildStatus,
    hash: String,
    errors: Vec<ParseError>,
}
impl FileCache {
    pub fn new() -> FileCache {
        FileCache {
            status: BuildStatus::Error,
            hash: String::new(),
            errors: Vec::new(),
        }
    }
}

enum BuildStatus {
    Success,
    Partial,
    Error,
}

pub fn write_cache(file: &PathBuf, version: String) {
    let mut cache = Cache {
        rom_id: String::new(),
        game_version: version,
        last_build: Local::now(),
        files: HashMap::new(),
    };
    if file.is_dir() {
        cache.rom_id = file
            .parent()
            .unwrap()
            .to_str()
            .unwrap()
            .trim_end_matches("_DSPRE_contents")
            .to_string();
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
    }
}
