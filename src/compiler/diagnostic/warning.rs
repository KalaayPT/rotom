use std::ops::Range;

use codespan_reporting::diagnostic::Severity;
use serde::Serialize;

use super::{render_diagnostic, serialize_range};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "details")]
pub enum CompileWarning {
    UnusedAlias {
        name: String,
        #[serde(serialize_with = "serialize_range")]
        span: Range<usize>,
    },
    ShadowedAlias {
        name: String,
        #[serde(serialize_with = "serialize_range")]
        span: Range<usize>,
        #[serde(serialize_with = "serialize_range")]
        previous_span: Range<usize>,
    },
}

impl CompileWarning {
    pub fn span(&self) -> Range<usize> {
        match self {
            Self::UnusedAlias { span, .. } | Self::ShadowedAlias { span, .. } => span.clone(),
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::UnusedAlias { name, .. } => format!("Alias '{name}' is never used"),
            Self::ShadowedAlias { name, .. } => {
                format!("Alias '{name}' shadows a previous alias definition")
            }
        }
    }
}

pub fn print_warning(filename: &str, source: &str, warning: &CompileWarning) {
    render_diagnostic(
        filename,
        source,
        Severity::Warning,
        "Compile warning",
        warning.span(),
        warning.message(),
    );
}
