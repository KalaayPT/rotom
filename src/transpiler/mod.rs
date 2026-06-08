//! Transpilers for converting other script formats to Rotoscript

#[allow(non_snake_case)]
pub mod DSPRE;
pub mod decomp;
pub mod levelscript_decomp;

pub use DSPRE::transpile as transpile_dspre;
pub use decomp::transpile as transpile_decomp;
pub use decomp::{TranspileError as DecompTranspileError, TranspileResult};
pub use levelscript_decomp::{
    TranspileError as LevelscriptTranspileError, is_levelscript_source, transpile_levelscript,
};
