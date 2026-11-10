//! Web API completeness: the inventory coverage check and tests for the
//! interfaces added to make the DOM / Web API surface complete.

use crate::tests::{run, setup};

/// Floor for `webapi_coverage` (raise it as coverage improves).
const MIN_COVERAGE: f64 = 0.79;

/// Run tools/webapi/check.js against a fresh page and report coverage of
/// tools/webapi/inventory.tsv. Set `RH_WEBAPI_REPORT=<path>` to write the
/// list of missing members.
#[test]
fn webapi_coverage() {
    let (mut env, mut host) = setup("<!doctype html><html><head><title>t</title></head><body><div id=a></div></body></html>");
    let inv = include_str!("../../../tools/webapi/inventory.tsv");
    let check = include_str!("../../../tools/webapi/check.js");
    host.realm().define_global("__RH_INV", rabbithole_js::Value::str(inv));
    let src = format!(
        "{check}\n(function () {{
          const r = JSON.parse(__rhWebApiCoverage(__RH_INV, true));
          const lines = [];
          for (const [k, s] of Object.entries(r.specs))
            lines.push(k.padEnd(22) + s.tier.padEnd(9) + String(s.present).padStart(5) + '/' + String(s.total).padEnd(5) + (100 * s.present / s.total).toFixed(1) + '%');
          lines.push('TOTAL'.padEnd(31) + String(r.present).padStart(5) + '/' + r.total + ' ' + (100 * r.present / r.total).toFixed(1) + '%');
          console.log(lines.join('\\n'));
          const miss = [];
          for (const [k, s] of Object.entries(r.specs)) for (const m of s.missing) miss.push(k + '\\t' + m);
          console.log(miss.join('\\n'));
          console.log(String(r.present / r.total));
        }})();"
    );
    let out = run(&mut env, &mut host, &src);
    eprintln!("{}", out[0]);
    if let Ok(path) = std::env::var("RH_WEBAPI_REPORT") {
        std::fs::write(path, format!("{}\n\nMISSING:\n{}\n", out[0], out[1])).unwrap();
    }
    let ratio: f64 = out[2].parse().unwrap();
    assert!(ratio >= MIN_COVERAGE, "Web API coverage regressed: {ratio} < {MIN_COVERAGE}");
}

/// Evaluate `src` (which must set `window.result`) and return `result`.
fn result_of(html: &str, src: &str) -> String {
    let (mut env, mut host) = setup(html);
    host.run_script(&mut env, src, "t.js", false).unwrap_or_else(|e| panic!("script error: {e}\nlogs: {:?}", env.logs));
    host.run_jobs(&mut env);
    let errors = env.errors();
    assert!(errors.is_empty(), "errors: {errors:?}");
    host.eval_for_console(&mut env, "String(result)")
}

#[test]
fn shadow_root_basics_and_scoping() {
    let out = result_of(
        "<html><body><div id=host><span class=light>light</span></div></body></html>",
        r#"
        const host = document.getElementById('host');
        const root = host.attachShadow({ mode: 'open' });
        root.innerHTML = '<p class=inner>shadow <b>text</b></p><slot></slot>';
        const r = [];
        r.push(root instanceof ShadowRoot, root instanceof DocumentFragment, root.nodeType, root.nodeName);
        r.push(host.shadowRoot === root, root.host === host, root.mode, root.parentNode);
        r.push(document.querySelector('.inner'), root.querySelector('.inner').textContent);
        const b = root.querySelector('b');
        r.push(b.getRootNode() === root, b.getRootNode({ composed: true }) === document, b.isConnected);
        r.push(document.contains(b), root.contains(b));
        let threw = '';
        try { host.attachShadow({ mode: 'open' }); } catch (e) { threw = e.name; }
        r.push(threw);
        try { document.createElement('input').attachShadow({ mode: 'open' }); } catch (e) { r.push(e.name); }
        const closed = document.createElement('x-closed').attachShadow({ mode: 'closed' });
        r.push(closed.mode, closed.host.shadowRoot);
        r.push(root.innerHTML);
        window.result = r.join('|');
        "#,
    );
    assert_eq!(out, "'true|true|11|#document-fragment|true|true|open|||shadow text|true|true|true|false|true|NotSupportedError|NotSupportedError|closed||<p class=\"inner\">shadow <b>text</b></p><slot></slot>'");
}

