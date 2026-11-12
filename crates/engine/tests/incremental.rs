//! Incremental restyle / relayout must give the same result as computing
//! everything from scratch (`Engine::check_incremental`).

use rabbithole_engine::dom::NodeId;
use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Page, Viewport};

fn load(html: &str) -> (Engine, Page) {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.scripting = false;
    engine.loader.load_scripts = false;
    engine.loader.load_images = false;
    let url = Url::parse("https://example.test/").unwrap();
    let page = engine.load_html(html, url, Viewport { width: 800.0, height: 600.0 });
    (engine, page)
}

fn id(page: &Page, id: &str) -> NodeId {
    page.doc.element_by_id(id).unwrap_or_else(|| panic!("no #{id}"))
}

/// Restyle incrementally and compare with a full computation.
fn check(engine: &Engine, page: &mut Page, what: &str) {
    engine.restyle(page);
    if let Err(e) = engine.check_incremental(page) {
        panic!("{what}: {e}");
    }
}

#[test]
fn class_toggles_with_descendant_and_sibling_selectors() {
    let (engine, mut page) = load(
        r#"<html><head><style>
        .on .child { color: red; padding: 10px }
        .a + .b { margin-top: 30px }
        .a ~ .c { font-size: 30px }
        .x > p { display: none }
        div.big { width: 300px; height: 50px }
        </style></head><body>
        <div id="p"><span class="child">c</span><p>text <b>bold</b></p></div>
        <div id="s1">one</div><div class="b">two</div><div class="c">three</div>
        <div id="w">w<div id="inner">i</div></div>
        </body></html>"#,
    );
    let p = id(&page, "p");
    let s1 = id(&page, "s1");
    let w = id(&page, "w");
    for round in 0..3 {
        page.doc.set_attr(p, "class", Some(if round % 2 == 0 { "on" } else { "" }));
        check(&engine, &mut page, "descendant class");
        page.doc.set_attr(s1, "class", Some(if round % 2 == 0 { "a" } else { "z" }));
        check(&engine, &mut page, "sibling class");
        page.doc.set_attr(p, "class", Some(if round % 2 == 0 { "x" } else { "on" }));
        check(&engine, &mut page, "child display none");
        page.doc.set_attr(w, "class", Some(if round % 2 == 0 { "big" } else { "" }));
        check(&engine, &mut page, "size change");
    }
}

#[test]
fn style_attribute_width_changes_relayout() {
    let (engine, mut page) = load(
        r#"<html><head><style>.f { float: left; width: 100px; height: 80px } p { margin: 10px 0 }</style></head><body>
        <div class="f">float</div>
        <p id="a">Some text that wraps around the float and keeps going for a while so lines matter.</p>
        <div id="box" style="width: 200px">box <span>inline</span> content</div>
        <p>after the box</p>
        <div style="position: relative" id="rel"><div style="position:absolute; top: 5px; left: 5px">abs</div><p>static</p></div>
        <table><tr><td id="cell">cell</td><td>other</td></tr></table>
        <ul><li>one</li><li id="li2">two</li></ul>
        </body></html>"#,
    );
    let b = id(&page, "box");
    for w in [120, 260, 90, 400] {
        page.doc.set_attr(b, "style", Some(&format!("width: {w}px; position: relative; top: 3px")));
        check(&engine, &mut page, "box width");
        let r = page.rect_of(b).unwrap();
        assert!((r.w - w as f32).abs() < 0.5, "width {w} -> {r:?}");
    }
    let cell = id(&page, "cell");
    let t = page.doc.create_text(" more cell text that is long");
    page.doc.append_child(cell, t);
    check(&engine, &mut page, "table cell text");
    let a = id(&page, "a");
    page.doc.set_attr(a, "style", Some("font-size: 30px"));
    check(&engine, &mut page, "paragraph next to float");
    let rel = id(&page, "rel");
    page.doc.set_attr(rel, "style", Some("position: relative; left: 20px; padding: 7px"));
    check(&engine, &mut page, "relative container");
}

