//! Extracted batch compilation — shared between `compile_path` and `compile_project`.
//!
//! This is the single place where parallel script compilation happens.
//! It takes explicit (input, output) pairs so callers can decide their own
//! path mapping (directory extension-swap, project config, etc.).

use rayon::prelude::{IntoParallelRefIterator, ParallelIterator};
use std::sync::Arc;

use crate::database::ConstantDb;
use crate::progress::CompileProgress;
use crate::{BinaryQuirk, CompileFailure, CompiledFile, DatabaseV2};

/// A single unit of work for [`compile_batch`].
pub struct CompileWorkItem {
    pub input: std::path::PathBuf,
    pub output: std::path::PathBuf,
    pub quirks: BinaryQuirk,
}

/// Compile a list of [`CompileWorkItem`]s in parallel.
///
/// `load_file_constants` is forwarded to each worker; when `true` the worker
/// clones the shared `ConstantDb` internally for its own script and drops it
/// after compilation.  No clones are kept at the batch level.
pub fn compile_batch(
    work: &[CompileWorkItem],
    db: &DatabaseV2,
    constants: &ConstantDb,
    load_file_constants: bool,
    progress: Option<&CompileProgress>,
    workspace: &Arc<uxie::Workspace>,
) -> crate::BatchCompileResult {
    let results: Vec<std::result::Result<CompiledFile, CompileFailure>> = work
        .par_iter()
        .map(|item| {
            let result = crate::compile_file_internal(
                &item.input,
                &item.output,
                db,
                constants,
                load_file_constants,
                item.quirks,
                workspace,
            );
            match &result {
                Ok(_) => {
                    if let Some(p) = progress {
                        p.inc_completed();
                    }
                }
                Err(_) => {
                    if let Some(p) = progress {
                        p.inc_failed();
                    }
                }
            }
            result
        })
        .collect();

    let mut successes = Vec::new();
    let mut failures = Vec::new();
    for result in results {
        match result {
            Ok(r) => successes.push(r),
            Err(f) => failures.push(f),
        }
    }

    crate::BatchCompileResult {
        successes,
        failures,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConstantDb, DatabaseV2};

    #[test]
    fn compile_batch_splits_successes_and_failures() {
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let input = temp.path().join("ok.rotom");
        let output = temp.path().join("ok.bin");
        std::fs::write(&input, "script Main #1:\nEnd\n").expect("failed to write script");

        let work = vec![
            CompileWorkItem {
                input: input.clone(),
                output: output.clone(),
                quirks: BinaryQuirk::default(),
            },
            CompileWorkItem {
                input: temp.path().join("missing.rotom"),
                output: temp.path().join("missing.bin"),
                quirks: BinaryQuirk::default(),
            },
        ];
        let progress = CompileProgress::new(work.len());
        let workspace = Arc::new(uxie::Workspace::new(
            std::path::PathBuf::new(),
            uxie::game::Game::Platinum,
        ));

        let result = compile_batch(
            &work,
            DatabaseV2::test_platinum(),
            &ConstantDb::new(),
            false,
            Some(&progress),
            &workspace,
        );

        assert_eq!(result.successes.len(), 1);
        assert_eq!(result.failures.len(), 1);
        assert_eq!(result.successes[0].input, input);
        assert_eq!(result.successes[0].output, output);
        assert!(output.exists());
    }
}
