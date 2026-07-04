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
    /// A slot ID is absent from an otherwise-contiguous run of public scripts.
    /// The emitter fills the gap by repeating the previous script's pointer,
    /// but the author likely forgot a header.
    MissingSlot {
        slot: u32,
        #[serde(serialize_with = "serialize_range")]
        span: Range<usize>,
    },
    /// A segment of a message string is wider than the dialog allows.
    /// For plain strings this means dialog overflow; for `format()` strings
    /// it means `format()` will insert an extra word-wrap break within it.
    MessageLineTooLong {
        #[serde(serialize_with = "serialize_range")]
        span: Range<usize>,
        line_index: usize,
    },
    /// A database variant condition could not be evaluated at compile time (e.g. unknown
    /// identifier in the condition expression). The default/else variant was used as a
    /// fallback; the emitted binary may not match the intended variant.
    VariantConditionUnresolvable {
        command: String,
        condition: String,
        #[serde(serialize_with = "serialize_range")]
        span: Range<usize>,
    },
}

impl CompileWarning {
    pub fn span(&self) -> Range<usize> {
        match self {
            Self::UnusedAlias { span, .. }
            | Self::ShadowedAlias { span, .. }
            | Self::MissingSlot { span, .. }
            | Self::MessageLineTooLong { span, .. }
            | Self::VariantConditionUnresolvable { span, .. } => span.clone(),
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::UnusedAlias { name, .. } => format!("Alias '{name}' is never used"),
            Self::ShadowedAlias { name, .. } => {
                format!("Alias '{name}' shadows a previous alias definition")
            }
            Self::MissingSlot { slot, .. } => format!(
                "Script slot #{slot} is empty; the next available script's pointer will be reused. \
                 Did you forget a header?"
            ),
            Self::MessageLineTooLong { line_index, .. } => format!(
                "Message line {line_index} exceeds the maximum dialog width — \
                 add explicit line breaks or wrap with format()"
            ),
            Self::VariantConditionUnresolvable {
                command, condition, ..
            } => format!(
                "Could not evaluate variant condition '{condition}' for command '{command}' \
                 at compile time; the default variant will be used and the output may be incorrect"
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warning_messages_and_spans_are_variant_specific() {
        let warnings = [
            CompileWarning::UnusedAlias {
                name: "FOO".to_string(),
                span: 1..4,
            },
            CompileWarning::ShadowedAlias {
                name: "BAR".to_string(),
                span: 5..8,
                previous_span: 0..3,
            },
            CompileWarning::MissingSlot {
                slot: 7,
                span: 9..10,
            },
            CompileWarning::MessageLineTooLong {
                span: 11..20,
                line_index: 2,
            },
            CompileWarning::VariantConditionUnresolvable {
                command: "ScrCmd_Test".to_string(),
                condition: "UNKNOWN".to_string(),
                span: 21..30,
            },
        ];

        assert_eq!(warnings[0].span(), 1..4);
        assert_eq!(warnings[1].span(), 5..8);
        assert_eq!(warnings[2].span(), 9..10);
        assert_eq!(warnings[3].span(), 11..20);
        assert_eq!(warnings[4].span(), 21..30);
        assert!(warnings[0].message().contains("FOO"));
        assert!(warnings[1].message().contains("shadows"));
        assert!(warnings[2].message().contains("#7"));
        assert!(warnings[3].message().contains("Message line 2"));
        assert!(warnings[4].message().contains("ScrCmd_Test"));
    }
}
