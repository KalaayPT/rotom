//! Full lifecycle integration tests: `rotom init` → `rotom convert` → `rotom compile`
//!
//! One test per game. Compile output must match known hashes (decomp: vendored
//! manifest; DSPRE: snapshot of binary folder taken before compile).
//!
//! ## Fixtures
//! Decomp repos are auto-cloned on first run via `ensure_decomp_fixtures`.
//! DSPRE fixtures must be present manually; tests are silently skipped otherwise.
//!
//! ## Verbose mode
//! Each game has an `#[ignore]`-gated verbose variant. Run with:
//! ```bash
//! cargo test <game>_verbose --ignored -- --nocapture
//! ```
//! The verbose variant uses `rotom compile --json` to capture per-file results
//! and prints a full breakdown of compile errors and hash mismatches.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use sha2::{Digest, Sha256};

mod common;
mod known_hashes;
use common::fixture_setup::{
    dspre_fixture_root, ensure_decomp_fixtures, fixture_root, persistent_test_dir,
};

// ── CLI helpers ────────────────────────────────────────────────────────────────

fn rotom_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rotom"))
}

fn run_rotom(args: &[&str], cwd: &Path) -> Result<String, String> {
    let mut cmd = rotom_bin();
    cmd.args(args);
    cmd.current_dir(cwd);
    // Isolate the per-project database cache so parallel tests don't race on
    // the shared default directory (~/.local/share/rotom/databases).
    let cache_dir = cwd.file_name().map_or_else(
        || {
            std::env::temp_dir()
                .join("rotom_test_cache")
                .join("default")
        },
        |n| std::env::temp_dir().join("rotom_test_cache").join(n),
    );
    cmd.env("XDG_DATA_HOME", &cache_dir);
    let output = cmd
        .output()
        .map_err(|e| format!("Failed to run rotom: {}", e))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        return Err(format!(
            "rotom {} exited with {}\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            output.status,
            stdout,
            stderr
        ));
    }
    Ok(stdout)
}

fn copy_fixture(src: &Path, dst: &Path) {
    let status = Command::new("cp")
        .arg("-r")
        .arg(format!("{}/.", src.display()))
        .arg(dst)
        .status()
        .expect("cp -r failed — is this a Unix system?");
    assert!(status.success(), "cp -r exited with failure");
}

// ── Hashing helpers ────────────────────────────────────────────────────────────

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Snapshot SHA256 of every regular file in `dir` (non-recursive).
/// Keys are filename stems (extension stripped); used for DSPRE round-trip.
fn snapshot_dir_hashes(dir: &Path) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for entry in std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("Cannot read {}: {}", dir.display(), e))
        .flatten()
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let stem = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let bytes = std::fs::read(&path).expect("read failed");
        map.insert(stem, sha256_hex(&bytes));
    }
    map
}

/// Parse a `sha256  rel/path` manifest into `stem → hash`.
fn parse_manifest_by_stem(manifest: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in manifest.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Some((hash, path)) = line.split_once("  ")
            && let Some(stem) = Path::new(path).file_stem()
        {
            map.insert(stem.to_string_lossy().to_string(), hash.to_string());
        }
    }
    map
}

