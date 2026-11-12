//! `rabbithole ui-bench <URL> [--legacy] [--seconds N]`: measure how long
//! the UI thread is kept busy while a page loads, without a window.
//!
//! The default mode drives a content thread like the GUI does (60 Hz loop
//! presenting the latest frame and scrolling 20 px per frame) and reports
//! the worst time one loop iteration took. `--legacy` replays the old
//! single-threaded design, where building the page (style, layout,
//! scripts), timers and painting ran on the UI thread, and reports the
//! worst of those steps.

use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rabbithole_engine::gfx::{Canvas, Rect};
use rabbithole_engine::net::{Request, Url};
use rabbithole_engine::{Engine, Loaded, LocalStorage, Viewport};

use super::content::{ContentConfig, ContentHandle, ContentSettings, Frame, FromContent, Sink, ToContent, ViewSize};

const W: u32 = 1200;
const H: u32 = 800;

pub fn run(args: &[String]) -> Result<(), String> {
    let mut url = None;
    let mut legacy = false;
    let mut seconds = 15u64;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--legacy" => legacy = true,
            "--seconds" => seconds = it.next().and_then(|s| s.parse().ok()).ok_or("--seconds needs a number")?,
            s if !s.starts_with('-') => url = Some(s.to_string()),
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    let url = url.ok_or("usage: rabbithole ui-bench <URL> [--legacy] [--seconds N]")?;
    let url = Url::parse(&super::omnibox::resolve_input(&url, crate::settings::SearchEngine::DuckDuckGo, "about:home")).ok_or("bad URL")?;
    if legacy {
        legacy_run(url, Duration::from_secs(seconds))
    } else {
        threaded_run(url, Duration::from_secs(seconds))
    }
}

fn threaded_run(url: Url, duration: Duration) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    let sink: Sink = Arc::new(move |_tab, _epoch, msg| {
        let _ = tx.lock().unwrap().send((Instant::now(), msg));
    });
    let view = ViewSize::new(W, H - 120, 1.0);
    let config = ContentConfig {
        loader: rabbithole_engine::Loader::new(),
        settings: ContentSettings::new(true, true, true),
        local_storage: Arc::new(Mutex::new(LocalStorage::default())),
        next_download_id: Arc::new(AtomicU64::new(1)),
        view,
        system_fonts: true,
        console_to_stderr: std::env::var_os("RH_BENCH_CONSOLE").is_some(),
    };
    let start = Instant::now();
    let handle = ContentHandle::spawn(1, 1, config, sink);
    handle.send(ToContent::SetActive(true));
    handle.send(ToContent::Navigate { request: Request::get(url.clone()), push_history: true });
    let mut window = Canvas::new(W, H);
    let area = Rect::new(0.0, 80.0, W as f32, (H - 120) as f32);
    let mut frame: Option<Frame> = None;
    let mut scroll = 0.0f32;
    let (mut first_paint, mut committed, mut done) = (None, None, None);
    let (mut frames, mut iterations) = (0u32, 0u32);
    let mut worst = Duration::ZERO;
    let mut busy_max = Duration::ZERO;
    let mut stalls_over_16 = 0u32;
    // Content-thread busy periods: (start stamp, longest duration seen).
    let mut periods: Vec<(u64, Duration)> = Vec::new();
    while start.elapsed() < duration {
        let tick = Instant::now();
        while let Ok((t, msg)) = rx.try_recv() {
            match msg {
                FromContent::Frame(f) => {
                    frames += 1;
                    first_paint.get_or_insert(t.duration_since(start));
                    frame = Some(f);
                }
                FromContent::Committed { .. } => {
                    committed.get_or_insert(t.duration_since(start));
                }
                FromContent::Loading(None) => {
                    done.get_or_insert(t.duration_since(start));
                }
                FromContent::ScrollTo { y, .. } => scroll = y,
                _ => {}
            }
        }
        // Present + scroll like a user flicking the wheel.
        window.clear(rabbithole_engine::gfx::Color::rgb(0xf1, 0xf3, 0xf4));
        if let Some(f) = &frame {
            let max = (f.content.1 - view.viewport.height).max(0.0);
            scroll = (scroll + 20.0).min(max);
            handle.send(ToContent::Scroll { x: 0.0, y: scroll });
            f.present(&mut window, area, (0.0, scroll), 1.0);
        }
        let spent = tick.elapsed();
        worst = worst.max(spent);
        if spent > Duration::from_millis(16) {
            stalls_over_16 += 1;
        }
        if let (Some(since), Some(b)) = (handle.busy_since(), handle.busy_for()) {
            busy_max = busy_max.max(b);
            match periods.last_mut() {
                Some((s, d)) if *s == since => *d = (*d).max(b),
                _ => periods.push((since, b)),
            }
        }
        iterations += 1;
        std::thread::sleep(Duration::from_millis(16).saturating_sub(spent));
    }
    println!("mode: content thread (new)");
    println!("url: {url}");
    println!("commit: {}", fmt(committed));
    println!("first paint: {}", fmt(first_paint));
    println!("load finished: {}", fmt(done));
    println!("frames received: {frames}, UI iterations: {iterations}, iterations over 16 ms: {stalls_over_16}");
    println!("worst UI-thread stall: {:.1} ms", worst.as_secs_f64() * 1000.0);
    println!("longest content-thread task seen (off the UI thread): {:.0} ms", busy_max.as_secs_f64() * 1000.0);
    let over = |ms: u64| periods.iter().filter(|(_, d)| *d >= Duration::from_millis(ms)).count();
    println!("content-thread busy periods >= 100 ms: {}, >= 500 ms: {}, >= 1 s: {}, >= 2.5 s: {}", over(100), over(500), over(1000), over(2500));
    let mut top: Vec<Duration> = periods.iter().map(|(_, d)| *d).collect();
    top.sort_by(|a, b| b.cmp(a));
    let top: Vec<String> = top.iter().take(8).map(|d| format!("{:.0}", d.as_secs_f64() * 1000.0)).collect();
    println!("longest busy periods (ms): {}", top.join(" "));
    Ok(())
}