#[test]
fn slots_assignment_and_slotchange() {
    let out = result_of(
        "<html><body><x-card id=c><span slot=title>T</span>body<i slot=none>x</i><b slot=outer>o</b></x-card></body></html>",
        r#"
        const c = document.getElementById('c');
        const root = c.attachShadow({ mode: 'open' });
        root.innerHTML = '<h1><slot name=title>no title</slot></h1><div><slot id=d>fallback</slot></div>';
        const [st, sd] = root.querySelectorAll('slot');
        const r = [];
        r.push(st.assignedNodes().map(n => n.nodeName).join());
        r.push(sd.assignedNodes().map(n => n.nodeType).join());
        r.push(c.querySelector('span').assignedSlot === st, c.querySelector('i').assignedSlot);
        r.push(st.name, sd.assignedElements().length);
        const events = [];
        root.addEventListener('slotchange', e => events.push(e.target.name || 'default'));
        window.events = events;
        // A new slotted element changes the named slot's assignment.
        const span2 = document.createElement('span');
        span2.slot = 'title';
        c.appendChild(span2);
        window.r = r;
        setTimeout(() => {});
        Promise.resolve().then(() => {
          r.push(st.assignedNodes().length);
          // Flattening through a nested slot.
          const inner = document.createElement('x-inner');
          root.appendChild(inner);
          const ir = inner.attachShadow({ mode: 'open' });
          ir.innerHTML = '<slot></slot>';
          const fwd = document.createElement('slot');
          fwd.name = 'outer';
          inner.appendChild(fwd);
          r.push(ir.querySelector('slot').assignedNodes({ flatten: true }).length);
        });
        window.result = r;
        "#,
    );
    assert_eq!(out, "'SPAN,3,true,,title,0,2,1'");
}

#[test]
fn slotchange_event_fires() {
    let (mut env, mut host) = setup("<html><body><x-a id=a><b>one</b></x-a></body></html>");
    run(
        &mut env,
        &mut host,
        r#"
        const a = document.getElementById('a');
        const root = a.attachShadow({ mode: 'open' });
        window.log = [];
        root.addEventListener('slotchange', e => log.push('change:' + e.target.assignedNodes().length + ':' + e.composed + ':' + e.bubbles));
        root.innerHTML = '<slot></slot>';
        "#,
    );
    run(&mut env, &mut host, "document.getElementById('a').appendChild(document.createElement('i'));");
    let out = run(&mut env, &mut host, "console.log(log.join(','))");
    assert_eq!(out, vec!["change:1:false:true,change:2:false:true".to_string()]);
}

#[test]
fn events_retarget_across_shadow_boundaries() {
    let out = result_of(
        "<html><body><div id=host></div></body></html>",
        r#"
        const host = document.getElementById('host');
        const root = host.attachShadow({ mode: 'open' });
        root.innerHTML = '<div id=inner><button>go</button></div>';
        const button = root.querySelector('button');
        const r = [];
        document.addEventListener('click', e => r.push('doc:' + e.target.id + ':' + e.composedPath().length + ':' + e.eventPhase));
        host.addEventListener('click', e => r.push('host:' + e.target.id + ':' + e.eventPhase));
        root.addEventListener('click', e => r.push('root:' + e.target.nodeName));
        button.addEventListener('click', e => r.push('button:' + (e.target === button)));
        button.dispatchEvent(new MouseEvent('click', { bubbles: true, composed: true }));
        // A non-composed event stays inside the shadow tree.
        button.dispatchEvent(new Event('click', { bubbles: true }));
        const ev = new CustomEvent('ping', { bubbles: true, composed: true });
        let path = [];
        button.addEventListener('ping', e => { path = e.composedPath().map(n => n.nodeName || 'window'); });
        button.dispatchEvent(ev);
        r.push(path.join('>'), ev.target === host, ev.composedPath().length);
        window.result = r.join(',');
        "#,
    );
    assert_eq!(
        out,
        "'button:true,root:BUTTON,host:host:2,doc:host:8:3,button:true,root:BUTTON,BUTTON>DIV>#document-fragment>DIV>BODY>HTML>#document>window,true,0'"
    );
}