/// Walk `binary_root` and assert every file's SHA256 matches `expected_by_stem`.
///
/// `binary_ext`:
/// - `None`        → only visit extensionless files (Platinum decomp, DSPRE)
/// - `Some("bin")` → only visit `.bin` files (HeartGold decomp)
fn verify_compiled_binaries(
    binary_root: &Path,
    expected_by_stem: &HashMap<String, String>,
    binary_ext: Option<&str>,
) {
    assert!(
        binary_root.exists(),
        "Binary output directory missing: {}",
        binary_root.display()
    );

    let mut actual: HashMap<String, String> = HashMap::new();
    for entry in std::fs::read_dir(binary_root)
        .expect("read_dir failed")
        .flatten()
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let has_expected_ext = match binary_ext {
            Some(ext) => path.extension().is_some_and(|e| e == ext),
            None => path.extension().is_none(),
        };
        if !has_expected_ext {
            continue;
        }
        let stem = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let bytes = std::fs::read(&path).expect("read binary failed");
        actual.insert(stem, sha256_hex(&bytes));
    }

    let mut failures: Vec<String> = Vec::new();
    for (stem, expected_hash) in expected_by_stem {
        match actual.get(stem) {
            Some(actual_hash) if actual_hash == expected_hash => {}
            Some(actual_hash) => failures.push(format!(
                "MISMATCH {}: expected {}..., got {}...",
                stem,
                &expected_hash[..8],
                &actual_hash[..8],
            )),
            None => failures.push(format!("MISSING: {}", stem)),
        }
    }
    for stem in actual.keys() {
        if !expected_by_stem.contains_key(stem) {
            failures.push(format!("UNEXPECTED: {}", stem));
        }
    }

    assert!(
        failures.is_empty(),
        "{}/{} files matched. Failures:\n{}",
        expected_by_stem.len().saturating_sub(failures.len()),
        expected_by_stem.len(),
        failures.join("\n"),
    );
}

// ── Verbose diagnostics ────────────────────────────────────────────────────────

/// Run rotom without asserting success. Returns `(exit_ok, stdout, stderr)`.
fn run_rotom_raw(args: &[&str], cwd: &Path) -> (bool, String, String) {
    let mut cmd = rotom_bin();
    cmd.args(args);
    cmd.current_dir(cwd);
    let cache_dir = cwd.file_name().map_or_else(
        || {
            std::env::temp_dir()
                .join("rotom_test_cache")
                .join("default")
        },
        |n| std::env::temp_dir().join("rotom_test_cache").join(n),
    );
    cmd.env("XDG_DATA_HOME", &cache_dir);
    let output = cmd.output().expect("Failed to spawn rotom");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// Parse `rotom compile --json` stdout and print a human-readable report.
/// Returns `(compiled, total)` counts.
fn print_compile_report(json_stdout: &str, game: &str) -> (usize, usize) {
    let v: serde_json::Value = match serde_json::from_str(json_stdout) {
        Ok(v) => v,
        Err(e) => {
            println!("[{game}] Could not parse compile JSON: {e}");
            println!("{json_stdout}");
            return (0, 0);
        }
    };

    let successes = v["successes"].as_array().map_or(0, |a| a.len());
    let failures = v["failures"].as_array().map_or(&[][..], |a| a.as_slice());
    let total = successes + failures.len();

    println!();
    println!("╔══════════════════════════════════════════════════════════╗");
    println!("  COMPILE REPORT: {game}");
    println!("╠══════════════════════════════════════════════════════════╣");
    println!("  Compiled:  {successes}/{total}");
    if !failures.is_empty() {
        println!("  Failed:    {}", failures.len());
    }
    println!("╚══════════════════════════════════════════════════════════╝");

    if !failures.is_empty() {
        println!();
        println!("Compile failures:");
        println!("──────────────────────────────────────────────────────────");
        let limit = std::env::var("VERBOSE_FAILURE_LIMIT")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(50);
        for failure in failures.iter().take(limit) {
            let path = failure["path"].as_str().unwrap_or("?");
            let name = std::path::Path::new(path)
                .file_name()
                .map_or(path, |n| n.to_str().unwrap_or(path));
            let error = failure["error"]["details"]["message"]
                .as_str()
                .or_else(|| failure["error"]["details"].as_str())
                .unwrap_or_else(|| failure["error"]["type"].as_str().unwrap_or("unknown error"));
            let kind = failure["error"]["type"].as_str().unwrap_or("Error");
            println!("  ✗ {name}  [{kind}] {error}");
        }
        if failures.len() > limit {
            println!(
                "  … and {} more (set VERBOSE_FAILURE_LIMIT to raise the cap)",
                failures.len() - limit
            );
        }
    }

    (successes, total)
}

/// Collect hash verification failures without asserting; also prints the report.
fn print_hash_report(
    binary_root: &Path,
    expected_by_stem: &HashMap<String, String>,
    binary_ext: Option<&str>,
    game: &str,
) -> Vec<String> {
    let mut actual: HashMap<String, String> = HashMap::new();
    if binary_root.exists() {
        for entry in std::fs::read_dir(binary_root).expect("read_dir").flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let has_expected_ext = match binary_ext {
                Some(ext) => path.extension().is_some_and(|e| e == ext),
                None => path.extension().is_none(),
            };
            if !has_expected_ext {
                continue;
            }
            let stem = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let bytes = std::fs::read(&path).expect("read binary");
            actual.insert(stem, sha256_hex(&bytes));
        }
    }

    let mut failures: Vec<String> = Vec::new();
    for (stem, expected_hash) in expected_by_stem {
        match actual.get(stem) {
            Some(actual_hash) if actual_hash == expected_hash => {}
            Some(actual_hash) => failures.push(format!(
                "MISMATCH {stem}: expected {expected_hash}, got {actual_hash}",
            )),
            None => failures.push(format!("MISSING: {stem}")),
        }
    }
    for stem in actual.keys() {
        if !expected_by_stem.contains_key(stem) {
            failures.push(format!("UNEXPECTED: {stem}"));
        }
    }

    let matched = expected_by_stem.len().saturating_sub(
        failures
            .iter()
            .filter(|f| !f.starts_with("UNEXPECTED"))
            .count(),
    );
    println!();
    println!("╔══════════════════════════════════════════════════════════╗");
    println!("  HASH REPORT: {game}");
    println!("╠══════════════════════════════════════════════════════════╣");
    println!("  Matched:   {matched}/{}", expected_by_stem.len());
    if !failures.is_empty() {
        println!("  Failures:  {}", failures.len());
    }
    println!("╚══════════════════════════════════════════════════════════╝");

    if !failures.is_empty() {
        println!();
        println!("Hash failures:");
        println!("──────────────────────────────────────────────────────────");
        let limit = std::env::var("VERBOSE_FAILURE_LIMIT")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(50);
        for f in failures.iter().take(limit) {
            println!("  {f}");
        }
        if failures.len() > limit {
            println!("  … and {} more", failures.len() - limit);
        }
    }

    failures
}

