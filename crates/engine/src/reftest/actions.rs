//! Harness-only input pump. JavaScript returns before any Engine input call.
use super::{check_watchdog, percent_decode, waiting};
use crate::scripting::{EngineEnv, ScriptOutcome};
use crate::{ClickAction, Engine, Modifiers, Page};
use std::time::{Duration, Instant};

pub(super) const VENDOR: &str = include_str!("testdriver-vendor.js");
const BRIDGE: &str = "window.__rhWptMouseActions";

/// Uses the normal task budget plus the harness deadline for poll/ack. This
/// does not add a global Realm ceiling or extend event/timer watchdog budgets.
fn checkpoint(engine: &Engine, page: &mut Page, source: &str, deadline: Instant, poll: bool) -> Result<String, String> {
    if Instant::now() >= deadline { return Err("WPT actions exceeded snapshot deadline".into()); }
    engine.ensure_script_host(page);
    let Some(mut host) = page.script.host.take() else { return Err("WPT actions need a script realm".into()) };
    let result = {
        let mut env = EngineEnv { engine, page };
        let budget = host.realm().task_budget;
        let limit = deadline.min(Instant::now() + budget.unwrap_or_else(rabbithole_script::task_budget));
        // begin_task inside the checkpoint must not restart this deadline.
        host.realm().set_task_budget(None);
        host.realm().set_deadline(Some(limit));
        // Poll drains first: no queued JS may move an Element after its
        // rectangle is read but before Rust dispatches the corresponding input.
        if poll { host.run_jobs(&mut env); }
        let result = if host.watchdog_abort_count() == 0 { host.eval_for_console(&mut env, source) }
            else { "checkpoint watchdog aborted".to_string() };
        if !poll && host.watchdog_abort_count() == 0 { host.run_jobs(&mut env); }
        host.realm().set_task_budget(budget);
        host.realm().set_deadline(None);
        result
    };
    page.script.host = Some(host);
    let outcome = engine.settle(page, false);
    observe(engine, page, outcome)?;
    check_watchdog(page)?;
    if Instant::now() >= deadline { return Err("WPT actions exceeded snapshot deadline".into()); }
    // inspect() wraps strings in single quotes without escaping their content.
    result.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')).map(str::to_owned)
        .ok_or_else(|| format!("WPT action checkpoint failed: {result}"))
}

fn observe(engine: &Engine, page: &mut Page, outcome: ScriptOutcome) -> Result<(), String> {
    if outcome.navigation.is_some() || !outcome.new_windows.is_empty() {
        return Err("WPT actions cancelled by navigation or a new window".into());
    }
    if !outcome.dialogs.is_empty() { return Err("WPT actions do not support dialogs".into()); }
    if let Some((x, y)) = outcome.scroll_to { engine.set_page_scroll(page, x, y); }
    Ok(())
}

fn context(page: &Page, frame: u64) -> Result<(), String> {
    check_watchdog(page)?;
    if page.frame_id != frame || page.frame_depth != 0 {
        return Err("WPT actions browsing context changed".into());
    }
    // Coordinates otherwise enter child Engine pointer APIs, whose navigation
    // can be consumed by the frame embedder. This first subset excludes frames.
    if !page.frames.children.is_empty() { return Err("WPT mouse actions do not support pages with frames".into()); }
    if page.script.pending_navigation.is_some() || !page.script.new_windows.is_empty() {
        return Err("WPT actions cancelled by pending navigation or a new window".into());
    }
    if !page.script.dialogs.is_empty() { return Err("WPT actions do not support pending dialogs".into()); }
    Ok(())
}

fn tick(engine: &Engine, page: &mut Page, frame: u64, deadline: Instant) -> Result<(), String> {
    context(page, frame)?;
    if Instant::now() >= deadline { return Err("WPT actions exceeded snapshot deadline".into()); }
    let outcome = engine.tick(page);
    observe(engine, page, outcome)?;
    context(page, frame)
}

#[derive(Debug)]
struct Step { request: u32, index: u32, verb: char, x: f32, y: f32, wait: u64 }

fn parse_step(text: &str) -> Result<Step, String> {
    let parts: Vec<_> = text.split(' ').collect();
    if parts.len() != 7 || parts[0] != "S" { return Err("Invalid WPT action protocol response".into()); }
    let number = |part: &str| part.parse::<u32>().map_err(|_| "Invalid WPT action integer".to_string());
    let request = number(parts[1])?;
    let index = number(parts[2])?;
    let verb = match parts[3] { "M" => 'M', "D" => 'D', "U" => 'U', "P" => 'P', _ => return Err("Invalid WPT input verb".into()) };
    let x = parts[4].parse::<f32>().map_err(|_| "Invalid WPT pointer coordinate")?;
    let y = parts[5].parse::<f32>().map_err(|_| "Invalid WPT pointer coordinate")?;
    let wait = number(parts[6])? as u64;
    if request == 0 || request > 65535 || index >= 128 || !x.is_finite() || !y.is_finite() || wait > 1000 {
        return Err("WPT action protocol limit exceeded".into());
    }
    Ok(Step { request, index, verb, x, y, wait })
}