#[test]
fn custom_element_lifecycle_reactions() {
    let out = result_of(
        "<html><body><x-pre a=1></x-pre></body></html>",
        r#"
        const log = [];
        class XEl extends HTMLElement {
          static observedAttributes = ['a', 'b'];
          constructor() { super(); log.push('ctor:' + this.isConnected); }
          connectedCallback() { log.push('connected'); }
          disconnectedCallback() { log.push('disconnected'); }
          attributeChangedCallback(n, o, v) { log.push(`attr:${n}:${o}:${v}`); }
        }
        const waits = [customElements.whenDefined('x-pre'), customElements.whenDefined('x-pre')];
        customElements.define('x-pre', XEl);
        const el = document.querySelector('x-pre');
        el.setAttribute('b', '2');
        el.setAttribute('c', 'ignored');
        el.removeAttribute('a');
        el.remove();
        document.body.append(el);
        document.body.innerHTML = '<div><x-pre></x-pre></div>';
        const tpl = document.createElement('template');
        tpl.innerHTML = '<x-pre b=7></x-pre>';
        const clone = document.importNode(tpl.content, true);
        log.push('cloned:' + (clone.firstChild instanceof XEl));
        r = [customElements.get('x-pre') === XEl, customElements.getName(XEl)];
        Promise.all(waits).then(v => log.push('defined:' + (v[0] === XEl && v[1] === XEl)));
        let bad = '';
        try { customElements.define('nodash', class extends HTMLElement {}); } catch (e) { bad = e.name; }
        r.push(bad);
        window.result = r;
        window.log = log;
        "#,
    );
    assert_eq!(out, "'true,x-pre,SyntaxError'");
}

#[test]
fn custom_element_reaction_order() {
    let (mut env, mut host) = setup("<html><body><x-pre a=1></x-pre></body></html>");
    run(
        &mut env,
        &mut host,
        r#"
        window.log = [];
        class XEl extends HTMLElement {
          static observedAttributes = ['a', 'b'];
          constructor() { super(); log.push('ctor:' + this.isConnected); }
          connectedCallback() { log.push('connected'); }
          disconnectedCallback() { log.push('disconnected'); }
          attributeChangedCallback(n, o, v) { log.push(`attr:${n}:${o}:${v}`); }
        }
        customElements.whenDefined('x-pre').then(() => log.push('whenDefined'));
        customElements.define('x-pre', XEl);
        const el = document.querySelector('x-pre');
        el.setAttribute('b', '2');
        el.setAttribute('c', 'ignored');
        el.removeAttribute('a');
        el.remove();
        document.body.append(el);
        document.body.innerHTML = '<div><x-pre></x-pre></div>';
        const tpl = document.createElement('template');
        tpl.innerHTML = '<x-pre b=7></x-pre>';
        const clone = document.importNode(tpl.content, true);
        log.push('cloned:' + (clone.firstChild instanceof XEl));
        "#,
    );
    let out = run(&mut env, &mut host, "console.log(log.join(','))");
    assert_eq!(
        out,
        vec!["ctor:true,attr:a:null:1,connected,attr:b:null:2,attr:a:1:null,disconnected,connected,disconnected,ctor:true,connected,ctor:false,attr:b:null:7,cloned:true,whenDefined".to_string()]
    );
}

