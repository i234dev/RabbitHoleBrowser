//! Style/layout pipeline benchmark:
//! `cargo run --release -p rabbithole-engine --example incr_bench -- <url|file>...`
//!
//! For every page: full style + layout + display list, a single class
//! toggle, a hover change, N alternating `style.width=` / `offsetWidth`
//! (and `getComputedStyle`) script round trips, and one scroll frame.
//! `RH_BENCH_N` sets the script loop count (default 1000).
use rabbithole_engine::{Engine, Page, Viewport};
use rabbithole_gfx::Canvas;
use std::time::{Duration, Instant};

fn ms(d: Duration) -> String {
    format!("{:.3} ms", d.as_secs_f64() * 1000.0)
}

fn time<F: FnMut()>(reps: u32, mut f: F) -> Duration {
    let t = Instant::now();
    for _ in 0..reps {
        f();
    }
    t.elapsed() / reps
}

fn pick_elements(page: &Page) -> Vec<usize> {
    let doc = &page.doc;
    let body = doc.body().unwrap_or(0);
    doc.descendants(body)
        .filter(|&n| matches!(doc.tag(n), Some("div" | "p" | "section" | "li" | "span")) && page.styles.get(n).is_some())
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let n_loop: u32 = std::env::var("RH_BENCH_N").ok().and_then(|v| v.parse().ok()).unwrap_or(1000);
    // Repetitions of the cheap measurements (full passes use a quarter).
    let reps: u32 = std::env::var("RH_BENCH_REPS").ok().and_then(|v| v.parse().ok()).unwrap_or(20);
    let few = (reps / 4).max(1);
    for url in &args {
        let mut engine = Engine::new();
        engine.loader.load_images = false;
        engine.loader.load_scripts = false;
        engine.scripting = false;
        let vp = Viewport { width: 1200.0, height: 900.0 };
        let url = if std::path::Path::new(url).exists() {
            format!("file://{}", std::fs::canonicalize(url).unwrap().display())
        } else {
            url.clone()
        };
        let t = Instant::now();
        let loaded = match engine.loader.load(&url, vp) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("{url}: {e}");
                continue;
            }
        };
        let fetch = t.elapsed();
        let mut page = engine.build_page(loaded, vp);
        engine.scripting = true;
        println!("== {url} ({} nodes, {} sheets, fetch {})", page.doc.nodes.len(), page.sheets.len(), ms(fetch));

        // Full pipeline (forced from scratch).
        let full = time(few, || {
            page.invalidate_all();
            engine.restyle(&mut page);
        });
        println!("full restyle+layout+dl     {}", ms(full));
        let layout = time(few, || {
            page.invalidate_layout();
            engine.relayout_only(&mut page);
        });
        println!("  layout+dl only           {}", ms(layout));
        let dl = time(few, || engine.rebuild_display_list(&mut page));
        println!("  display list only        {}", ms(dl));
        let noop = time(reps, || engine.restyle(&mut page));
        println!("restyle with nothing dirty {}", ms(noop));

        let els = pick_elements(&page);
        if els.is_empty() {
            continue;
        }
        let target = els[els.len() / 2];

        // Class toggle on one element.
        let mut on = false;
        let toggle = time(reps, || {
            on = !on;
            let cls = page.doc.attr(target, "class").unwrap_or("").to_string();
            let new = if on { format!("{cls} rh-bench-toggle") } else { cls.replace(" rh-bench-toggle", "") };
            page.doc.set_attr(target, "class", Some(&new));
            engine.restyle(&mut page);
        });
        println!("class toggle               {}", ms(toggle));

        // Hover between two links (or elements).
        let links: Vec<usize> = page.doc.descendants(0).filter(|&n| page.doc.is_element(n, "a") && page.styles.get(n).is_some()).collect();
        let (h1, h2) = if links.len() >= 2 { (links[links.len() / 3], links[2 * links.len() / 3]) } else { (els[0], target) };
        let mut flip = false;
        let hover = time(reps, || {
            flip = !flip;
            engine.set_hover(&mut page, Some(if flip { h1 } else { h2 }));
        });
        println!("hover change               {} (dynamic rules: {})", ms(hover), page.has_dynamic_rules);
        let mut canvas = Canvas::new(1200, 900);
        engine.render_damage(&mut page, &mut canvas);
        let full_paint = time(few, || engine.render(&page, &mut canvas, page.page_scroll.0, page.page_scroll.1));
        engine.render_damage(&mut page, &mut canvas);
        let hover_paint = time(reps, || {
            flip = !flip;
            engine.set_hover(&mut page, Some(if flip { h1 } else { h2 }));
            engine.render_damage(&mut page, &mut canvas);
        });
        println!("hover + damage repaint     {} (full repaint {})", ms(hover_paint), ms(full_paint));

        // Script: alternating style writes and layout reads.
        page.doc.set_attr(target, "id", Some("rh-bench-target"));
        engine.restyle(&mut page);
        let loops = [
            ("style.width=/offsetWidth     ", "el.style.width=(100+(i%50))+'px';var w=el.offsetWidth;"),
            ("style.width=/getComputedStyle().color", "el.style.width=(100+(i%50))+'px';var w=getComputedStyle(el).color;"),
            ("offsetWidth+gBCR (clean)     ", "var w=el.offsetWidth+el.getBoundingClientRect().width;"),
        ];
        for (name, body) in loops {
            let js = format!(
                "(function(){{var el=document.getElementById('rh-bench-target');var t=performance.now();for(var i=0;i<{n_loop};i++){{{body}}}var a=performance.now()-t;el.style.width='';return a.toFixed(2);}})()"
            );
            let t = Instant::now();
            let out = engine.eval_in_page(&mut page, &js);
            let wall = t.elapsed();
            match out.trim_matches(|c| c == '"' || c == '\'').parse::<f64>() {
                Ok(v) => println!("{n_loop}x {name} {:.1} ms in script ({:.3} ms each; {} wall incl. settle)", v, v / n_loop as f64, ms(wall)),
                Err(_) => println!("{n_loop}x {name} failed: {out}"),
            }
        }

        // One scroll frame: update scroll + repaint the viewport.
        let mut y = 0.0;
        engine.set_page_scroll(&mut page, 0.0, 0.0);
        engine.render(&page, &mut canvas, 0.0, 0.0);
        let scroll = time(reps, || {
            y += 40.0;
            engine.set_page_scroll(&mut page, 0.0, y);
            engine.render(&page, &mut canvas, 0.0, y);
        });
        println!("scroll frame (scroll+paint) {}", ms(scroll));
        let mut old = (0.0, y);
        engine.render_damage(&mut page, &mut canvas);
        let scroll_dmg = time(reps, || {
            y += 40.0;
            engine.set_page_scroll(&mut page, 0.0, y);
            engine.render_scrolled(&mut page, &mut canvas, old);
            old = (0.0, y);
        });
        println!("scroll frame (damage only) {}", ms(scroll_dmg));
        let scroll_only = time(reps, || {
            y += 40.0;
            engine.set_page_scroll(&mut page, 0.0, y);
        });
        println!("  scroll update only        {}", ms(scroll_only));
    }
}
