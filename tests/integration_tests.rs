//! Integration tests for the Rotom compiler pipeline
//!
//! These tests verify that compiling decomp scripts produces binaries
//! that match the known-good hashes from the decomp build.
//!
//! The tests require the POKEPLATINUM_ROOT environment variable to be set
//! to the path of the pokeplatinum decomp project for loading constants.

use sha2::{Sha256, Digest};
use std::path::Path;

// Re-export what we need from the main crate
use rotom::compiler::{Lexer, Parser, Analyzer, Lowerer};
use rotom::Emitter;
use rotom::database::{DatabaseV2, ConstantDb};
use rotom::transpiler::decomp::transpile as transpile_decomp;

/// Compute SHA256 hash of data as hex string
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    hex::encode(result)
}

/// Compile a rotoscript source to binary bytes
fn compile_to_binary(
    source: &str,
    db: &DatabaseV2,
    constants: &ConstantDb,
) -> Result<Vec<u8>, String> {
    // Parse
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let file = parser.parse_script_file().map_err(|e| format!("Parse error: {}", e))?;

    // Analyze
    let mut analyzer = Analyzer::with_constants(constants);
    analyzer.analyze(&file).map_err(|e| format!("Analysis error: {}", e))?;

    // Lower to IR
    let mut lowerer = Lowerer::new(&analyzer.symbols, db);
    let items = lowerer.lower_script_file(&file).map_err(|e| format!("Lowering error: {}", e))?;

    // Emit binary
    let mut emitter = Emitter::new(db);
    let binary = emitter.emit_script_file(&items).map_err(|e| format!("Codegen error: {}", e))?;

    Ok(binary)
}

/// Load database and constants for testing
fn load_test_db_and_constants() -> (DatabaseV2, ConstantDb) {
    let db = DatabaseV2::load(Path::new("src/db/platinum_v2.json"))
        .expect("Failed to load test database");

    let mut constants = ConstantDb::new();
    constants.load_from_db(&db);

    // Try to load decomp constants if POKEPLATINUM_ROOT is set
    if let Ok(decomp_root) = std::env::var("POKEPLATINUM_ROOT") {
        let decomp_path = Path::new(&decomp_root);
        if decomp_path.exists() {
            constants.load_decomp_project(decomp_path).expect("Failed to load decomp project constants");
        }
    }

    (db, constants)
}

/// Test helper that compiles a decomp script fixture and compares hash
fn test_script_hash(script_name: &str) {
    // Load the decomp script
    let script_path = format!("tests/fixtures/scripts/{}.s", script_name);
    let decomp_source = std::fs::read_to_string(&script_path)
        .expect(&format!("Failed to read fixture: {}", script_path));

    // Load expected hash
    let hash_path = format!("tests/fixtures/expected/{}.sha256", script_name);
    let expected_hash = std::fs::read_to_string(&hash_path)
        .expect(&format!("Failed to read expected hash: {}", hash_path))
        .trim()
        .to_string();

    // Load database and constants
    let (db, constants) = load_test_db_and_constants();

    // Transpile decomp script to rotoscript
    let rotoscript = transpile_decomp(&decomp_source);

    if std::env::var("ROTOM_DUMP").is_ok() {
        println!("--- Rotoscript for {} ---\n{}\n---", script_name, rotoscript);
    }

    // Compile to binary
    let binary = compile_to_binary(&rotoscript, &db, &constants)
        .expect(&format!("Failed to compile {}", script_name));

    // Compute hash and compare
    let actual_hash = sha256_hex(&binary);
    
    if actual_hash != expected_hash {
        let actual_path = format!("tests/fixtures/actual_{}.bin", script_name);
        std::fs::write(&actual_path, &binary).unwrap();
        println!("Wrote actual binary to {}", actual_path);
    }
    
    assert_eq!(
        actual_hash, expected_hash,
        "Hash mismatch for {}.\n\nExpected: {}\nActual:   {}\n\nBinary length: {} bytes",
        script_name, expected_hash, actual_hash, binary.len()
    );
}

#[test]
fn test_compile_acuity_lakefront_matches_decomp() {
    test_script_hash("scripts_acuity_lakefront");
}

#[test]
fn test_compile_oreburgh_mine_b1f_matches_decomp() {
    test_script_hash("scripts_oreburgh_mine_b1f");
}

#[test]
fn test_compile_common_matches_decomp() {
    test_script_hash("scripts_common");
}

#[test]
fn test_compile_pokemon_center_daily_trainers_matches_decomp() {
    test_script_hash("scripts_pokemon_center_daily_trainers");
}

#[test]
fn test_compile_unk_0406_matches_decomp() {
    test_script_hash("scripts_unk_0406");
}
