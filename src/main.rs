use anyhow::Result;
use chrono::Local;
use clap::{Args, Parser, Subcommand};
use rayon::prelude::*;
use regex::Regex;
use std;
use std::collections::HashMap;
use std::io::{self};
use std::path::PathBuf;

use crate::assembler::{assemble, parse_plaintext_file};
use crate::cache::{BuildStatus, Cache, FileCache};
use crate::database::{Enums, ScriptDatabase};
use crate::disassembler::{disassemble, parse_script_file_bin};
use crate::helpers::get_hash;

mod assembler;
mod cache;
mod database;
mod disassembler;
mod helpers;
mod levelscript;
mod parser;

#[derive(Debug, Parser)]
#[command(version, about = "A pokemon script assembler/disassembler", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}
#[derive(Debug, Subcommand)]
enum Commands {
    #[command(arg_required_else_help = true)]
    Assemble(CommandArgs),
    Disassemble(CommandArgs),
}
#[derive(Debug, Args)]
struct CommandArgs {
    #[arg(short, long)]
    database: PathBuf,
    #[arg(short, long, group = "script")]
    script_file: PathBuf,
}
enum Directive {
    Assemble,
    Disassemble,
}
pub struct ParseResult {
    pub file_name: String,
    pub cache_entry: FileCache,
}

fn main() -> Result<()> {
    let args = Cli::parse();
    // println!("{args:?}");
    match args.command {
        Commands::Disassemble(args) => disassemble(args),
        Commands::Assemble(args) => assemble(args),
    }
}

fn parse_directory(
    folder: &PathBuf,
    db: &ScriptDatabase,
    enums: &Enums,
    cache: &Cache,
    directive: Directive,
    cachemap: &mut HashMap<String, FileCache>,
) -> Result<()> {
    let entries = std::fs::read_dir(folder)?
        .map(|res| res.map(|e| e.path()))
        .collect::<Result<Vec<_>, io::Error>>()?;
    let results: Vec<(PathBuf, Result<ParseResult>)> = entries
        .par_iter()
        .map(|file| {
            let regex = Regex::new(r"(?m)^\w+ [\w\d#]+:").unwrap();

            let result = match directive {
                Directive::Assemble => parse_plaintext_file(&file, &db, &enums, &regex),
                Directive::Disassemble => parse_script_file_bin(&file, &db, &enums, cache),
            };
            (file.clone(), result)
        })
        .collect();
    for (file, result) in results {
        match result {
            Ok(parse_result) => {
                cachemap.insert(parse_result.file_name, parse_result.cache_entry);
            }

            Err(e) => {
                eprintln!("Error processing {}: {}", file.display(), e);
                let name = file
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                cachemap.insert(
                    name,
                    FileCache {
                        status: BuildStatus::Error,
                        // hash: get_hash(&file)?
                        //     .iter()
                        //     .map(|byte| format!("{byte:02x}"))
                        //     .collect(),
                        error_message: Some(e.to_string()),
                        build_time: Local::now(),
                    },
                );
            }
        }
    }
    Ok(())
}
