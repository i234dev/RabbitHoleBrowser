//! Rooted, document-bound console observations. Dropping one stops observation,
//! without cancelling unrelated page timers, fetches or promise reactions.
use crate::{env::HostEnv, host::HostState, ScriptHost};
use rabbithole_js::{heap::PromiseState, ObjectKind, Root};
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::time::Instant;

pub struct ConsoleEvaluation {
    root: Root,
    owner: Weak<RefCell<HostState>>,
    document: Weak<()>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ConsolePoll {
    Pending,
    Resolved(String),
    Rejected(String),
}

impl ScriptHost {
    fn console_owner(&self, evaluation: &ConsoleEvaluation) -> bool {
        evaluation.owner.upgrade().is_some_and(|owner| Rc::ptr_eq(&owner, &self.state))
            && evaluation.document.upgrade().is_some_and(|document| Rc::ptr_eq(&document, &self.state.borrow().document_token))
    }

    // The wall cap is thread-local and inherited by callbacks/child realms.
    // Restore task settings even if host-side code panics outside enter().
    fn console_deadline<T>(&mut self, deadline: Instant, f: impl FnOnce(&mut Self) -> Result<T, String>) -> Result<T, String> {
        if !self.realm.frames.is_empty() || self.realm.native_depth != 0 {
            return Err("Console evaluation cannot reenter running JavaScript".into());
        }
        let now = Instant::now();
        let mut cap = self.realm.wall_deadline().map(|old| old.min(deadline)).unwrap_or(deadline);
        if let Some(remaining) = self.realm.remaining_watchdog_budget() {
            cap = cap.min(now.checked_add(remaining).unwrap_or(cap));
        }
        if now >= cap { return Err("Console evaluation deadline expired".into()); }
        let saved = self.realm.save_watchdog();
        self.realm.replace_wall_deadline(Some(cap));
        self.realm.set_task_budget(None);
        self.realm.set_deadline(Some(cap));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self)));
        self.realm.restore_watchdog(saved);
        match result {
            Ok(result) if Instant::now() < cap => result,
            Ok(_) => Err("Console evaluation deadline expired".into()),
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    pub fn start_console_evaluation(&mut self, env: &mut dyn HostEnv, source: &str, deadline: Instant) -> Result<ConsoleEvaluation, String> {
        let owner = Rc::downgrade(&self.state);
        let document = Rc::downgrade(&self.state.borrow().document_token);
        let root = self.console_deadline(deadline, |host| host.enter(env, |realm| {
            let value = realm.eval_console(source).map_err(|error| realm.error_to_string(&error))?;
            let root = realm.root(value);
            if let Some(id) = root.object() {
                if matches!(realm.obj(id).kind, ObjectKind::Promise(_)) { rabbithole_js::builtins::promise::observe(realm, id); }
            }
            Ok(root)
        }))?;
        let evaluation = ConsoleEvaluation { root, owner, document };
        if !self.console_owner(&evaluation) { return Err("Console evaluation document was replaced".into()); }
        Ok(evaluation)
    }

    pub fn poll_console_evaluation(&mut self, env: &mut dyn HostEnv, evaluation: &ConsoleEvaluation, deadline: Instant) -> Result<ConsolePoll, String> {
        if !self.console_owner(evaluation) { return Err("Console evaluation document was replaced".into()); }
        let result = self.console_deadline(deadline, |host| host.enter(env, |realm| {
            let value = evaluation.root.value();
            let settled = value.as_object().and_then(|id| match &realm.obj(id).kind {
                ObjectKind::Promise(promise) => Some((promise.state, promise.value.clone())),
                _ => None,
            });
            Ok(match settled {
                Some((PromiseState::Pending, _)) => ConsolePoll::Pending,
                Some((PromiseState::Rejected, value)) => ConsolePoll::Rejected(realm.error_to_string(&value)),
                Some((PromiseState::Fulfilled, value)) => ConsolePoll::Resolved(rabbithole_js::builtins::global::inspect(realm, &value, 0)),
                None => ConsolePoll::Resolved(rabbithole_js::builtins::global::inspect(realm, &value, 0)),
            })
        }));
        if !self.console_owner(evaluation) { return Err("Console evaluation document was replaced".into()); }
        result
    }

    /// Advance ready microtasks/timers/background responses without waiting.
    pub fn pump_console_tasks(&mut self, env: &mut dyn HostEnv, now_ms: f64, deadline: Instant) -> Result<(), String> {
        self.console_deadline(deadline, |host| {
            host.run_timers(env, now_ms);
            if Instant::now() < deadline { host.run_jobs(env); }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{setup, run};
    use std::time::Duration;

    fn deadline() -> Instant { Instant::now() + Duration::from_secs(2) }
    fn evaluate(host: &mut ScriptHost, env: &mut crate::tests::MockEnv, source: &str) -> ConsolePoll {
        let deadline = deadline();
        let observation = host.start_console_evaluation(env, source, deadline).unwrap();
        let now = env.now;
        host.pump_console_tasks(env, now, deadline).unwrap();
        host.poll_console_evaluation(env, &observation, deadline).unwrap()
    }

    #[test]
    fn console_await_tracks_completion_and_preserves_normal_script_scope() {
        let (mut env, mut host) = setup("<p>console</p>");
        host.mark_loaded();
        for (source, expected) in [
            ("var normalAwaitWord = 'await'; /* await */ 3", "3"),
            ("async function nestedAwait(){return await 9}; var normalNested = 4; normalNested", "4"),
            ("await(Promise.resolve(42))", "42"),
            ("const localAwait = await Promise.resolve(40); localAwait + 2;;;;", "42"),
            ("await 0; 7; const trailingDeclaration=1", "7"),
            ("{ await Promise.resolve(42); }", "42"),
            ("await 0; if (true) { 8; } else { 9; }", "8"),
            ("await 0; this === globalThis", "true"),
            ("await 0; consoleSloppyAssignment=11; consoleSloppyAssignment", "11"),
            ("class AwaitClass { [await Promise.resolve('method')](){return 1} }; typeof AwaitClass.prototype.method", "'function'"),
            ("for await (const x of [1,2]) { x; }", "2"),
            ("try { await 0; 9; } finally { 10; }", "9"),
        ] {
            assert_eq!(evaluate(&mut host, &mut env, source), ConsolePoll::Resolved(expected.into()), "{source}");
        }
        assert_eq!(evaluate(&mut host, &mut env, "normalAwaitWord + normalNested"), ConsolePoll::Resolved("'await4'".into()));
        assert_eq!(evaluate(&mut host, &mut env, "typeof localAwait"), ConsolePoll::Resolved("'undefined'".into()));
        // Fallback parses everything before running any side effect.
        for source in ["globalThis.shouldNotRun=1; await 0; const =", "globalThis.shouldNotRun=1; await 0; import.meta", "globalThis.shouldNotRun=1; await 0; function f(){return import.meta}", "globalThis.shouldNotRun=1; await 0; export {}", "import x from 'missing'; await 0"] {
            assert!(host.start_console_evaluation(&mut env, source, deadline()).is_err(), "{source}");
        }
        assert_eq!(evaluate(&mut host, &mut env, "typeof shouldNotRun"), ConsolePoll::Resolved("'undefined'".into()));
    }

    #[test]
    fn console_observation_is_rooted_and_rejects_foreign_or_replaced_documents() {
        let (mut env, mut host) = setup("<p>console</p>");
        host.mark_loaded();
        let handle = host.start_console_evaluation(&mut env, "new Promise(r => { globalThis.resolveConsole = r; })", deadline()).unwrap();
        host.gc();
        host.gc_idle(Instant::now() + Duration::from_millis(20));
        run(&mut env, &mut host, "document.body.appendChild(document.createElement('span')); resolveConsole({answer:42})");
        assert!(matches!(host.poll_console_evaluation(&mut env, &handle, deadline()), Ok(ConsolePoll::Resolved(value)) if value.contains("42")));
        let (mut other_env, mut other) = setup("<p>other</p>");
        assert!(other.poll_console_evaluation(&mut other_env, &handle, deadline()).unwrap_err().contains("document was replaced"));
        let old = host.start_console_evaluation(&mut env, "new Promise(r=>setTimeout(()=>r(8),10))", deadline()).unwrap();
        run(&mut env, &mut host, "document.open(); document.write('<p>replacement</p>'); document.close()");
        host.gc();
        assert!(host.poll_console_evaluation(&mut env, &old, deadline()).unwrap_err().contains("document was replaced"));
        drop(old);
        drop(handle);
        assert_eq!(evaluate(&mut host, &mut env, "5"), ConsolePoll::Resolved("5".into()));
    }

    #[test]
    fn console_timeout_drops_observation_without_cancelling_later_page_work() {
        let (mut env, mut host) = setup("<p>console</p>");
        host.mark_loaded();
        let handle = host.start_console_evaluation(&mut env, "new Promise(r=>setTimeout(()=>{globalThis.afterTimeout=7;r(7)},50))", deadline()).unwrap();
        assert_eq!(host.poll_console_evaluation(&mut env, &handle, deadline()).unwrap(), ConsolePoll::Pending);
        drop(handle);
        env.now += 60.0;
        host.pump_console_tasks(&mut env, 1060.0, deadline()).unwrap();
        assert_eq!(evaluate(&mut host, &mut env, "afterTimeout"), ConsolePoll::Resolved("7".into()));
        assert!(matches!(evaluate(&mut host, &mut env, "Promise.reject(new Error('reject-marker'))"), ConsolePoll::Rejected(error) if error.contains("reject-marker")));
        assert!(matches!(evaluate(&mut host, &mut env, "await ({get then(){throw new Error('then-marker')}})"), ConsolePoll::Rejected(error) if error.contains("then-marker")));
    }

    #[test]
    fn console_wall_cap_bounds_microtasks_and_restores_watchdog_on_unwind() {
        let (mut env, mut host) = setup("<p>console</p>");
        host.mark_loaded();
        let budget = host.realm.task_budget;
        let deadline = Instant::now() + Duration::from_millis(30);
        let handle = host.start_console_evaluation(&mut env, "globalThis.stopConsoleJobs=false; Promise.resolve().then(function recur(){if(!stopConsoleJobs) Promise.resolve().then(recur)}); new Promise(()=>{})", deadline).unwrap();
        let started = Instant::now();
        assert!(host.pump_console_tasks(&mut env, 1000.0, deadline).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(host.realm.task_budget, budget);
        assert!(host.realm.wall_deadline().is_none());
        drop(handle);
        assert_eq!(evaluate(&mut host, &mut env, "stopConsoleJobs=true; 6"), ConsolePoll::Resolved("6".into()));
        let saved = host.realm.save_watchdog();
        let outer = Instant::now() + Duration::from_millis(20);
        host.realm.replace_wall_deadline(Some(outer));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _: Result<(), String> = host.console_deadline(Instant::now() + Duration::from_secs(10), |host| {
                assert_eq!(host.realm.wall_deadline(), Some(outer));
                panic!("scope-panic")
            });
        }));
        assert!(result.is_err());
        assert_eq!(host.realm.wall_deadline(), Some(outer));
        host.realm.restore_watchdog(saved);
        host.realm.set_deadline(Some(Instant::now() - Duration::from_secs(1)));
        assert!(host.start_console_evaluation(&mut env, "42", Instant::now() + Duration::from_secs(1)).is_err());
        host.realm.set_deadline(None);
    }
}
