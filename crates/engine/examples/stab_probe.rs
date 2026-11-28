//! Layout stability probe: load a page, then hover over a grid of points,
//! scroll through it and hover again, checking after every interaction
//! that the incrementally maintained styles, layout and display list equal
//! a from-scratch computation, and that a from-scratch layout is
//! idempotent. Prints the style / layout work each interaction caused.
//!
//! `cargo run --release -p rabbithole-engine --example stab_probe -- <url|file> [width] [height]`
//!
//! * `RH_JS_WAIT` (ms, default 5000): script time before interacting.
//! * `RH_STAB_GRID` (px, default 80): hover grid spacing.
//! * `RH_STAB_SCROLLS` (default 6): scroll steps (half a viewport each).
//! * `RH_STAB_SETTLE` (ms, default 150): script time after each step.
//! * `RH_STAB_MAX_DIVERGENCES` (default 20): stop after this many.
//! * `RH_NO_JS`, `RH_NO_IMAGES`: as in `shot`.
use rabbithole_engine::incremental::IncrementalStats;
use rabbithole_engine::{Engine, Page, Viewport};
use std::time::{Duration, Instant};

#[global_allocator]
static ALLOC: rabbithole_engine::script::js::alloc::RhAlloc = rabbithole_engine::script::js::alloc::RhAlloc;

fn env_num<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// Run scripts (timers, animation frames, image loads) for `ms`.
fn settle(engine: &Engine, page: &mut Page, ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        let now = engine.start.elapsed().as_secs_f64() * 1000.0;
        let wait = match page.next_script_due() {
            Some(due) if due > now => ((due - now) as u64).min(10),
            Some(_) => 0,
            None if page.images_in_flight() > 0 => 10,
            None => break,
        };
        if wait > 0 {
            std::thread::sleep(Duration::from_millis(wait).min(deadline.saturating_duration_since(Instant::now())));
        }
        let _ = engine.tick(page);
    }
}

fn desc(page: &Page, n: Option<usize>) -> String {
    let Some(n) = n else { return "-".into() };
    let d = &page.doc;
    let one = |n: usize| {
        format!(
            "{}{}{}",
            d.tag(n).unwrap_or("#"),
            d.attr(n, "id").map(|v| format!("#{v}")).unwrap_or_default(),
            d.attr(n, "class").map(|v| v.split_whitespace().take(2).map(|c| format!(".{c}")).collect::<String>()).unwrap_or_default()
        )
    };
    let mut parts = vec![one(n)];
    let mut cur = d.parent(n);
    while let Some(p) = cur {
        if parts.len() >= 4 || d.element(p).is_none() {
            break;
        }
        parts.push(one(p));
        cur = d.parent(p);
    }
    parts.reverse();
    parts.join(" > ")
}

struct Probe {
    restyled: u64,
    styles: u64,
    divergences: usize,
    max: usize,
    checks: usize,
}

impl Probe {
    /// Flush and compare; `before` / `t` are the stats and time when the
    /// interaction started (its work is reported).
    fn check(&mut self, engine: &Engine, page: &mut Page, label: &str, before: IncrementalStats, t: Instant) {
        // What the interaction left pending is flushed as a paint would.
        engine.relayout_only(page);
        let s = &page.incr.stats;
        let flush = format!(
            "work: style incr {} full {} restyled {} layout incr {} full {} dl {} in {:.1} ms",
            s.style_incremental - before.style_incremental,
            s.style_full - before.style_full,
            s.elements_restyled - before.elements_restyled,
            s.layout_incremental - before.layout_incremental,
            s.layout_full - before.layout_full,
            s.display_lists - before.display_lists,
            t.elapsed().as_secs_f64() * 1000.0
        );
        self.checks += 1;
        self.restyled += s.elements_restyled - before.elements_restyled;
        self.styles += s.style_incremental - before.style_incremental;
        let mut bad = Vec::new();
        if let Err(e) = engine.check_incremental(page) {
            bad.push(format!("incremental != full: {e}"));
        }
        if let Err(e) = engine.check_layout_idempotent(page) {
            bad.push(format!("full layout not idempotent: {e}"));
        }
        if bad.is_empty() {
            println!("ok   {label} | {flush}");
        } else {
            self.divergences += 1;
            println!("DIVERGENCE {label} | {flush}");
            for b in bad {
                println!("{b}");
            }
            if self.divergences >= self.max {
                println!("stopping: {} divergences in {} checks", self.divergences, self.checks);
                std::process::exit(2);
            }
        }
    }
}

