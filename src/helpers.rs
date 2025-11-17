use std::path::PathBuf;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::parser::{CommandContainer, ContainerType};

pub fn parse_number_str(s: &str) -> Result<i32> {
    if s.starts_with("0x") {
        i32::from_str_radix(s.trim_start_matches("0x"), 16)
            .with_context(|| format!("Invalid hex number: '{}'", s))
    } else {
        s.parse()
            .with_context(|| format!("Invalid decimal number: '{}'", s))
    }
}

pub fn format_container_header(container: &CommandContainer) -> String {
    let mut str = String::from("\n\n");
    for id in &container.reference.id {
        match container.kind {
            ContainerType::Script => str.push_str(format!("Script {}:\n", id).as_str()),
            ContainerType::Function => str.push_str(format!("Function {}:\n", id).as_str()),
            ContainerType::Action => str.push_str(format!("Action {}:\n", id).as_str()),
        }
    }
    str = str.trim_end_matches("\n").to_string();
    str
}

pub fn get_hash(path: &PathBuf) -> Result<[u8; 32]> {
    let mut file = std::fs::File::open(&path)?;
    let mut hasher = Sha256::new();
    let _ = std::io::copy(&mut file, &mut hasher)?;
    Ok(hasher.finalize().into())
}
