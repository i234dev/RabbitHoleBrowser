//! Text selection end to end: the Selection API over the document's
//! selection, user-agent selection (points, words, blocks, `user-select`),
//! copied text and painting (default colors and `::selection`).
use rabbithole_engine::dom::BoundaryPoint;
use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Page, SelectStart, Viewport};

fn load(engine: &mut Engine, body: &str) -> Page {
    engine.load_html(body, Url::parse("https://example.test/selection").unwrap(), Viewport { width: 400., height: 200. })
}

/// Run a script that returns "ok" or a description of the first failure.
fn check(engine: &Engine, page: &mut Page, script: &str) {
    let src = format!("(() => {{ const fails = []; const eq = (a, b, what) => {{ if (a !== b) fails.push(what + ': ' + String(a) + ' !== ' + String(b)); }}; {script}; return fails.length ? fails.join('; ') : 'ok'; }})()");
    let r = engine.eval_in_page(page, &src);
    assert!(r.contains("ok") && !r.contains("!=="), "{r}");
}

fn text_node(page: &Page, id: &str) -> usize {
    let el = page.doc.element_by_id(id).unwrap();
    page.doc.children(el).find(|&c| page.doc.is_text(c)).unwrap()
}

/// Count pixels of a color in a region of the rendered page.
fn count(engine: &Engine, page: &Page, color: (u8, u8, u8), region: (u32, u32, u32, u32)) -> usize {
    let f = engine.snapshot(page);
    let mut n = 0;
    for y in region.1..(region.1 + region.3).min(f.height) {
        for x in region.0..(region.0 + region.2).min(f.width) {
            let i = ((y * f.width + x) * 4) as usize;
            if (f.rgba[i], f.rgba[i + 1], f.rgba[i + 2]) == color {
                n += 1;
            }
        }
    }
    n
}

const PAGE: &str = "<!doctype html><style>body{margin:0;font:20px sans-serif}p{margin:0 0 10px}</style>\
    <p id=a>Hello brave world</p><p id=b>Second <b id=bold>bold</b> line</p>";

