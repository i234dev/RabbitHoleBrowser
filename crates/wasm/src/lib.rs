//! RabbitHole's own WebAssembly engine.
//!
//! Pipeline: [`binary`] decodes a module, [`validate`] checks it and
//! [`compile`] validates each function body while translating it into the
//! register IR of [`ir`]; [`runtime`] instantiates modules into a
//! [`runtime::Store`] and [`interp`] executes them. [`text`] implements the
//! WebAssembly text format (WAT → binary) and the `.wast` script format of
//! the specification test suite.
//!
//! ```ignore
//! use rabbithole_wasm::{runtime::*, CompiledModule};
//! let bytes = rabbithole_wasm::text::wat_to_binary("(module (func (export \"f\") (result i32) i32.const 42))").unwrap();
//! let m = std::rc::Rc::new(CompiledModule::new(&bytes).unwrap());
//! let store = Store::new();
//! let inst = store.instantiate(&mut NoHost, &m, &[]).unwrap();
//! ```

pub mod binary;
pub mod compile;
pub mod interp;
pub mod ir;
pub mod module;
pub mod runtime;
pub mod simd;
pub mod spec;
pub mod text;
pub mod types;
pub mod validate;

pub use runtime::{CompiledModule, Extern, Host, Instance, Store, Trap, Val};

/// Decoding or validation error.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// The binary (or text) is not well-formed.
    Malformed(String),
    /// The module is well-formed but does not validate.
    Invalid(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Malformed(m) => write!(f, "malformed module: {m}"),
            Error::Invalid(m) => write!(f, "invalid module: {m}"),
        }
    }
}

/// A host with no host functions.
pub struct NoHost;

impl Host for NoHost {
    fn call_host(&mut self, _store: &Store, _func: &std::rc::Rc<runtime::FuncInst>, _args: &[Val]) -> Result<Vec<Val>, Trap> {
        Err(Trap::Other("no host functions".into()))
    }
}

/// Validate a binary module.
pub fn validate(bytes: &[u8]) -> Result<(), Error> {
    CompiledModule::new(bytes).map(|_| ())
}