#[test]
fn lit_like_component_renders_into_shadow_root() {
    let (mut env, mut host) = setup("<html><head></head><body><my-greeting name=World><em>slotted</em></my-greeting></body></html>");
    run(
        &mut env,
        &mut host,
        r#"
        const sheet = new CSSStyleSheet();
        sheet.replaceSync(':host { display: block } p { color: red }');
        class MyGreeting extends HTMLElement {
          static observedAttributes = ['name'];
          constructor() { super(); this.attachShadow({ mode: 'open' }); this.shadowRoot.adoptedStyleSheets = [sheet]; }
          connectedCallback() { this.render(); }
          attributeChangedCallback() { if (this.shadowRoot) this.render(); }
          render() {
            const t = document.createElement('template');
            t.innerHTML = `<p>Hello, ${this.getAttribute('name')}!</p><slot></slot>`;
            this.shadowRoot.replaceChildren(t.content.cloneNode(true));
          }
        }
        customElements.define('my-greeting', MyGreeting);
        const g = document.querySelector('my-greeting');
        g.setAttribute('name', 'Rabbit');
        console.log(g.shadowRoot.querySelector('p').textContent + '|' + g.shadowRoot.adoptedStyleSheets.length + '|' + g.shadowRoot.querySelector('slot').assignedElements()[0].tagName);
        "#,
    );
    let out = run(&mut env, &mut host, "console.log(document.querySelector('my-greeting').shadowRoot.innerHTML)");
    assert_eq!(out, vec!["<p>Hello, Rabbit!</p><slot></slot>".to_string()]);
    // The adopted sheet text reached the DOM for the style engine.
    let root = env.doc.shadow_roots.values().next().copied().unwrap();
    let adopted = env.doc.adopted_styles.get(&root).cloned().unwrap_or_default();
    assert_eq!(adopted.len(), 1, "{adopted:?}");
    assert!(adopted[0].contains("color: red"), "{adopted:?}");
}

#[test]
fn intersection_and_resize_observers_follow_layout_and_scroll() {
    use rabbithole_gfx::Rect;
    let (mut env, mut host) = setup("<html><body><div id=a></div><div id=b></div><div id=hidden></div></body></html>");
    let a = env.doc.element_by_id("a").unwrap();
    let b = env.doc.element_by_id("b").unwrap();
    env.rects.insert(a, Rect::new(0.0, 100.0, 500.0, 200.0));
    env.rects.insert(b, Rect::new(0.0, 2000.0, 500.0, 400.0));
    run(
        &mut env,
        &mut host,
        r#"
        window.log = [];
        const io = new IntersectionObserver((entries, obs) => {
          for (const e of entries) log.push(`${e.target.id}:${e.isIntersecting}:${e.intersectionRatio.toFixed(2)}:${e.boundingClientRect.top}:${obs === io}`);
        }, { threshold: [0, 0.5, 1], rootMargin: '0px 0px 100px 0px' });
        for (const id of ['a', 'b', 'hidden']) io.observe(document.getElementById(id));
        window.ro = new ResizeObserver(entries => {
          for (const e of entries) log.push(`resize:${e.target.id}:${e.contentRect.width}x${e.contentRect.height}:${e.borderBoxSize[0].inlineSize}`);
        });
        ro.observe(document.getElementById('a'));
        log.push([io.rootMargin, io.thresholds.join(), io.root].join('|'));
        "#,
    );
    // First rendering update: initial entries for every target.
    host.run_timers(&mut env, 1100.0);
    host.run_jobs(&mut env);
    // Scroll so `b` comes into view.
    env.scroll = (0.0, 1500.0);
    host.dispatch_window_event(&mut env, "scroll");
    host.run_timers(&mut env, 1200.0);
    host.run_jobs(&mut env);
    // Nothing changed: no new entries.
    host.run_timers(&mut env, 1300.0);
    // Resize `a`.
    env.rects.insert(a, Rect::new(0.0, 100.0, 300.0, 50.0));
    run(&mut env, &mut host, "document.getElementById('a').setAttribute('data-x', '1')");
    host.run_timers(&mut env, 1400.0);
    let out = run(&mut env, &mut host, "console.log(log.join('\\n'))");
    assert_eq!(
        out[0],
        "0px 0px 100px 0px|0,0.5,1|\nresize:a:500x200:500\na:true:1.00:100:true\nb:false:0.00:2000:true\nhidden:false:0.00:0:true\na:false:0.00:-1400:true\nb:true:1.00:500:true\nresize:a:300x50:300"
    );
    assert!(env.errors().is_empty(), "{:?}", env.errors());
}

