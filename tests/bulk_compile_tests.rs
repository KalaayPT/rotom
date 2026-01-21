//! Bulk integration tests for the Rotom compiler pipeline
//!
//! These tests compile ALL scripts from the pokeplatinum decomp project
//! and compare the resulting binaries against the known-good pre-built binaries.
//!
//! ## Test Categories
//! - **Normal Scripts**: Standard script files (no `_init_` in filename)
//! - **Levelscripts**: Initialization scripts (contain `_init_` in filename)
//!
//! ## Environment Variables
//! - `POKEPLATINUM_ROOT`: Path to pokeplatinum checkout (default: ~/dev/pokeplatinum)
//!
//! ## Usage
//! ```bash
//! # Run bulk tests (summary only)
//! cargo test bulk_compile -- --nocapture
//!
//! # Run with verbose output (shows failures)
//! cargo test bulk_compile --ignored -- --nocapture
//!
//! # Run specific verbose test
//! cargo test test_bulk_compile_normal_scripts_verbose --ignored -- --nocapture
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::prelude::*;
use sha2::{Digest, Sha256};

use rotom::compile_levelscript_to_bytes;
use rotom::compile_to_bytes_with_options;
use rotom::database::{ConstantDb, DatabaseV2};
use rotom::transpiler::decomp::transpile as transpile_decomp;
use rotom::transpiler::is_levelscript_source;

/// Result category for a single script compilation attempt
#[derive(Debug, Clone)]
pub enum CompileOutcome {
    /// Compilation succeeded and hash matches expected binary
    Match,
    /// Compilation succeeded but hash differs from expected binary
    HashMismatch {
        expected_hash: String,
        actual_hash: String,
        expected_size: usize,
        actual_size: usize,
    },
    /// Compilation failed (rotoscript -> binary)
    CompileError(String),
    /// Expected binary file not found in build directory
    MissingExpectedBinary(PathBuf),
    /// Source file could not be read
    IoError(String),
}

/// Statistics for a bulk compile run
#[derive(Debug, Default)]
pub struct BulkCompileStats {
    pub total: usize,
    pub matches: AtomicUsize,
    pub hash_mismatches: AtomicUsize,
    pub compile_errors: AtomicUsize,
    pub missing_binaries: AtomicUsize,
    pub io_errors: AtomicUsize,
}

/// Detailed results for reporting
pub struct BulkCompileResult {
    pub stats: BulkCompileStats,
    pub outcomes: Mutex<HashMap<String, CompileOutcome>>,
}

const DEFAULT_POKEPLATINUM_ROOT: &str = "C:/dev/pokeplatinum";

fn get_pokeplatinum_root() -> PathBuf {
    std::env::var("POKEPLATINUM_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_POKEPLATINUM_ROOT))
}

fn get_scripts_dir() -> PathBuf {
    get_pokeplatinum_root().join("res/field/scripts")
}

fn get_binaries_dir() -> PathBuf {
    get_pokeplatinum_root().join("build/res/field/scripts/scr_seq.narc.p")
}

fn script_to_binary_path(script_path: &Path) -> PathBuf {
    let stem = script_path.file_stem().unwrap().to_str().unwrap();
    get_binaries_dir().join(stem)
}

fn find_normal_scripts() -> Vec<PathBuf> {
    let scripts_dir = get_scripts_dir();
    let mut scripts: Vec<PathBuf> = std::fs::read_dir(&scripts_dir)
        .expect("Failed to read scripts directory")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().map(|e| e == "s").unwrap_or(false)
                && !path
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .contains("_init_")
        })
        .collect();
    scripts.sort();
    scripts
}

fn find_levelscripts() -> Vec<PathBuf> {
    let scripts_dir = get_scripts_dir();
    let mut scripts: Vec<PathBuf> = std::fs::read_dir(&scripts_dir)
        .expect("Failed to read scripts directory")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().map(|e| e == "s").unwrap_or(false)
                && path
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .contains("_init_")
        })
        .collect();
    scripts.sort();
    scripts
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

fn load_test_db_and_constants() -> (DatabaseV2, ConstantDb) {
    let db = DatabaseV2::load(Path::new("src/db/platinum_v2.json"))
        .expect("Failed to load test database");

    let mut constants = ConstantDb::new();
    constants.load_from_db(&db);

    let decomp_root = get_pokeplatinum_root();
    if decomp_root.exists() {
        constants
            .load_decomp_project(&decomp_root)
            .unwrap_or_else(|_| {
                panic!(
                    "Failed to load decomp project constants from '{}'. \
                     Set POKEPLATINUM_ROOT env var or ensure path exists.",
                    decomp_root.display()
                )
            });
    } else {
        panic!(
            "Decomp project not found at '{}'. \
             Set POKEPLATINUM_ROOT env var to the path of your pokeplatinum checkout.",
            decomp_root.display()
        );
    }

    (db, constants)
}

