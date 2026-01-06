//! Transpilers for converting other script formats to Rotoscript

#[allow(non_snake_case)]
pub mod DSPRE;
pub mod decomp;

pub use DSPRE::transpile as transpile_dspre;
pub use decomp::transpile as transpile_decomp;