#[test]
fn child_list_changes_with_structural_selectors() {
    let (engine, mut page) = load(
        r#"<html><head><style>
        li:first-child { color: green } li:last-child { font-weight: bold }
        li:nth-child(2n) { margin-left: 20px } div:empty { height: 10px; background: red }
        </style></head><body><ol id="list"><li>a</li><li>b</li><li>c</li></ol><div id="e"></div></body></html>"#,
    );
    let list = id(&page, "list");
    let li = page.doc.create_element("li", vec![]);
    let t = page.doc.create_text("new");
    page.doc.append_child(li, t);
    let first = page.doc.first_child(list);
    page.doc.insert_before(list, li, first);
    check(&engine, &mut page, "insert first li");
    page.doc.detach(li);
    check(&engine, &mut page, "remove li");
    page.doc.append_child(list, li);
    check(&engine, &mut page, "append li");
    let e = id(&page, "e");
    let t2 = page.doc.create_text("x");
    page.doc.append_child(e, t2);
    check(&engine, &mut page, ":empty");
    page.doc.set_text(t2, "longer text");
    check(&engine, &mut page, "text change");
}

#[test]
fn insertions_and_moves_with_positional_selectors() {
    let (engine, mut page) = load(
        r#"<html><head><style>
        p:first-child { color: green } p:last-child { font-weight: bold } p:only-child { font-size: 30px }
        p:nth-child(3n+1) { margin-left: 20px } p:nth-last-child(2) { padding-left: 7px }
        span:first-of-type { color: red } span:last-of-type { color: blue } em:only-of-type { color: orange }
        p + span { margin-left: 3px } p ~ em { margin-left: 5px }
        div:has(> em) { border-left: 2px solid red } body:has(span.zz) p { color: purple }
        </style></head><body><div id="box"><p>a</p></div></body></html>"#,
    );
    let bx = id(&page, "box");
    let mut nodes = vec![page.doc.first_child(bx).unwrap()];
    for (i, tag) in ["p", "span", "p", "em", "span", "p", "em"].iter().enumerate() {
        let n = page.doc.create_element(tag, vec![]);
        let t = page.doc.create_text(&format!("{tag}{i}"));
        page.doc.append_child(n, t);
        // Append, prepend, insert in the middle.
        let reference = match i % 3 {
            0 => None,
            1 => page.doc.first_child(bx),
            _ => nodes.get(nodes.len() / 2).copied(),
        };
        page.doc.insert_before(bx, n, reference);
        nodes.push(n);
        check(&engine, &mut page, &format!("insert {tag} #{i}"));
    }
    // Moves within the parent (removed and inserted in one batch).
    for k in 0..4 {
        let n = nodes[k * 2 % nodes.len()];
        page.doc.detach(n);
        let reference = if k % 2 == 0 { page.doc.first_child(bx) } else { None };
        page.doc.insert_before(bx, n, reference);
        check(&engine, &mut page, &format!("move #{k}"));
    }
    // Several insertions in one batch.
    for tag in ["em", "p", "span"] {
        let n = page.doc.create_element(tag, vec![]);
        let reference = page.doc.first_child(bx).and_then(|f| page.doc.next_sibling(f));
        page.doc.insert_before(bx, n, reference);
    }
    check(&engine, &mut page, "batch of insertions");
    // `:has()` arguments: only subtrees with their features matter.
    let zz = page.doc.create_element("span", vec![("class".into(), "zz".into())]);
    page.doc.append_child(bx, zz);
    check(&engine, &mut page, ":has(span.zz) appears");
    page.doc.detach(zz);
    check(&engine, &mut page, ":has(span.zz) goes");
    let b = page.doc.create_element("b", vec![]);
    page.doc.append_child(bx, b);
    check(&engine, &mut page, "unrelated insertion next to :has() rules");
}