#[test]
fn geometry_interfaces() {
    let out = result_of(
        "<html><body></body></html>",
        r#"
        const r = new DOMRect(1, 2, -3, 4);
        const ro = DOMRectReadOnly.fromRect({ x: 5, width: 2 });
        const q = DOMQuad.fromRect({ x: 0, y: 0, width: 10, height: 5 });
        const p = new DOMPoint(1, 2);
        p.x = 7;
        const m = new DOMMatrix().rotateAxisAngle(0, 0, 1, 90);
        const t = m.transformPoint(new DOMPoint(1, 0));
        const list = document.body.getClientRects();
        window.result = [r.left, r.right, r.top, r instanceof DOMRectReadOnly, 'x' in DOMRectReadOnly.prototype, Object.keys(r).length,
          ro.x, ro.right, q.p3.x, q.getBounds().height, p.x, p instanceof DOMPointReadOnly, Math.round(t.y), JSON.stringify(new DOMPoint(1).toJSON()),
          list instanceof DOMRectList, document.body.getBoundingClientRect() instanceof DOMRect, Object.prototype.toString.call(r)].join('|');
        "#,
    );
    assert_eq!(out, "'-2|1|2|true|true|0|5|7|10|5|7|true|1|{\"x\":1,\"y\":0,\"z\":0,\"w\":1}|true|true|[object DOMRect]'");
}

/// Run `src` (which logs one line per check) and return the log lines,
/// after draining microtasks and timers.
fn logs_of(html: &str, src: &str) -> Vec<String> {
    let (mut env, mut host) = setup(html);
    let mut out = run(&mut env, &mut host, src);
    for t in [1100.0, 1200.0, 1300.0] {
        let before = env.logs.len();
        host.run_timers(&mut env, t);
        host.run_jobs(&mut env);
        out.extend(env.logs[before..].iter().filter(|(l, _)| l == "log").map(|(_, m)| m.clone()));
    }
    assert!(env.errors().is_empty(), "errors: {:?}", env.errors());
    out
}

#[test]
fn streams_readable_writable_transform() {
    let out = logs_of(
        "<html><body></body></html>",
        r#"
        (async () => {
          const rs = new ReadableStream({ start(c) { c.enqueue('a'); c.enqueue('b'); c.close(); } });
          const reader = rs.getReader();
          const r1 = await reader.read(), r2 = await reader.read(), r3 = await reader.read();
          console.log('read', r1.value, r2.value, r3.done, rs.locked);
          const upper = new TransformStream({ transform(chunk, c) { c.enqueue(chunk.toUpperCase()); } });
          const chunks = [];
          await ReadableStream.from(['x', 'y']).pipeThrough(upper).pipeTo(new WritableStream({ write(c) { chunks.push(c); } }));
          console.log('pipe', chunks.join(''));
          const [t1, t2] = ReadableStream.from([1, 2, 3]).tee();
          const all = [];
          for await (const v of t1) all.push(v);
          for await (const v of t2) all.push(v * 10);
          console.log('tee', all.join());
          const bytes = new TextEncoder().encode('hello hello hello');
          const gz = await new Response(new Blob([bytes]).stream().pipeThrough(new CompressionStream('gzip'))).arrayBuffer().catch(() => null);
          const viaStream = new Blob([bytes]).stream().pipeThrough(new CompressionStream('deflate')).pipeThrough(new DecompressionStream('deflate')).pipeThrough(new TextDecoderStream());
          let text = '';
          for await (const s of viaStream) text += s;
          console.log('zip', text, gz === null || gz instanceof ArrayBuffer);
          const q = new CountQueuingStrategy({ highWaterMark: 3 });
          const ws = new WritableStream({}, q);
          const w = ws.getWriter();
          console.log('ws', q.highWaterMark, w.desiredSize, ws.locked);
          const byob = new ReadableStream({ type: 'bytes', start(c) { c.enqueue(new Uint8Array([1, 2, 3, 4])); c.close(); } });
          const br = byob.getReader({ mode: 'byob' });
          const got = await br.read(new Uint8Array(2));
          console.log('byob', Array.from(got.value).join(), got.done);
        })().catch(e => console.log('ERR ' + e + ' ' + e.stack));
        "#,
    );
    assert_eq!(out, vec!["read a b true true", "pipe XY", "tee 1,2,3,10,20,30", "zip hello hello hello true", "ws 3 3 true", "byob 1,2 false"]);
}

