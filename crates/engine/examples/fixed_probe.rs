//! Report how a page's viewport-fixed and sticky content can be split from
//! the scrolling content (the GUI's page layers):
//! `cargo run -p rabbithole-engine --example fixed_probe -- <url> [width] [height] [scroll...]`
//! `RH_JS_WAIT` (ms, default 3000) lets scripts settle before probing;
//! `RH_PROBE_JS` is evaluated after that.
use rabbithole_engine::layout::{DisplayItem as D, DisplayList};
use rabbithole_engine::{Engine, Page, Viewport};
use rabbithole_gfx::Rect;

#[global_allocator]
static ALLOC: rabbithole_engine::script::js::alloc::RhAlloc = rabbithole_engine::script::js::alloc::RhAlloc;

fn kind(item: &D) -> String {
    match item {
        D::Rect { rect, color, .. } => format!("Rect {rect:?} {color:?}"),
        D::RectRadii { rect, .. } => format!("RectRadii {rect:?}"),
        D::Border { rect, .. } => format!("Border {rect:?}"),
        D::BoxShadow { rect, .. } => format!("BoxShadow {rect:?}"),
        D::Text { x, y, text, .. } => format!("Text ({x:.1}, {y:.1}) {:?}", text.chars().take(30).collect::<String>()),
        D::Image { rect, .. } => format!("Image {rect:?}"),
        D::ImageTiled { area, .. } => format!("ImageTiled {area:?}"),
        D::Line { y0, .. } => format!("Line y={y0}"),
        D::PushClip(r) => format!("PushClip {r:?}"),
        D::PushClipRounded(r, _) => format!("PushClipRounded {r:?}"),
        D::PushClipPath { bounds, .. } => format!("PushClipPath {bounds:?}"),
        D::PopClip => "PopClip".into(),
        D::PushTransform(m) => format!("PushTransform {m:?}"),
        D::PopTransform => "PopTransform".into(),
        D::PushLayer(r) => format!("PushLayer {r:?}"),
        D::PopLayer(a) => format!("PopLayer {a}"),
        D::LayerMask(_) => "LayerMask".into(),
        D::LayerBlend(m) => format!("LayerBlend {m}"),
        D::Filtered { bounds, items, .. } => format!("Filtered {bounds:?} ({} items)", items.len()),
        D::BackdropFilter { rect, .. } => format!("BackdropFilter {rect:?}"),
        D::PushFixed => "PushFixed".into(),
        D::PopFixed => "PopFixed".into(),
        D::Blended { bounds, items, .. } => format!("Blended {bounds:?} ({} items)", items.len()),
        D::Isolated { bounds, items, .. } => format!("Isolated {bounds:?} ({} items)", items.len()),
        D::Transform3d(g) => format!("Transform3d ({} items)", g.items.len()),
    }
}

fn bounds(item: &D) -> Option<Rect> {
    Some(match item {
        D::Rect { rect, .. } | D::RectRadii { rect, .. } | D::Border { rect, .. } | D::Image { rect, .. } | D::BoxShadow { rect, .. } => *rect,
        D::Text { x, y, size, text, .. } => Rect::new(*x, y - size, text.chars().count() as f32 * size * 0.6, size * 1.3),
        D::ImageTiled { area, .. } => *area,
        D::Filtered { bounds, .. } | D::Isolated { bounds, .. } | D::Blended { bounds, .. } => *bounds,
        _ => return None,
    })
}

