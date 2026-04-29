//! Bulk integration tests for the Rotom compiler pipeline
//!
//! These tests compile ALL scripts from the pokeplatinum decomp project
//! and compare the resulting binaries against the known-good pre-built binaries.
//!
//! ## Test Categories
//! - **Normal Scripts**: Standard script files that do not match levelscript naming.
//! - **Levelscripts**: Initialization scripts using `_init_` or `_hdr` naming.
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

use rotom::compile_levelscript_json_to_bytes;
use rotom::compile_levelscript_to_bytes;
use rotom::database::{ConstantDb, DatabaseV2};
use rotom::decompiler::disassembler::ScriptOutput;
use rotom::decompiler::ir_to_source;
use rotom::is_levelscript_path;
use rotom::transpiler::decomp::transpile as transpile_decomp;
use rotom::transpiler::is_levelscript_source;
use rotom::transpiler::transpile_dspre;
use rotom::{compile_to_bytes, compile_to_bytes_with_options};
use uxie::{GameLanguage, RomHeader};

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

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn get_pokeplatinum_root() -> PathBuf {
    std::env::var("POKEPLATINUM_ROOT").map_or_else(
        |_| fixture_root().join("decomp/pokeplatinum"),
        PathBuf::from,
    )
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

fn fixture_panic(path: &Path, what: &str) -> ! {
    panic!(
        "Missing fixture: {} at {}\n\
         Run `cargo xtask setup-fixtures` and copy DSPRE outputs per CONTRIBUTING.md.",
        what,
        path.display()
    )
}

fn find_normal_scripts() -> Vec<PathBuf> {
    let scripts_dir = get_scripts_dir();
    let mut scripts: Vec<PathBuf> = std::fs::read_dir(&scripts_dir)
        .unwrap_or_else(|_| fixture_panic(&scripts_dir, "decomp scripts"))
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "s") && !is_levelscript_path(path))
        .collect();
    scripts.sort();
    scripts
}

fn find_levelscripts() -> Vec<PathBuf> {
    let scripts_dir = get_scripts_dir();
    let mut scripts: Vec<PathBuf> = std::fs::read_dir(&scripts_dir)
        .unwrap_or_else(|_| fixture_panic(&scripts_dir, "decomp scripts"))
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "s") && is_levelscript_path(path))
        .collect();
    scripts.sort();
    scripts
}

fn get_pokeheartgold_root() -> PathBuf {
    std::env::var("POKEHEARTGOLD_ROOT").map_or_else(
        |_| fixture_root().join("decomp/pokeheartgold"),
        PathBuf::from,
    )
}

fn get_dspre_platinum_root() -> PathBuf {
    std::env::var("DSPRE_PLATINUM_ROOT").map_or_else(
        |_| fixture_root().join("dspre/pt_DSPRE_contents"),
        PathBuf::from,
    )
}

fn get_dspre_heartgold_root() -> PathBuf {
    std::env::var("DSPRE_HEARTGOLD_ROOT").map_or_else(
        |_| fixture_root().join("dspre/hg_DSPRE_contents"),
        PathBuf::from,
    )
}

fn get_dspre_platinum_scripts_dir() -> PathBuf {
    get_dspre_platinum_root().join("expanded/scripts")
}

fn get_dspre_platinum_binaries_dir() -> PathBuf {
    get_dspre_platinum_root().join("unpacked/scripts")
}

fn get_dspre_heartgold_scripts_dir() -> PathBuf {
    get_dspre_heartgold_root().join("expanded/scripts")
}

fn get_dspre_heartgold_binaries_dir() -> PathBuf {
    get_dspre_heartgold_root().join("unpacked/scripts")
}

fn get_pokeheartgold_scripts_dir() -> PathBuf {
    get_pokeheartgold_root().join("files/fielddata/script/scr_seq")
}

fn find_dspre_script_files(dir: &Path) -> Vec<PathBuf> {
    let mut scripts: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|_| fixture_panic(dir, "DSPRE scripts"))
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "script"))
        .collect();
    scripts.sort();
    scripts
}

fn find_dspre_binary_files(dir: &Path) -> Vec<PathBuf> {
    let mut binaries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|_| fixture_panic(dir, "DSPRE binaries"))
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_none())
        .collect();
    binaries.sort();
    binaries
}