// ── Lifecycle tests ────────────────────────────────────────────────────────────

/// Pokeplatinum: init → convert (.s → .rotom/.json) → compile → verify hashes.
///
/// Binary output: extensionless files under `build/res/field/scripts/scr_seq.narc.p/`.
/// Known hashes: vendored SHA-256 manifest of pokeplatinum compiled binaries.
#[test]
fn test_lifecycle_platinum_decomp() {
    ensure_decomp_fixtures();

    let project = persistent_test_dir("lifecycle_platinum_decomp");
    copy_fixture(&fixture_root().join("decomp/pokeplatinum"), &project);

    run_rotom(&["init", "--non-interactive"], &project).expect("rotom init failed");
    run_rotom(&["convert", "--non-interactive"], &project).expect("rotom convert failed");
    run_rotom(&["compile"], &project).expect("rotom compile failed");

    let expected = parse_manifest_by_stem(known_hashes::POKEPLATINUM_FIELD_SCRIPT_HASHES);
    verify_compiled_binaries(
        &project.join("build/res/field/scripts/scr_seq.narc.p"),
        &expected,
        None,
    );
}

/// Pokeheartgold: init → convert (.s → .rotom/.json) → compile → verify hashes.
///
/// Binary output: `.bin` files alongside source in `files/fielddata/script/scr_seq/`.
/// Known hashes: vendored SHA-256 manifest of pokeheartgold compiled binaries.
#[test]
fn test_lifecycle_heartgold_decomp() {
    ensure_decomp_fixtures();

    let project = persistent_test_dir("lifecycle_heartgold_decomp");
    copy_fixture(&fixture_root().join("decomp/pokeheartgold"), &project);

    run_rotom(&["init", "--non-interactive"], &project).expect("rotom init failed");
    run_rotom(&["convert", "--non-interactive"], &project).expect("rotom convert failed");
    run_rotom(&["compile"], &project).expect("rotom compile failed");

    let expected = parse_manifest_by_stem(known_hashes::POKEHEARTGOLD_FIELD_SCRIPT_HASHES);
    verify_compiled_binaries(
        &project.join("files/fielddata/script/scr_seq"),
        &expected,
        Some("bin"),
    );
}

