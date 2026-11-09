//! RabbitHole's own JavaScript engine.
//!
//! Pipeline: [`lexer`] → [`parser`] (producing [`ast`]) → [`compiler`]
//! (bytecode) → [`vm`] (stack machine with generators/async support) on a
//! garbage-collected [`heap`]. [`builtins`] provides the standard library,
//! [`regex`] the ECMAScript regular expression engine.
//!
//! # Embedding
//!
//! ```ignore
//! use rabbithole_js::{Realm, Value};
//! let mut realm = Realm::new();
//! let v = realm.eval("1 + 2", "inline.js").unwrap();
//! assert_eq!(v.as_number(), Some(3.0));
//! realm.run_jobs(); // drain promise reactions
//! ```
//!
//! Host objects: create them with [`Realm::new_object`], attach Rust state
//! with [`Realm::set_host_data`], and expose behaviour through
//! [`Realm::define_method`] / [`Realm::define_accessor`]. Values that the
//! host keeps across calls must be kept alive with [`Realm::root`].

pub mod ast;
pub mod builtins;
pub mod bytecode;
pub mod compiler;
pub mod heap;
pub mod interop;
pub mod lexer;
pub mod parser;
pub mod realm;
pub mod regex;
pub mod value;
pub mod vm;

#[cfg(test)]
mod tests;

pub use heap::{NativeFn, ObjectKind, Property, ALL, CONFIGURABLE, ENUMERABLE, HIDDEN, WRITABLE};
pub use interop::JsResult;
pub use parser::{parse_program, ParseError};
pub use realm::{Realm, Root};
pub use value::{JsStr, ObjectId, PropertyKey, SymbolId, Value};
pub use vm::ResumeMode;
