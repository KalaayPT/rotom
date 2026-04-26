//! Extracted batch compilation — shared between `compile_path` and `compile_project`.
//!
//! This is the single place where parallel script compilation happens.
//! It takes explicit (input, output) pairs so callers can decide their own
//! path mapping (directory extension-swap, project config, etc.).

use rayon::prelude::{ParallelIterator, IntoParallelRefIterator};

use crate::database::ConstantDb;
use crate::progress::CompileProgress;
use crate::{CompileFailure, CompileResult, DatabaseV2};

/// Compile a list of (`input_path`, `output_path`) pairs in parallel.
///
/// `load_file_constants` is forwarded to each worker; when `true` the worker
/// clones the shared `ConstantDb` internally for its own script and drops it
/// after compilation.  No clones are kept at the batch level.
pub fn compile_batch(
    work: &[(std::path::PathBuf, std::path::PathBuf)],
    db: &DatabaseV2,
    constants: &ConstantDb,
    load_file_constants: bool,
    progress: Option<&CompileProgress>,
) -> crate::BatchCompileResult {
    let results: Vec<std::result::Result<CompileResult, CompileFailure>> = work
        .par_iter()
        .map(|(input, output)| {
            let result = crate::compile_file_internal(input, output, db, constants, load_file_constants).map_err(
                |e| match e {
                    crate::CompileFileError::IoError(error) => CompileFailure {
                        path: input.clone(),
                        error,
                        source: String::new(),
                    },
                    crate::CompileFileError::CompileError { error, source } => CompileFailure {
                        path: input.clone(),
                        error,
                        source,
                    },
                },
            );
            match &result {
                Ok(_) => { if let Some(p) = progress { p.inc_completed(); } }
                Err(_) => { if let Some(p) = progress { p.inc_failed(); } }
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