fn execute(engine: &Engine, page: &mut Page, step: &Step, frame: u64, deadline: Instant) -> Result<(), String> {
    context(page, frame)?;
    if Instant::now() >= deadline { return Err("WPT actions exceeded snapshot deadline".into()); }
    if step.x < 0.0 || step.y < 0.0 || step.x > page.viewport.width || step.y > page.viewport.height {
        return Err("WPT pointer coordinates outside viewport".into());
    }
    engine.flush_display_list(page);
    let modifiers = Modifiers::default();
    let outcome = match step.verb {
        'M' => engine.pointer_move(page, step.x, step.y, modifiers),
        'D' => engine.pointer_down(page, step.x, step.y, 0, modifiers).outcome,
        'U' => {
            let up = engine.pointer_up(page, step.x, step.y, 0, modifiers);
            let mut outcome = up.outcome;
            match up.action {
                ClickAction::Navigate { .. } | ClickAction::Submit { .. } => {
                    return Err("WPT mouse activation requested navigation or form submission".into());
                }
                ClickAction::FocusControl(node) => outcome.merge(engine.focus_element(page, Some(node))),
                ClickAction::None | ClickAction::Changed => {}
            }
            outcome
        }
        'P' => ScriptOutcome::default(),
        _ => unreachable!(),
    };
    observe(engine, page, outcome)?;
    context(page, frame)?;
    // A tick lasts the maximum pause duration, not the sum across sources.
    let until = Instant::now() + Duration::from_millis(step.wait);
    loop {
        tick(engine, page, frame, deadline)?;
        if Instant::now() >= until { break; }
        let remaining = until.min(deadline).saturating_duration_since(Instant::now());
        if remaining.is_zero() { return Err("WPT actions exceeded snapshot deadline".into()); }
        std::thread::sleep(remaining.min(Duration::from_millis(5)));
    }
    let answer = checkpoint(engine, page, &format!("{BRIDGE}.ack({},{})", step.request, step.index), deadline, false)?;
    if let Some(error) = answer.strip_prefix("E ") {
        return Err(format!("WPT mouse actions: {}", percent_decode(error)));
    }
    if answer != "A" { return Err(format!("WPT action acknowledgement failed: {answer}")); }
    context(page, frame)
}

fn pump(engine: &Engine, page: &mut Page, deadline: Instant) -> Result<super::Frame, String> {
    let frame = page.frame_id;
    context(page, frame)?;
    // Each nonempty sequence has at least one tick; this also bounds chains
    // enqueued by Promise callbacks, beyond the per-queue JavaScript limits.
    let mut steps = 0usize;
    let mut idle_deadline = None;
    loop {
        context(page, frame)?;
        let answer = checkpoint(engine, page, &format!("{BRIDGE} ? {BRIDGE}.poll() : 'I'"), deadline, true)?;
        if answer.starts_with("E ") { return Err(format!("WPT mouse actions: {}", percent_decode(&answer[2..]))); }
        if answer != "I" {
            idle_deadline = None;
            steps += 1;
            if steps > 1024 { return Err("WPT action run step limit exceeded".into()); }
            let step = parse_step(&answer)?;
            execute(engine, page, &step, frame, deadline)?;
            continue;
        }
        if waiting(page) {
            idle_deadline = None;
            tick(engine, page, frame, deadline)?;
            std::thread::sleep(deadline.saturating_duration_since(Instant::now()).min(Duration::from_millis(5)));
            continue;
        }
        // Observe every tick outcome: opaque settle_for_snapshot would swallow
        // location changes from a timer/RAF and let a stale request continue.
        tick(engine, page, frame, deadline)?;
        let pending = checkpoint(engine, page, &format!("{BRIDGE} ? {BRIDGE}.poll() : 'I'"), deadline, true)?;
        if pending == "I" && !waiting(page) {
            let settle_until = *idle_deadline.get_or_insert_with(|| deadline.min(Instant::now() + Duration::from_secs(2)));
            let now = engine.start.elapsed().as_secs_f64() * 1000.0;
            let scripts_due = page.next_script_due().is_some_and(|due| due <= now + 50.0);
            if Instant::now() < settle_until && (scripts_due || page.images_in_flight() > 0) {
                std::thread::sleep(settle_until.saturating_duration_since(Instant::now()).min(Duration::from_millis(5)));
                continue;
            }
            engine.flush_display_list(page);
            return Ok(engine.snapshot(page));
        }
        if pending.starts_with("E ") { return Err(format!("WPT mouse actions: {}", percent_decode(&pending[2..]))); }
        // poll reserves a step, so execute it rather than polling it twice.
        if pending != "I" {
            idle_deadline = None;
            steps += 1;
            if steps > 1024 { return Err("WPT action run step limit exceeded".into()); }
            execute(engine, page, &parse_step(&pending)?, frame, deadline)?;
        }
    }
}

pub(super) fn finish(engine: &Engine, page: &mut Page, max_wait: Duration) -> Result<super::Frame, String> {
    let deadline = Instant::now() + max_wait;
    let result = (|| {
        let frame = page.frame_id;
        context(page, frame)?;
        tick(engine, page, frame, deadline)?;
        if waiting(page) {
            let _ = engine.snapshot(page);
            // The same bounded checkpoint pattern, but retain ScriptOutcome.
            checkpoint(engine, page, "document.documentElement.dispatchEvent(new Event('TestRendered',{bubbles:true})); 'rendered'", deadline, false)?;
        }
        pump(engine, page, deadline)
    })();
    if result.is_err() && Instant::now() < deadline && page.script.watchdog_abort_count() == 0 {
        // No recovery input, button releases or move-away: the failed page is
        // discarded. Its queue receives rejection before the harness returns.
        let _ = checkpoint(engine, page, &format!("{BRIDGE} ? {BRIDGE}.cancel('Harness cancelled actions') : 'cancelled'"), deadline, false);
    }
    result
}
