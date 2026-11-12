//! Rust panics inside native functions.
//!
//! A bug in a built-in or a host binding (an out-of-range index, an
//! `unwrap` on a missing value) must not take the browser down: every
//! native call goes through [`crate::realm::Realm`]'s single dispatch point,
//! which catches the unwind, restores the interpreter's stacks to what they
//! were when the native was entered, and throws a catchable JavaScript
//! `Error("internal error: ...")` at the call site instead.
//!
//! The panic hook installed here remembers where the last panic of the
//! thread happened, so the console message can name the Rust location.

use std::any::Any;
use std::cell::RefCell;
use std::sync::Once;

thread_local! {
    /// (message, "file:line:column") of this thread's last panic.
    static LAST: RefCell<Option<(String, String)>> = const { RefCell::new(None) };
}

/// Install (once per process) a panic hook that records each panic's
/// message and location for [`take_last`], then runs the previous hook
/// (which prints the usual report to stderr).
pub fn install_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let loc = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
            let msg = payload_message(info.payload());
            let _ = LAST.try_with(|l| {
                if let Ok(mut l) = l.try_borrow_mut() {
                    *l = Some((msg, loc));
                }
            });
            prev(info);
        }));
    });
}

/// The message and location recorded for this thread's last panic.
pub fn take_last() -> Option<(String, String)> {
    LAST.try_with(|l| l.try_borrow_mut().ok().and_then(|mut l| l.take())).ok().flatten()
}

/// Text of a panic payload (`panic!("...")` / `panic!("{}", x)`).
pub fn payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// Run `f`, turning a panic into `Err((message, location))` (see
/// [`install_hook`]). For hosts that want the same protection around their
/// own entry points (event dispatch, layout, ...).
pub fn catch<R>(f: impl FnOnce() -> R) -> Result<R, (String, String)> {
    install_hook();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => Ok(r),
        Err(payload) => Err(describe(&*payload)),
    }
}

/// (message, location) of a caught panic: the location comes from the
/// hook's record when it is about this panic.
pub fn describe(payload: &(dyn Any + Send)) -> (String, String) {
    let msg = payload_message(payload);
    let loc = match take_last() {
        Some((m, loc)) if m == msg => loc,
        _ => String::new(),
    };
    (msg, loc)
}