/// Top-level runs after the first `PushFixed`: fixed groups and the
/// scrolling content between them.
fn report(list: &DisplayList) {
    println!("  items={} fixed_bounds={:?} sticky={}", list.items.len(), list.fixed_bounds, list.sticky.len());
    match list.layer_plan() {
        Ok(plan) => println!("  layer_plan base={} layers={:?}", plan.base, plan.layers),
        Err(reason) => println!("  layer_plan={reason}"),
    }
    let Some(first) = list.items.iter().position(|i| matches!(i, D::PushFixed)) else {
        println!("  no top-level PushFixed");
        return;
    };
    let mut stack = Vec::new();
    for (i, item) in list.items[..first].iter().enumerate() {
        match item {
            D::PushClip(_) | D::PushClipRounded(..) | D::PushClipPath { .. } | D::PushTransform(_) | D::PushLayer(_) => stack.push(i),
            D::PopClip | D::PopTransform | D::PopLayer(_) => { stack.pop(); }
            _ => {}
        }
    }
    for i in &stack {
        println!("  open at first PushFixed: {i}: {}", kind(&list.items[*i]));
    }
    let mut fixed = false;
    let mut run: Option<(usize, bool, Option<Rect>, Vec<String>)> = None;
    let flush = |run: &mut Option<(usize, bool, Option<Rect>, Vec<String>)>, end: usize| {
        if let Some((start, fixed, b, notes)) = run.take() {
            println!("  {} [{start}..{end}) bounds={b:?}", if fixed { "FIXED " } else { "scroll" });
            for n in notes.iter().take(6) { println!("      {n}"); }
        }
    };
    for (i, item) in list.items.iter().enumerate().skip(first) {
        let now_fixed = match item { D::PushFixed => true, D::PopFixed => { fixed = false; true } _ => fixed };
        if matches!(item, D::PushFixed) { fixed = true; }
        if run.as_ref().map(|r| r.1) != Some(now_fixed) {
            flush(&mut run, i);
            run = Some((i, now_fixed, None, Vec::new()));
        }
        let r = run.as_mut().unwrap();
        if let Some(b) = bounds(item) { r.2 = Some(r.2.map_or(b, |u| u.union(&b))); }
        if !matches!(item, D::PushFixed | D::PopFixed) && (r.3.len() < 6 || matches!(item, D::Text { .. })) && r.3.len() < 12 {
            r.3.push(format!("{i}: {}", kind(item)));
        }
    }
    flush(&mut run, list.items.len());
}

fn describe(page: &mut Page, engine: &Engine) {
    let focus = engine.eval_in_page(page, "(()=>{const e=document.activeElement;return e?e.tagName+'#'+e.id+'.'+e.className:'none'})()");
    println!("  activeElement={focus} state.focused={:?}", page.state.focused);
}

fn settle(engine: &Engine, page: &mut Page, ms: u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
        let _ = engine.tick(page);
    }
    engine.present(page);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let url = args.get(1).map(String::as_str).unwrap_or("about:home");
    let w: f32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1280.0);
    let h: f32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(800.0);
    let scrolls: Vec<f32> = args.iter().skip(4).filter_map(|s| s.parse().ok()).collect();
    let wait = std::env::var("RH_JS_WAIT").ok().and_then(|v| v.parse().ok()).unwrap_or(3000);
    let mut engine = Engine::new();
    engine.scripting = engine.loader.load_scripts;
    let vp = Viewport { width: w, height: h };
    let loaded = engine.loader.load(url, vp).unwrap_or_else(|e| { eprintln!("error: {e}"); std::process::exit(1) });
    let mut page = engine.build_page(loaded, vp);
    settle(&engine, &mut page, wait);
    // RH_PROBE_JS: evaluated once the page settled (e.g. to open a dialog).
    if let Ok(js) = std::env::var("RH_PROBE_JS") {
        println!("eval {}", engine.eval_in_page(&mut page, &js));
        settle(&engine, &mut page, 3000);
    }
    println!("scroll 0");
    describe(&mut page, &engine);
    report(&page.display_list);
    for y in scrolls {
        engine.set_page_scroll(&mut page, 0.0, y);
        let _ = engine.dispatch_window_event(&mut page, "scroll");
        settle(&engine, &mut page, 300);
        println!("scroll {y}");
        describe(&mut page, &engine);
        report(&page.display_list);
    }
}
