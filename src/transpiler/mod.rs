//! Transpilers for converting other script formats to Rotoscript

#[allow(non_snake_case)]
pub mod DSPRE;
pub mod decomp;
pub mod levelscript_decomp;

pub use DSPRE::transpile as transpile_dspre;
pub use decomp::{TranspileResult, transpile as transpile_decomp};
pub use levelscript_decomp::{
    TranspileError as LevelscriptTranspileError, is_levelscript_source, transpile_levelscript,
};

pub fn transpile_decomp_simple(input: &str, db: Option<&crate::database::DatabaseV2>) -> String {
    transpile_decomp(input, db).source
}
