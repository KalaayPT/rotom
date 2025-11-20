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
use crate::helpers::PathExt;
use crate::parser::ParseContext;

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
    cache: &Cache,
    directive: Directive,
    cachemap: &mut HashMap<String, FileCache>,
    ctx: &ParseContext,
) -> Result<()> {
    let entries = std::fs::read_dir(folder)?
        .map(|res| res.map(|e| e.path()))
        .collect::<Result<Vec<_>, io::Error>>()?;
    let results: Vec<(PathBuf, Result<ParseResult>)> = entries
        .par_iter()
        .map(|file| {
            let regex = Regex::new(r"(?m)^\w+ [\w\d#]+:").unwrap();

            let result = match directive {
                Directive::Assemble => parse_plaintext_file(&file, &regex, &ctx),
                Directive::Disassemble => parse_script_file_bin(&file, cache, &ctx),
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
                let name = file.name_to_str()?;
                match directive {
                    Directive::Assemble => {
                        cachemap.insert(
                            name,
                            FileCache {
                                status: BuildStatus::AssembleError,
                                error_message: Some(e.to_string()),
                                build_time: Local::now(),
                            },
                        );
                    }
                    Directive::Disassemble => {
                        cachemap.insert(
                            name,
                            FileCache {
                                status: BuildStatus::PartialDisassembly,
                                error_message: Some(e.to_string()),
                                build_time: Local::now(),
                            },
                        );
                    }
                }
            }
        }
    }
    Ok(())
}
