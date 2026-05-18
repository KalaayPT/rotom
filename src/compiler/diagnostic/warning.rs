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
    /// A segment of a message string is wider than the dialog allows.
    /// For plain strings this means dialog overflow; for format() strings
    /// it means format() will insert an extra word-wrap break within it.
    MessageLineTooLong {
        #[serde(serialize_with = "serialize_range")]
        span: Range<usize>,
        line_index: usize,
    },
}

impl CompileWarning {
    pub fn span(&self) -> Range<usize> {
        match self {
            Self::UnusedAlias { span, .. }
            | Self::ShadowedAlias { span, .. }
            | Self::MessageLineTooLong { span, .. } => span.clone(),
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::UnusedAlias { name, .. } => format!("Alias '{name}' is never used"),
            Self::ShadowedAlias { name, .. } => {
                format!("Alias '{name}' shadows a previous alias definition")
            }
            Self::MessageLineTooLong { line_index, .. } => format!(
                "Message line {line_index} exceeds the maximum dialog width — \
                 add explicit line breaks or wrap with format()"
            ),
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