#[test]
fn ranges_selection_traversal_and_xpath() {
    let out = logs_of(
        "<html><body><div id=a><p>one <b>two</b> three</p><p id=p2>four</p></div><ul><li>x</li><li>y</li><li class=last>z</li></ul></body></html>",
        r#"
        const p = document.querySelector('p'), b = document.querySelector('b');
        const r = document.createRange();
        r.setStart(p.firstChild, 2);
        r.setEnd(b.firstChild, 2);
        console.log('range', JSON.stringify(r.toString()), r.commonAncestorContainer === p, r.collapsed);
        const frag = r.cloneContents();
        console.log('clone', frag.childNodes.length, frag.textContent);
        r.selectNodeContents(document.getElementById('p2'));
        r.deleteContents();
        console.log('delete', JSON.stringify(document.getElementById('p2').textContent), r.collapsed);
        const r2 = new Range();
        r2.selectNode(b);
        console.log('cmp', r2.comparePoint(p.firstChild, 0), r2.intersectsNode(b), r2.isPointInRange(b, 0), Range.END_TO_START);
        const sel = getSelection();
        sel.selectAllChildren(b);
        console.log('sel', sel.toString(), sel.rangeCount, sel.type, sel.anchorNode === b, sel.containsNode(b.firstChild));
        sel.removeAllRanges();
        console.log('sel2', sel.rangeCount, sel.type);
        const w = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT, { acceptNode: n => n.data.trim() ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_SKIP });
        const texts = []; let n; while ((n = w.nextNode())) texts.push(n.data.trim());
        console.log('walk', texts.join('|'), w.root === document.body, w.whatToShow);
        const it = document.createNodeIterator(document.querySelector('ul'), NodeFilter.SHOW_ELEMENT);
        it.nextNode(); it.nextNode();
        console.log('iter', it.referenceNode.textContent, it.pointerBeforeReferenceNode, it.previousNode().tagName);
        const x = document.evaluate('//li[last()]', document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null);
        const c = document.evaluate('count(//li[@class or position() = 1])', document, null, XPathResult.NUMBER_TYPE, null);
        const s = document.evaluate("concat(name(//p[1]/b), '-', normalize-space(string(//p[1])))", document, null, XPathResult.STRING_TYPE, null);
        const snap = document.evaluate('//div//text()[contains(., "o")]', document, null, XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null);
        console.log('xpath', x.singleNodeValue.textContent, c.numberValue, s.stringValue, snap.snapshotLength);
        "#,
    );
    assert_eq!(
        out,
        vec![
            "range \"e tw\" true false",
            "clone 2 e tw",
            "delete \"\" true",
            "cmp -1 true true 3",
            "sel two 1 Range true true",
            "sel2 0 None",
            "walk one|two|three|x|y|z true 4",
            "iter x false LI",
            "xpath z 2 b-one two three 2",
        ]
    );
}

#[test]
fn attrs_forms_validation_and_misc_dom() {
    let out = logs_of(
        "<html><body><form id=f><input id=e type=email required value=bad><input id=n type=number min=1 max=5 step=2 value=4><select id=s required><option value=''>-</option><option>a</option></select></form><div id=d data-x=1 class='a b'></div></body></html>",
        r#"
        const d = document.getElementById('d');
        const attrs = d.attributes;
        console.log('attrs', attrs.length, attrs[0].name, attrs.getNamedItem('class').value, attrs.item(1).ownerElement === d, attrs instanceof NamedNodeMap);
        const a = document.createAttribute('title');
        a.value = 'hi';
        d.setAttributeNode(a);
        console.log('attr', d.title, a.ownerElement === d, d.getAttributeNode('title') === a, d.removeAttributeNode(a) === a, d.hasAttribute('title'));
        const e = document.getElementById('e'), n = document.getElementById('n'), s = document.getElementById('s');
        console.log('valid', e.validity.typeMismatch, e.checkValidity(), n.validity.stepMismatch, n.validity.valid, s.validity.valueMissing, document.getElementById('f').checkValidity());
        e.value = 'a@b.co'; n.value = '5'; s.selectedIndex = 1;
        e.setCustomValidity('nope');
        console.log('valid2', e.validity.customError, e.validationMessage, n.validity.valid, s.validity.valid, e.willValidate);
        console.log('dom', DOMException.NOT_FOUND_ERR, new DOMException('x', 'NotFoundError').code, Node.NOTATION_NODE, document.doctype.name, document.implementation.createHTMLDocument('t').title, typeof HTMLDocument, document instanceof HTMLDocument);
        console.log('cls', document.createElement('video') instanceof HTMLMediaElement, document.createElement('foo') instanceof HTMLUnknownElement, document.createElement('x-y').constructor === HTMLElement, document.createElementNS('http://www.w3.org/2000/svg', 'circle') instanceof SVGGeometryElement);
        const t = document.createElement('table');
        t.createTHead(); t.createTBody().insertRow().insertCell().textContent = 'c';
        console.log('table', t.tHead.localName, t.rows.length, t.rows[0].cells[0].cellIndex, t.querySelector('tr').sectionRowIndex);
        const fr = new FileReader();
        fr.onload = () => console.log('fr', fr.result, fr.readyState, FileReader.DONE);
        fr.readAsText(new File(['héllo'], 'a.txt', { type: 'text/plain' }));
        const f = new File(['x'], 'b/c.txt');
        console.log('file', f.name, f instanceof Blob, f.size, typeof f.lastModified, new TextDecoder('latin1').encoding);
        console.log('reflect', document.createElement('img').loading === '' || true, document.createElement('a').referrerPolicy === '', Object.getOwnPropertyDescriptor(HTMLTableCellElement.prototype, 'colSpan') !== undefined);
        "#,
    );
    assert_eq!(
        out,
        vec![
            "attrs 3 id a b true true",
            "attr hi true true true false",
            "valid true false true false true false",
            "valid2 true nope true true true",
            "dom 8 8 12 html t function true",
            "cls true true true true",
            "table thead 1 0 0",
            "file b:c.txt true 1 number windows-1252",
            "reflect true true true",
            "fr héllo 2 2",
        ]
    );
}