fn stats_line(page: &Page) -> String {
    let s = &page.incr.stats;
    format!(
        "style incr {} full {} restyled {} | layout incr {} full {} | dl {}",
        s.style_incremental, s.style_full, s.elements_restyled, s.layout_incremental, s.layout_full, s.display_lists
    )
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let url = args.get(1).cloned().unwrap_or_else(|| "about:home".into());
    let url = if std::path::Path::new(&url).exists() { format!("file://{}", std::fs::canonicalize(&url).unwrap().display()) } else { url };
    let w: f32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1280.0);
    let h: f32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(900.0);
    let mut engine = Engine::new();
    engine.loader.load_images = std::env::var("RH_NO_IMAGES").is_err();
    engine.loader.load_scripts = std::env::var("RH_NO_JS").is_err();
    engine.scripting = engine.loader.load_scripts;
    let vp = Viewport { width: w, height: h };
    let loaded = match engine.loader.load(&url, vp) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    let mut page = engine.build_page(loaded, vp);
    settle(&engine, &mut page, env_num("RH_JS_WAIT", 5000));
    let settle_ms: u64 = env_num("RH_STAB_SETTLE", 150);
    let grid: f32 = env_num("RH_STAB_GRID", 80.0);
    let scrolls: usize = env_num("RH_STAB_SCROLLS", 6);
    let mut probe = Probe { restyled: 0, styles: 0, divergences: 0, max: env_num("RH_STAB_MAX_DIVERGENCES", 20), checks: 0 };
    println!("loaded {url}: {} nodes, content {}x{} | {}", page.doc.nodes.len(), page.content_width(), page.content_height(), stats_line(&page));
    let before = page.incr.stats.clone();
    probe.check(&engine, &mut page, "after load", before, Instant::now());

    // Idle: work the page does on its own (timers, animation frames).
    let (before, t) = (page.incr.stats.clone(), Instant::now());
    settle(&engine, &mut page, 2000);
    probe.check(&engine, &mut page, "idle 2 s", before, t);
    let (r0, s0, c0) = (probe.restyled, probe.styles, probe.checks);

    let hover_sweep = |engine: &Engine, page: &mut Page, probe: &mut Probe, tag: &str| {
        let mut y = grid / 2.0;
        while y < h {
            let mut x = grid / 2.0;
            while x < w {
                let old = page.state.hovered;
                let (before, t) = (page.incr.stats.clone(), Instant::now());
                let _ = engine.pointer_move(page, x, y, Default::default());
                let new = page.state.hovered;
                if new != old {
                    settle(engine, page, settle_ms);
                    let label = format!("{tag} hover ({x:.0},{y:.0}) {} -> {}", desc(page, old), desc(page, new));
                    probe.check(engine, page, &label, before, t);
                }
                x += grid;
            }
            y += grid;
        }
    };

    if std::env::var_os("RH_STAB_SKIP_TOP").is_none() {
        hover_sweep(&engine, &mut page, &mut probe, "top");
    }
    // Scroll down by half viewports (window scroll events like the GUI),
    // hovering at each position.
    for i in 1..=scrolls {
        let max_y = (page.content_height() - h).max(0.0);
        let y = (i as f32 * h / 2.0).min(max_y);
        let (before, t) = (page.incr.stats.clone(), Instant::now());
        engine.set_page_scroll(&mut page, 0.0, y);
        if page.has_scripts() {
            let _ = engine.dispatch_window_event(&mut page, "scroll");
        }
        settle(&engine, &mut page, settle_ms);
        probe.check(&engine, &mut page, &format!("scroll to {y:.0}"), before, t);
        let tag = format!("scroll {y:.0}");
        hover_sweep(&engine, &mut page, &mut probe, &tag);
        if y >= max_y {
            break;
        }
    }
    let (before, t) = (page.incr.stats.clone(), Instant::now());
    engine.set_page_scroll(&mut page, 0.0, 0.0);
    let _ = engine.pointer_move(&mut page, -10.0, -10.0, Default::default());
    settle(&engine, &mut page, settle_ms);
    probe.check(&engine, &mut page, "back to top, pointer out", before, t);
    let n = (probe.checks - c0).max(1) as u64;
    println!(
        "interactions: {} | style passes {} | elements restyled {} ({} per interaction, {} per pass)",
        n,
        probe.styles - s0,
        probe.restyled - r0,
        (probe.restyled - r0) / n,
        (probe.restyled - r0) / (probe.styles - s0).max(1)
    );
    println!("done: {} divergences in {} checks | {}", probe.divergences, probe.checks, stats_line(&page));
    if probe.divergences > 0 {
        std::process::exit(2);
    }
}
