//! Early errors of the function bodies the lazy parse skips, checked in
//! parallel while the script loads.
//!
//! The specification rejects a whole script when any of its function
//! bodies has an early error, but the lazy parse (see
//! [`crate::parser::skips_bodies`]) only finds where a skipped body ends.
//! Each body it skips becomes a job: a worker thread parses the body with
//! the context the main parse recorded ([`LazyBody`]) and runs the early
//! error checks on it, skipping long nested bodies in turn and submitting
//! them as jobs of their own, so a large body splits into pieces the
//! workers share. The jobs start while the main thread is still parsing
//! the rest of the script and compiling its top level;
//! [`Validation::finish`] waits for the remaining ones (the main thread
//! runs queued jobs of its script meanwhile) and reports the first error
//! in source order.
//!
//! The pre-parser records the long blocks it goes past
//! ([`crate::lexer::BlockEnd`]); they travel with the jobs, so a worker
//! skips the long function bodies nested in its body without scanning
//! them again.
//!
//! The workers are a small process-wide pool; a machine with a single
//! core runs every job on the main thread in `finish`. Opt-in for now (see
//! [`enabled`]).

use crate::ast::{Function, FunctionBody, LazyBody, Name, Param, Pattern, Pos};
use crate::parser::ParseError;
use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

/// A skipped body and what its check needs to know about its function,
/// in a form another thread can own.
pub struct BodyJob {
    start: usize,
    end: usize,
    line: u32,
    line_start: usize,
    is_module: bool,
    in_class_body: bool,
    super_call_ok: bool,
    super_prop_ok: bool,
    name: Option<String>,
    self_binding: bool,
    /// Names bound by the parameters (the parameters themselves were
    /// checked by the main parse).
    params: Vec<String>,
    is_arrow: bool,
    is_async: bool,
    is_generator: bool,
    strict: bool,
    pos: Pos,
    /// The long blocks inside the body (see [`crate::lexer::BlockEnd`]).
    blocks: Vec<crate::lexer::BlockEnd>,
}

impl BodyJob {
    pub(crate) fn new(f: &Function, lb: &LazyBody, blocks: Vec<crate::lexer::BlockEnd>) -> BodyJob {
        let mut names = Vec::new();
        for p in &f.params {
            crate::early::bound_names(&p.pattern, &mut names);
        }
        BodyJob {
            start: lb.start,
            end: lb.end,
            line: lb.line,
            line_start: lb.line_start,
            is_module: lb.is_module,
            in_class_body: lb.in_class_body,
            super_call_ok: lb.super_call_ok,
            super_prop_ok: lb.super_prop_ok,
            name: f.name.as_ref().map(|n| n.to_string()),
            self_binding: f.self_binding,
            params: names.into_iter().map(str::to_string).collect(),
            is_arrow: f.is_arrow,
            is_async: f.is_async,
            is_generator: f.is_generator,
            strict: f.strict,
            pos: f.pos,
            blocks,
        }
    }

    fn run(self, source: &std::rc::Rc<str>, batch: &Arc<Batch>) -> Result<(), ParseError> {
        let lb = LazyBody {
            source: source.clone(),
            start: self.start,
            end: self.end,
            line: self.line,
            line_start: self.line_start,
            names: Box::new([]),
            eval: false,
            is_module: self.is_module,
            in_class_body: self.in_class_body,
            super_call_ok: self.super_call_ok,
            super_prop_ok: self.super_prop_ok,
        };
        let f = Function {
            name: self.name.as_deref().map(Name::new),
            params: self.params.iter().map(|n| Param { pattern: Pattern::Identifier(Name::new(n)), rest: false }).collect(),
            body: FunctionBody::Block(Vec::new()),
            is_arrow: self.is_arrow,
            is_async: self.is_async,
            is_generator: self.is_generator,
            strict: self.strict,
            pos: self.pos,
            span: (0, 0),
            self_binding: self.self_binding,
        };
        crate::parser::validate_lazy_body(&f, &lb, batch, self.blocks)
    }
}

/// The parallel check of one script.
pub struct Batch {
    id: u64,
    source: Arc<str>,
    state: Mutex<BatchState>,
    done: Condvar,
}

#[derive(Default)]
struct BatchState {
    /// Jobs submitted and not finished.
    pending: usize,
    /// Body starts already submitted (a speculative parse may build a
    /// function twice).
    seen: HashSet<usize, crate::value::FxBuild>,
    /// The first error found so far (in source order).
    error: Option<ParseError>,
}

/// A running check (see [`start`]).
pub struct Validation {
    batch: Arc<Batch>,
}

struct Pool {
    queue: Mutex<VecDeque<(Arc<Batch>, Vec<BodyJob>)>>,
    ready: Condvar,
}

fn pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| {
        let workers = match std::env::var("RH_VALIDATE_THREADS").ok().and_then(|v| v.parse().ok()) {
            Some(n) => n,
            None => std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).saturating_sub(1).min(4),
        };
        for i in 0..workers {
            // The parser recurses with the nesting of the source: a large
            // stack (reserved, not committed).
            let _ = std::thread::Builder::new().name(format!("js-validate-{i}")).stack_size(64 << 20).spawn(worker);
        }
        Pool { queue: Mutex::new(VecDeque::new()), ready: Condvar::new() }
    })
}

