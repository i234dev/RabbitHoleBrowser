//! Observation of one `layout_incremental` call, not a process-wide count.
//! Entirely absent without `layout-diagnostics`; no clocks, heap or callbacks.

use crate::{LayoutContext, LayoutDamage, LayoutTree};
use std::cell::Cell;
use std::marker::PhantomData;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    /// The full-layout function returned after an explicit full request.
    FullRequested,
    /// The full-layout function returned because the previous tree was unusable.
    FullFallback,
    /// The previous tree was returned without invoking either layout algorithm.
    NoopReuse,
    /// The incremental layouter returned with a nonempty dirty set.
    Incremental,
}

thread_local! {
    // None=inactive, Some(None)=pending, Some(Some(d))=completed decision.
    static OBSERVATION: Cell<Option<Option<Decision>>> = const { Cell::new(None) };
}

struct Observation {
    previous: Option<Option<Decision>>,
    _same_thread: PhantomData<Rc<()>>,
}

impl Observation {
    fn begin() -> Self {
        Self { previous: OBSERVATION.with(|slot| slot.replace(Some(None))), _same_thread: PhantomData }
    }

    fn decision(&self) -> Option<Decision> {
        OBSERVATION.with(|slot| slot.get().flatten())
    }
}

impl Drop for Observation {
    fn drop(&mut self) {
        OBSERVATION.with(|slot| slot.set(self.previous));
    }
}

pub(crate) fn record(decision: Decision) {
    OBSERVATION.with(|slot| {
        if slot.get().is_some() {
            slot.set(Some(Some(decision)));
        }
    });
}

/// Return the actual completed branch along with the normal layout result.
/// Missing observation stays `None`; it never defaults to incremental.
/// An unwind restores the enclosing observation and returns no result.
pub fn layout_incremental_observed(prev: LayoutTree, ctx: &LayoutContext, damage: &LayoutDamage) -> (LayoutTree, Option<Decision>) {
    let observation = Observation::begin();
    let tree = crate::layout_incremental(prev, ctx, damage);
    let decision = observation.decision();
    (tree, decision)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_observations_restore_pending_and_completed_enclosing_state() {
        assert_eq!(OBSERVATION.with(Cell::get), None);
        let outer = Observation::begin();
        {
            let inner = Observation::begin();
            record(Decision::NoopReuse);
            assert_eq!(inner.decision(), Some(Decision::NoopReuse));
        }
        assert_eq!(outer.decision(), None);
        record(Decision::FullRequested);
        {
            let inner = Observation::begin();
            record(Decision::Incremental);
            assert_eq!(inner.decision(), Some(Decision::Incremental));
        }
        assert_eq!(outer.decision(), Some(Decision::FullRequested));
        drop(outer);
        assert_eq!(OBSERVATION.with(Cell::get), None);
    }

    #[test]
    fn unwind_restores_enclosing_observation_and_missing_hook_stays_missing() {
        let outer = Observation::begin();
        record(Decision::FullFallback);
        assert!(std::panic::catch_unwind(|| {
            let _inner = Observation::begin();
            record(Decision::Incremental);
            panic!("controlled observation unwind");
        }).is_err());
        assert_eq!(outer.decision(), Some(Decision::FullFallback));
        {
            let missing = Observation::begin();
            assert_eq!(missing.decision(), None);
        }
        assert_eq!(outer.decision(), Some(Decision::FullFallback));
    }
}