fn legacy_run(url: Url, duration: Duration) -> Result<(), String> {
    let mut engine = Engine::new();
    let vp = Viewport { width: W as f32, height: (H - 120) as f32 };
    let loader = engine.loader.clone();
    let start = Instant::now();
    // Fetching ran on a worker thread in the old design too.
    let req = Request::get(url.clone());
    let loaded = std::thread::spawn(move || loader.load_request(&req, vp)).join().map_err(|_| "loader panicked")?.map_err(|e| e.to_string())?;
    let Loaded::Page(loaded) = loaded else { return Err("not a page".into()) };
    let fetched = start.elapsed();
    let mut stalls: Vec<(&str, Duration)> = Vec::new();
    // On the UI thread: build the page (style, layout, scripts) ...
    let t = Instant::now();
    let mut page = engine.build_page(loaded, vp);
    stalls.push(("build page (style+layout+scripts)", t.elapsed()));
    let first_paint = start.elapsed();
    let mut canvas = Canvas::new(W, H - 120);
    let t = Instant::now();
    engine.render(&page, &mut canvas, 0.0, 0.0);
    stalls.push(("paint", t.elapsed()));
    // ... then timers, image loads and repaints, plus scrolling.
    let mut scroll = 0.0f32;
    while start.elapsed() < duration {
        let t = Instant::now();
        let out = engine.tick(&mut page);
        let tick = t.elapsed();
        stalls.push(("timers", tick));
        let t = Instant::now();
        scroll = (scroll + 20.0).min((page.content_height() - vp.height).max(0.0));
        engine.set_page_scroll(&mut page, 0.0, scroll);
        engine.render(&page, &mut canvas, 0.0, scroll);
        stalls.push(("scroll + paint", t.elapsed()));
        let _ = out;
        std::thread::sleep(Duration::from_millis(16));
    }
    let worst = stalls.iter().max_by_key(|(_, d)| *d).cloned().unwrap_or(("", Duration::ZERO));
    println!("mode: everything on the UI thread (old)");
    println!("url: {url}");
    println!("fetch (worker thread): {:.0} ms", fetched.as_secs_f64() * 1000.0);
    println!("first paint: {:.0} ms", first_paint.as_secs_f64() * 1000.0);
    println!("iterations over 16 ms: {}", stalls.iter().filter(|(_, d)| *d > Duration::from_millis(16)).count());
    println!("worst UI-thread stall: {:.1} ms ({})", worst.1.as_secs_f64() * 1000.0, worst.0);
    Ok(())
}

fn fmt(d: Option<Duration>) -> String {
    d.map(|d| format!("{:.0} ms", d.as_secs_f64() * 1000.0)).unwrap_or_else(|| "-".into())
}