/// DSPRE Platinum: snapshot binary hashes → init → convert (.script → .rotom/.json)
/// → compile → verify compiled output matches pre-compile snapshot.
///
/// Tests that the round-trip (binary → source → binary) is lossless.
/// Skipped when the DSPRE fixture tree is absent.
#[test]
fn test_lifecycle_platinum_dspre() {
    let Some(fixture) = dspre_fixture_root("pt") else {
        eprintln!("Skipping test_lifecycle_platinum_dspre: DSPRE Platinum fixture not found");
        return;
    };

    let project = persistent_test_dir("lifecycle_platinum_dspre");
    copy_fixture(&fixture, &project);

    let binary_root = project.join("unpacked/scripts");
    let expected = snapshot_dir_hashes(&binary_root);

    run_rotom(&["init", "--non-interactive"], &project).expect("rotom init failed");
    run_rotom(&["convert", "--non-interactive"], &project).expect("rotom convert failed");
    run_rotom(&["compile"], &project).expect("rotom compile failed");

    verify_compiled_binaries(&binary_root, &expected, None);
}

/// DSPRE HeartGold: snapshot binary hashes → init → convert (.script → .rotom/.json)
/// → compile → verify compiled output matches pre-compile snapshot.
///
/// Tests that the round-trip (binary → source → binary) is lossless.
/// Skipped when the DSPRE fixture tree is absent.
#[test]
fn test_lifecycle_heartgold_dspre() {
    let Some(fixture) = dspre_fixture_root("hg") else {
        eprintln!("Skipping test_lifecycle_heartgold_dspre: DSPRE HeartGold fixture not found");
        return;
    };

    let project = persistent_test_dir("lifecycle_heartgold_dspre");
    copy_fixture(&fixture, &project);

    let binary_root = project.join("unpacked/scripts");
    let expected = snapshot_dir_hashes(&binary_root);

    run_rotom(&["init", "--non-interactive"], &project).expect("rotom init failed");
    run_rotom(&["convert", "--non-interactive"], &project).expect("rotom convert failed");
    run_rotom(&["compile"], &project).expect("rotom compile failed");

    verify_compiled_binaries(&binary_root, &expected, None);
}

// ── Verbose variants (opt-in with --ignored --nocapture) ──────────────────────

#[test]
#[ignore = "verbose diagnostics are opt-in: cargo test platinum_decomp_verbose -- --ignored --nocapture"]
fn test_lifecycle_platinum_decomp_verbose() {
    ensure_decomp_fixtures();

    let project = persistent_test_dir("lifecycle_platinum_decomp_verbose");
    copy_fixture(&fixture_root().join("decomp/pokeplatinum"), &project);

    run_rotom(&["init", "--non-interactive"], &project).expect("rotom init failed");
    run_rotom(&["convert", "--non-interactive"], &project).expect("rotom convert failed");

    let (compile_ok, json_stdout, compile_stderr) = run_rotom_raw(&["compile", "--json"], &project);
    let (compiled, total) = print_compile_report(&json_stdout, "Platinum Decomp");
    if !compile_stderr.is_empty() {
        println!("\nCompile stderr:\n{compile_stderr}");
    }

    let expected = parse_manifest_by_stem(known_hashes::POKEPLATINUM_FIELD_SCRIPT_HASHES);
    let hash_failures = print_hash_report(
        &project.join("build/res/field/scripts/scr_seq.narc.p"),
        &expected,
        None,
        "Platinum Decomp",
    );

    assert!(
        compile_ok && hash_failures.is_empty(),
        "{compiled}/{total} compiled; {}/{} hashes matched",
        expected.len().saturating_sub(hash_failures.len()),
        expected.len(),
    );
}

