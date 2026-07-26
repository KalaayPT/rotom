//! Extracted batch compilation — shared between `compile_path` and `compile_project`.
//!
//! This is the single place where parallel script compilation happens.
//! It takes explicit (input, output) pairs so callers can decide their own
//! path mapping (directory extension-swap, project config, etc.).

use rayon::prelude::{IntoParallelRefIterator, ParallelIterator};

use crate::progress::CompileProgress;
use crate::{BinaryQuirk, CompileContext, CompileFailure, CompiledFile};

/// A single unit of work for [`compile_batch`].
pub struct CompileWorkItem {
    pub input: std::path::PathBuf,
    pub output: std::path::PathBuf,
    pub quirks: BinaryQuirk,
}

/// Compile a list of [`CompileWorkItem`]s in parallel.
///
/// In standalone mode, `load_file_constants` controls whether each worker
/// augments the shared constants with directives from its script.
pub fn compile_batch(
    work: &[CompileWorkItem],
    context: CompileContext<'_>,
    load_file_constants: bool,
    progress: Option<&CompileProgress>,
) -> crate::BatchCompileResult {
    let results: Vec<std::result::Result<CompiledFile, CompileFailure>> = work
        .par_iter()
        .map(|item| {
            let file_constants = match context {
                CompileContext::Standalone { constants, .. } if !load_file_constants => {
                    Some(constants)
                }
                CompileContext::Standalone { .. } | CompileContext::Project(_) => None,
            };
            let result = crate::compile_file_internal(
                &item.input,
                &item.output,
                context,
                file_constants,
                item.quirks,
            )
            .map(|(compiled, _, _)| compiled);
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
        let constants = ConstantDb::new();
        let result = compile_batch(
            &work,
            CompileContext::Standalone {
                db: DatabaseV2::test_platinum(),
                constants: &constants,
            },
            false,
            Some(&progress),
        );

        assert_eq!(result.successes.len(), 1);
        assert_eq!(result.failures.len(), 1);
        assert_eq!(result.successes[0].input, input);
        assert_eq!(result.successes[0].output, output);
        assert!(output.exists());
    }
}
