//! Opt-in, thread-local event counters; no clocks or per-call output.
//!
//! This entire module and its call sites require `layout-diagnostics`.
//! In such a build, `RH_LAYOUT_DIAGNOSTICS=1` enables capture. Each thread
//! retains counts across page/frame destruction. A snapshot describes only
//! its calling thread, not all process threads or CPU attributed to phases.

use std::cell::RefCell;
use std::fmt;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::OnceLock;
use rabbithole_layout::diagnostics::Decision;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Snapshot {
    pub flush_entries: u64,
    /// Returned because both existing layout dirty/full flags were clear.
    pub clean_flag_returns: u64,
    /// Engine damage requests, not evidence that either algorithm ran.
    pub full_damage_requests: u64,
    pub incremental_damage_requests: u64,
    /// Actual returned branches observed in layout_incremental, not global
    /// process layout work or a count of individual boxes reconstructed.
    pub noop_layout_completions: u64,
    pub full_layout_requested_completions: u64,
    pub full_layout_fallback_completions: u64,
    pub incremental_layout_completions: u64,
    pub missing_layout_decisions: u64,
    pub aborted_flushes: u64,
    pub active_flushes: u64,
    pub max_flush_depth: u64,
    /// Events at the existing incremental pipeline counter boundaries.
    /// These are separate units and must not be summed as elapsed time.
    pub style_full_updates: u64,
    pub style_incremental_updates: u64,
    pub incremental_elements_restyled: u64,
    pub style_overlay_patches: u64,
    pub display_list_commits: u64,
    pub overflowed: bool,
}

impl Snapshot {
    /// A final capture is incomplete if any call is active, aborted, or saturated.
    pub fn is_complete(&self) -> bool {
        let layouts = self.noop_layout_completions.checked_add(self.full_layout_requested_completions)
            .and_then(|sum| sum.checked_add(self.full_layout_fallback_completions))
            .and_then(|sum| sum.checked_add(self.incremental_layout_completions));
        let completed = layouts.and_then(|sum| sum.checked_add(self.clean_flag_returns));
        let requested = self.full_damage_requests.checked_add(self.incremental_damage_requests);
        !self.overflowed && self.active_flushes == 0 && self.aborted_flushes == 0
            && self.missing_layout_decisions == 0 && completed == Some(self.flush_entries)
            && requested == layouts
    }
}

// Integer/boolean-only JSON; no page identities, URLs, contents or clock values.
impl fmt::Display for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{\"complete\":{},\"flush_entries\":{},\"clean_flag_returns\":{},\"full_damage_requests\":{},\"incremental_damage_requests\":{},\"noop_layout_completions\":{},\"full_layout_requested_completions\":{},\"full_layout_fallback_completions\":{},\"incremental_layout_completions\":{},\"missing_layout_decisions\":{},\"aborted_flushes\":{},\"active_flushes\":{},\"max_flush_depth\":{},\"style_full_updates\":{},\"style_incremental_updates\":{},\"incremental_elements_restyled\":{},\"style_overlay_patches\":{},\"display_list_commits\":{},\"overflowed\":{}}}",
            self.is_complete(), self.flush_entries, self.clean_flag_returns,
            self.full_damage_requests, self.incremental_damage_requests,
            self.noop_layout_completions, self.full_layout_requested_completions,
            self.full_layout_fallback_completions, self.incremental_layout_completions,
            self.missing_layout_decisions,
            self.aborted_flushes, self.active_flushes, self.max_flush_depth,
            self.style_full_updates, self.style_incremental_updates,
            self.incremental_elements_restyled, self.style_overlay_patches,
            self.display_list_commits, self.overflowed)
    }
}

struct State {
    enabled: bool,
    counts: Snapshot,
}

impl State {
    fn configured() -> Self {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        let enabled = *ENABLED.get_or_init(|| std::env::var_os("RH_LAYOUT_DIAGNOSTICS").is_some_and(|value| value == "1"));
        Self { enabled, counts: Snapshot::default() }
    }
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::configured());
}

/// Current calling-thread capture. `None` means runtime opt-in was absent.
pub fn snapshot() -> Option<Snapshot> {
    STATE.with(|state| {
        let state = state.borrow();
        state.enabled.then_some(state.counts)
    })
}