fn compile_single_script(
    script_path: &Path,
    db: &DatabaseV2,
    base_constants: &ConstantDb,
) -> CompileOutcome {
    let source = match std::fs::read_to_string(script_path) {
        Ok(s) => s,
        Err(e) => return CompileOutcome::IoError(format!("{}", e)),
    };

    let binary_path = script_to_binary_path(script_path);
    let expected_bytes = match std::fs::read(&binary_path) {
        Ok(b) => b,
        Err(_) => return CompileOutcome::MissingExpectedBinary(binary_path),
    };
    let expected_hash = sha256_hex(&expected_bytes);

    let mut constants = base_constants.clone();
    let decomp_root = get_pokeplatinum_root();
    let _ = constants.load_map_events(&decomp_root, script_path);

    let is_levelscript = is_levelscript_source(&source);

    let actual_bytes = if is_levelscript {
        match compile_levelscript_to_bytes(&source, &constants) {
            Ok(b) => b,
            Err(e) => return CompileOutcome::CompileError(format!("{:?}", e)),
        }
    } else {
        let transpile_result = transpile_decomp(&source, Some(db));

        match compile_to_bytes_with_options(
            &transpile_result.source,
            db,
            &constants,
            transpile_result.emit_end_marker,
        ) {
            Ok(b) => b,
            Err(e) => return CompileOutcome::CompileError(format!("{:?}", e)),
        }
    };

    let actual_hash = sha256_hex(&actual_bytes);
    if actual_hash == expected_hash {
        CompileOutcome::Match
    } else {
        CompileOutcome::HashMismatch {
            expected_hash,
            actual_hash,
            expected_size: expected_bytes.len(),
            actual_size: actual_bytes.len(),
        }
    }
}

fn bulk_compile_scripts(
    scripts: Vec<PathBuf>,
    db: &DatabaseV2,
    constants: &ConstantDb,
) -> BulkCompileResult {
    let result = BulkCompileResult {
        stats: BulkCompileStats {
            total: scripts.len(),
            ..Default::default()
        },
        outcomes: Mutex::new(HashMap::new()),
    };

    scripts.par_iter().for_each(|script_path| {
        let script_name = script_path
            .file_stem()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let outcome = compile_single_script(script_path, db, constants);

        match &outcome {
            CompileOutcome::Match => {
                result.stats.matches.fetch_add(1, Ordering::Relaxed);
            }
            CompileOutcome::HashMismatch { .. } => {
                result.stats.hash_mismatches.fetch_add(1, Ordering::Relaxed);
            }
            CompileOutcome::CompileError(_) => {
                result.stats.compile_errors.fetch_add(1, Ordering::Relaxed);
            }
            CompileOutcome::MissingExpectedBinary(_) => {
                result
                    .stats
                    .missing_binaries
                    .fetch_add(1, Ordering::Relaxed);
            }
            CompileOutcome::IoError(_) => {
                result.stats.io_errors.fetch_add(1, Ordering::Relaxed);
            }
        }

        result.outcomes.lock().unwrap().insert(script_name, outcome);
    });

    result
}

fn print_bulk_compile_report(name: &str, result: &BulkCompileResult, verbose: bool) {
    let stats = &result.stats;
    let total = stats.total as f64;

    let matches = stats.matches.load(Ordering::Relaxed);
    let hash_mismatches = stats.hash_mismatches.load(Ordering::Relaxed);
    let compile_errors = stats.compile_errors.load(Ordering::Relaxed);
    let missing_binaries = stats.missing_binaries.load(Ordering::Relaxed);
    let io_errors = stats.io_errors.load(Ordering::Relaxed);

    println!();
    println!("============================================================");
    println!("BULK COMPILE REPORT: {}", name);
    println!("============================================================");
    println!("Total scripts:        {}", stats.total);
    println!("------------------------------------------------------------");
    println!(
        "Matches:              {:>4} ({:>5.1}%)",
        matches,
        100.0 * matches as f64 / total
    );
    println!(
        "Hash mismatches:      {:>4} ({:>5.1}%)",
        hash_mismatches,
        100.0 * hash_mismatches as f64 / total
    );
    println!(
        "Compile errors:       {:>4} ({:>5.1}%)",
        compile_errors,
        100.0 * compile_errors as f64 / total
    );
    println!(
        "Missing binaries:     {:>4} ({:>5.1}%)",
        missing_binaries,
        100.0 * missing_binaries as f64 / total
    );
    println!(
        "IO errors:            {:>4} ({:>5.1}%)",
        io_errors,
        100.0 * io_errors as f64 / total
    );
    println!("============================================================");

    if verbose {
        let outcomes = result.outcomes.lock().unwrap();

        let mut failures: Vec<_> = outcomes
            .iter()
            .filter(|(_, o)| !matches!(o, CompileOutcome::Match))
            .collect();
        failures.sort_by_key(|(name, _)| *name);

        if !failures.is_empty() {
            println!();
            println!("Failures (first 50):");
            println!("------------------------------------------------------------");
            for (name, outcome) in failures.iter().take(50) {
                match outcome {
                    CompileOutcome::Match => {}
                    CompileOutcome::HashMismatch {
                        expected_size,
                        actual_size,
                        ..
                    } => {
                        println!(
                            "  {} - HASH MISMATCH (expected {} bytes, got {} bytes)",
                            name, expected_size, actual_size
                        );
                    }
                    CompileOutcome::CompileError(msg) => {
                        let short_msg = if msg.len() > 80 {
                            format!("{}...", &msg[..80])
                        } else {
                            msg.clone()
                        };
                        println!("  {} - COMPILE ERROR: {}", name, short_msg);
                    }
                    CompileOutcome::MissingExpectedBinary(path) => {
                        println!("  {} - MISSING BINARY: {:?}", name, path);
                    }
                    CompileOutcome::IoError(msg) => {
                        println!("  {} - IO ERROR: {}", name, msg);
                    }
                }
            }
            if failures.len() > 50 {
                println!("  ... and {} more failures", failures.len() - 50);
            }
        }
    }
}

