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
//! Modules: install a loader with [`Realm::set_module_loader`], then
//! [`Realm::compile_module`] → [`Realm::link_module`] →
//! [`Realm::evaluate_module`] (or [`Realm::run_module`] for all three).
//!
//! Host objects: create them with [`Realm::new_object`], attach Rust state
//! with [`Realm::set_host_data`], and expose behaviour through
//! [`Realm::define_method`] / [`Realm::define_accessor`]. Values that the
//! host keeps across calls must be kept alive with [`Realm::root`].

pub mod ast;
pub mod bigint;
pub mod builtins;
pub mod bytecode;
pub mod captures;
pub mod compiler;
pub mod heap;
pub mod ic;
pub mod interop;
pub mod lexer;
pub mod modules;
pub mod parser;
pub mod realm;
pub mod regex;
pub mod stack;
pub mod regs;
pub mod sampler;
pub mod shape;
pub mod value;
pub mod vm;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_vm;

pub use heap::{NativeFn, ObjectKind, Property, ALL, CONFIGURABLE, ENUMERABLE, HIDDEN, WRITABLE};
pub use interop::JsResult;
pub use modules::{ImportMetaHook, ModuleId, ModuleLoader, ModuleStatus};
pub use parser::{parse_program, ParseError};
pub use realm::{exclude_from_watchdog, Realm, Root};
pub use value::{JsStr, ObjectId, PropertyKey, SymbolId, Value, V};
pub use vm::ResumeMode;