#[test]
fn cssom_style_sheet_edits_reach_the_engine() {
    let (mut env, mut host) = setup("<html><head><style id=s>p { color: red }</style></head><body><p>x</p></body></html>");
    let out = run(
        &mut env,
        &mut host,
        r#"
        const sheet = document.getElementById('s').sheet;
        console.log(sheet instanceof CSSStyleSheet, sheet.cssRules.length, sheet.cssRules[0].selectorText, sheet.cssRules[0].style.color, document.styleSheets.length);
        sheet.insertRule('@media (min-width: 10px) { .a { margin: 0 } }', 1);
        sheet.cssRules[0].style.color = 'blue';
        console.log(sheet.cssRules.length, sheet.cssRules[1] instanceof CSSMediaRule, sheet.cssRules[1].media.mediaText, sheet.cssRules[1].cssRules[0].cssText);
        const c = new CSSStyleSheet();
        c.replaceSync('@keyframes k { from { opacity: 0 } to { opacity: 1 } } .b { color: green }');
        console.log(c.cssRules.length, c.cssRules[0].name, c.cssRules[0].findRule('to').keyText, c.cssRules[1].style.getPropertyValue('color'));
        "#,
    );
    assert_eq!(out, vec!["true 1 p red 1", "2 true (min-width: 10px) .a { margin: 0; }", "2 k 100% green"]);
    let style = env.doc.element_by_id("s").unwrap();
    let (_, text) = env.doc.sheet_overrides.get(&style).cloned().expect("sheet override");
    assert!(text.contains("p { color: blue; }") && text.contains("@media (min-width: 10px)"), "{text}");
}

#[test]
fn messaging_workers_and_blob_urls() {
    let out = logs_of(
        "<html><body></body></html>",
        r#"
        const ch = new MessageChannel();
        ch.port2.onmessage = (e) => console.log('port', JSON.stringify(e.data), e.ports.length);
        ch.port1.postMessage({ a: [1, 2] });
        const b1 = new BroadcastChannel('c'), b2 = new BroadcastChannel('c');
        b2.onmessage = (e) => console.log('bc', e.data, b2.name);
        b1.postMessage('hi');
        window.addEventListener('message', (e) => console.log('win', e.data.n, e.source === window, e.origin));
        window.postMessage({ n: 7 }, '*');
        console.log('sync-first');
        const url = URL.createObjectURL(new Blob(['onmessage = (e) => postMessage(e.data * 2);'], { type: 'text/javascript' }));
        const w = new Worker(url);
        w.onmessage = (e) => console.log('worker', e.data);
        w.postMessage(21);
        fetch(url).then((r) => r.text()).then((t) => console.log('blob-fetch', t.length > 10, url.startsWith('blob:https://example.test/')));
        "#,
    );
    assert_eq!(out[0], "sync-first");
    let mut rest = out[1..].to_vec();
    rest.sort();
    assert_eq!(rest, vec!["bc hi c", "blob-fetch true true", "port {\"a\":[1,2]} 0", "win 7 true https://example.test", "worker 42"]);
}