/// Explicit window boundary: returns prior counters and starts from zero.
/// Resetting while a flush is active is rejected, including nested calls.
/// The caller must preserve the returned capture, including aborted/overflow data.
pub fn reset() -> Result<Option<Snapshot>, &'static str> {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if !state.enabled {
            return Ok(None);
        }
        if state.counts.active_flushes != 0 {
            return Err("layout flush is active");
        }
        Ok(Some(std::mem::take(&mut state.counts)))
    })
}

fn add(counter: &mut u64, amount: u64) -> bool {
    match counter.checked_add(amount) {
        Some(value) => { *counter = value; false }
        None => { *counter = u64::MAX; true }
    }
}

#[derive(Clone, Copy)]
enum Outcome {
    Clean,
    Layout(Option<Decision>),
}

pub(crate) struct FlushCapture {
    outcome: Option<Outcome>,
    // A capture and its drop must remain on the thread whose TLS it updates.
    _same_thread: PhantomData<Rc<()>>,
}

pub(crate) fn begin_flush() -> Option<FlushCapture> {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if !state.enabled {
            return None;
        }
        let s = &mut state.counts;
        s.overflowed |= add(&mut s.flush_entries, 1);
        s.overflowed |= add(&mut s.active_flushes, 1);
        s.max_flush_depth = s.max_flush_depth.max(s.active_flushes);
        Some(FlushCapture { outcome: None, _same_thread: PhantomData })
    })
}

impl FlushCapture {
    pub(crate) fn clean(&mut self) {
        self.outcome = Some(Outcome::Clean);
    }

    pub(crate) fn request(&mut self, full: bool) {
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            let s = &mut state.counts;
            s.overflowed |= if full {
                add(&mut s.full_damage_requests, 1)
            } else {
                add(&mut s.incremental_damage_requests, 1)
            };
        });
    }

    pub(crate) fn completed(&mut self, decision: Option<Decision>) {
        self.outcome = Some(Outcome::Layout(decision));
    }
}

impl Drop for FlushCapture {
    fn drop(&mut self) {
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            let s = &mut state.counts;
            if s.active_flushes == 0 {
                s.overflowed = true;
            } else {
                s.active_flushes -= 1;
            }
            // The guard is created first and drops last in flush_layout: a
            // panic in layout, verification or teardown cannot count success.
            let outcome = if std::thread::panicking() { None } else { self.outcome };
            s.overflowed |= match outcome {
                Some(Outcome::Clean) => add(&mut s.clean_flag_returns, 1),
                Some(Outcome::Layout(Some(Decision::NoopReuse))) => add(&mut s.noop_layout_completions, 1),
                Some(Outcome::Layout(Some(Decision::FullRequested))) => add(&mut s.full_layout_requested_completions, 1),
                Some(Outcome::Layout(Some(Decision::FullFallback))) => add(&mut s.full_layout_fallback_completions, 1),
                Some(Outcome::Layout(Some(Decision::Incremental))) => add(&mut s.incremental_layout_completions, 1),
                Some(Outcome::Layout(None)) => add(&mut s.missing_layout_decisions, 1),
                None => add(&mut s.aborted_flushes, 1),
            };
        });
    }
}

pub(crate) enum Event {
    StyleFull,
    StyleIncremental(u64),
    StylePatch,
    DisplayListCommit,
}