/// Whether skipped bodies are checked at load (opt-in: `RH_LAZY_CHECKED`;
/// otherwise their early errors are reported when the function is first
/// compiled).
///
/// The check costs about as much CPU as a complete parse (3-4x the lazy
/// one): on four cores loading the framework bundles of `parsebench`
/// takes about 3x as long as without it, short of the 1.5x that would
/// make it worth having on by default.
fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    match OVERRIDE.load(std::sync::atomic::Ordering::Relaxed) {
        1 => false,
        2 => true,
        _ => *ON.get_or_init(|| std::env::var_os("RH_LAZY_CHECKED").is_some()),
    }
}

static OVERRIDE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Turn the check on or off for the whole process (`None`: as the
/// environment says); for benchmarks.
pub fn set_enabled(on: Option<bool>) {
    OVERRIDE.store(on.map_or(0, |o| if o { 2 } else { 1 }), std::sync::atomic::Ordering::Relaxed);
}

/// Start checking the bodies the lazy parse of `source` will skip; `None`
/// when it skips none. Pass the result to
/// [`crate::parser::parse_program_validating`].
pub fn start(source: &str) -> Option<Validation> {
    if !crate::parser::skips_bodies(source.len()) || !enabled() {
        return None;
    }
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    Some(Validation {
        batch: Arc::new(Batch {
            id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            source: Arc::from(source),
            state: Mutex::new(BatchState::default()),
            done: Condvar::new(),
        }),
    })
}

/// Jobs are queued in chunks of about this many bytes of source (one
/// queue operation and wake-up per chunk, not per body).
pub(crate) const CHUNK: usize = 16 * 1024;

/// Queue the checks of skipped bodies (a chunk run by one thread).
pub(crate) fn submit(batch: &Arc<Batch>, mut jobs: Vec<BodyJob>) {
    {
        let mut st = batch.state.lock().unwrap();
        jobs.retain(|j| !past_error(&st, j) && st.seen.insert(j.start));
        if jobs.is_empty() {
            return;
        }
        st.pending += 1;
    }
    let p = pool();
    p.queue.lock().unwrap().push_back((batch.clone(), jobs));
    p.ready.notify_one();
}

impl BodyJob {
    /// Bytes of source the job parses.
    pub(crate) fn len(&self) -> usize {
        self.end - self.start
    }
}

/// The job starts after an error already found: its own errors would not
/// be the first.
fn past_error(st: &BatchState, job: &BodyJob) -> bool {
    st.error.as_ref().is_some_and(|e| job.line > e.line)
}

fn run(batch: &Arc<Batch>, jobs: Vec<BodyJob>) {
    for job in jobs {
        run_one(batch, job);
    }
    let mut st = batch.state.lock().unwrap();
    st.pending -= 1;
    if st.pending == 0 {
        batch.done.notify_all();
    }
}

fn run_one(batch: &Arc<Batch>, job: BodyJob) {
    thread_local! {
        /// The source of the batch this thread last worked on, shared by
        /// the bodies it parses.
        static SOURCE: std::cell::RefCell<Option<(u64, std::rc::Rc<str>)>> = const { std::cell::RefCell::new(None) };
    }
    let skip = past_error(&batch.state.lock().unwrap(), &job);
    if !skip {
        let src = SOURCE.with(|s| {
            let mut s = s.borrow_mut();
            match &*s {
                Some((id, src)) if *id == batch.id => src.clone(),
                _ => {
                    let src: std::rc::Rc<str> = std::rc::Rc::from(&*batch.source);
                    *s = Some((batch.id, src.clone()));
                    src
                }
            }
        });
        // A panic in the parser must not leave the batch waiting.
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job.run(&src, batch))).unwrap_or(Ok(()));
        if let Err(e) = r {
            let mut st = batch.state.lock().unwrap();
            if st.error.as_ref().is_none_or(|f| (e.line, e.column) < (f.line, f.column)) {
                st.error = Some(e);
            }
        }
    }
}

fn worker() {
    crate::alloc::enable_for_this_thread();
    let p = pool();
    loop {
        let (batch, job) = {
            let mut q = p.queue.lock().unwrap();
            loop {
                if let Some(j) = q.pop_front() {
                    break j;
                }
                q = p.ready.wait(q).unwrap();
            }
        };
        run(&batch, job);
    }
}

impl Validation {
    pub(crate) fn batch(&self) -> &Arc<Batch> {
        &self.batch
    }

    /// Wait for the check (running queued jobs of this script on the
    /// calling thread meanwhile): the first early error of a skipped body.
    pub fn finish(self) -> Option<ParseError> {
        let p = pool();
        loop {
            let mine = {
                let mut q = p.queue.lock().unwrap();
                // Usually the only script being checked: its chunks are
                // at the front.
                match q.front() {
                    Some((b, _)) if Arc::ptr_eq(b, &self.batch) => q.pop_front(),
                    _ => None,
                }
            };
            if let Some((b, job)) = mine {
                run(&b, job);
                continue;
            }
            let mut st = self.batch.state.lock().unwrap();
            if st.pending == 0 {
                return st.error.take();
            }
            // Workers may queue nested bodies meanwhile: look again soon.
            let (mut st, _) = self.batch.done.wait_timeout(st, std::time::Duration::from_micros(200)).unwrap();
            if st.pending == 0 {
                return st.error.take();
            }
        }
    }
}
