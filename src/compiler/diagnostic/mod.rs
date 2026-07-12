use std::ops::Range;

use codespan_reporting::{
    diagnostic::{Diagnostic, Label, Severity},
    files::SimpleFiles,
    term::{
        self,
        termcolor::{ColorChoice, StandardStream},
    },
};
use serde::{Serialize, Serializer};

pub mod error;
pub mod warning;

pub use error::{
    CompileError, ParseResult, analysis_error, codegen_error, database_error, lowering_error,
    lowering_error_at, parse_error, print_error,
};
pub use warning::{CompileWarning, print_warning};

pub fn serialize_range<S>(range: &Range<usize>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    #[derive(Serialize)]
    struct RangeHelper {
        start: usize,
        end: usize,
    }
    RangeHelper {
        start: range.start,
        end: range.end,
    }
    .serialize(serializer)
}

pub fn serialize_optional_range<S>(
    range: &Option<Range<usize>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match range {
        Some(range) => serialize_range(range, serializer),
        None => serializer.serialize_none(),
    }
}

fn render_diagnostic(
    filename: &str,
    source: &str,
    severity: Severity,
    title: &str,
    span: Range<usize>,
    message: String,
) {
    let mut files = SimpleFiles::new();
    let file_id = files.add(filename, source);
    let diagnostic = Diagnostic::new(severity)
        .with_message(title)
        .with_labels(vec![Label::primary(file_id, span).with_message(message)]);
    let writer = StandardStream::stderr(ColorChoice::Always);
    let config = term::Config::default();
    if let Err(e) = term::emit_to_io_write(&mut writer.lock(), &config, &files, &diagnostic) {
        eprintln!(
            "Internal Compiler Error: Could not print diagnostics: {}",
            e
        );
    }
}
