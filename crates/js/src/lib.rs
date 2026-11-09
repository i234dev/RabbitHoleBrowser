//! RabbitHole's own JavaScript engine.
//!
//! Pipeline: [`lexer`] → [`parser`] (producing [`ast`]) → [`compiler`]
//! (bytecode) → [`vm`] (stack machine with generators/async support) on a
//! garbage-collected [`heap`]. [`builtins`] provides the standard library,
//! [`regex`] the ECMAScript regular expression engine.

pub mod ast;
pub mod lexer;
pub mod parser;
pub mod regex;

pub use parser::{parse_program, ParseError};
