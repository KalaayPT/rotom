use std::{collections::HashMap, path::PathBuf};

use anyhow::{Context, Result};

use crate::parser::{CommandContainer, ContainerType};

pub enum Directive {
    Assemble,
    Disassemble,
}

pub trait PathExt {
    fn name_to_str(&self) -> Result<String>;
}
impl PathExt for PathBuf {
    fn name_to_str(&self) -> Result<String> {
        Ok(self
            .file_name()
            .context("filename couldnt be assessed")?
            .to_str()
            .context("filename has invalid utf-8")?
            .to_string())
    }
}

pub fn number_from_str(s: &str) -> Result<i32> {
    if s.starts_with("0x") {
        i32::from_str_radix(s.trim_start_matches("0x"), 16)
            .with_context(|| format!("Invalid hex number: '{}'", s))
    } else {
        s.parse()
            .with_context(|| format!("Invalid decimal number: '{}'", s))
    }
}

pub fn number_to_str(num: &i32) -> String {
    if num > &0x4000 {
        format!("0x{}", format!("{num:x}").to_uppercase())
    } else {
        format!("{}", num)
    }
}

pub fn get_rom_root(path: &PathBuf) -> Result<PathBuf> {
    if path.is_dir() {
        path.ancestors()
            .nth(2)
            .map(PathBuf::from)
            .context("couldnt find rom root, folder too shallow")
    } else {
        path.ancestors()
            .nth(3)
            .map(PathBuf::from)
            .context("couldnt find rom root, folder too shallow")
    }
}

pub fn get_output_dir(path: &PathBuf, directive: Directive) -> Result<PathBuf> {
    let root = get_rom_root(path)?;
    let output = match directive {
        Directive::Assemble => root.join("unpacked").join("testscripts"),
        Directive::Disassemble => root.join("expanded").join("scripts"),
    };
    Ok(output)
}

pub fn format_parameter_enum(map: &HashMap<String, String>, parameter: i32) -> String {
    match map.get(format!("{}", parameter).as_str()) {
        Some(moves) => format!("{moves}"),
        None => {
            match map.get(format!("0x{}", format!("{:0>4x}", parameter).to_uppercase()).as_str()) {
                Some(moves) => moves.clone(),
                None => number_to_str(&parameter),
            }
        }
    }
}

pub fn format_container_header(container: &CommandContainer) -> String {
    let mut str = String::from("\n\n");
    for id in &container.reference.id {
        match container.kind {
            ContainerType::Script => str.push_str(format!("Script {}:\n", id).as_str()),
            ContainerType::Function => str.push_str(format!("Function {}:\n", id).as_str()),
            ContainerType::Action => str.push_str(format!("Action {}:\n", id).as_str()),
            ContainerType::LevelScript => {
                if container.reference.id.first().unwrap() == &0 {
                    str.push_str("");
                } else {
                    str.push_str("InitScriptFrameTable:");
                }
            }
        }
    }
    str = str.trim_end_matches("\n").to_string();
    str
}

// pub fn get_hash(path: &PathBuf) -> Result<[u8; 32]> {
//     let mut file = std::fs::File::open(&path)?;
//     let mut hasher = Sha256::new();
//     let _ = std::io::copy(&mut file, &mut hasher)?;
//     Ok(hasher.finalize().into())
// }