fn find_heartgold_scripts() -> Vec<PathBuf> {
    let scripts_dir = get_pokeheartgold_scripts_dir();
    let mut scripts: Vec<PathBuf> = std::fs::read_dir(&scripts_dir)
        .unwrap_or_else(|_| fixture_panic(&scripts_dir, "HeartGold scripts"))
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "s"))
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
    let db =
        DatabaseV2::load(DatabaseV2::test_platinum_path()).expect("Failed to load test database");

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

fn load_platinum_db_and_constants() -> (DatabaseV2, ConstantDb) {
    let db = DatabaseV2::load(DatabaseV2::test_platinum_path())
        .expect("Failed to load Platinum database");
    let mut constants = ConstantDb::new();
    constants.load_from_db(&db);
    constants
        .load_directory(DatabaseV2::test_db_root())
        .expect("Failed to load Platinum constants directory");
    let dspre_root = get_dspre_platinum_root();
    if dspre_root.is_dir() {
        let language = RomHeader::open(&dspre_root)
            .map(|h| h.detect_language())
            .unwrap_or(GameLanguage::English);
        let _ = constants.load_dspre_text_archives(&dspre_root, language);
    }
    (db, constants)
}

fn load_heartgold_db_and_constants() -> (DatabaseV2, ConstantDb) {
    let db =
        DatabaseV2::load(DatabaseV2::test_hgss_path()).expect("Failed to load HeartGold database");
    let mut constants = ConstantDb::new();
    constants.load_from_db(&db);
    constants
        .load_directory(DatabaseV2::test_db_root().join("hgss"))
        .expect("Failed to load HeartGold constants directory");
    let dspre_root = get_dspre_heartgold_root();
    if dspre_root.is_dir() {
        let language = RomHeader::open(&dspre_root)
            .map(|h| h.detect_language())
            .unwrap_or(GameLanguage::English);
        let _ = constants.load_dspre_text_archives(&dspre_root, language);
    }
    (db, constants)
}