#[test]
fn hover_inherit_vars_and_rem() {
    let (engine, mut page) = load(
        r##"<html><head><style>
        :root { --gap: 4px }
        nav a:hover { color: red; text-decoration: underline }
        nav:hover .tip { display: block }
        .tip { display: none }
        .outer { border: 2px solid blue; --c: green }
        .inner { border: inherit; color: var(--c); margin: var(--gap) }
        .remmy { width: 10rem }
        </style></head><body>
        <nav><a id="l1" href="#">one</a> <a id="l2" href="#">two</a><span class="tip">tip</span></nav>
        <div class="outer" id="o"><div class="inner">in</div></div>
        <div class="remmy">rem</div>
        </body></html>"##,
    );
    let l1 = id(&page, "l1");
    let l2 = id(&page, "l2");
    engine.set_hover(&mut page, Some(l1));
    check(&engine, &mut page, "hover l1");
    engine.set_hover(&mut page, Some(l2));
    check(&engine, &mut page, "hover l2");
    engine.set_hover(&mut page, None);
    check(&engine, &mut page, "hover none");
    let o = id(&page, "o");
    page.doc.set_attr(o, "style", Some("border: 5px dashed red; --c: purple"));
    check(&engine, &mut page, "inherit + vars");
    let html = page.doc.html_element().unwrap();
    page.doc.set_attr(html, "style", Some("font-size: 20px; --gap: 9px"));
    check(&engine, &mut page, "root font size");
}

#[test]
fn flex_grid_and_scroll_containers() {
    let (engine, mut page) = load(
        r#"<html><head><style>
        .flex { display: flex; gap: 5px } .flex > div { flex: 1 }
        .grid { display: grid; grid-template-columns: 1fr 2fr }
        .sc { overflow: auto; height: 50px }
        </style></head><body>
        <div class="flex"><div id="f1">one</div><div>two two two</div></div>
        <div class="grid"><div id="g1">a</div><div>b</div><div>c</div></div>
        <div class="sc" id="sc"><p>1</p><p>2</p><p>3</p><p>4</p><p>5</p></div>
        </body></html>"#,
    );
    let f1 = id(&page, "f1");
    page.doc.set_attr(f1, "style", Some("padding: 20px"));
    check(&engine, &mut page, "flex item");
    let g1 = id(&page, "g1");
    page.doc.set_attr(g1, "style", Some("height: 70px"));
    check(&engine, &mut page, "grid item");
    let sc = id(&page, "sc");
    let r = page.rect_of(sc).unwrap();
    assert!(engine.scroll_at(&mut page, r.x + 5.0, r.y + 5.0, 0.0, 30.0));
    if let Err(e) = engine.check_incremental(&mut page) {
        panic!("scroll: {e}");
    }
    let p = page.doc.create_element("p", vec![]);
    page.doc.append_child(sc, p);
    check(&engine, &mut page, "content added to scrolled container");
}

#[test]
fn layout_reuse_skips_clean_subtrees() {
    let mut html = String::from("<html><body>");
    for i in 0..300 {
        html.push_str(&format!("<div class=\"item\"><p>Paragraph {i} with <b>some</b> text to lay out.</p></div>"));
    }
    html.push_str("<div id=\"t\">target</div></body></html>");
    let (engine, mut page) = load(&html);
    let t = id(&page, "t");
    let before = page.incr.stats.clone();
    page.doc.set_attr(t, "style", Some("width: 123px"));
    check(&engine, &mut page, "single change");
    let after = page.incr.stats.clone();
    assert_eq!(after.style_full, before.style_full, "no full restyle");
    assert_eq!(after.layout_full, before.layout_full, "no full relayout");
    assert!(after.elements_restyled - before.elements_restyled < 10, "restyled {}", after.elements_restyled - before.elements_restyled);
    // Reading twice without changes costs nothing.
    let n = page.incr.stats.clone();
    engine.restyle(&mut page);
    assert_eq!(page.incr.stats.style_incremental, n.style_incremental);
    assert_eq!(page.incr.stats.layout_incremental, n.layout_incremental);
}