pub(crate) fn record(event: Event) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if !state.enabled {
            return;
        }
        let s = &mut state.counts;
        s.overflowed |= match event {
            Event::StyleFull => add(&mut s.style_full_updates, 1),
            Event::StyleIncremental(elements) => {
                let overflow = add(&mut s.style_incremental_updates, 1);
                add(&mut s.incremental_elements_restyled, elements) | overflow
            }
            Event::StylePatch => add(&mut s.style_overlay_patches, 1),
            Event::DisplayListCommit => add(&mut s.display_list_commits, 1),
        };
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Engine, Page, Viewport};
    use crate::net::Url;

    // Thread-local override: no process environment mutation or parallel-test race.
    struct TestScope(State);

    impl TestScope {
        fn new(enabled: bool) -> Self {
            Self(STATE.with(|state| {
                let mut state = state.borrow_mut();
                assert_eq!(state.counts.active_flushes, 0);
                std::mem::replace(&mut *state, State { enabled, counts: Snapshot::default() })
            }))
        }
    }

    impl Drop for TestScope {
        fn drop(&mut self) {
            STATE.with(|state| {
                assert_eq!(state.borrow().counts.active_flushes, 0);
                std::mem::swap(&mut *state.borrow_mut(), &mut self.0);
            });
        }
    }

    fn load(html: &str) -> (Engine, Page) {
        let mut engine = Engine::new_bundled_fonts_only();
        engine.scripting = false;
        engine.loader.load_scripts = false;
        engine.loader.load_images = false;
        let mut page = engine.load_html(html, Url::parse("https://example.test/").unwrap(), Viewport { width: 800.0, height: 600.0 });
        engine.flush_layout(&mut page);
        (engine, page)
    }

    #[test]
    fn clean_queries_are_entries_without_reconstruction() {
        let (engine, mut page) = load("<div id=box style='width:10px;height:10px'></div>");
        let node = page.doc.element_by_id("box").unwrap();
        let before = page.rect_of(node).unwrap();
        let _capture = TestScope::new(true);
        for _ in 0..5 {
            engine.flush_layout(&mut page);
            assert_eq!(page.rect_of(node).unwrap(), before);
        }
        let s = snapshot().unwrap();
        assert_eq!(s.flush_entries, 5);
        assert_eq!(s.clean_flag_returns, 5);
        assert_eq!(s.full_layout_requested_completions + s.full_layout_fallback_completions + s.incremental_layout_completions + s.noop_layout_completions, 0);
        assert_eq!(s.style_full_updates + s.style_incremental_updates, 0);
        assert!(s.is_complete());
    }

    #[test]
    fn mutation_and_explicit_full_invalidation_rebuild_then_become_clean() {
        let (engine, mut page) = load("<div id=box style='width:10px;height:10px'></div>");
        let node = page.doc.element_by_id("box").unwrap();
        let before = page.incr.stats.clone();
        let _capture = TestScope::new(true);
        page.doc.set_attr(node, "style", Some("width:20px;height:10px"));
        engine.flush_layout(&mut page);
        assert_eq!(page.rect_of(node).unwrap().w, 20.0);
        page.incr.invalidate_layout();
        engine.flush_layout(&mut page);
        engine.flush_display_list(&mut page);
        let s = snapshot().unwrap();
        assert_eq!(s.flush_entries, 3);
        assert_eq!(s.clean_flag_returns, 1);
        assert_eq!(s.full_damage_requests, 1);
        assert_eq!(s.incremental_damage_requests, 1);
        assert_eq!(s.full_layout_requested_completions, 1);
        assert_eq!(s.incremental_layout_completions, 1);
        assert_eq!(s.full_layout_fallback_completions, 0);
        assert_eq!(s.noop_layout_completions, 0);
        assert_eq!(s.style_full_updates, page.incr.stats.style_full - before.style_full);
        assert_eq!(s.style_incremental_updates + s.style_overlay_patches, page.incr.stats.style_incremental - before.style_incremental);
        assert_eq!(s.incremental_elements_restyled, page.incr.stats.elements_restyled - before.elements_restyled);
        assert_eq!(s.display_list_commits, 1);
        assert!(s.is_complete());
    }

    #[test]
    fn empty_incremental_damage_returns_the_previous_tree_without_rebuilding() {
        let (engine, mut page) = load("<div id=box style='width:10px;height:10px'></div>");
        let before_tree = page.layout_dump();
        let before_legacy_incremental = page.incr.stats.layout_incremental;
        let _capture = TestScope::new(true);
        page.incr.diagnostic_test_empty_incremental_request();
        engine.flush_layout(&mut page);
        assert_eq!(page.layout_dump(), before_tree);
        let s = snapshot().unwrap();
        assert_eq!(s.flush_entries, 1);
        assert_eq!(s.clean_flag_returns, 0);
        assert_eq!(s.incremental_damage_requests, 1);
        assert_eq!(s.noop_layout_completions, 1);
        assert_eq!(s.incremental_layout_completions, 0);
        assert_eq!(s.full_layout_fallback_completions, 0);
        // The old request counter increments even though no layout ran.
        assert_eq!(page.incr.stats.layout_incremental, before_legacy_incremental + 1);
        assert!(s.is_complete());
    }

    #[test]
    fn unusable_previous_tree_falls_back_to_full_despite_incremental_request() {
        let (engine, mut page) = load("<div id=box style='width:37px;height:19px'></div>");
        let node = page.doc.element_by_id("box").unwrap();
        let before_legacy_incremental = page.incr.stats.layout_incremental;
        let _capture = TestScope::new(true);
        page.layout = rabbithole_layout::LayoutTree::empty();
        assert_eq!(page.layout.root.node, None);
        page.incr.diagnostic_test_empty_incremental_request();
        engine.flush_layout(&mut page);
        assert_eq!(page.layout.root.node, page.doc.html_element());
        let rect = page.rect_of(node).unwrap();
        assert_eq!((rect.w, rect.h), (37.0, 19.0));
        let s = snapshot().unwrap();
        assert_eq!(s.full_damage_requests, 0);
        assert_eq!(s.incremental_damage_requests, 1);
        assert_eq!(s.full_layout_fallback_completions, 1);
        assert_eq!(s.full_layout_requested_completions, 0);
        assert_eq!(s.incremental_layout_completions, 0);
        assert_eq!(s.noop_layout_completions, 0);
        // Same legacy class as the no-op case; independent geometry and
        // the observed branch distinguish the actual outcomes.
        assert_eq!(page.incr.stats.layout_incremental, before_legacy_incremental + 1);
        assert!(s.is_complete());
    }

    #[test]
    fn frame_counts_survive_retirement_and_reset_has_an_explicit_boundary() {
        let (engine, mut page) = load("<iframe id=f></iframe>");
        let node = page.doc.element_by_id("f").unwrap();
        engine.create_blank_frame(&mut page, node).unwrap();
        engine.flush_layout(&mut page);
        engine.flush_layout(page.frames.children[0].page.as_mut().unwrap());
        let _capture = TestScope::new(true);
        engine.flush_layout(&mut page);
        engine.flush_layout(page.frames.children[0].page.as_mut().unwrap());
        page.frames.children.clear();
        let s = snapshot().unwrap();
        assert_eq!(s.flush_entries, 2);
        assert_eq!(s.clean_flag_returns, 2);
        assert_eq!(reset().unwrap(), Some(s));
        assert_eq!(snapshot().unwrap(), Snapshot::default());
    }

    #[test]
    fn disabled_capture_preserves_real_layout_without_recording() {
        let (engine, mut page) = load("<div id=box style='width:10px'></div>");
        let node = page.doc.element_by_id("box").unwrap();
        let _capture = TestScope::new(false);
        page.doc.set_attr(node, "style", Some("width:20px"));
        engine.flush_layout(&mut page);
        assert_eq!(page.rect_of(node).unwrap().w, 20.0);
        assert_eq!(snapshot(), None);
        assert_eq!(reset().unwrap(), None);
        assert_eq!(STATE.with(|state| state.borrow().counts), Snapshot::default());
    }

    #[test]
    fn nested_outcomes_reset_rejection_and_unwind_are_accounted_once() {
        let _capture = TestScope::new(true);
        let mut outer = begin_flush().unwrap();
        assert!(reset().is_err());
        {
            let mut inner = begin_flush().unwrap();
            inner.clean();
        }
        outer.request(false);
        outer.completed(Some(Decision::Incremental));
        drop(outer);
        let s = snapshot().unwrap();
        assert_eq!(s.flush_entries, 2);
        assert_eq!(s.clean_flag_returns, 1);
        assert_eq!(s.incremental_layout_completions, 1);
        assert_eq!(s.max_flush_depth, 2);
        assert!(s.is_complete());
        assert!(std::panic::catch_unwind(|| {
            let mut capture = begin_flush().unwrap();
            capture.request(true);
            capture.completed(Some(Decision::FullRequested));
            panic!("controlled unwind after outcome selection");
        }).is_err());
        let s = snapshot().unwrap();
        assert_eq!(s.flush_entries, 3);
        assert_eq!(s.full_layout_requested_completions, 0);
        assert_eq!(s.aborted_flushes, 1);
        assert_eq!(s.active_flushes, 0);
        assert!(!s.is_complete());
    }

    #[test]
    fn saturated_counts_are_flagged_and_never_wrap() {
        let _capture = TestScope::new(true);
        STATE.with(|state| state.borrow_mut().counts.flush_entries = u64::MAX);
        let mut capture = begin_flush().unwrap();
        capture.clean();
        drop(capture);
        let s = snapshot().unwrap();
        assert_eq!(s.flush_entries, u64::MAX);
        assert!(s.overflowed);
        assert!(!s.is_complete());
    }

    #[test]
    fn missing_layout_decision_is_incomplete_and_never_defaults_to_incremental() {
        let _capture = TestScope::new(true);
        let mut capture = begin_flush().unwrap();
        capture.request(false);
        capture.completed(None);
        drop(capture);
        let s = snapshot().unwrap();
        assert_eq!(s.missing_layout_decisions, 1);
        assert_eq!(s.incremental_layout_completions, 0);
        assert!(!s.is_complete());
    }
}