#[test]
#[ignore = "verbose diagnostics are opt-in: cargo test heartgold_decomp_verbose -- --ignored --nocapture"]
fn test_lifecycle_heartgold_decomp_verbose() {
    ensure_decomp_fixtures();

    let project = persistent_test_dir("lifecycle_heartgold_decomp_verbose");
    copy_fixture(&fixture_root().join("decomp/pokeheartgold"), &project);

    run_rotom(&["init", "--non-interactive"], &project).expect("rotom init failed");
    run_rotom(&["convert", "--non-interactive"], &project).expect("rotom convert failed");

    let (compile_ok, json_stdout, compile_stderr) = run_rotom_raw(&["compile", "--json"], &project);
    let (compiled, total) = print_compile_report(&json_stdout, "HeartGold Decomp");
    if !compile_stderr.is_empty() {
        println!("\nCompile stderr:\n{compile_stderr}");
    }

    let expected = parse_manifest_by_stem(known_hashes::POKEHEARTGOLD_FIELD_SCRIPT_HASHES);
    let hash_failures = print_hash_report(
        &project.join("files/fielddata/script/scr_seq"),
        &expected,
        Some("bin"),
        "HeartGold Decomp",
    );

    assert!(
        compile_ok && hash_failures.is_empty(),
        "{compiled}/{total} compiled; {}/{} hashes matched",
        expected.len().saturating_sub(hash_failures.len()),
        expected.len(),
    );
}

#[test]
#[ignore = "verbose diagnostics are opt-in: cargo test platinum_dspre_verbose -- --ignored --nocapture"]
fn test_lifecycle_platinum_dspre_verbose() {
    let Some(fixture) = dspre_fixture_root("pt") else {
        eprintln!("Skipping: DSPRE Platinum fixture not found");
        return;
    };

    let project = persistent_test_dir("lifecycle_platinum_dspre_verbose");
    copy_fixture(&fixture, &project);

    let binary_root = project.join("unpacked/scripts");
    let expected = snapshot_dir_hashes(&binary_root);

    run_rotom(&["init", "--non-interactive"], &project).expect("rotom init failed");
    run_rotom(&["convert", "--non-interactive"], &project).expect("rotom convert failed");

    let (compile_ok, json_stdout, compile_stderr) = run_rotom_raw(&["compile", "--json"], &project);
    let (compiled, total) = print_compile_report(&json_stdout, "Platinum DSPRE");
    if !compile_stderr.is_empty() {
        println!("\nCompile stderr:\n{compile_stderr}");
    }

    let hash_failures = print_hash_report(&binary_root, &expected, None, "Platinum DSPRE");

    assert!(
        compile_ok && hash_failures.is_empty(),
        "{compiled}/{total} compiled; {}/{} hashes matched",
        expected.len().saturating_sub(hash_failures.len()),
        expected.len(),
    );
}

#[test]
#[ignore = "verbose diagnostics are opt-in: cargo test heartgold_dspre_verbose -- --ignored --nocapture"]
fn test_lifecycle_heartgold_dspre_verbose() {
    let Some(fixture) = dspre_fixture_root("hg") else {
        eprintln!("Skipping: DSPRE HeartGold fixture not found");
        return;
    };

    let project = persistent_test_dir("lifecycle_heartgold_dspre_verbose");
    copy_fixture(&fixture, &project);

    let binary_root = project.join("unpacked/scripts");
    let expected = snapshot_dir_hashes(&binary_root);

    run_rotom(&["init", "--non-interactive"], &project).expect("rotom init failed");
    run_rotom(&["convert", "--non-interactive"], &project).expect("rotom convert failed");

    let (compile_ok, json_stdout, compile_stderr) = run_rotom_raw(&["compile", "--json"], &project);
    let (compiled, total) = print_compile_report(&json_stdout, "HeartGold DSPRE");
    if !compile_stderr.is_empty() {
        println!("\nCompile stderr:\n{compile_stderr}");
    }

    let hash_failures = print_hash_report(&binary_root, &expected, None, "HeartGold DSPRE");

    assert!(
        compile_ok && hash_failures.is_empty(),
        "{compiled}/{total} compiled; {}/{} hashes matched",
        expected.len().saturating_sub(hash_failures.len()),
        expected.len(),
    );
}