fn run_normal_scripts_test(verbose: bool) -> BulkCompileResult {
    let scripts_dir = get_scripts_dir();
    if !scripts_dir.exists() {
        panic!(
            "Bulk test failed: scripts directory not found at {:?}. \
             Set POKEPLATINUM_ROOT environment variable to run this test.",
            scripts_dir
        );
    }

    let binaries_dir = get_binaries_dir();
    if !binaries_dir.exists() {
        panic!(
            "Bulk test failed: binaries directory not found at {:?}. \
             Make sure you've built the pokeplatinum project first.",
            binaries_dir
        );
    }

    let (db, constants) = load_test_db_and_constants();

    let scripts = find_normal_scripts();
    assert!(!scripts.is_empty(), "No normal scripts found");
    println!("Found {} normal scripts to compile", scripts.len());

    let result = bulk_compile_scripts(scripts, &db, &constants);

    print_bulk_compile_report("Normal Scripts", &result, verbose);

    result
}

fn run_levelscripts_test(verbose: bool) -> BulkCompileResult {
    let scripts_dir = get_scripts_dir();
    if !scripts_dir.exists() {
        panic!(
            "Bulk test failed: scripts directory not found at {:?}. \
             Set POKEPLATINUM_ROOT environment variable to run this test.",
            scripts_dir
        );
    }

    let binaries_dir = get_binaries_dir();
    if !binaries_dir.exists() {
        panic!(
            "Bulk test failed: binaries directory not found at {:?}. \
             Make sure you've built the pokeplatinum project first.",
            binaries_dir
        );
    }

    let (db, constants) = load_test_db_and_constants();

    let scripts = find_levelscripts();
    assert!(!scripts.is_empty(), "No levelscripts found");
    println!("Found {} levelscripts to compile", scripts.len());

    let result = bulk_compile_scripts(scripts, &db, &constants);

    print_bulk_compile_report("Levelscripts", &result, verbose);

    println!();
    println!("NOTE: Levelscript compilation uses the same pipeline as normal scripts.");
    println!("Levelscripts have a different binary format (InitScript* commands).");
    println!("Expect failures until a dedicated levelscript compiler is implemented.");

    result
}

fn assert_100_percent_match(result: &BulkCompileResult, script_type: &str) {
    let matches = result.stats.matches.load(Ordering::Relaxed);
    let total = result.stats.total;

    assert_eq!(
        matches,
        total,
        "{} bulk compile requires 100% hash matches. Got {}/{} ({:.1}%)",
        script_type,
        matches,
        total,
        100.0 * matches as f64 / total as f64
    );
}

#[test]
fn test_bulk_compile_normal_scripts() {
    let result = run_normal_scripts_test(false);
    assert_100_percent_match(&result, "Normal scripts");
}

#[test]
#[ignore]
fn test_bulk_compile_normal_scripts_verbose() {
    let result = run_normal_scripts_test(true);
    assert_100_percent_match(&result, "Normal scripts");
}

#[test]
fn test_bulk_compile_levelscripts() {
    let result = run_levelscripts_test(false);
    assert_100_percent_match(&result, "Levelscripts");
}

#[test]
#[ignore]
fn test_bulk_compile_levelscripts_verbose() {
    let result = run_levelscripts_test(true);
    assert_100_percent_match(&result, "Levelscripts");
}
