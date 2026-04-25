use std::path::{Path, PathBuf};
use std::process::Command;

use super::fixture_pins::ALL_PINS;

pub fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Ensure all decomp fixtures are present. Auto-clones via git if missing.
/// Panics with clear instructions if git is not available or clone fails.
pub fn ensure_decomp_fixtures() {
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

        println!("[fixtures] Cloning {} into {}", pin.repo_url, dest.display());
        clone_or_die(pin.repo_url, &dest);
        checkout_or_die(&dest, pin.commit);
        println!("[fixtures] {} ready at {}", pin.name, pin.commit);
    }
}

/// Verify DSPRE fixtures exist. Panics with instructions if not.
pub fn ensure_dspre_fixtures() {
    let root = fixture_root();
    let mut missing = Vec::new();

    for (game, name) in [("pt", "Platinum"), ("hg", "HeartGold")] {
        let dspre = root.join("dspre").join(format!("{}_DSPRE_contents", game));
        let required = ["expanded/scripts", "expanded/textArchives", "unpacked/scripts"];
        for sub in required {
            let path = dspre.join(sub);
            if !path.exists() {
                missing.push(format!(
                    "  DSPRE {}: {} (copy your DSPRE output to {})",
                    name,
                    path.display(),
                    dspre.display()
                ));
            }
        }
    }

    if !missing.is_empty() {
        panic!(
            "Missing DSPRE fixtures. See CONTRIBUTING.md for setup instructions.\n\n{}",
            missing.join("\n")
        );
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
        .unwrap_or_else(|e| {
            panic!(
                "Failed to run `git clone`. Is git installed?\nError: {}",
                e
            )
        });
    if !status.success() {
        panic!("git clone failed for {}", url);
    }
}

fn checkout_or_die(path: &Path, commit: &str) {
    let status = Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("checkout")
        .arg(commit)
        .status()
        .expect("git checkout failed");
    if !status.success() {
        panic!("git checkout {} failed in {}", commit, path.display());
    }
}

/// Return a persistent temp directory for project lifecycle tests.
/// These are NOT auto-deleted so you can inspect uxie/rotom artifacts.
pub fn persistent_test_dir(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join("rotom_lifecycle_tests");
    // Use a counter file to avoid collisions across test runs
    let counter_path = base.join(".counter");
    let counter: u64 = std::fs::read_to_string(&counter_path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
        + 1;
    std::fs::create_dir_all(&base).ok();
    std::fs::write(&counter_path, counter.to_string()).ok();

    let dir = base.join(format!("{}_{}", name, counter));
    // Clean up any previous run with the same counter
    if dir.exists() {
        std::fs::remove_dir_all(&dir).ok();
    }
    std::fs::create_dir_all(&dir).expect("Failed to create test dir");
    dir
}
