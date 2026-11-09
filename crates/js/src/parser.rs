//! ECMAScript parser producing [`crate::ast`] (to be implemented).

use crate::ast::Program;

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub line: u32,
    pub column: u32,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SyntaxError: {} ({}:{})", self.message, self.line, self.column)
    }
}

/// Parse a script (or module when `is_module`).
pub fn parse_program(source: &str, is_module: bool) -> Result<Program, ParseError> {
    let _ = (source, is_module);
    unimplemented!()
}
