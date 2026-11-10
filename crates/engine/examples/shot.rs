//! Headless screenshot helper used during development:
//! `cargo run --release -p rabbithole-engine --example shot -- <url> <out.png> [width] [height]`
use rabbithole_engine::{Engine, Viewport};
use rabbithole_gfx::Canvas;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let url = args.get(1).map(String::as_str).unwrap_or("about:home");
    let out = args.get(2).map(String::as_str).unwrap_or("shot.png");
    let w: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1200);
    let h: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(900);
    let t0 = std::time::Instant::now();
    let mut engine = Engine::new();
    engine.loader.load_fonts = std::env::var("RH_NO_FONTS").is_err();
    engine.loader.load_images = std::env::var("RH_NO_IMAGES").is_err();
    engine.loader.load_scripts = std::env::var("RH_NO_JS").is_err();
    engine.scripting = engine.loader.load_scripts;
    let t1 = std::time::Instant::now();
    let vp = Viewport { width: w as f32, height: h as f32 };
    let loaded = match engine.loader.load(url, vp) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    let t1b = std::time::Instant::now();
    eprintln!("fetch+discovery {:?} ({} sheets, {} images, {} fonts)", t1b - t1, loaded.sheets.len(), loaded.images.len(), loaded.fonts.len());
    let page = engine.build_page(loaded, vp);
    let tb = std::time::Instant::now();
    eprintln!("build_page {:?}", tb - t1b);
    let page: Result<_, rabbithole_engine::EngineError> = Ok(page);
    let page = match page {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    let t2 = std::time::Instant::now();
    if std::env::var("RH_DEBUG_ITEMS").is_ok() {
        for it in &page.display_list.items {
            if let rabbithole_engine::layout::DisplayItem::Image { rect, src, image, opacity, .. } = it {
                eprintln!("Image item: dest={rect:?} src={src:?} image={}x{} opacity={opacity}", image.width, image.height);
            }
        }
        eprintln!("images map: {}", page.images.len());
    }
    if let Ok(needle) = std::env::var("RH_DEBUG_TEXT") {
        use rabbithole_engine::layout::DisplayItem as D;
        let items = &page.display_list.items;
        for (i, it) in items.iter().enumerate() {
            if let D::Text { x, y, text, .. } = it {
                if text.contains(&needle) {
                    eprintln!("{i}: Text {text:?} at ({x}, {y})");
                    // Print the open stack above it.
                    let mut depth: i32 = 0;
                    for j in (0..i).rev() {
                        match &items[j] {
                            D::PopClip | D::PopTransform | D::PopLayer(_) | D::PopFixed => depth += 1,
                            D::PushClip(_) | D::PushClipRounded(..) | D::PushTransform(_) | D::PushLayer(_) | D::PushFixed => {
                                if depth == 0 {
                                    eprintln!("   open at {j}: {:?}", items[j]);
                                } else {
                                    depth -= 1;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    if std::env::var("RH_DEBUG_DL").is_ok() {
        use rabbithole_engine::layout::DisplayItem as D;
        for (i, it) in page.display_list.items.iter().enumerate() {
            match it {
                D::PushTransform(m) => eprintln!("{i}: PushTransform {m:?}"),
                D::PushFixed => eprintln!("{i}: PushFixed"),
                D::PushLayer(r) => eprintln!("{i}: PushLayer {r:?}"),
                D::PushClip(r) => eprintln!("{i}: PushClip {r:?}"),
                D::PushClipRounded(r, _) => eprintln!("{i}: PushClipRounded {r:?}"),
                _ => {}
            }
        }
    }
    // Let timers run for a bit (like a browser would) before painting.
    let mut page = page;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(std::env::var("RH_JS_WAIT").ok().and_then(|v| v.parse().ok()).unwrap_or(300));
    while std::time::Instant::now() < deadline {
        match page.next_script_due() {
            Some(due) => {
                let now = engine.start.elapsed().as_secs_f64() * 1000.0;
                if due > now {
                    std::thread::sleep(std::time::Duration::from_millis(((due - now) as u64).min(50)));
                }
                let _ = engine.tick(&mut page);
            }
            None => break,
        }
    }
    // Then let background image loads (lazy, srcset, script-driven) land.
    let img_deadline = std::time::Instant::now() + std::time::Duration::from_millis(std::env::var("RH_IMG_WAIT").ok().and_then(|v| v.parse().ok()).unwrap_or(4000));
    while page.images_in_flight() > 0 && std::time::Instant::now() < img_deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
        let _ = engine.tick(&mut page);
    }
    let _ = engine.tick(&mut page);
    if std::env::var("RH_DEBUG_ITEMS").is_ok() {
        eprintln!("images after wait: {} (in flight {})", page.images.len(), page.images_in_flight());
    }
    for (level, msg) in page.take_console() {
        eprintln!("[console.{level}] {msg}");
    }
    let mut canvas = Canvas::new(w, h);
    if std::env::var("RH_PROFILE").is_ok() {
        let t = std::time::Instant::now();
        engine.restyle(&mut page);
        eprintln!("[profile] restyle+layout+display list {:?}", t.elapsed());
        let t = std::time::Instant::now();
        engine.relayout_only(&mut page);
        eprintln!("[profile] layout+display list {:?}", t.elapsed());
        let t = std::time::Instant::now();
        engine.rebuild_display_list(&mut page);
        eprintln!("[profile] display list {:?}", t.elapsed());
        // Per-item-kind paint cost.
        use rabbithole_engine::layout::{DisplayItem as D, DisplayList};
        let mut kinds: std::collections::BTreeMap<&'static str, (usize, std::time::Duration)> = Default::default();
        let mut c2 = Canvas::new(w, h);
        let mut stack_items = Vec::new();
        for it in &page.display_list.items {
            let name = match it {
                D::Rect { .. } => "Rect", D::RectRadii { .. } => "RectRadii", D::Border { .. } => "Border", D::BoxShadow { .. } => "BoxShadow",
                D::Text { .. } => "Text", D::Image { .. } => "Image", D::ImageTiled { .. } => "ImageTiled", D::Line { .. } => "Line",
                D::PushClip(_) | D::PushClipRounded(..) | D::PopClip => "Clip", D::PushTransform(_) | D::PopTransform => "Transform",
                D::PushLayer(_) | D::PopLayer(_) => "Layer", D::PushFixed | D::PopFixed => "Fixed",
            };
            let single = DisplayList { items: vec![it.clone()] };
            let t = std::time::Instant::now();
            if matches!(name, "Clip" | "Transform" | "Layer" | "Fixed") {
                stack_items.push(it.clone());
                continue;
            }
            rabbithole_engine::layout::paint(&single, &mut c2, &engine.fonts, 0.0, 0.0);
            let e = kinds.entry(name).or_insert((0, std::time::Duration::ZERO));
            e.0 += 1;
            e.1 += t.elapsed();
        }
        for (k, (n, d)) in &kinds {
            eprintln!("[profile] paint {k}: {n} items {d:?}");
        }
        let mut flat = page.display_list.clone();
        for it in &mut flat.items {
            if let D::PushClipRounded(r, _) = it {
                *it = D::PushClip(*r);
            }
        }
        let mut c3 = Canvas::new(w, h);
        let t = std::time::Instant::now();
        rabbithole_engine::layout::paint(&flat, &mut c3, &engine.fonts, 0.0, 0.0);
        eprintln!("[profile] full paint with no-rounded-clips {:?}", t.elapsed());
        let mut c4 = Canvas::new(w, h);
        let t = std::time::Instant::now();
        rabbithole_engine::layout::paint(&page.display_list, &mut c4, &engine.fonts, 0.0, 0.0);
        eprintln!("[profile] full paint {:?}", t.elapsed());
        let mut nolayer = page.display_list.clone();
        nolayer.items.retain(|it| !matches!(it, D::PushLayer(_) | D::PopLayer(_)));
        let mut c5 = Canvas::new(w, h);
        let t = std::time::Instant::now();
        rabbithole_engine::layout::paint(&nolayer, &mut c5, &engine.fonts, 0.0, 0.0);
        eprintln!("[profile] full paint without layers {:?}", t.elapsed());
    }
    engine.render(&page, &mut canvas, 0.0, 0.0);
    let t3 = std::time::Instant::now();
    image::save_buffer(out, &canvas.to_rgba8(), w, h, image::ExtendedColorType::Rgba8).expect("write png");
    eprintln!(
        "{} | fonts {:?} load+layout {:?} paint {:?} | {}x{} content {}x{} | {} display items",
        page.title,
        t1 - t0,
        t2 - t1,
        t3 - t2,
        w,
        h,
        page.content_width(),
        page.content_height(),
        page.display_list.items.len()
    );
}
