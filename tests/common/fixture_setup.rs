use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;

use super::fixture_pins::ALL_PINS;

pub fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

static FIXTURE_INIT: Once = Once::new();

/// Ensure all decomp fixtures are present. Auto-clones via git if missing.
///
/// Uses `std::sync::Once` so that even when `cargo test` runs tests in
/// parallel within the same binary, git operations happen exactly once.
pub fn ensure_decomp_fixtures() {
    FIXTURE_INIT.call_once(|| {
        let root = fixture_root();
        std::fs::create_dir_all(root.join("decomp")).expect("Failed to create fixtures/decomp");

        for pin in ALL_PINS {
            let dest = root.join("decomp").join(pin.name);
            if dest.exists() {
                match verify_commit(&dest, pin.commit) {
                    Ok(true) => continue,
                    Ok(false) => {
                        eprintln!(
                            "[fixtures] {} commit mismatch — removing and re-cloning",
                            pin.name
                        );
                        std::fs::remove_dir_all(&dest).ok();
                    }
                    Err(e) => {
                        eprintln!(
                            "[fixtures] {} unreadable ({}). removing and re-cloning",
                            pin.name, e
                        );
                        std::fs::remove_dir_all(&dest).ok();
                    }
                }
            }

            println!(
                "[fixtures] Cloning {} into {}",
                pin.repo_url,
                dest.display()
            );
            clone_or_die(pin.repo_url, &dest);
            checkout_or_die(&dest, pin.commit);
            println!("[fixtures] {} ready at {}", pin.name, pin.commit);
        }
    });
}

/// Return a DSPRE fixture root when the local fixture tree is present.
pub fn dspre_fixture_root(game: &str) -> Option<PathBuf> {
    let root = fixture_root();
    let dspre = root.join("dspre").join(format!("{}_DSPRE_contents", game));
    let required = [
        "expanded/scripts",
        "expanded/textArchives",
        "unpacked/scripts",
    ];

    if required.iter().all(|sub| dspre.join(sub).exists()) {
        Some(dspre)
    } else {
        None
    }
}

fn verify_commit(path: &Path, expected: &str) -> Result<bool, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .map_err(|e| format!("git failed: {}", e))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(head.starts_with(expected))
}

fn clone_or_die(url: &str, dest: &Path) {
    let status = Command::new("git")
        .arg("clone")
        .arg(url)
        .arg(dest)
        .status()
        .unwrap_or_else(|e| panic!("Failed to run `git clone`. Is git installed?\nError: {}", e));
    assert!(status.success(), "git clone failed for {}", url)
}

fn checkout_or_die(path: &Path, commit: &str) {
    let status = Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("checkout")
        .arg(commit)
        .status()
        .expect("git checkout failed");
    assert!(
        status.success(),
        "git checkout {} failed in {}",
        commit,
        path.display()
    )
}

/// Return a persistent temp directory for project lifecycle tests.
/// These are NOT auto-deleted so you can inspect uxie/rotom artifacts.
pub fn persistent_test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("rotom_lifecycle_tests")
        .join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).ok();
    }
    std::fs::create_dir_all(&dir).expect("Failed to create test dir");
    dir
}