/// Random DOM mutations against a stylesheet exercising every
/// invalidation path; after each step the incremental result must equal
/// a full recomputation.
#[test]
fn randomized_mutations_match_full_recompute() {
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
            &v[(self.next() % v.len() as u64) as usize]
        }
    }
    const CSS: &str = r#"
        .a { color: red } .b { padding: 3px 5px } .c { display: none } .d { float: left; width: 60px }
        .e { position: relative; top: 4px; left: 2px } .f { position: absolute; right: 0; top: 10px; width: 50px }
        .g { display: flex; gap: 3px } .h { display: grid; grid-template-columns: 1fr 1fr } .i { display: inline-block; width: 40px }
        .a .b { margin: 6px } .b > .c { display: block; height: 7px } .a + .b { border: 2px solid blue } .d ~ .e { font-size: 20px }
        li:first-child { font-weight: bold } li:nth-child(2n) { margin-left: 9px } div:empty { min-height: 5px }
        .h:hover .a { background: yellow } span:hover { text-decoration: underline } .j { overflow: hidden; height: 30px }
        .k { --w: 70px } .k .l { width: var(--w) } .m { border-left: inherit } .n { display: contents } ul { list-style: decimal }
        body { counter-reset: c } .o { counter-increment: c } .o::before { content: counter(c) ". " } .p::after { content: open-quote "q" close-quote }
        .q:has(> .a) { outline: 2px solid red; padding-top: 2px } .r { position: absolute; left: 5px; width: 20px }
    "#;
    let classes = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", ""];
    let tags = ["div", "p", "span", "ul", "li", "section", "b"];
    let styles = ["", "width: 120px", "padding: 4px", "display: none", "display: inline", "height: 25px", "position: relative; left: 3px", "float: right; width: 30px", "border: 1px solid; border-left-width: 4px"];
    let words = ["alpha", "beta gamma", "a much longer run of words that wraps", "", "x"];
    let seeds: u64 = std::env::var("RH_FUZZ_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(4);
    for seed in 1..=seeds {
        let mut rng = Rng(0x9e3779b97f4a7c15 ^ seed.wrapping_mul(0x1234_5678_9abc));
        let mut html = format!("<html><head><style>{CSS}</style></head><body>");
        for i in 0..40 {
            let t = *rng.pick(&tags);
            html.push_str(&format!("<{t} class=\"{}\">{} <i>{i}</i></{t}>", rng.pick(&classes), rng.pick(&words)));
            if i % 7 == 0 {
                html.push_str("<div class=\"g\"><div class=\"a\">f1</div><div class=\"b\">f2</div></div><ul><li>1</li><li>2</li><li>3</li></ul>");
            }
        }
        html.push_str("</body></html>");
        let (engine, mut page) = load(&html);
        for step in 0..60 {
            let body = page.doc.body().unwrap();
            let els: Vec<NodeId> = page.doc.descendants(body).filter(|&n| page.doc.element(n).is_some()).collect();
            let el = *rng.pick(&els);
            let what = rng.next() % 7;
            match what {
                0 => {
                    let c = format!("{} {}", rng.pick(&classes), rng.pick(&classes));
                    page.doc.set_attr(el, "class", Some(&c));
                }
                1 => {
                    let s = *rng.pick(&styles);
                    page.doc.set_attr(el, "style", Some(s));
                }
                2 => {
                    let t = *rng.pick(&tags);
                    let n = page.doc.create_element(t, vec![("class".into(), rng.pick(&classes).to_string())]);
                    let txt = page.doc.create_text(rng.pick(&words));
                    page.doc.append_child(n, txt);
                    let parent = page.doc.parent(el).unwrap_or(body);
                    page.doc.insert_before(parent, n, Some(el));
                }
                3 => {
                    if el != body {
                        page.doc.detach(el);
                    }
                }
                4 => {
                    if let Some(t) = page.doc.children(el).find(|&c| page.doc.is_text(c)) {
                        page.doc.set_text(t, rng.pick(&words));
                    }
                }
                5 => {
                    engine.set_hover(&mut page, Some(el));
                }
                _ => {
                    // Move an element elsewhere.
                    let other = *rng.pick(&els);
                    if other != el && !page.doc.is_ancestor(el, other) && el != body {
                        page.doc.insert_before(other, el, None);
                    }
                }
            }
            engine.restyle(&mut page);
            if let Err(e) = engine.check_incremental(&mut page) {
                panic!("seed {seed} step {step} (op {what}): {e}");
            }
        }
        let st = &page.incr.stats;
        eprintln!("seed {seed}: {st:?}");
        assert!(st.layout_incremental > st.layout_full, "layout mostly incremental: {st:?}");
    }
}

