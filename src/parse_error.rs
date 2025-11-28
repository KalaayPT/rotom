use std::ops::Range;

#[derive(Debug)]
pub struct ParseError {
    pub span: Range<usize>,
    pub message: String,
}

pub type ParseResult<T> = Result<T, ParseError>;
