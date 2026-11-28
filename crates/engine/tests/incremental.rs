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

/// Inserting or removing list items renumbers the ordinal markers after
/// them (all of them in reversed lists); bullets stay as they are.
#[test]
fn list_item_changes_renumber_markers() {
    let (engine, mut page) = load(
        r#"<html><body><ol id="o"><li>a</li><li id="o2">b</li><li>c</li><li>d</li></ol>
        <ol id="r" reversed><li>a</li><li id="r2">b</li><li>c</li></ol>
        <ul id="u"><li>a</li><li id="u2">b</li><li>c</li></ul>
        <ul id="m" style="list-style-type: lower-roman"><li>a</li><li id="m2" style="list-style-type: disc">b</li><li>c</li></ul></body></html>"#,
    );
    for (list, at) in [("o", "o2"), ("r", "r2"), ("u", "u2"), ("m", "m2")] {
        let parent = id(&page, list);
        let before = id(&page, at);
        let li = page.doc.create_element("li", vec![]);
        let t = page.doc.create_text("new");
        page.doc.append_child(li, t);
        page.doc.insert_before(parent, li, Some(before));
        check(&engine, &mut page, &format!("insert into #{list}"));
        let li2 = page.doc.create_element("li", vec![]);
        page.doc.append_child(parent, li2);
        let first = page.doc.first_child(parent);
        let li3 = page.doc.create_element("li", vec![]);
        page.doc.insert_before(parent, li3, first);
        check(&engine, &mut page, &format!("append and prepend in #{list}"));
        page.doc.detach(li);
        check(&engine, &mut page, &format!("remove from #{list}"));
        let d = page.doc.create_element("div", vec![]);
        page.doc.insert_before(parent, d, Some(before));
        check(&engine, &mut page, &format!("insert a non-item into #{list}"));
    }
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
    // Custom properties reused from the previous restyle must follow both
    // the element's own declarations and its parent's map.
    page.doc.set_attr(o, "style", Some("--c: purple; --c: orange"));
    check(&engine, &mut page, "own vars change");
    page.doc.set_attr(html, "style", Some("--gap: 3px"));
    check(&engine, &mut page, "parent vars change");
    page.doc.set_attr(o, "style", Some("--c: purple; --c: orange"));
    page.doc.set_attr(html, "style", Some("--gap: 3px; --c: teal"));
    check(&engine, &mut page, "parent vars change, own unchanged");
    page.doc.set_attr(o, "style", None);
    check(&engine, &mut page, "own vars removed");
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

/// `:has()` in compounds that style other elements than the subject
/// (descendants, following siblings): only subjects whose `:has()` result
/// flipped restyle those, after the first evaluation without full restyles.
#[test]
fn has_with_non_subject_relations_restyles_only_flipped_subjects() {
    let mut html = String::from(r#"<html><head><style>
        .g:has(.on) .t { color: red; padding-left: 4px }
        .g:has(> .k) + .s { margin-top: 7px }
        .h:has(+ .m) .t { padding-top: 3px }
        .e:not(:has(> :not(:empty))) { width: 10px; height: 10px; background: blue }
        </style></head><body>"#);
    html.push_str(r#"<div class="g" id="g"><div><span id="deep">deep</span></div><p class="t">t</p></div><div class="s">s</div>"#);
    html.push_str(r#"<div class="h" id="h"><p class="t">h</p></div><div id="after">x</div>"#);
    html.push_str(r#"<div class="e" id="e"><i id="ei"></i></div>"#);
    for i in 0..200 {
        html.push_str(&format!("<div class=\"item\"><p>Paragraph {i}</p></div>"));
    }
    html.push_str("</body></html>");
    let (engine, mut page) = load(&html);
    let (g, deep, h, after, ei) = (id(&page, "g"), id(&page, "deep"), id(&page, "h"), id(&page, "after"), id(&page, "ei"));
    let mut warm = None;
    for round in 0..4 {
        page.doc.set_attr(deep, "class", Some(if round % 2 == 0 { "on" } else { "off" }));
        check(&engine, &mut page, "descendant flips .g:has(.on)");
        page.doc.set_attr(deep, "title", Some(&format!("r{round}")));
        check(&engine, &mut page, "change that leaves :has() as it was");
        let k = page.doc.create_element("b", vec![("class".into(), "k".into())]);
        page.doc.append_child(g, k);
        check(&engine, &mut page, "child .k makes .g:has(> .k) + .s match");
        page.doc.detach(k);
        check(&engine, &mut page, "child .k removed");
        page.doc.set_attr(after, "class", Some(if round % 2 == 0 { "m" } else { "" }));
        check(&engine, &mut page, "next sibling flips .h:has(+ .m)");
        let text = page.doc.create_text(if round % 2 == 0 { "now not empty" } else { "" });
        page.doc.append_child(ei, text);
        check(&engine, &mut page, "text inside a :not(:has(> :not(:empty))) child");
        page.doc.detach(text);
        check(&engine, &mut page, "text removed");
        if round == 1 {
            warm = Some(page.incr.stats.clone());
        }
    }
    let warm = warm.unwrap();
    assert_eq!(page.incr.stats.style_full, warm.style_full, "no full restyle once :has() results are known");
    let _ = h;
}

/// `:has()` arguments whose inner compounds match outside the subject:
/// sibling insertions before the subject's following siblings and class
/// changes left of `+` / `~` inside `:is()` (WPT selectors/invalidation).
#[test]
fn has_arguments_matching_outside_the_subject() {
    let (engine, mut page) = load(r#"<html><head><style>
        div, main { color: grey }
        div:has(~ div .test) #subject { color: purple }
        div:has(+ .test) #subject { color: blue }
        .green:has(#descendant:is(.p + .c ~ .d .e)) { color: green }
        .orange:has(#descendant:is(:is(.m, .n) .e)) { color: orange }
        </style></head><body>
        <main id="main"><div id="a"><div><div id="subject">s</div></div></div><div id="n"><div><div id="nd"></div></div></div></main>
        <div id="has_scope" class="green"><div class="p"></div><div id="prev"></div><div class="d"><div id="descendant" class="e">x</div></div></div>
        </body></html>"#);
    let (main, n, nd, a, prev) = (id(&page, "main"), id(&page, "n"), id(&page, "nd"), id(&page, "a"), id(&page, "prev"));
    for _ in 0..2 {
        page.doc.set_attr(nd, "class", Some("test"));
        check(&engine, &mut page, "class in a following sibling's subtree");
        page.doc.set_attr(nd, "class", Some(""));
        check(&engine, &mut page, "class removed again");
        let t = page.doc.create_element("div", vec![("class".into(), "test".into())]);
        page.doc.insert_before(main, t, Some(n));
        check(&engine, &mut page, "div.test inserted after the subject's ancestor");
        page.doc.detach(t);
        check(&engine, &mut page, "div.test removed");
        let u = page.doc.create_element("div", vec![("class".into(), "test".into())]);
        page.doc.append_child(main, u);
        check(&engine, &mut page, "div.test appended after the next sibling");
        page.doc.detach(u);
        check(&engine, &mut page, "appended div.test removed");
        page.doc.set_attr(prev, "class", Some("c"));
        check(&engine, &mut page, ".c before .d inside :is() in :has()");
        let scope = id(&page, "has_scope");
        page.doc.set_attr(scope, "class", Some("green orange m"));
        check(&engine, &mut page, ".m on an ancestor inside nested :is() in :has()");
        page.doc.set_attr(scope, "class", Some("green orange"));
        check(&engine, &mut page, ".m removed");
        page.doc.set_attr(prev, "class", Some(""));
        check(&engine, &mut page, ".c removed");
    }
    let _ = a;
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
        .l:has(.c) .b { padding-left: 6px } .m:has(+ .d) ~ .p { color: green } .n:not(:has(> :not(:empty))) + .i { width: 12px }
        .p:not(:has(.a)) .c { display: block; height: 3px } .k:is(:has(+ .e)) { color: blue }
        div:has(~ div .h) .b { padding-top: 4px } .g:has(.b:is(.e + .k ~ .m .c)) { color: teal } .q:has(+ .d .a) ~ .r { margin-left: 3px }
        section:has(~ .h) > span { margin-right: 5px }
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

#[test]
fn line_clamp_removal_restores_descendant_lines() {
    let (engine, mut page) = load(r#"<style>.clamp { line-clamp: 1; font: 16px/32px serif }</style><div class="clamp" id="container"><div>Line 1</div><div>Line 2</div><div>Line 3</div></div>"#);
    let container = id(&page, "container");
    let clamped = page.rect_of(container).unwrap().h;
    for clamp in ["none", "2", "none", "1", "none"] {
        page.doc.set_attr(container, "style", Some(&format!("line-clamp: {clamp}")));
        check(&engine, &mut page, "changing line-clamp must restore discarded lines");
        let height = page.rect_of(container).unwrap().h;
        if clamp == "none" { assert!(height >= clamped + 63.0, "{height} versus {clamped}"); }
    }
}

#[test]
fn line_clamp_removal_from_script_after_measuring_layout() {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.loader.load_images = false;
    let html = r#"<style>.clamp { line-clamp: 1; font: 16px/32px serif }</style><div class="clamp" id="container"><div>Line 1</div><div>Line 2</div><div>Line 3</div></div><script>document.body.offsetHeight; document.getElementById("container").style.lineClamp = "none";</script>"#;
    let mut page = engine.load_html(html, Url::parse("https://example.test/").unwrap(), Viewport { width: 800.0, height: 600.0 });
    let container = id(&page, "container");
    engine.restyle(&mut page);
    assert_eq!(page.doc.attr(container, "style"), Some("line-clamp: none;"));
    assert!(page.rect_of(container).unwrap().h >= 95.0);
}

/// Flex items are laid out once to measure them and again stretched to the
/// line's height. When their contents do not depend on that height, the
/// layout cache serves both passes when the row is laid out again instead
/// of laying every card out twice; contents that do depend on it (a
/// percentage height) are laid out again, and the result equals a full
/// layout.
#[test]
fn stretched_flex_items_are_reused_from_the_layout_cache() {
    let mut html = String::from("<html><head><style>.row{display:flex} .card{flex:1;padding:4px} .tall{height:80px} .tall.alt{color:red} .pct{height:50%}</style></head><body>");
    for i in 0..40 {
        html.push_str(&format!("<div class=\"row\"><div class=\"card\"><p>Card {i} with a few words to wrap</p></div><div class=\"card tall\" id=\"t{i}\">side</div><div class=\"card\"><div class=\"pct\">half</div></div></div>"));
    }
    html.push_str("</body></html>");
    let (engine, mut page) = load(&html);
    let talls: Vec<NodeId> = (0..40).map(|i| id(&page, &format!("t{i}"))).collect();
    let mut last = (0, 0, 0);
    for round in 0..3 {
        // A color change rebuilds the tall card, so every row is laid out
        // again while its cards keep their widths and heights.
        for &t in &talls {
            page.doc.set_attr(t, "class", Some(if round % 2 == 0 { "card tall alt" } else { "card tall" }));
        }
        check(&engine, &mut page, "row relayout");
        let _ = rabbithole_engine::layout::take_cache_stats();
        for &t in &talls {
            page.doc.set_attr(t, "class", Some(if round % 2 == 0 { "card tall" } else { "card tall alt" }));
        }
        engine.restyle(&mut page);
        last = rabbithole_engine::layout::take_cache_stats();
    }
    let (hits, misses, _) = last;
    // Per row: the text card hits in both passes; the percentage-height
    // card is laid out again in both.
    assert!(hits >= 80, "hits {hits}, misses {misses}");
    assert!(misses <= 100, "hits {hits}, misses {misses}");
}

/// Grid items are measured (used grid tracks not published) and then laid
/// out (published). The measuring pass reuses the published layout from the
/// cache (same geometry), so grids are not laid out from scratch on every
/// relayout of their container.
#[test]
fn measured_grid_items_reuse_published_layouts() {
    let mut html = String::from("<html><head><style>.g{display:grid;grid-template-columns:120px 1fr;gap:2px} .side.alt{color:red}</style></head><body>");
    for i in 0..40 {
        html.push_str(&format!("<div class=\"wrap\"><div class=\"g\"><div><p>a{i} text</p></div><div><p>b {i} with more words</p></div><div><p>c</p></div><div class=\"side\" id=\"s{i}\"><p>d</p></div></div></div>"));
    }
    html.push_str("</body></html>");
    let (engine, mut page) = load(&html);
    let sides: Vec<NodeId> = (0..40).map(|i| id(&page, &format!("s{i}"))).collect();
    let mut last = (0, 0, 0);
    for round in 0..3 {
        for &s in &sides {
            page.doc.set_attr(s, "class", Some(if round % 2 == 0 { "side alt" } else { "side" }));
        }
        check(&engine, &mut page, "grid relayout");
        let _ = rabbithole_engine::layout::take_cache_stats();
        for &s in &sides {
            page.doc.set_attr(s, "class", Some(if round % 2 == 0 { "side" } else { "side alt" }));
        }
        engine.restyle(&mut page);
        last = rabbithole_engine::layout::take_cache_stats();
    }
    let (hits, misses, _) = last;
    // Without reuse every measuring pass misses (0 hits, 560 misses); the
    // remaining misses measure items at other widths (min/max-content).
    assert!(hits >= 200, "hits {hits}, misses {misses}");
}

/// Random mutations against layout-heavy styles (flex and grid with
/// intrinsic sizes, percentages, min/max-content, aspect ratios, absolute
/// and sticky boxes, scroll containers, transforms, hover rules that change
/// geometry), with element and page scrolling between them: after each
/// step the incremental result must equal a full recomputation.
/// `RH_FUZZ_LAYOUT_SEEDS` sets the number of seeds (default 4).
#[test]
fn randomized_layout_mutations_match_full_recompute() {
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
        body { margin: 0 }
        .a { display: flex; flex-wrap: wrap; gap: 4px } .a > * { flex: 1 1 0; min-width: 0 }
        .b { display: flex } .b > * { flex: 0 0 auto } .c { display: grid; grid-template-columns: repeat(auto-fill, minmax(90px, 1fr)); gap: 6px }
        .d { display: grid; grid-template-columns: auto 1fr max-content } .e { width: 50%; padding: 2% } .f { width: max-content }
        .g { width: min-content } .h { width: fit-content; max-width: 70% } .i { aspect-ratio: 16 / 9; width: 30% }
        .j { position: relative } .k { position: absolute; right: 0; bottom: 0; width: 40%; height: 20px }
        .l { position: sticky; top: 0; height: 15px } .m { overflow: auto; height: 60px } .n { overflow: hidden; max-height: 40px }
        .o { transform: translateX(10px) scale(1.1) } .p { display: inline-block; width: 33% } .q { float: left; width: 25% }
        .r { display: none } .s { white-space: nowrap } .t { margin: 0 auto; width: 60% } .u { height: 100% }
        .v { display: inline-flex; align-items: center } .w { min-height: 30px } .x { contain: layout } .y { position: fixed; top: 5px; left: 5px; width: 30px; height: 10px }
        .z { flex: 2 1 120px }
        .a:hover > * { padding: 3px } .c:hover .p { width: 45% } .j:hover .k { width: 60% } div:hover > .s { white-space: normal }
        .b:has(.r) { gap: 7px } .m:hover { height: 80px }
    "#;
    let classes = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s", "t", "u", "v", "w", "x", "y", "z", ""];
    let tags = ["div", "div", "div", "span", "p", "section"];
    let styles = ["", "width: 120px", "width: 40%", "padding: 4px 8%", "display: none", "height: 25px", "min-width: max-content", "flex-basis: 30%", "grid-column: span 2", "margin-left: -10px", "font-size: 20px", "position: relative; left: 7px"];
    let words = ["alpha", "beta gamma", "a much longer run of words that wraps across lines", "", "x", "supercalifragilisticexpialidocious"];
    let seeds: u64 = std::env::var("RH_FUZZ_LAYOUT_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(4);
    for seed in 1..=seeds {
        let mut rng = Rng(0x2545f4914f6cdd1d ^ seed.wrapping_mul(0x9e37_79b9_7f4a));
        let mut html = format!("<html><head><style>{CSS}</style></head><body>");
        fn tree(rng: &mut Rng, html: &mut String, depth: u32, classes: &[&str], tags: &[&str], words: &[&str]) {
            let n = 1 + rng.next() % 4;
            for _ in 0..n {
                let t = *rng.pick(tags);
                html.push_str(&format!("<{t} class=\"{} {}\">{}", rng.pick(classes), rng.pick(classes), rng.pick(words)));
                if depth < 3 && rng.next() % 3 != 0 {
                    tree(rng, html, depth + 1, classes, tags, words);
                }
                html.push_str(&format!("</{t}>"));
            }
        }
        for _ in 0..12 {
            tree(&mut rng, &mut html, 0, &classes, &tags, &words);
        }
        html.push_str("</body></html>");
        let (engine, mut page) = load(&html);
        for step in 0..80 {
            let body = page.doc.body().unwrap();
            let els: Vec<NodeId> = page.doc.descendants(body).filter(|&n| page.doc.element(n).is_some()).collect();
            let el = *rng.pick(&els);
            let what = rng.next() % 9;
            match what {
                0 => {
                    let c = format!("{} {}", rng.pick(&classes), rng.pick(&classes));
                    page.doc.set_attr(el, "class", Some(&c));
                }
                1 => page.doc.set_attr(el, "style", Some(rng.pick(&styles))),
                2 => {
                    let t = *rng.pick(&tags);
                    let n = page.doc.create_element(t, vec![("class".into(), rng.pick(&classes).to_string())]);
                    let txt = page.doc.create_text(rng.pick(&words));
                    page.doc.append_child(n, txt);
                    page.doc.append_child(el, n);
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
                5 | 6 => {
                    engine.set_hover(&mut page, Some(el));
                }
                7 => {
                    // Scroll an element (only scroll containers move).
                    engine.restyle(&mut page);
                    let y = (rng.next() % 50) as f32;
                    engine.set_element_scroll(&mut page, el, 0.0, y);
                }
                _ => {
                    let y = (rng.next() % 400) as f32;
                    engine.set_page_scroll(&mut page, 0.0, y);
                }
            }
            engine.restyle(&mut page);
            if let Err(e) = engine.check_incremental(&mut page) {
                panic!("seed {seed} step {step} (op {what}): {e}");
            }
            if let Err(e) = engine.check_layout_idempotent(&mut page) {
                panic!("seed {seed} step {step} (op {what}): {e}");
            }
        }
    }
}

/// A scroll container's overflow leaves out absolutely positioned boxes
/// whose containing block is an ancestor (they are positioned after the
/// container is laid out, so their old geometry made full and incremental
/// layouts disagree), and keeps those it contains.
#[test]
fn scroll_overflow_ignores_out_of_flow_boxes_contained_by_ancestors() {
    let (engine, mut page) = load(
        r#"<html><head><style>
        body { margin: 0 } .cb { position: relative; width: 400px; height: 200px }
        .clip { display: inline-flex; width: 32px; height: 32px; overflow: hidden }
        .label { position: absolute; left: 5px; top: 5px; width: 300px; height: 16px }
        .own { position: relative } .inner { position: absolute; left: 0; top: 0; width: 200px; height: 10px }
        </style></head><body>
        <div class="cb"><span id="pad">text</span><div class="clip" id="clip"><i>x</i><span class="label" id="label">a long label</span></div>
        <div class="clip own" id="own"><span class="inner">inner</span></div></div>
        </body></html>"#,
    );
    let (clip, own, pad, label) = (id(&page, "clip"), id(&page, "own"), id(&page, "pad"), id(&page, "label"));
    let scroll_w = |page: &Page, n: NodeId| page.layout.box_of(&page.doc, n).map(|b| b.scroll_width).unwrap();
    assert!((scroll_w(&page, clip) - 32.0).abs() < 0.5, "not the container's overflow: {}", scroll_w(&page, clip));
    assert!((scroll_w(&page, own) - 200.0).abs() < 0.5, "its own absolute child overflows: {}", scroll_w(&page, own));
    for (i, w) in ["x", "longer text", "t"].iter().enumerate() {
        page.doc.set_attr(pad, "title", Some(&i.to_string()));
        if let Some(t) = page.doc.children(pad).next() {
            page.doc.set_text(t, w);
        }
        check(&engine, &mut page, "text before the container");
        page.doc.set_attr(label, "style", Some(&format!("left: {}px", 5 + 40 * i)));
        check(&engine, &mut page, "label moved");
        assert!((scroll_w(&page, clip) - 32.0).abs() < 0.5);
    }
}

/// Scripts scroll within the CSSOM scrolling area; layout keeps that
/// offset deterministically (a later relayout must neither clamp it nor
/// differ from a layout from scratch).
#[test]
fn element_scroll_offset_survives_relayout() {
    let (engine, mut page) = load(
        r#"<html><head><style>
        #s { width: 100px; height: 50px; overflow: auto; display: flex; flex-direction: row }
        #s > div { flex: 0 0 80px; height: 30px; margin-right: -20px }
        </style></head><body><div id="s"><div>a</div><div>b</div><div>c</div><div id="last">d</div></div><p id="p">p</p></body></html>"#,
    );
    let (s, p) = (id(&page, "s"), id(&page, "p"));
    engine.set_element_scroll(&mut page, s, 10_000.0, 0.0);
    let (offset, _) = engine.element_scroll(&page, s).unwrap();
    assert!(offset.0 > 0.0, "scrolled: {offset:?}");
    check(&engine, &mut page, "after scrolling");
    page.doc.set_attr(s, "class", Some("x"));
    page.doc.set_attr(p, "style", Some("width: 50px"));
    check(&engine, &mut page, "after an unrelated relayout");
    let (again, _) = engine.element_scroll(&page, s).unwrap();
    assert_eq!(offset, again, "offset kept");
}

/// A class change matched by an inner compound of a `:has()` argument
/// whose subject can be the root must not restyle the whole document
/// (the root's cached `:has()` result stays known).
#[test]
fn has_inner_compound_changes_do_not_restyle_the_document() {
    let mut html = String::from(
        r#"<html><head><style>
        :has(.open .menu) .badge { color: red }
        .open { padding: 1px }
        </style></head><body><div id="w"><div id="t"><span class="menu">m</span></div></div>"#,
    );
    for i in 0..300 {
        html.push_str(&format!("<div class=\"item\"><p>Paragraph {i} <span class=\"badge\">b</span></p></div>"));
    }
    html.push_str("</body></html>");
    let (engine, mut page) = load(&html);
    let t = id(&page, "t");
    // Flips restyle the subjects' subtrees (the document): results only.
    for round in 0..4 {
        page.doc.set_attr(t, "class", Some(if round % 2 == 0 { "open" } else { "" }));
        check(&engine, &mut page, "inner :has() compound flips");
    }
    // With `.open .menu` matched through #t, the same class on an ancestor
    // flips nothing.
    page.doc.set_attr(t, "class", Some("open"));
    check(&engine, &mut page, "inner :has() compound matched");
    let w = id(&page, "w");
    let mut costs = Vec::new();
    for round in 0..4 {
        let before = page.incr.stats.clone();
        page.doc.set_attr(w, "class", Some(if round % 2 == 0 { "open x" } else { "x" }));
        check(&engine, &mut page, "class on an ancestor, :has() unchanged");
        costs.push(page.incr.stats.elements_restyled - before.elements_restyled);
    }
    assert!(costs.iter().skip(1).all(|&c| c < 50), "restyled per change: {costs:?}");
}

/// Inserting into an element that keeps other content cannot flip its
/// `:empty`: following siblings related through `:empty + ...` are not
/// restyled.
#[test]
fn insertions_into_non_empty_elements_do_not_restyle_empty_relations() {
    let mut html = String::from(
        r#"<html><head><style>
        div:empty + section { color: red }
        </style></head><body><div id="d"><b>x</b></div>"#,
    );
    for i in 0..300 {
        html.push_str(&format!("<section><p>Paragraph {i}</p></section>"));
    }
    html.push_str("</body></html>");
    let (engine, mut page) = load(&html);
    let d = id(&page, "d");
    let mut costs = Vec::new();
    for _ in 0..3 {
        let before = page.incr.stats.clone();
        let n = page.doc.create_element("i", vec![]);
        page.doc.append_child(d, n);
        check(&engine, &mut page, "child added to a non-empty element");
        costs.push(page.incr.stats.elements_restyled - before.elements_restyled);
    }
    assert!(costs.iter().all(|&c| c < 20), "restyled per insertion: {costs:?}");
    // Emptying and filling it still restyles correctly.
    let kids: Vec<NodeId> = page.doc.children(d).collect();
    for k in kids {
        page.doc.detach(k);
    }
    check(&engine, &mut page, "emptied");
    let t = page.doc.create_text("now");
    page.doc.append_child(d, t);
    check(&engine, &mut page, "text makes it non-empty");
    page.doc.set_text(t, "");
    check(&engine, &mut page, "empty text: empty again");
}

/// A table wider than its specified width (its columns need more) keeps
/// that width when its layout is reused: the cache records the widening.
#[test]
fn reused_table_layout_keeps_widening_for_its_columns() {
    let (engine, mut page) = load(
        r#"<html><head><style>
        .box { float: right; width: 10em; border-collapse: collapse; font-size: 88%; padding: 0.2em }
        .box td { padding: 0.4em 0.6em; border-top: 1px solid }
        .wide { width: 250px; height: 10px }
        a:hover { text-decoration: underline; color: red }
        </style></head><body><section>
        <table class="box" id="t"><tr><td><div class="wide"></div></td><td>cell</td></tr><tr><td colspan="2">more text</td></tr></table>
        <p>Text beside the table with <a href="x" id="a">a link</a> and more words.</p>
        </section></body></html>"#,
    );
    let (t, a) = (id(&page, "t"), id(&page, "a"));
    let w0 = page.rect_of(t).unwrap().w;
    assert!(w0 > 250.0, "widened for its columns: {w0}");
    for round in 0..4 {
        engine.set_hover(&mut page, if round % 2 == 0 { Some(a) } else { None });
        check(&engine, &mut page, "hover beside a widened table");
        assert_eq!(page.rect_of(t).unwrap().w, w0);
    }
}

/// An empty box moved by relative offsets adds no scrollable overflow
/// (WPT css-overflow overflow-empty-child-box).
#[test]
fn empty_relative_box_adds_no_scrollable_overflow() {
    let (engine, mut page) = load(
        r#"<html><head><style>
        .container { border: 1px solid black; width: 200px; height: 100px }
        .empty { position: relative; width: 0px; height: 0px; left: 500px; top: 300px; box-shadow: 0 0 0 10px blue }
        </style></head><body>
        <div class="container" id="c" style="overflow: auto"><div class="empty"></div></div>
        </body></html>"#,
    );
    let c = id(&page, "c");
    let (_, size) = engine.element_scroll(&page, c).unwrap();
    assert_eq!(size, (200.0, 100.0), "{}", page.layout.dump());
    check(&engine, &mut page, "empty relative box");
}