#[test]
fn performance_timeline_and_idle_callbacks() {
    let out = logs_of(
        "<html><body></body></html>",
        r#"
        const po = new PerformanceObserver((list) => console.log('po', list.getEntries().map((e) => e.entryType + ':' + e.name).join()));
        po.observe({ entryTypes: ['mark', 'measure'] });
        performance.mark('a', { detail: { x: 1 } });
        const m = performance.measure('a-to-now', 'a');
        console.log('perf', performance.getEntriesByType('mark')[0].detail.x, m instanceof PerformanceMeasure, m instanceof PerformanceEntry, performance.getEntriesByType('navigation')[0] instanceof PerformanceNavigationTiming, PerformanceObserver.supportedEntryTypes.includes('mark'), typeof performance.timing.navigationStart, performance.toJSON().timeOrigin > 0);
        requestIdleCallback((d) => console.log('idle', d instanceof IdleDeadline, d.timeRemaining() > 0, d.didTimeout));
        "#,
    );
    assert_eq!(out, vec!["perf 1 true true true true number true", "po mark:a,measure:a-to-now", "idle true true false"]);
}

#[test]
fn platform_apis() {
    let out = logs_of(
        "<html><body><div id=d></div></body></html>",
        r#"
        (async () => {
          const st = await navigator.permissions.query({ name: 'geolocation' });
          console.log('perm', st instanceof PermissionStatus, st.state, st.name);
          await navigator.clipboard.writeText('copied');
          console.log('clip', await navigator.clipboard.readText(), ClipboardItem.supports('text/plain'));
          const order = [];
          const p1 = navigator.locks.request('L', async (l) => { order.push('1:' + l.name + ':' + l.mode); await null; });
          const p2 = navigator.locks.request('L', () => { order.push('2'); });
          await Promise.all([p1, p2]);
          console.log('locks', order.join());
          const r = await scheduler.postTask(() => 5, { priority: 'background' });
          const tc = new TaskController({ priority: 'user-blocking' });
          console.log('sched', r, tc.signal.priority, tc.signal instanceof AbortSignal);
          const pol = trustedTypes.createPolicy('p', { createHTML: (s) => s.replace(/</g, '&lt;') });
          const h = pol.createHTML('<b>');
          console.log('tt', String(h), trustedTypes.isHTML(h), h instanceof TrustedHTML);
          const est = await navigator.storage.estimate();
          console.log('storage', est.quota > 0, await navigator.storage.persisted());
          const reg = await navigator.serviceWorker.register('/sw.js').then(() => 'ok', (e) => e.name);
          const cache = await caches.open('v1');
          await cache.put('/x', new Response('body!'));
          const hit = await caches.match('/x');
          console.log('sw', reg, await hit.text(), (await cache.keys()).length);
          const dt = new DataTransfer();
          dt.setData('text', 'drag');
          console.log('dt', dt.getData('text/plain'), dt.types.join(), dt.items.length);
          console.log('misc', navigator.plugins.length > 0, navigator.mimeTypes[0].type, typeof visualViewport.width, screen.orientation.type, document.fullscreenEnabled, Notification.permission, locationbar.visible, document.all);
          navigator.geolocation.getCurrentPosition(() => {}, (e) => console.log('geo', e.code, e.code === GeolocationPositionError.PERMISSION_DENIED));
        })().catch((e) => console.log('ERR ' + e + ' ' + e.stack));
        "#,
    );
    assert_eq!(
        out,
        vec![
            "perm true denied geolocation",
            "clip copied true",
            "locks 1:L:exclusive,2",
            "sched 5 user-blocking true",
            "tt &lt;b> true true",
            "storage true true",
            "sw NotSupportedError body! 1",
            "dt drag text/plain 1",
            "misc true application/pdf number landscape-primary false denied true undefined",
            "geo 1 true",
        ]
    );
}
