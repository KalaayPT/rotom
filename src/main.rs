use clap::{Args, Parser, Subcommand};
use rayon::prelude::*;
use regex::Regex;
use std;
use std::io::{self};
use std::path::PathBuf;

use crate::assembler::{assemble, parse_plaintext_file};
use crate::database::{Enums, ScriptDatabase};
use crate::disassembler::{disassemble, parse_script_file_bin};

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

fn main() {
    let args = Cli::parse();
    // println!("{args:?}");
    match args.command {
        Commands::Disassemble(args) => {
            disassemble(args).unwrap();
        }
        Commands::Assemble(args) => {
            assemble(args).unwrap();
        }
    }
}

fn parse_directory(
    folder: &PathBuf,
    db: &ScriptDatabase,
    enums: &Enums,
    directive: Directive,
) -> io::Result<()> {
    let entries = std::fs::read_dir(folder)?
        .map(|res| res.map(|e| e.path()))
        .collect::<Result<Vec<_>, io::Error>>()?;
    entries.par_iter().for_each(
        |file|
    // for file in entries {
        match directive {
            Directive::Assemble => {
                let regex = Regex::new(r"(?m)^\w+ [\w\d#]+:").unwrap();
                parse_plaintext_file(&file, &db, &enums, &regex).unwrap()
            },
            Directive::Disassemble => {
                if let Err(e) = parse_script_file_bin(&file, &db, &enums){
                    println!("Error encountered during parsing: {e}");
                }
            }
        }, // }
    );
    Ok(())
}