#[test]
fn selection_api_semantics() {
    let mut engine = Engine::new_bundled_fonts_only();
    let mut page = load(&mut engine, PAGE);
    check(&engine, &mut page, r#"
        const s = getSelection();
        eq(s, document.getSelection(), 'same object');
        eq(s.rangeCount, 0, 'empty'); eq(s.type, 'None', 'type None'); eq(s.anchorNode, null, 'anchor null');
        const t = document.getElementById('a').firstChild;
        s.collapse(t, 2);
        eq(s.rangeCount, 1, 'collapsed count'); eq(s.type, 'Caret', 'caret'); eq(s.isCollapsed, true, 'isCollapsed');
        s.extend(t, 7);
        eq(s.type, 'Range', 'range'); eq(s.toString(), 'llo b', 'extend text'); eq(s.direction, 'forward', 'forward');
        s.extend(t, 0);
        eq(s.anchorOffset, 2, 'anchor kept'); eq(s.focusOffset, 0, 'focus'); eq(s.direction, 'backward', 'backward');
        const r = s.getRangeAt(0);
        eq(r.startOffset, 0, 'range start'); eq(r.endOffset, 2, 'range end');
        eq(s.getRangeAt(0), r, 'stable range object');
        // The selection's range is live and shared.
        r.setEnd(t, 5);
        eq(s.toString(), 'Hello', 'range mutation shows');
        const t2 = document.getElementById('b').firstChild;
        s.setBaseAndExtent(t2, 3, t, 1);
        eq(s.anchorNode, t2, 'base'); eq(s.focusNode, t, 'extent'); eq(s.direction, 'backward', 'base after extent');
        eq(s.containsNode(document.getElementById('a'), false), false, 'p#a not fully contained');
        eq(s.containsNode(document.getElementById('a'), true), true, 'p#a partly contained');
        s.selectAllChildren(document.getElementById('b'));
        eq(s.toString(), 'Second bold line', 'selectAllChildren');
        eq(s.containsNode(document.getElementById('bold'), false), true, 'bold contained');
        s.collapseToEnd(); eq(s.isCollapsed, true, 'collapseToEnd');
        let threw = '';
        try { s.getRangeAt(1); } catch (e) { threw = e.name; } eq(threw, 'IndexSizeError', 'getRangeAt(1)');
        try { s.collapse(t, 99); } catch (e) { threw = e.name; } eq(threw, 'IndexSizeError', 'collapse offset');
        s.removeAllRanges(); eq(s.rangeCount, 0, 'removeAllRanges');
        threw = ''; try { s.extend(t, 1); } catch (e) { threw = e.name; } eq(threw, 'InvalidStateError', 'extend empty');
        const nr = document.createRange(); nr.setStart(t, 6); nr.setEnd(t, 11);
        s.addRange(nr); eq(s.getRangeAt(0), nr, 'addRange keeps the object'); eq(s.toString(), 'brave', 'addRange text');
        const other = document.createRange(); s.addRange(other); eq(s.getRangeAt(0), nr, 'second addRange ignored');
        threw = ''; try { s.removeRange(other); } catch (e) { threw = e.name; } eq(threw, 'NotFoundError', 'removeRange other');
        s.removeRange(nr); eq(s.rangeCount, 0, 'removeRange');
        // Detached nodes are ignored.
        const d = document.createElement('div'); d.textContent = 'x';
        s.collapse(d.firstChild, 0); eq(s.rangeCount, 0, 'collapse in detached node ignored');
        s.collapse(t, 0); s.modify('extend', 'forward', 'word'); eq(s.toString(), 'Hello', 'modify word');
        s.modify('extend', 'forward', 'character'); eq(s.toString(), 'Hello ', 'modify character');
        s.modify('move', 'forward', 'character'); eq(s.isCollapsed, true, 'modify move collapses');
        s.selectAllChildren(document.getElementById('a'));
        s.deleteFromDocument(); eq(document.getElementById('a').textContent, '', 'deleteFromDocument');
    "#);
}

#[test]
fn selection_is_shared_with_the_user_agent_and_live() {
    let mut engine = Engine::new_bundled_fonts_only();
    let mut page = load(&mut engine, PAGE);
    let a = text_node(&page, "a");
    // A user-agent selection is visible to scripts.
    page.doc.set_selection(BoundaryPoint { node: a, offset: 11 }, BoundaryPoint { node: a, offset: 6 });
    check(&engine, &mut page, r#"
        const s = getSelection();
        eq(s.toString(), 'brave', 'UA selection text'); eq(s.direction, 'backward', 'UA direction');
        eq(s.anchorOffset, 11, 'UA anchor');
        window.kept = s.getRangeAt(0);
    "#);
    // A script selection is the document's.
    check(&engine, &mut page, "const t = document.getElementById('b').firstChild; getSelection().setBaseAndExtent(t, 0, t, 6);");
    let b = text_node(&page, "b");
    let sel = page.doc.selection().unwrap();
    assert_eq!((sel.start, sel.end, sel.backward), (BoundaryPoint { node: b, offset: 0 }, BoundaryPoint { node: b, offset: 6 }, false));
    // The UA replacing the selection gives scripts a new range; the old
    // one keeps its boundary points.
    page.doc.set_selection(BoundaryPoint { node: a, offset: 0 }, BoundaryPoint { node: a, offset: 5 });
    check(&engine, &mut page, "const s = getSelection(); eq(s.toString(), 'Hello', 'replaced'); eq(s.getRangeAt(0) === window.kept, false, 'new range'); eq(window.kept.toString(), 'brave', 'old range kept');");
    // Mutations move the selection (live range).
    check(&engine, &mut page, "const t = document.getElementById('a').firstChild; t.insertData(0, '>> ');");
    let sel = page.doc.selection().unwrap();
    // (DOM "replace data": a boundary at the insertion offset stays.)
    assert_eq!((sel.start.offset, sel.end.offset), (0, 8));
    check(&engine, &mut page, "eq(getSelection().toString(), '>> Hello', 'moved with the text');");
    check(&engine, &mut page, "document.getElementById('a').remove(); const s = getSelection(); eq(s.rangeCount, 1, 'still selected'); eq(s.isCollapsed, true, 'collapsed at the removal point'); eq(s.anchorNode, document.body, 'at the parent');");
}

#[test]
fn selectionchange_is_fired() {
    let mut engine = Engine::new_bundled_fonts_only();
    let mut page = load(&mut engine, PAGE);
    check(&engine, &mut page, "window.changes = 0; document.addEventListener('selectionchange', () => window.changes++); getSelection().selectAllChildren(document.body); getSelection().collapseToStart();");
    engine.settle_for_snapshot(&mut page, std::time::Duration::from_millis(500));
    check(&engine, &mut page, "eq(window.changes, 1, 'one coalesced selectionchange');");
}

#[test]
fn user_agent_positions_words_and_blocks() {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.scripting = false;
    let page = load(&mut engine, PAGE);
    let a = text_node(&page, "a");
    let fonts = page.fonts.clone();
    // Start and end of the first line.
    assert_eq!(page.position_at(&fonts, 0.0, 10.0), Some(BoundaryPoint { node: a, offset: 0 }));
    assert_eq!(page.position_at(&fonts, 399.0, 10.0), Some(BoundaryPoint { node: a, offset: 17 }));
    let mid = page.position_at(&fonts, 70.0, 10.0).unwrap();
    assert!(mid.node == a && mid.offset > 3 && mid.offset < 12, "{mid:?}");
    let (s, e) = page.word_range_at(&fonts, 70.0, 10.0).unwrap();
    assert_eq!((s.offset, e.offset), (6, 11), "double click selects 'brave'");
    let p = page.doc.element_by_id("b").unwrap();
    let (s, e) = page.block_range_at(&fonts, 120.0, 45.0).unwrap();
    assert_eq!((s, e), (BoundaryPoint { node: p, offset: 0 }, BoundaryPoint { node: p, offset: 3 }));
}

#[test]
fn copied_text_follows_the_rendering() {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.scripting = false;
    let mut page = load(&mut engine, "<!doctype html><style>body{margin:0;font:20px sans-serif}</style><p id=a>  Hello\n   brave   world  </p><p id=b>Second <b>bold</b> line</p><p style='user-select:none' id=c>hidden</p><p id=d>tail</p>");
    let a = text_node(&page, "a");
    let d = text_node(&page, "d");
    page.doc.set_selection(BoundaryPoint { node: a, offset: 4 }, BoundaryPoint { node: d, offset: 2 });
    let text = page.selection_text(&page.fonts.clone());
    assert_eq!(text, "llo brave world\nSecond bold line\nta");
    page.select_all();
    assert_eq!(page.selection_text(&page.fonts.clone()), "Hello brave world\nSecond bold line\ntail");
}

#[test]
fn user_select_controls_where_selections_start() {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.scripting = false;
    let page = load(&mut engine, "<!doctype html><style>body{margin:0;font:20px sans-serif}div{height:30px}</style><div id=n style='user-select:none'>none</div><div id=all style='user-select:all'>all <i>of</i> it</div><div id=c style='user-select:contain'>contained</div>");
    let fonts = page.fonts.clone();
    let n = page.doc.element_by_id("n").unwrap();
    assert_eq!(page.selection_start_at(&fonts, 10.0, 10.0, Some(n)), SelectStart::None);
    let all = page.doc.element_by_id("all").unwrap();
    let i = page.doc.children(all).nth(1).unwrap();
    assert_eq!(page.selection_start_at(&fonts, 10.0, 40.0, Some(i)), SelectStart::All(BoundaryPoint { node: all, offset: 0 }, BoundaryPoint { node: all, offset: 3 }));
    let c = page.doc.element_by_id("c").unwrap();
    let ct = page.doc.first_child(c).unwrap();
    assert!(matches!(page.selection_start_at(&fonts, 10.0, 70.0, Some(c)), SelectStart::At(p) if p.node == ct));
    assert_eq!(page.select_contain_root(ct), Some(c));
}

const HIGHLIGHT: (u8, u8, u8) = (181, 213, 255);

#[test]
fn selection_is_painted_with_default_colors() {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.scripting = false;
    let mut page = load(&mut engine, PAGE);
    let line = (0, 0, 400, 25);
    assert_eq!(count(&engine, &page, HIGHLIGHT, line), 0);
    let a = text_node(&page, "a");
    page.doc.set_selection(BoundaryPoint { node: a, offset: 6 }, BoundaryPoint { node: a, offset: 11 });
    assert!(engine.sync_selection(&mut page));
    assert!(!engine.sync_selection(&mut page), "no rebuild without a change");
    let lit = count(&engine, &page, HIGHLIGHT, line);
    assert!(lit > 200, "highlight painted: {lit}");
    // Only the selected word: nothing on the left part of the line.
    let x_brave = page.selection_rects(&page.fonts.clone())[0].x as u32;
    assert_eq!(count(&engine, &page, HIGHLIGHT, (0, 0, x_brave.saturating_sub(1), 25)), 0);
    // Not the second paragraph.
    assert_eq!(count(&engine, &page, HIGHLIGHT, (0, 30, 400, 25)), 0);
    page.doc.clear_selection();
    assert!(engine.sync_selection(&mut page));
    assert_eq!(count(&engine, &page, HIGHLIGHT, line), 0);
}

#[test]
fn selection_pseudo_element_colors_and_inheritance() {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.scripting = false;
    let mut page = load(&mut engine, "<!doctype html><style>body{margin:0;font:20px sans-serif}p{margin:0 0 10px}\
        #a::selection{background:rgb(0,128,0);color:rgb(255,255,0)} div::selection{background-color:rgb(0,0,255)}\
        #n{user-select:none}</style>\
        <p id=a>Styled text</p><div id=d><span id=s>Inherited text</span></div><p id=n>Never selected</p><p id=z>Default</p>");
    page.select_all();
    engine.sync_selection(&mut page);
    // #a: green background, yellow text, no default highlight.
    assert!(count(&engine, &page, (0, 128, 0), (0, 0, 400, 25)) > 200);
    assert!(count(&engine, &page, (255, 255, 0), (0, 0, 400, 25)) > 10);
    assert_eq!(count(&engine, &page, HIGHLIGHT, (0, 0, 400, 25)), 0);
    // The span's highlight inherits div::selection's background (highlight
    // inheritance), which also suppresses the default colors.
    let d_top = page.rect_of(page.doc.element_by_id("d").unwrap()).unwrap().y as u32;
    assert!(count(&engine, &page, (0, 0, 255), (0, d_top, 400, 22)) > 100);
    assert_eq!(count(&engine, &page, HIGHLIGHT, (0, d_top, 400, 22)), 0);
    // user-select: none text is not highlighted.
    let n_top = page.rect_of(page.doc.element_by_id("n").unwrap()).unwrap().y as u32;
    assert_eq!(count(&engine, &page, HIGHLIGHT, (0, n_top, 400, 22)), 0);
    // Elements without ::selection rules use the defaults.
    let z_top = page.rect_of(page.doc.element_by_id("z").unwrap()).unwrap().y as u32;
    assert!(count(&engine, &page, HIGHLIGHT, (0, z_top, 400, 22)) > 100);
}

#[test]
fn custom_highlights_api_and_painting() {
    let mut engine = Engine::new_bundled_fonts_only();
    let mut page = load(&mut engine, "<!doctype html><style>body{margin:0;font:20px sans-serif}p{margin:0 0 10px}\
        ::highlight(hot){background-color:rgb(255,0,0);color:rgb(0,0,255)} ::highlight(cold){background-color:rgb(0,255,0)}</style>\
        <p id=a>Hello brave world</p><p id=b>Second line</p>");
    let red = (255, 0, 0);
    let line = (0, 0, 400, 25);
    check(&engine, &mut page, r#"
        const t = document.getElementById('a').firstChild;
        const r = new Range(); r.setStart(t, 6); r.setEnd(t, 11);
        const h = new Highlight(r);
        eq(h.size, 1, 'size'); eq(h.has(r), true, 'has'); eq(h.priority, 0, 'priority'); eq(h.type, 'highlight', 'type');
        h.type = 'spelling-error'; eq(h.type, 'spelling-error', 'type set'); h.type = 'bogus'; eq(h.type, 'spelling-error', 'invalid type ignored'); h.type = 'highlight';
        let threw = ''; try { h.add({}); } catch (e) { threw = e.name; } eq(threw, 'TypeError', 'add non-range');
        threw = ''; try { CSS.highlights.set('x', {}); } catch (e) { threw = e.name; } eq(threw, 'TypeError', 'set non-highlight');
        threw = ''; try { new HighlightRegistry(); } catch (e) { threw = e.name; } eq(threw, 'TypeError', 'registry constructor');
        eq(CSS.highlights.set('hot', h), CSS.highlights, 'set returns the registry');
        eq(CSS.highlights.size, 1, 'registry size'); eq(CSS.highlights.get('hot'), h, 'get');
        eq([...CSS.highlights].map(([k]) => k).join(), 'hot', 'iteration'); eq([...h].length, 1, 'highlight iteration');
        const seen = []; h.forEach((v, k, s) => seen.push(v === r && k === r && s === h)); eq(seen.join(), 'true', 'forEach');
        Set.prototype.add = () => { throw new Error('tampered'); };
        h.add(r); eq(h.size, 1, 'tampered Set prototype is not used');
        window.r = r; window.h = h; window.t = t;
    "#);
    let lit = count(&engine, &page, red, line);
    assert!(lit > 200, "custom highlight painted: {lit}");
    // Live ranges: moving the range repaints.
    check(&engine, &mut page, "r.setEnd(t, 17);");
    assert!(count(&engine, &page, red, line) > lit + 100);
    // Priority and order: the later 'cold' highlight paints over 'hot'
    // unless 'hot' has a higher priority.
    check(&engine, &mut page, "CSS.highlights.set('cold', new Highlight(r));");
    assert_eq!(count(&engine, &page, red, line), 0);
    check(&engine, &mut page, "h.priority = 1;");
    assert!(count(&engine, &page, red, line) > 200);
    check(&engine, &mut page, "CSS.highlights.clear();");
    assert_eq!(count(&engine, &page, red, line), 0);
    assert_eq!(count(&engine, &page, (0, 255, 0), line), 0);
    // Static ranges.
    check(&engine, &mut page, "CSS.highlights.set('hot', new Highlight(new StaticRange({ startContainer: t, startOffset: 0, endContainer: t, endOffset: 5 })));");
    assert!(count(&engine, &page, red, line) > 200);
    check(&engine, &mut page, "const hits = CSS.highlights.highlightsFromPoint(10, 10); eq(hits.length, 1, 'highlightsFromPoint'); eq(hits[0].ranges.length, 1, 'hit ranges'); eq(CSS.highlights.highlightsFromPoint(300, 10).length, 0, 'no hit');");
}

#[test]
fn caret_position_from_point_finds_text_offsets() {
    let mut engine = Engine::new_bundled_fonts_only();
    let mut page = load(&mut engine, PAGE);
    check(&engine, &mut page, r#"
        const p = document.caretPositionFromPoint(1, 10);
        eq(p.offsetNode, document.getElementById('a').firstChild, 'node'); eq(p.offset, 0, 'start');
        const e = document.caretPositionFromPoint(399, 10); eq(e.offset, 17, 'end of line');
        const r = document.caretRangeFromPoint(70, 10); eq(r.collapsed, true, 'collapsed'); eq(r.startOffset > 3 && r.startOffset < 12, true, 'inside');
    "#);
}

#[test]
fn frames_have_their_own_selection() {
    let mut engine = Engine::new_bundled_fonts_only();
    let mut page = load(&mut engine, "<!doctype html><p id=a>Top text</p><iframe id=f srcdoc='<p id=i>Inner text</p>'></iframe>");
    engine.settle_for_snapshot(&mut page, std::time::Duration::from_millis(500));
    check(&engine, &mut page, r#"
        const f = document.getElementById('f');
        const w = f.contentWindow, d = f.contentDocument;
        const s = w.getSelection();
        eq(d.getSelection(), s, 'document and window agree');
        eq(s === getSelection(), false, 'not the top selection');
        s.selectAllChildren(d.getElementById('i'));
        eq(s.toString(), 'Inner text', 'frame selection text');
        eq(getSelection().rangeCount, 0, 'top selection untouched');
        eq(document.implementation.createHTMLDocument('x').getSelection(), null, 'no browsing context');
    "#);
    // The frame paints its selection.
    let f = page.rect_of(page.doc.element_by_id("f").unwrap()).unwrap();
    assert!(count(&engine, &page, HIGHLIGHT, (f.x as u32, f.y as u32, f.w as u32, f.h as u32)) > 50);
}
