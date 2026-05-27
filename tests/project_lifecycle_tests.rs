//! Project lifecycle tests: init → compile → decompile round-trip through the CLI.
//!
//! These tests shell out to the rotom binary to exercise the actual user-facing path.
//! Temp directories are persistent (under `/tmp/rotom_lifecycle_tests/`) so you
//! can inspect rotom/uxie artifacts after a run.

use std::path::PathBuf;
use std::process::Command;

mod common;
use common::fixture_setup::{
    dspre_fixture_root, ensure_decomp_fixtures, fixture_root, persistent_test_dir,
};

fn rotom_bin() -> Command {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    Command::new(manifest_dir.join("target/debug/rotom"))
}

fn run_rotom(args: &[&str], cwd: &std::path::Path) -> Result<String, String> {
    let mut cmd = rotom_bin();
    cmd.args(args);
    cmd.current_dir(cwd);
    // Isolate the per-user database cache so parallel tests don't race on
    // the shared default directory (~/.local/share/rotom/databases).
    // Use the *project* directory name so different test projects get
    // different caches, while repeated calls from the same test reuse it.
    let cache_dir = cwd.file_name().map_or_else(
        || std::env::temp_dir().join("rotom_test_cache").join("default"),
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
            "rotom exited with {}\nstdout:\n{}\nstderr:\n{}",
            output.status, stdout, stderr
        ));
    }
    Ok(stdout)
}

fn copy_decomp(decomp_root: &std::path::Path, project: &std::path::Path) {
    // Use system cp -r for speed. Pure Rust recursive copy is too slow for 200k+ files.
    let status = Command::new("cp")
        .arg("-r")
        .arg(format!("{}/.", decomp_root.display()))
        .arg(project)
        .status()
        .expect("cp -r failed — is this a Unix system?");
    assert!(status.success(), "cp -r exited with failure");
}

fn copy_fixture(fixture: &std::path::Path, project: &std::path::Path) {
    // Use system cp -r for speed. Pure Rust recursive copy is too slow for large fixture trees.
    let status = Command::new("cp")
        .arg("-r")
        .arg(format!("{}/.", fixture.display()))
        .arg(project)
        .status()
        .expect("cp -r failed — is this a Unix system?");
    assert!(status.success(), "cp -r exited with failure");
}

fn remove_files_with_extension(dir: &std::path::Path, extension: &str) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == extension) {
            std::fs::remove_file(&path).unwrap();
        }
    }
}

#[test]
fn test_project_init_platinum() {
    ensure_decomp_fixtures();

    let decomp = fixture_root().join("decomp/pokeplatinum");
    let project = persistent_test_dir("platinum_init");
    copy_decomp(&decomp, &project);

    let out = run_rotom(&["init", "--non-interactive"], &project).unwrap();
    println!("{}", out);

    assert!(project.join("rotom.toml").exists());
    assert!(project.join(".rotom/status").exists());
    assert!(project.join(".rotom/cache").exists());

    println!("Project at {}", project.display());
}

#[test]
fn test_project_compile_platinum() {
    ensure_decomp_fixtures();

    let decomp = fixture_root().join("decomp/pokeplatinum");
    let project = persistent_test_dir("platinum_compile");
    copy_decomp(&decomp, &project);

    run_rotom(&["init", "--non-interactive"], &project).unwrap();
    let out = run_rotom(&["compile"], &project).unwrap();
    println!("{}", out);

    assert!(project.join(".rotom/status/compile-state.json").exists());
    println!("Project at {}", project.display());
}

#[test]
fn test_project_lifecycle_platinum() {
    ensure_decomp_fixtures();

    let decomp = fixture_root().join("decomp/pokeplatinum");
    let project = persistent_test_dir("platinum_lifecycle");
    copy_decomp(&decomp, &project);

    run_rotom(&["init", "--non-interactive"], &project).unwrap();

    // First compile
    run_rotom(&["compile"], &project).unwrap();

    // Decompile writes .rotom files alongside existing .s files.
    // In a real workflow the user would have converted first; here we
    // remove the stale .s files so the next compile doesn't collide.
    let out = run_rotom(&["decompile"], &project).unwrap();
    println!("{}", out);

    remove_files_with_extension(&project.join("res/field/scripts"), "s");

    // Second compile (should skip unchanged files)
    let out = run_rotom(&["compile"], &project).unwrap();
    println!("{}", out);

    // Verify state
    let state_raw =
        std::fs::read_to_string(project.join(".rotom/status/compile-state.json")).unwrap();
    let state: rotom::compile_state::CompileState = serde_json::from_str(&state_raw).unwrap();
    assert!(!state.entries.is_empty());

    println!("Lifecycle at {}", project.display());
}

#[test]
fn test_project_lifecycle_heartgold() {
    ensure_decomp_fixtures();

    let decomp = fixture_root().join("decomp/pokeheartgold");
    let project = persistent_test_dir("heartgold_lifecycle");
    copy_decomp(&decomp, &project);

    run_rotom(&["init", "--non-interactive"], &project).unwrap();

    // HGSS places binaries alongside sources, so we must compile before
    // decompiling to produce the .bin files.
    run_rotom(&["compile"], &project).unwrap();

    let out = run_rotom(&["decompile"], &project).unwrap();
    println!("{}", out);

    // Remove stale .s files after decompile to avoid output collisions
    remove_files_with_extension(&project.join("files/fielddata/script/scr_seq"), "s");

    run_rotom(&["compile"], &project).unwrap();

    assert!(project.join(".rotom/status/compile-state.json").exists());
    println!("Lifecycle at {}", project.display());
}

#[test]
fn test_project_lifecycle_dspre_platinum() {
    let Some(dspre) = dspre_fixture_root("pt") else {
        eprintln!("Skipping DSPRE Platinum lifecycle test: fixture tree is missing");
        return;
    };

    let project = persistent_test_dir("dspre_platinum_lifecycle");
    copy_fixture(&dspre, &project);

    run_rotom(&["init", "--non-interactive"], &project).unwrap();
    run_rotom(&["compile"], &project).unwrap();

    let out = run_rotom(&["decompile"], &project).unwrap();
    println!("{}", out);

    remove_files_with_extension(&project.join("expanded/scripts"), "script");

    run_rotom(&["compile"], &project).unwrap();

    assert!(project.join(".rotom/status/compile-state.json").exists());
    println!("Lifecycle at {}", project.display());
}

#[test]
fn test_project_lifecycle_dspre_heartgold() {
    let Some(dspre) = dspre_fixture_root("hg") else {
        eprintln!("Skipping DSPRE HeartGold lifecycle test: fixture tree is missing");
        return;
    };

    let project = persistent_test_dir("dspre_heartgold_lifecycle");
    copy_fixture(&dspre, &project);

    run_rotom(&["init", "--non-interactive"], &project).unwrap();
    run_rotom(&["compile"], &project).unwrap();

    let out = run_rotom(&["decompile"], &project).unwrap();
    println!("{}", out);

    remove_files_with_extension(&project.join("expanded/scripts"), "script");

    run_rotom(&["compile"], &project).unwrap();

    assert!(project.join(".rotom/status/compile-state.json").exists());
    println!("Lifecycle at {}", project.display());
}
