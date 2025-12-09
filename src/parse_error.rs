use std::ops::Range;

use codespan_reporting::{
    diagnostic::{Diagnostic, Label},
    files::SimpleFiles,
    term::{
        self,
        termcolor::{ColorChoice, StandardStream},
    },
};

#[derive(Debug)]
pub struct ParseError {
    pub span: Range<usize>,
    pub message: String,
}

pub type ParseResult<T> = Result<T, ParseError>;

pub fn print_error(filename: &str, source: &str, error: ParseError) {
    let mut files = SimpleFiles::new();
    let file_id = files.add(filename, source);
    let diagnostic = Diagnostic::error()
        .with_message("Failed to parse script")
        .with_labels(vec![
            Label::primary(file_id, error.span).with_message(error.message),
        ]);
    let writer = StandardStream::stderr(ColorChoice::Always);
    let config = term::Config::default();
    if let Err(e) = term::emit_to_write_style(&mut writer.lock(), &config, &files, &diagnostic) {
        eprintln!(
            "Internal Compiler Error: Could not print diagnostics: {}",
            e
        );
    }
}