/// Scrolling by shifting pixels and repainting the damage gives the same
/// frame as a full repaint (fixed header, sticky bar, text, borders).
#[test]
fn scroll_damage_repaint_matches_full_paint() {
    let mut html = String::from(
        r#"<html><head><style>
        header { position: fixed; top: 0; left: 0; right: 0; height: 30px; background: #369; color: white; box-shadow: 0 2px 6px #000 }
        .bar { position: sticky; top: 30px; background: #fc0; height: 20px }
        p { border: 1px solid #ccc; padding: 4px; border-radius: 6px }
        </style></head><body><header>Header</header><div class="bar">sticky</div>"#,
    );
    for i in 0..80 {
        html.push_str(&format!("<p>Paragraph {i} with some text to paint.</p>"));
    }
    html.push_str("</body></html>");
    let (engine, mut page) = load(&html);
    let (w, h) = (800u32, 600u32);
    let mut canvas = rabbithole_engine::gfx::Canvas::new(w, h);
    let mut old = (0.0, 0.0);
    engine.render_damage(&mut page, &mut canvas);
    for y in [37.0f32, 120.0, 95.0, 400.0, 401.0, 0.0] {
        engine.set_page_scroll(&mut page, 0.0, y);
        let damage = engine.render_scrolled(&mut page, &mut canvas, old);
        // Rows touched (rectangles overlap: count each row once).
        let mut rows = vec![false; h as usize];
        for r in &damage {
            for row in (r.y.max(0.0) as usize)..(r.bottom().min(h as f32).ceil() as usize) {
                rows[row] = true;
            }
        }
        let area = rows.iter().filter(|r| **r).count() as f32 * w as f32;
        // Exposed strip + fixed header + sticky bar, not the whole viewport.
        assert!(area < w as f32 * ((y - old.1).abs() + 300.0), "scroll {y}: damage {damage:?}");
        let mut reference = rabbithole_engine::gfx::Canvas::new(w, h);
        engine.render(&page, &mut reference, 0.0, y);
        let diff = canvas.data.iter().zip(&reference.data).filter(|(a, b)| a != b).count();
        if diff > 0 {
            let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
            for (i, (a, b)) in canvas.data.iter().zip(&reference.data).enumerate() {
                if a != b {
                    let (x, yy) = (i as u32 % w, i as u32 / w);
                    x0 = x0.min(x);
                    y0 = y0.min(yy);
                    x1 = x1.max(x);
                    y1 = y1.max(yy);
                }
            }
            eprintln!("diff bbox {x0},{y0} - {x1},{y1}; damage {damage:?}");
        }
        assert_eq!(diff, 0, "scroll {old:?} -> {y}: {diff} pixels differ");
        old = (0.0, y);
    }
}

/// Hover / class changes repaint only the rectangles whose display items
/// changed, with the same pixels as a full repaint.
#[test]
fn style_change_damage_repaint_matches_full_paint() {
    let mut html = String::from(
        r#"<html><head><style>a:hover { color: red; background: #eef } .on { border: 2px solid green }</style></head><body>"#,
    );
    for i in 0..40 {
        html.push_str(&format!("<p id=\"p{i}\">Paragraph {i} with <a id=\"a{i}\" href=\"x\">a link</a> inside.</p>"));
    }
    html.push_str("</body></html>");
    let (engine, mut page) = load(&html);
    let (w, h) = (800u32, 600u32);
    let mut canvas = rabbithole_engine::gfx::Canvas::new(w, h);
    engine.render_damage(&mut page, &mut canvas);
    let steps: Vec<Box<dyn Fn(&Engine, &mut Page)>> = vec![
        Box::new(|e, p| {
            let a = id(p, "a3");
            e.set_hover(p, Some(a));
        }),
        Box::new(|e, p| {
            let a = id(p, "a5");
            e.set_hover(p, Some(a));
        }),
        Box::new(|e, p| {
            let n = id(p, "p7");
            p.doc.set_attr(n, "class", Some("on"));
            e.restyle(p);
        }),
        Box::new(|e, p| {
            e.set_hover(p, None);
        }),
    ];
    for (i, step) in steps.iter().enumerate() {
        step(&engine, &mut page);
        let damage = engine.render_damage(&mut page, &mut canvas);
        let area: f32 = damage.iter().map(|r| r.w * r.h).sum();
        let mut reference = rabbithole_engine::gfx::Canvas::new(w, h);
        engine.render(&page, &mut reference, 0.0, 0.0);
        let diff = canvas.data.iter().zip(&reference.data).filter(|(a, b)| a != b).count();
        assert_eq!(diff, 0, "step {i}: {diff} pixels differ (damage {damage:?})");
        // Hover changes only touch the links (step 2 moves later content).
        if i != 2 {
            assert!(area > 0.0 && area < (w * h) as f32 * 0.2, "step {i}: damage {damage:?}");
        }
    }
}

/// Acid2 (tables, collapsed margins, floats, fixed boxes, `:hover`): hover
/// changes and class toggles applied incrementally equal a full recompute.
#[test]
fn acid2_incremental_matches_full() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/acid2/test.html");
    let html = std::fs::read_to_string(&path).expect("acid2 fixture");
    let (engine, mut page) = load(&html);
    let els: Vec<NodeId> = page.doc.descendants(0).filter(|&n| page.doc.element(n).is_some() && page.styles.get(n).is_some()).collect();
    for (i, &e) in els.iter().enumerate().step_by(3) {
        engine.set_hover(&mut page, Some(e));
        if let Err(err) = engine.check_incremental(&mut page) {
            panic!("hover {i}: {err}");
        }
        if i % 2 == 0 {
            let cls = page.doc.attr(e, "class").unwrap_or("").to_string();
            page.doc.set_attr(e, "class", Some(&format!("{cls} rh-x")));
            check(&engine, &mut page, "class");
            page.doc.set_attr(e, "class", Some(&cls));
            check(&engine, &mut page, "class back");
        }
    }
}

const CARD_SHEET: &str = ":host { display: block; border: 1px solid red } :host(.on) { padding: 10px } ::slotted(.light) { color: blue } .in { margin: 3px } slot[name=title] { font-weight: bold }";

/// Attach a shadow root with an adopted sheet, a named slot with fallback
/// text and a default slot.
fn build_shadow(page: &mut Page, host: NodeId) -> NodeId {
    let root = page.doc.attach_shadow(host);
    page.doc.adopted_styles.insert(root, vec![CARD_SHEET.to_string()]);
    let wrap = page.doc.create_element("div", vec![("class".into(), "in".into())]);
    let title = page.doc.create_element("slot", vec![("name".into(), "title".into())]);
    let fallback = page.doc.create_text("no title");
    page.doc.append_child(title, fallback);
    let body = page.doc.create_element("slot", vec![]);
    page.doc.append_child(wrap, title);
    page.doc.append_child(wrap, body);
    page.doc.append_child(root, wrap);
    page.doc.defined_custom.insert(host);
    root
}

/// Shadow trees: attaching roots, adopted sheets, slot assignment,
/// fallback content, `:host` / `::slotted` / `:defined` and light-DOM
/// insertions are restyled incrementally (not a full restyle each time).
#[test]
fn shadow_dom_changes_restyle_incrementally() {
    let (engine, mut page) = load(
        r#"<html><head><style>
        x-card { display: block; margin: 4px } x-card:not(:defined) { color: gray }
        .light { padding: 2px } .big { font-size: 30px }
        </style></head><body>
        <div id="feed"><x-card id="c1"><span class="light" slot="title">t1</span><p class="light">body 1</p></x-card></div>
        <x-card id="c2"><b>light</b></x-card>
        </body></html>"#,
    );
    let feed = id(&page, "feed");
    let c1 = id(&page, "c1");
    let c2 = id(&page, "c2");
    check(&engine, &mut page, "initial");
    let r1 = build_shadow(&mut page, c1);
    check(&engine, &mut page, "first shadow root");
    let full_before = page.incr.stats.style_full;
    let r2 = build_shadow(&mut page, c2);
    check(&engine, &mut page, "second shadow root (same sheet)");
    page.doc.set_attr(c1, "class", Some("on"));
    check(&engine, &mut page, ":host(.on)");
    // Light children: insertion, slot reassignment, fallback content.
    for i in 0..3 {
        let card = page.doc.create_element("x-card", vec![]);
        let t = page.doc.create_element("span", vec![("class".into(), "light".into()), ("slot".into(), "title".into())]);
        let tt = page.doc.create_text(&format!("new {i}"));
        page.doc.append_child(t, tt);
        page.doc.append_child(card, t);
        page.doc.append_child(feed, card);
        check(&engine, &mut page, "new undefined card in feed");
        build_shadow(&mut page, card);
        check(&engine, &mut page, "card upgraded");
    }
    let b = page.doc.child_elements(c2).next().unwrap();
    page.doc.set_attr(b, "slot", Some("title"));
    check(&engine, &mut page, "slot attribute: fallback hidden");
    page.doc.set_attr(b, "slot", Some("nowhere"));
    check(&engine, &mut page, "slot attribute: unassigned, fallback back");
    page.doc.set_attr(b, "slot", None);
    page.doc.set_attr(b, "class", Some("light big"));
    check(&engine, &mut page, "slotted class");
    // Slot renamed / removed inside the shadow tree.
    let wrap = page.doc.child_elements(r2).next().unwrap();
    let title_slot = page.doc.child_elements(wrap).next().unwrap();
    page.doc.set_attr(title_slot, "name", Some("other"));
    check(&engine, &mut page, "slot renamed");
    page.doc.detach(title_slot);
    check(&engine, &mut page, "slot removed");
    // A light child removed, a shadow child added.
    let p = page.doc.child_elements(c1).nth(1).unwrap();
    page.doc.detach(p);
    check(&engine, &mut page, "light child removed");
    let extra = page.doc.create_element("div", vec![("class".into(), "in big".into())]);
    page.doc.append_child(r1, extra);
    check(&engine, &mut page, "shadow child added");
    // The adopted sheets of one root replaced.
    page.doc.adopted_styles.insert(r1, vec![":host { display: block; margin-left: 40px } .in { color: green }".to_string()]);
    check(&engine, &mut page, "adopted sheet replaced");
    // Document-level changes next to shadow trees.
    page.doc.set_attr(feed, "class", Some("big"));
    check(&engine, &mut page, "document class");
    assert_eq!(page.incr.stats.style_full, full_before, "shadow DOM changes must not restyle the whole document");
}