fn clone_map_events(
    base_constants: &ConstantDb,
    decomp_root: &Path,
    script_path: &Path,
) -> ConstantDb {
    let mut constants = base_constants.clone();
    let _ = constants.load_map_events(decomp_root, script_path);
    constants
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

    let decomp_root = get_pokeplatinum_root();
    let constants = clone_map_events(base_constants, &decomp_root, script_path);

    let is_levelscript = is_levelscript_source(&source);

    let actual_bytes = if is_levelscript {
        match compile_levelscript_to_bytes(&source, &constants) {
            Ok(b) => b,
            Err(e) => return CompileOutcome::CompileError(format!("{:?}", e)),
        }
    } else {
        let transpile_result = match transpile_decomp(&source, Some(db)) {
            Ok(result) => result,
            Err(e) => {
                return CompileOutcome::CompileError(format!("Decomp transpile error: {}", e));
            }
        };

        match compile_to_bytes_with_options(
            &transpile_result.source,
            db,
            &constants,
            transpile_result.jump_table_end_marker_count,
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

fn round_trip_single_binary(
    binary_path: &Path,
    db: &DatabaseV2,
    constants: &ConstantDb,
) -> CompileOutcome {
    let original_bytes = match std::fs::read(binary_path) {
        Ok(b) => b,
        Err(e) => return CompileOutcome::IoError(format!("{}", e)),
    };
    let expected_size = original_bytes.len();
    let expected_hash = sha256_hex(&original_bytes);

    let ir = match rotom::decompile_to_ir(original_bytes, db) {
        Ok(ir) => ir,
        Err(e) => return CompileOutcome::CompileError(format!("Decompile failed: {:?}", e)),
    };
    let source = ir_to_source(&ir, db);

    let actual_bytes = match &ir {
        ScriptOutput::Levelscript(_) => match compile_levelscript_json_to_bytes(&source) {
            Ok(b) => b,
            Err(e) => {
                return CompileOutcome::CompileError(format!(
                    "Recompile failed (levelscript): {:?}",
                    e
                ));
            }
        },
        ScriptOutput::Normal {
            jump_table_end_marker_count,
            ..
        } => {
            // Round-trip through an actual file to ensure source text round-trips
            let temp_path = std::env::temp_dir().join(format!(
                "rotom_roundtrip_{}.rotom",
                binary_path.file_stem().unwrap().to_string_lossy()
            ));
            if let Err(e) = std::fs::write(&temp_path, &source) {
                return CompileOutcome::IoError(format!(
                    "Failed to write round-trip temp file: {}",
                    e
                ));
            }
            let source_from_disk = match std::fs::read_to_string(&temp_path) {
                Ok(s) => s,
                Err(e) => {
                    return CompileOutcome::IoError(format!(
                        "Failed to read round-trip temp file: {}",
                        e
                    ));
                }
            };
            match compile_to_bytes_with_options(
                &source_from_disk,
                db,
                constants,
                *jump_table_end_marker_count,
            ) {
                Ok(b) => b,
                Err(e) => {
                    return CompileOutcome::CompileError(format!("Recompile failed: {:?}", e));
                }
            }
        }
    };

    // 4. Compare hashes
    let actual_hash = sha256_hex(&actual_bytes);
    if actual_hash == expected_hash {
        CompileOutcome::Match
    } else {
        CompileOutcome::HashMismatch {
            expected_hash,
            actual_hash,
            expected_size,
            actual_size: actual_bytes.len(),
        }
    }
}

fn compile_dspre_script(
    script_path: &Path,
    db: &DatabaseV2,
    constants: &ConstantDb,
) -> CompileOutcome {
    // 1. Read DSPRE source
    let source = match std::fs::read_to_string(script_path) {
        Ok(s) => s,
        Err(e) => return CompileOutcome::IoError(format!("{}", e)),
    };

    // 2. Transpile DSPRE to rotom
    let rotom_source = transpile_dspre(&source, Some(db));

    // 3. Compile to binary (no expected hash - just check if it compiles)
    match compile_to_bytes(&rotom_source, db, constants) {
        Ok(_) => CompileOutcome::Match, // "Match" means successful compile
        Err(e) => CompileOutcome::CompileError(format!("{:?}", e)),
    }
}

fn bulk_compile_scripts(
    scripts: &[PathBuf],
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

fn bulk_round_trip_binaries(
    binaries: &[PathBuf],
    db: &DatabaseV2,
    constants: &ConstantDb,
) -> BulkCompileResult {
    let result = BulkCompileResult {
        stats: BulkCompileStats {
            total: binaries.len(),
            ..Default::default()
        },
        outcomes: Mutex::new(HashMap::new()),
    };

    binaries.par_iter().for_each(|binary_path| {
        let binary_name = binary_path
            .file_name()
            .unwrap_or_default()
            .to_str()
            .unwrap_or("")
            .to_string();
        let outcome = round_trip_single_binary(binary_path, db, constants);

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

        result.outcomes.lock().unwrap().insert(binary_name, outcome);
    });

    result
}

fn bulk_compile_dspre_scripts(
    scripts: &[PathBuf],
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
        let outcome = compile_dspre_script(script_path, db, constants);

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

fn to_f64_count(count: u64) -> f64 {
    f64::from(u32::try_from(count).expect("bulk test counts should fit in u32"))
}

fn bulk_failure_print_limit() -> usize {
    const DEFAULT_LIMIT: usize = 50;
    std::env::var("BULK_FAILURE_PRINT_LIMIT")
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .filter(|&limit| limit > 0)
        .unwrap_or(DEFAULT_LIMIT)
}

fn print_bulk_compile_report(name: &str, result: &BulkCompileResult, verbose: bool) {
    let stats = &result.stats;
    let total = stats.total as u64;
    let total_f = to_f64_count(total);

    let matches = stats.matches.load(Ordering::Relaxed) as u64;
    let hash_mismatches = stats.hash_mismatches.load(Ordering::Relaxed) as u64;
    let compile_errors = stats.compile_errors.load(Ordering::Relaxed) as u64;
    let missing_binaries = stats.missing_binaries.load(Ordering::Relaxed) as u64;
    let io_errors = stats.io_errors.load(Ordering::Relaxed) as u64;

    println!();
    println!("============================================================");
    println!("BULK COMPILE REPORT: {}", name);
    println!("============================================================");
    println!("Total scripts:        {}", stats.total);
    println!("------------------------------------------------------------");
    println!(
        "Matches:              {:>4} ({:>5.1}%)",
        matches,
        100.0 * to_f64_count(matches) / total_f
    );
    println!(
        "Hash mismatches:      {:>4} ({:>5.1}%)",
        hash_mismatches,
        100.0 * to_f64_count(hash_mismatches) / total_f
    );
    println!(
        "Compile errors:       {:>4} ({:>5.1}%)",
        compile_errors,
        100.0 * to_f64_count(compile_errors) / total_f
    );
    println!(
        "Missing binaries:     {:>4} ({:>5.1}%)",
        missing_binaries,
        100.0 * to_f64_count(missing_binaries) / total_f
    );
    println!(
        "IO errors:            {:>4} ({:>5.1}%)",
        io_errors,
        100.0 * to_f64_count(io_errors) / total_f
    );
    println!("============================================================");

    if verbose {
        let outcomes = result.outcomes.lock().unwrap();

        let mut failures: Vec<_> = outcomes
            .iter()
            .filter(|(_, o)| !matches!(o, CompileOutcome::Match))
            .collect();
        failures.sort_by_key(|(name, _)| *name);
        let print_limit = bulk_failure_print_limit();

        if !failures.is_empty() {
            println!();
            println!("Failures (first {}):", print_limit);
            println!("------------------------------------------------------------");
            for (name, outcome) in failures.iter().take(print_limit) {
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
                        println!("  {} - COMPILE ERROR: {}", name, msg);
                    }
                    CompileOutcome::MissingExpectedBinary(path) => {
                        println!("  {} - MISSING BINARY: {}", name, path.display());
                    }
                    CompileOutcome::IoError(msg) => {
                        println!("  {} - IO ERROR: {}", name, msg);
                    }
                }
            }
            if failures.len() > print_limit {
                println!("  ... and {} more failures", failures.len() - print_limit);
            }

            let mut compile_error_categories: HashMap<String, usize> = HashMap::new();
            for (_, outcome) in &failures {
                if let CompileOutcome::CompileError(msg) = outcome {
                    let category = classify_compile_error(msg);
                    *compile_error_categories.entry(category).or_default() += 1;
                }
            }

            if !compile_error_categories.is_empty() {
                let mut category_counts: Vec<_> = compile_error_categories.into_iter().collect();
                category_counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

                println!();
                println!("Compile error categories:");
                println!("------------------------------------------------------------");
                for (category, count) in category_counts.iter().take(20) {
                    println!("  {:>4}  {}", count, category);
                }
                if category_counts.len() > 20 {
                    println!("  ... and {} more categories", category_counts.len() - 20);
                }
            }
        }
    }
}

fn classify_compile_error(msg: &str) -> String {
    let normalized = msg.trim();

    if normalized.is_empty() {
        return "empty".to_string();
    }

    if let Some(idx) = normalized.find(':') {
        let head = normalized[..idx].trim();
        if !head.is_empty() {
            return head.to_string();
        }
    }

    normalized
        .split_whitespace()
        .take(6)
        .collect::<Vec<_>>()
        .join(" ")
}

fn run_normal_scripts_test(verbose: bool) -> BulkCompileResult {
    let scripts_dir = get_scripts_dir();
    assert!(
        scripts_dir.exists(),
        "Bulk test failed: scripts directory not found at {}. \
             Set POKEPLATINUM_ROOT environment variable to run this test.",
        scripts_dir.display()
    );

    let binaries_dir = get_binaries_dir();
    assert!(
        binaries_dir.exists(),
        "Bulk test failed: binaries directory not found at {}. \
             Make sure you've built the pokeplatinum project first.",
        binaries_dir.display()
    );

    let (db, constants) = load_test_db_and_constants();

    let scripts = find_normal_scripts();
    assert!(!scripts.is_empty(), "No normal scripts found");
    println!("Found {} normal scripts to compile", scripts.len());

    let result = bulk_compile_scripts(&scripts, &db, &constants);

    print_bulk_compile_report("Normal Scripts", &result, verbose);

    result
}

fn run_levelscripts_test(verbose: bool) -> BulkCompileResult {
    let scripts_dir = get_scripts_dir();
    assert!(
        scripts_dir.exists(),
        "Bulk test failed: scripts directory not found at {}. \
             Set POKEPLATINUM_ROOT environment variable to run this test.",
        scripts_dir.display()
    );

    let binaries_dir = get_binaries_dir();
    assert!(
        binaries_dir.exists(),
        "Bulk test failed: binaries directory not found at {}. \
             Make sure you've built the pokeplatinum project first.",
        binaries_dir.display()
    );

    let (db, constants) = load_test_db_and_constants();

    let scripts = find_levelscripts();
    assert!(!scripts.is_empty(), "No levelscripts found");
    println!("Found {} levelscripts to compile", scripts.len());

    let result = bulk_compile_scripts(&scripts, &db, &constants);

    print_bulk_compile_report("Levelscripts", &result, verbose);

    println!();
    println!("NOTE: Levelscript compilation uses the dedicated levelscript compile path.");
    println!("Levelscripts have a different binary format (InitScript* commands).");
    println!(
        "This suite is expected to reach 100% matching once remaining known deltas are resolved."
    );

    result
}

fn percent(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        return 0.0;
    }

    100.0 * to_f64_count(numerator) / to_f64_count(denominator)
}

fn assert_100_percent_match(result: &BulkCompileResult, script_type: &str) {
    let matches = result.stats.matches.load(Ordering::Relaxed) as u64;
    let total = result.stats.total as u64;

    assert_eq!(
        matches,
        total,
        "{} bulk compile requires 100% hash matches. Got {}/{} ({:.1}%)",
        script_type,
        matches,
        total,
        percent(matches, total)
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

// === DSPRE PLATINUM ROUND-TRIP TESTS ===

fn run_dspre_platinum_round_trip_test(verbose: bool) -> BulkCompileResult {
    let binaries_dir = get_dspre_platinum_binaries_dir();
    assert!(
        binaries_dir.exists(),
        "DSPRE Platinum binaries not found at {}",
        binaries_dir.display()
    );

    let (db, constants) = load_platinum_db_and_constants();
    let binaries = find_dspre_binary_files(&binaries_dir);
    println!(
        "Found {} DSPRE Platinum binaries for round-trip",
        binaries.len()
    );

    let result = bulk_round_trip_binaries(&binaries, &db, &constants);
    print_bulk_compile_report("DSPRE Platinum Round-Trip", &result, verbose);
    result
}

#[test]
fn test_dspre_platinum_round_trip() {
    let result = run_dspre_platinum_round_trip_test(false);
    let matches = result.stats.matches.load(Ordering::Relaxed) as u64;
    let total = result.stats.total as u64;
    let rate = percent(matches, total);
    assert!(rate >= 100.0, "Expected 100% match rate, got {:.1}%", rate);
}

#[test]
#[ignore]
fn test_dspre_platinum_round_trip_verbose() {
    let result = run_dspre_platinum_round_trip_test(true);
    let matches = result.stats.matches.load(Ordering::Relaxed) as u64;
    let total = result.stats.total as u64;
    let rate = percent(matches, total);
    assert!(rate >= 100.0, "Expected 100% match rate, got {:.1}%", rate);
}

// === DSPRE PLATINUM COMPILE TESTS ===

fn run_dspre_platinum_compile_test(verbose: bool) -> BulkCompileResult {
    let scripts_dir = get_dspre_platinum_scripts_dir();
    assert!(
        scripts_dir.exists(),
        "DSPRE Platinum scripts not found at {}",
        scripts_dir.display()
    );

    let (db, constants) = load_platinum_db_and_constants();
    let scripts = find_dspre_script_files(&scripts_dir);
    println!("Found {} DSPRE Platinum scripts to compile", scripts.len());

    let result = bulk_compile_dspre_scripts(&scripts, &db, &constants);
    print_bulk_compile_report("DSPRE Platinum Compile", &result, verbose);
    result
}

#[test]
fn test_dspre_platinum_compile() {
    let result = run_dspre_platinum_compile_test(false);
    let matches = result.stats.matches.load(Ordering::Relaxed) as u64;
    let total = result.stats.total as u64;
    let rate = percent(matches, total);
    assert!(
        rate >= 100.0,
        "Expected 100% compile success rate, got {:.1}%",
        rate
    );
}

#[test]
#[ignore]
fn test_dspre_platinum_compile_verbose() {
    let result = run_dspre_platinum_compile_test(true);
    let matches = result.stats.matches.load(Ordering::Relaxed) as u64;
    let total = result.stats.total as u64;
    let rate = percent(matches, total);
    assert!(
        rate >= 100.0,
        "Expected 100% compile success rate, got {:.1}%",
        rate
    );
}

// === DSPRE HEARTGOLD ROUND-TRIP TESTS ===

fn run_dspre_heartgold_round_trip_test(verbose: bool) -> BulkCompileResult {
    let binaries_dir = get_dspre_heartgold_binaries_dir();
    assert!(
        binaries_dir.exists(),
        "DSPRE HeartGold binaries not found at {}",
        binaries_dir.display()
    );

    let (db, constants) = load_heartgold_db_and_constants();
    let binaries = find_dspre_binary_files(&binaries_dir);
    println!(
        "Found {} DSPRE HeartGold binaries for round-trip",
        binaries.len()
    );

    let result = bulk_round_trip_binaries(&binaries, &db, &constants);
    print_bulk_compile_report("DSPRE HeartGold Round-Trip", &result, verbose);
    result
}

#[test]
fn test_dspre_heartgold_round_trip() {
    let result = run_dspre_heartgold_round_trip_test(false);
    let matches = result.stats.matches.load(Ordering::Relaxed) as u64;
    let total = result.stats.total as u64;
    let rate = percent(matches, total);
    assert!(rate >= 100.0, "Expected 100% match rate, got {:.1}%", rate);
}

#[test]
#[ignore]
fn test_dspre_heartgold_round_trip_verbose() {
    let result = run_dspre_heartgold_round_trip_test(true);
    let matches = result.stats.matches.load(Ordering::Relaxed) as u64;
    let total = result.stats.total as u64;
    let rate = percent(matches, total);
    assert!(rate >= 100.0, "Expected 100% match rate, got {:.1}%", rate);
}

// === DSPRE HEARTGOLD COMPILE TESTS ===

fn run_dspre_heartgold_compile_test(verbose: bool) -> BulkCompileResult {
    let scripts_dir = get_dspre_heartgold_scripts_dir();
    assert!(
        scripts_dir.exists(),
        "DSPRE HeartGold scripts not found at {}",
        scripts_dir.display()
    );

    let (db, constants) = load_heartgold_db_and_constants();
    let scripts = find_dspre_script_files(&scripts_dir);
    println!("Found {} DSPRE HeartGold scripts to compile", scripts.len());

    let result = bulk_compile_dspre_scripts(&scripts, &db, &constants);
    print_bulk_compile_report("DSPRE HeartGold Compile", &result, verbose);
    result
}

#[test]
fn test_dspre_heartgold_compile() {
    let result = run_dspre_heartgold_compile_test(false);
    let matches = result.stats.matches.load(Ordering::Relaxed) as u64;
    let total = result.stats.total as u64;
    let rate = percent(matches, total);
    assert!(
        rate >= 100.0,
        "Expected 100% compile success rate, got {:.1}%",
        rate
    );
}

#[test]
#[ignore]
fn test_dspre_heartgold_compile_verbose() {
    let result = run_dspre_heartgold_compile_test(true);
    let matches = result.stats.matches.load(Ordering::Relaxed) as u64;
    let total = result.stats.total as u64;
    let rate = percent(matches, total);
    assert!(
        rate >= 100.0,
        "Expected 100% compile success rate, got {:.1}%",
        rate
    );
}

// === HEARTGOLD DECOMP COMPILE TESTS ===

fn load_heartgold_decomp_db_and_constants() -> (DatabaseV2, ConstantDb) {
    let db =
        DatabaseV2::load(DatabaseV2::test_hgss_path()).expect("Failed to load HeartGold database");

    let mut constants = ConstantDb::new();
    constants.load_from_db(&db);

    let decomp_root = get_pokeheartgold_root();
    if decomp_root.exists() {
        constants
            .load_decomp_project(&decomp_root)
            .expect("Failed to load HeartGold decomp constants");
    }

    (db, constants)
}

#[test]
fn test_clone_map_events_preserves_base_when_script_has_no_map_events() {
    let base_constants = ConstantDb::new();
    let script_path = Path::new("not_a_map_script.s");
    let decomp_root = Path::new(".");

    let cloned_constants = clone_map_events(&base_constants, decomp_root, script_path);

    assert_eq!(
        base_constants.len(),
        cloned_constants.len(),
        "loading map events for non-map scripts should not mutate or expand the cloned constants"
    );
    assert_eq!(
        base_constants.get("VAR_RESULT"),
        cloned_constants.get("VAR_RESULT"),
        "base and cloned constants should remain semantically identical for non-map scripts"
    );
}

#[test]
fn test_clone_map_events_does_not_mutate_base_constants() {
    let (_, base_constants) = load_platinum_db_and_constants();
    let original_len = base_constants.len();

    let script_path = Path::new("scripts_test_map.s");
    let decomp_root = Path::new(".");

    let cloned_constants = clone_map_events(&base_constants, decomp_root, script_path);

    assert_eq!(
        base_constants.len(),
        original_len,
        "base constants should remain unchanged after cloning + map event loading"
    );
    assert_eq!(
        cloned_constants.get("TRUE"),
        base_constants.get("TRUE"),
        "cloned constants should retain base constants even when no map events are loaded"
    );
}

fn compile_heartgold_single_script(
    script_path: &Path,
    db: &DatabaseV2,
    base_constants: &ConstantDb,
) -> CompileOutcome {
    let source = match std::fs::read_to_string(script_path) {
        Ok(s) => s,
        Err(e) => return CompileOutcome::IoError(format!("{}", e)),
    };

    // Check for levelscript
    let is_levelscript = is_levelscript_source(&source);

    let decomp_root = get_pokeheartgold_root();
    let constants = clone_map_events(base_constants, &decomp_root, script_path);

    if is_levelscript {
        match compile_levelscript_to_bytes(&source, &constants) {
            Ok(_) => CompileOutcome::Match, // Compiled successfully
            Err(e) => CompileOutcome::CompileError(format!("{:?}", e)),
        }
    } else {
        let transpile_result = match transpile_decomp(&source, Some(db)) {
            Ok(result) => result,
            Err(e) => {
                return CompileOutcome::CompileError(format!("Decomp transpile error: {}", e));
            }
        };
        match compile_to_bytes_with_options(
            &transpile_result.source,
            db,
            &constants,
            transpile_result.jump_table_end_marker_count,
        ) {
            Ok(_) => CompileOutcome::Match, // Compiled successfully
            Err(e) => CompileOutcome::CompileError(format!("{:?}", e)),
        }
    }
}

fn bulk_compile_heartgold_scripts(
    scripts: &[PathBuf],
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
        let outcome = compile_heartgold_single_script(script_path, db, constants);

        match &outcome {
            CompileOutcome::Match => {
                result.stats.matches.fetch_add(1, Ordering::Relaxed);
            }
            CompileOutcome::CompileError(_) => {
                result.stats.compile_errors.fetch_add(1, Ordering::Relaxed);
            }
            CompileOutcome::IoError(_) => {
                result.stats.io_errors.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }

        result.outcomes.lock().unwrap().insert(script_name, outcome);
    });

    result
}

fn run_heartgold_scripts_test(verbose: bool) -> BulkCompileResult {
    let scripts_dir = get_pokeheartgold_scripts_dir();
    assert!(
        scripts_dir.exists(),
        "HeartGold scripts directory not found at {}. Set POKEHEARTGOLD_ROOT env var.",
        scripts_dir.display()
    );

    let (db, constants) = load_heartgold_decomp_db_and_constants();
    let scripts = find_heartgold_scripts();
    println!(
        "Found {} HeartGold decomp scripts to compile",
        scripts.len()
    );

    let result = bulk_compile_heartgold_scripts(&scripts, &db, &constants);
    print_bulk_compile_report("HeartGold Decomp Scripts", &result, verbose);
    result
}

#[test]
fn test_bulk_compile_heartgold_scripts() {
    let result = run_heartgold_scripts_test(false);
    // Just report stats, don't fail - no reference binaries exist
    let matches = result.stats.matches.load(Ordering::Relaxed) as u64;
    let total = result.stats.total as u64;
    println!(
        "HeartGold decomp compile: {}/{} ({:.1}%)",
        matches,
        total,
        percent(matches, total)
    );
}

#[test]
#[ignore]
fn test_bulk_compile_heartgold_scripts_verbose() {
    let result = run_heartgold_scripts_test(true);
    let matches = result.stats.matches.load(Ordering::Relaxed) as u64;
    let total = result.stats.total as u64;
    println!(
        "HeartGold decomp compile: {}/{} ({:.1}%)",
        matches,
        total,
        percent(matches, total)
    );
}
