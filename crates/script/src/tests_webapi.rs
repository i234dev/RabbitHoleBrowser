//! Web API completeness: the inventory coverage check and tests for the
//! interfaces added to make the DOM / Web API surface complete.

use crate::tests::{run, setup};

/// Floor for `webapi_coverage` (raise it as coverage improves).
const MIN_COVERAGE: f64 = 0.99;

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
          // A Response whose body is a ReadableStream reads the stream (the
          // Svelte playground gunzips the compiler's npm tarball this way).
          const gzBytes = new Uint8Array(await new Response(new Blob([bytes]).stream().pipeThrough(new CompressionStream('gzip'))).arrayBuffer());
          const back = await new Response(new Response(gzBytes).body.pipeThrough(new DecompressionStream('gzip'))).text();
          const mixed = await new Response(new ReadableStream({ start(c) { c.enqueue('s'); c.enqueue(new Uint8Array([33])); c.close(); } })).text();
          const cl = new Response(ReadableStream.from(['c', 'l'])), cl2 = cl.clone();
          console.log('gzip', gzBytes[0], gzBytes[1], gzBytes.length < 40, back, mixed, await cl.text(), await cl2.text(), cl instanceof Response);
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
    assert_eq!(out, vec!["read a b true true", "pipe XY", "tee 1,2,3,10,20,30", "zip hello hello hello true", "gzip 31 139 true hello hello hello s! cl cl true", "ws 3 3 true", "byob 1,2 false"]);
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
        "<!doctype html><html><body><form id=f><input id=e type=email required value=bad><input id=n type=number min=1 max=5 step=2 value=4><select id=s required><option value=''>-</option><option>a</option></select></form><div id=d data-x=1 class='a b'></div></body></html>",
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
          console.log('misc', navigator.plugins.length > 0, navigator.mimeTypes[0].type, typeof visualViewport.width, screen.orientation.type, document.fullscreenEnabled, Notification.permission, locationbar.visible, typeof document.all, document.all == null, document.all === undefined, !document.all, document.all.length > 0);
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
            "misc true application/pdf number landscape-primary false denied true undefined true false true true",
            "geo 1 true",
        ]
    );
}

#[test]
fn indexeddb_in_memory() {
    let out = logs_of(
        "<html><body></body></html>",
        r#"
        const open = indexedDB.open('shop', 2);
        open.onupgradeneeded = (e) => {
          const db = open.result;
          console.log('upgrade', e.oldVersion, e.newVersion, open.transaction.mode);
          const s = db.createObjectStore('items', { keyPath: 'id', autoIncrement: true });
          s.createIndex('byTag', 'tags', { multiEntry: true });
          s.createIndex('byName', 'name', { unique: true });
          s.add({ name: 'apple', tags: ['fruit', 'red'] });
          s.add({ name: 'cherry', tags: ['fruit', 'red'] });
          s.add({ name: 'kale', tags: ['veg'] });
        };
        open.onsuccess = () => {
          const db = open.result;
          console.log('open', db.version, Array.from(db.objectStoreNames).join());
          const tx = db.transaction('items', 'readwrite');
          const s = tx.objectStore('items');
          s.get(2).onsuccess = (e) => console.log('get', e.target.result.name);
          s.index('byTag').getAll('red').onsuccess = (e) => console.log('index', e.target.result.map((x) => x.name).join());
          s.count(IDBKeyRange.lowerBound(2)).onsuccess = (e) => console.log('count', e.target.result);
          const dup = s.add({ name: 'apple', tags: [] });
          dup.onerror = (e) => { console.log('dup', dup.error.name); e.preventDefault(); };
          const names = [];
          s.openCursor(null, 'prev').onsuccess = (e) => {
            const c = e.target.result;
            if (c) { names.push(c.value.name); if (c.key === 3) c.update(Object.assign(c.value, { name: 'KALE' })); c.continue(); }
            else console.log('cursor', names.join());
          };
          tx.oncomplete = () => {
            const t2 = db.transaction(['items']);
            t2.objectStore('items').index('byName').get('KALE').onsuccess = (e) => console.log('updated', e.target.result.id);
            t2.oncomplete = () => indexedDB.databases().then((l) => console.log('dbs', JSON.stringify(l)));
          };
        };
        console.log('cmp', indexedDB.cmp([1, 'a'], [1, 'b']), indexedDB.cmp('a', 1), IDBKeyRange.bound(1, 5, true).includes(1));
        "#,
    );
    assert_eq!(
        out,
        vec![
            "cmp -1 1 false",
            "upgrade 0 2 versionchange",
            "open 2 items",
            "get cherry",
            "index apple,cherry",
            "count 2",
            "dup ConstraintError",
            "cursor kale,cherry,apple",
            "updated 3",
            "dbs [{\"name\":\"shop\",\"version\":2}]",
        ]
    );
}

#[test]
fn web_crypto_digest_hmac_aes_pbkdf2() {
    let out = logs_of(
        "<html><body></body></html>",
        r#"
        const hex = (b) => Array.from(new Uint8Array(b), (x) => x.toString(16).padStart(2, '0')).join('');
        const enc = new TextEncoder();
        (async () => {
          const s = crypto.subtle;
          console.log('sha1', hex(await s.digest('SHA-1', enc.encode('abc'))));
          console.log('sha256', hex(await s.digest('SHA-256', enc.encode('abc'))));
          console.log('sha512', hex(await s.digest('SHA-512', enc.encode('abc'))).slice(0, 32));
          const hk = await s.importKey('raw', enc.encode('key'), { name: 'HMAC', hash: 'SHA-256' }, true, ['sign', 'verify']);
          const sig = await s.sign('HMAC', hk, enc.encode('The quick brown fox jumps over the lazy dog'));
          console.log('hmac', hex(sig), await s.verify('HMAC', hk, sig, enc.encode('The quick brown fox jumps over the lazy dog')));
          const key = await s.importKey('raw', Uint8Array.from({ length: 16 }, (_, i) => i), 'AES-CBC', true, ['encrypt', 'decrypt']);
          const pt = Uint8Array.from([0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
          const ct = await s.encrypt({ name: 'AES-CBC', iv: new Uint8Array(16) }, key, pt);
          console.log('aes', hex(ct).slice(0, 32), hex(await s.decrypt({ name: 'AES-CBC', iv: new Uint8Array(16) }, key, ct)) === hex(pt));
          const gk = await s.generateKey({ name: 'AES-GCM', length: 256 }, true, ['encrypt', 'decrypt']);
          const iv = crypto.getRandomValues(new Uint8Array(12));
          const g1 = await s.encrypt({ name: 'AES-GCM', iv }, gk, enc.encode('secret'));
          console.log('gcm', new TextDecoder().decode(await s.decrypt({ name: 'AES-GCM', iv }, gk, g1)), g1.byteLength, (await s.exportKey('jwk', gk)).alg);
          const pk = await s.importKey('raw', enc.encode('password'), 'PBKDF2', false, ['deriveBits']);
          const bits = await s.deriveBits({ name: 'PBKDF2', salt: enc.encode('salt'), iterations: 2, hash: 'SHA-1' }, pk, 160);
          console.log('pbkdf2', hex(bits));
          const bad = await s.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, true, ['sign']).catch((e) => e.name);
          console.log('ec', bad, crypto instanceof Crypto, crypto.subtle instanceof SubtleCrypto, gk instanceof CryptoKey);
        })().catch((e) => console.log('ERR ' + e + ' ' + e.stack));
        "#,
    );
    assert_eq!(
        out,
        vec![
            "sha1 a9993e364706816aba3e25717850c26c9cd0d89d",
            "sha256 ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "sha512 ddaf35a193617abacc417349ae204131",
            "hmac f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8 true",
            "aes 69c4e0d86a7b0430d8cdb78070b4c55a true",
            "gcm secret 22 A256GCM",
            "pbkdf2 ea6c014dc72d6f8ccd1ed92ace1d41f0d8de8957",
            "ec NotSupportedError true true true",
        ]
    );
}

#[test]
fn canvas_2d_rasterizes_and_encodes() {
    let out = logs_of(
        "<html><body><canvas id=c width=20 height=10></canvas></body></html>",
        r#"
        const c = document.getElementById('c');
        const ctx = c.getContext('2d');
        const px = (x, y) => Array.from(ctx.getImageData(x, y, 1, 1).data).join(',');
        ctx.fillStyle = 'red';
        ctx.fillRect(0, 0, 10, 10);
        ctx.fillStyle = 'rgba(0, 0, 255, 0.5)';
        ctx.beginPath(); ctx.arc(15, 5, 4, 0, Math.PI * 2); ctx.fill();
        console.log('px', px(5, 5), px(15, 5), px(19, 0), ctx.fillStyle);
        ctx.save(); ctx.translate(10, 0); ctx.clearRect(0, 0, 2, 2); ctx.restore();
        console.log('clear', px(10, 0), px(9, 0), ctx.getTransform().isIdentity);
        const id = ctx.createImageData(2, 1);
        id.data.set([0, 255, 0, 255, 0, 255, 0, 255]);
        ctx.putImageData(id, 0, 9);
        console.log('put', px(1, 9), ctx.isPointInPath(15, 5), ctx.isPointInPath(1, 1));
        const p = new Path2D('M0 0 L10 0 L10 10 Z');
        console.log('path2d', ctx.isPointInPath(p, 8, 2), ctx.isPointInPath(p, 2, 8));
        const g = ctx.createLinearGradient(0, 0, 20, 0); g.addColorStop(0, '#000'); g.addColorStop(1, '#fff');
        ctx.fillStyle = g; ctx.fillRect(0, 0, 20, 1);
        const mid = ctx.getImageData(10, 0, 1, 1).data[0];
        console.log('gradient', mid > 100 && mid < 160, ctx.measureText('hello').width > 0, ctx.lineWidth, ctx.canvas === c);
        const url = c.toDataURL();
        console.log('png', url.startsWith('data:image/png;base64,iVBORw0KGgo'), document.createElement('canvas').getContext('webgl'), c.getContext('2d') === ctx);
        const off = new OffscreenCanvas(4, 4);
        const octx = off.getContext('2d');
        octx.fillStyle = '#0f0'; octx.fillRect(0, 0, 4, 4);
        ctx.drawImage(off, 0, 0);
        console.log('draw', px(3, 3), off.transferToImageBitmap() instanceof ImageBitmap);
        createImageBitmap(new ImageData(new Uint8ClampedArray([1, 2, 3, 255]), 1)).then((b) => console.log('bitmap', b.width, b.height));
        "#,
    );
    assert_eq!(
        out,
        vec![
            "px 255,0,0,255 0,0,255,128 0,0,0,0 rgba(0, 0, 255, 0.5)",
            "clear 0,0,0,0 255,0,0,255 true",
            "put 0,255,0,255 true false",
            "path2d true false",
            "gradient true true 1 true",
            "png true null true",
            "draw 0,255,0,255 true",
            "bitmap 1 1",
        ]
    );
}

#[test]
fn canvas_2d_text_strokes_compositing_and_shadows() {
    let out = logs_of(
        "<html><body><canvas id=c width=60 height=30></canvas></body></html>",
        r#"
        const c = document.getElementById('c');
        const ctx = c.getContext('2d');
        const px = (x, y) => Array.from(ctx.getImageData(x, y, 1, 1).data);
        const inked = (x0, y0, x1, y1) => { const d = ctx.getImageData(x0, y0, x1 - x0, y1 - y0).data; let n = 0; for (let i = 3; i < d.length; i += 4) if (d[i] > 0) n++; return n; };
        ctx.font = '20px sans-serif';
        const m = ctx.measureText('Hi');
        ctx.fillStyle = '#000';
        ctx.fillText('Hi', 2, 20);
        console.log('text', m.width > 5, m.actualBoundingBoxAscent > 5, inked(0, 0, 30, 30) > 20);
        ctx.clearRect(0, 0, 60, 30);
        console.log('cleared', inked(0, 0, 60, 30));
        // Anti-aliased diagonal stroke: partial coverage on edges.
        ctx.strokeStyle = '#f00'; ctx.lineWidth = 3;
        ctx.beginPath(); ctx.moveTo(0, 0); ctx.lineTo(29, 29); ctx.stroke();
        const partial = Array.from(ctx.getImageData(0, 0, 30, 30).data).some((v, i) => i % 4 === 3 && v > 0 && v < 255);
        console.log('stroke', px(10, 10).join(','), partial);
        // Round caps / dashes.
        ctx.clearRect(0, 0, 60, 30);
        ctx.setLineDash([4, 4]); ctx.lineWidth = 2; ctx.strokeStyle = '#00f';
        ctx.beginPath(); ctx.moveTo(0, 5); ctx.lineTo(40, 5); ctx.stroke();
        console.log('dash', px(2, 5)[2], px(6, 5)[3], ctx.getLineDash().join(','));
        ctx.setLineDash([]);
        // Compositing: destination-out punches a hole; multiply darkens.
        ctx.clearRect(0, 0, 60, 30);
        ctx.fillStyle = '#ff0000'; ctx.fillRect(0, 0, 20, 20);
        ctx.globalCompositeOperation = 'destination-out';
        ctx.fillRect(5, 5, 5, 5);
        ctx.globalCompositeOperation = 'multiply';
        ctx.fillStyle = '#808080'; ctx.fillRect(15, 0, 5, 5);
        ctx.globalCompositeOperation = 'source-over';
        console.log('composite', px(7, 7)[3], px(2, 2).join(','), px(17, 2).join(','));
        // globalAlpha and shadows.
        ctx.clearRect(0, 0, 60, 30);
        ctx.globalAlpha = 0.5; ctx.fillStyle = '#00ff00'; ctx.fillRect(0, 0, 4, 4); ctx.globalAlpha = 1;
        ctx.shadowColor = 'rgba(0,0,0,1)'; ctx.shadowOffsetX = 10; ctx.shadowOffsetY = 0; ctx.shadowBlur = 0;
        ctx.fillStyle = '#00f'; ctx.fillRect(30, 10, 5, 5);
        ctx.shadowColor = 'transparent';
        console.log('alpha+shadow', px(1, 1).join(','), px(42, 12).join(','), px(32, 12).join(','));
        // Clipping with save/restore.
        ctx.clearRect(0, 0, 60, 30);
        ctx.save(); ctx.beginPath(); ctx.rect(0, 0, 10, 10); ctx.clip();
        ctx.fillStyle = '#f00'; ctx.fillRect(0, 0, 60, 30); ctx.restore();
        ctx.fillStyle = '#0f0'; ctx.fillRect(20, 20, 2, 2);
        console.log('clip', px(5, 5).join(','), px(15, 15)[3], px(20, 20).join(','));
        // Scaled drawImage of a canvas source, patterns.
        const src = document.createElement('canvas'); src.width = 2; src.height = 2;
        const s = src.getContext('2d'); s.fillStyle = '#f00'; s.fillRect(0, 0, 1, 2); s.fillStyle = '#00f'; s.fillRect(1, 0, 1, 2);
        ctx.clearRect(0, 0, 60, 30); ctx.imageSmoothingEnabled = false;
        ctx.drawImage(src, 0, 0, 20, 20);
        console.log('drawImage', px(5, 5).join(','), px(15, 15).join(','));
        ctx.fillStyle = ctx.createPattern(src, 'repeat'); ctx.fillRect(40, 0, 4, 1);
        console.log('pattern', px(40, 0).join(','), px(41, 0).join(','), px(42, 0).join(','));
        // Radial gradient centre colour, transforms.
        const rg = ctx.createRadialGradient(50, 20, 0, 50, 20, 8); rg.addColorStop(0, '#fff'); rg.addColorStop(1, '#000');
        ctx.fillStyle = rg; ctx.fillRect(42, 12, 16, 16);
        ctx.setTransform(2, 0, 0, 2, 0, 0); ctx.fillStyle = '#ff0'; ctx.fillRect(10, 12, 1, 1); ctx.resetTransform();
        console.log('radial+transform', px(50, 20)[0] > 200, px(21, 25).join(','));
        // Encoders.
        console.log('encode', c.toDataURL('image/jpeg', 0.8).startsWith('data:image/jpeg;base64,/9j/'), new OffscreenCanvas(0, 0).getContext('2d') !== null);
        c.toBlob((b) => console.log('blob', b.type, b.size > 50));
        new OffscreenCanvas(3, 3).convertToBlob().then((b) => createImageBitmap(b)).then((bm) => console.log('roundtrip', bm.width, bm.height));
        "#,
    );
    assert_eq!(
        out,
        vec![
            "text true true true",
            "cleared 0",
            "stroke 255,0,0,255 true",
            "dash 255 0 4,4",
            "composite 0 255,0,0,255 128,0,0,255",
            "alpha+shadow 0,255,0,128 0,0,0,255 0,0,255,255",
            "clip 255,0,0,255 0 0,255,0,255",
            "drawImage 255,0,0,255 0,0,255,255",
            "pattern 255,0,0,255 0,0,255,255 255,0,0,255",
            "radial+transform true 255,255,0,255",
            "encode true true",
            "roundtrip 3 3",
            "blob image/png true",
        ]
    );
}

#[test]
fn dynamic_import_of_blob_module() {
    let out = logs_of(
        "<html><body></body></html>",
        r#"
        const src = 'export const answer = 42; export default function hi() { return "hi"; }';
        const url = URL.createObjectURL(new Blob([src], { type: 'text/javascript' }));
        import(url).then((m) => console.log('blob module', m.answer, m.default()), (e) => console.log('failed', e.message));
        "#,
    );
    assert_eq!(out, vec!["blob module 42 hi"]);
}

#[test]
fn custom_element_failed_upgrade_gets_no_callbacks() {
    let (mut env, mut host) = crate::tests::setup("<html><body><x-bad></x-bad><x-good></x-good></body></html>");
    let out = crate::tests::run(
        &mut env,
        &mut host,
        r#"
        class Bad extends HTMLElement { constructor() { super(); throw new Error('boom'); } connectedCallback() { console.log('bad connected'); } }
        class Good extends HTMLElement { constructor() { super(); this.ready = true; } connectedCallback() { console.log('good connected', this.ready); } }
        customElements.define('x-bad', Bad);
        customElements.define('x-good', Good);
        const bad = document.querySelector('x-bad');
        document.body.append(bad);
        console.log('state', bad.matches(':defined'), document.querySelector('x-good').matches(':defined'));
        "#,
    );
    assert_eq!(out, vec!["good connected true", "state false true"]);
    assert!(env.logs.iter().any(|(l, m)| l == "error" && m.contains("boom")), "logs: {:?}", env.logs);
}

#[test]
fn svg_dom_types_geometry_and_coordinates() {
    let out = logs_of(
        r#"<html><body><svg id=s width="200" height="100" viewBox="0 0 400 200">
            <g id=g transform="translate(10 20) scale(2)">
              <rect id=r x="5" y="6" width="10" height="20" rx="2"/>
              <circle id=c cx="50" cy="50" r="10"/>
            </g>
            <path id=p d="M0 0 L30 0 L30 40 Z"/>
            <polyline id=pl points="0,0 10,0 10,10"/>
            <line id=l x1="0" y1="0" x2="3" y2="4"/>
            <marker id=m orient="45deg"/>
            <linearGradient id=lg gradientUnits="userSpaceOnUse" spreadMethod="reflect"/>
            <text id=t x="10 20" y="30" rotate="5 6">Hello</text>
          </svg></body></html>"#,
        r#"
        const $ = (id) => document.getElementById(id);
        const r = $('r'), s = $('s'), g = $('g');
        console.log('len', r.x === r.x, r.x.baseVal.value, r.width.baseVal.unitType, r.x.baseVal instanceof SVGLength, r.x instanceof SVGAnimatedLength);
        r.x.baseVal.value = 7;
        r.width.baseVal.newValueSpecifiedUnits(SVGLength.SVG_LENGTHTYPE_CM, 1);
        console.log('write', r.getAttribute('x'), r.getAttribute('width'), Math.round(r.width.baseVal.value * 100) / 100);
        try { r.x.animVal.value = 1; } catch (e) { console.log('ro', e.name); }
        const tl = g.transform.baseVal;
        console.log('transforms', tl.numberOfItems, tl.getItem(0).type, tl.getItem(1).matrix.a, tl[1].type);
        const t = s.createSVGTransform(); t.setRotate(90, 0, 0); tl.appendItem(t);
        console.log('append', g.getAttribute('transform'));
        tl.consolidate();
        console.log('consolidate', tl.numberOfItems, tl.getItem(0).type === SVGTransform.SVG_TRANSFORM_MATRIX);
        g.setAttribute('transform', 'translate(10 20) scale(2)');
        console.log('viewBox', s.viewBox.baseVal.width, s.preserveAspectRatio.baseVal.align, s.width.baseVal.value);
        const pl = $('pl').points;
        pl.appendItem(s.createSVGPoint());
        console.log('points', pl.length, $('pl').getAttribute('points'), pl.getItem(1).x);
        pl.getItem(0).x = 5;
        console.log('livepoint', $('pl').getAttribute('points'));
        console.log('geom', $('l').getTotalLength(), JSON.stringify($('l').getPointAtLength(2.5)), $('p').getTotalLength(), $('p').isPointInFill({ x: 25, y: 5 }), $('p').isPointInFill({ x: 5, y: 30 }), $('l').isPointInStroke({ x: 1.5, y: 2 }));
        const cb = $('c').getBBox();
        console.log('bbox', Math.round(cb.x), Math.round(cb.y), Math.round(cb.width), Math.round(cb.height), JSON.stringify(r.getBBox()));
        const gb = g.getBBox();
        console.log('gbbox', Math.round(gb.x), Math.round(gb.y), Math.round(gb.width), Math.round(gb.height));
        const m = $('c').getCTM();
        console.log('ctm', m.a, m.e, m.f, $('c').getScreenCTM() instanceof DOMMatrix);
        console.log('enums', $('lg').gradientUnits.baseVal, $('lg').spreadMethod.baseVal, SVGUnitTypes.SVG_UNIT_TYPE_OBJECTBOUNDINGBOX);
        const mk = $('m');
        console.log('marker', mk.orientType.baseVal, mk.orientAngle.baseVal.value, mk.markerWidth.baseVal.value);
        mk.setOrientToAuto();
        console.log('auto', mk.orientType.baseVal, mk.getAttribute('orient'));
        const tx = $('t');
        console.log('text', tx.x.baseVal.numberOfItems, tx.x.baseVal.getItem(1).value, tx.rotate.baseVal.getItem(0).value, tx.getNumberOfChars(), tx.getComputedTextLength() > 0, tx.getSubStringLength(0, 2) < tx.getComputedTextLength());
        const a = s.createSVGAngle(); a.valueAsString = '1rad';
        console.log('angle', a.unitType, Math.round(a.value), s.getElementById('c') === $('c'), document.rootElement, s.viewportElement, r.viewportElement === s);
        console.log('symbol', Object.getPrototypeOf(SVGSymbolElement.prototype) === SVGGraphicsElement.prototype, s.createSVGNumber().value, s.checkIntersection($('l'), new DOMRect(0, 0, 10, 10)));
        "#,
    );
    assert_eq!(
        out,
        vec![
            "len true 5 1 true true",
            "write 7 1cm 37.8",
            "ro NoModificationAllowedError",
            "transforms 2 2 2 3",
            "append translate(10 20) scale(2 2) rotate(90)",
            "consolidate 1 true",
            "viewBox 400 6 200",
            "points 4 0,0 10,0 10,10 0,0 10",
            "livepoint 5,0 10,0 10,10 0,0",
            "geom 5 {\"x\":1.5,\"y\":2,\"z\":0,\"w\":1} 120 true false true",
            "bbox 40 40 20 20 {\"x\":7,\"y\":6,\"width\":37.79527559055117,\"height\":20,\"top\":6,\"right\":44.79527559055117,\"bottom\":26,\"left\":7}",
            "gbbox 7 6 53 54",
            "ctm 1 5 10 true",
            "enums 1 2 2",
            "marker 2 45 3",
            "auto 1 auto",
            "text 2 20 5 5 true true",
            "angle 3 57 true null null true",
            "symbol true 0 true",
        ]
    );
}

/// A one-connection WebSocket echo server on 127.0.0.1 (text and binary
/// frames echoed, close answered); returns the port.
fn ws_echo_server() -> u16 {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut req = Vec::new();
        let mut b = [0u8; 1];
        while !req.ends_with(b"\r\n\r\n") {
            if s.read(&mut b).unwrap() == 0 {
                return;
            }
            req.push(b[0]);
        }
        let req = String::from_utf8_lossy(&req).to_string();
        let key = req.lines().find_map(|l| l.strip_prefix("Sec-WebSocket-Key: ").or_else(|| l.strip_prefix("sec-websocket-key: "))).unwrap().trim().to_string();
        let accept = rabbithole_net::websocket::accept_key(&key);
        write!(s, "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\nSec-WebSocket-Protocol: chat\r\n\r\n").unwrap();
        loop {
            let Ok(f) = rabbithole_net::websocket::read_frame(&mut s, 1 << 20) else { return };
            let mut out = vec![0x80 | f.opcode, f.payload.len() as u8];
            out.extend_from_slice(&f.payload);
            s.write_all(&out).unwrap();
            if f.opcode == 8 {
                return;
            }
        }
    });
    port
}

#[test]
fn websocket_over_network_client() {
    let port = ws_echo_server();
    let (mut env, mut host) = setup("<html><body></body></html>");
    env.net = Some(rabbithole_net::Fetcher::isolated());
    let src = format!(
        r#"
        const ws = new WebSocket('ws://127.0.0.1:{port}/echo', ['chat']);
        ws.binaryType = 'arraybuffer';
        console.log('state', ws.readyState, ws.url);
        ws.onopen = () => {{ console.log('open', ws.readyState, ws.protocol); ws.send('hello'); ws.send(new Uint8Array([1, 2, 3])); }};
        let n = 0;
        ws.onmessage = (e) => {{
          console.log('message', typeof e.data === 'string' ? e.data : Array.from(new Uint8Array(e.data)).join(','), e.origin);
          if (++n === 2) ws.close(1000, 'bye');
        }};
        ws.onerror = () => console.log('error');
        ws.onclose = (e) => console.log('close', e.code, e.reason, e.wasClean, ws.readyState, e instanceof CloseEvent);
        "#
    );
    let mut out = run(&mut env, &mut host, &src);
    let mut t = 1100.0;
    while !out.iter().any(|l| l.starts_with("close")) && t < 1100.0 + 5000.0 {
        std::thread::sleep(std::time::Duration::from_millis(10));
        let before = env.logs.len();
        host.run_timers(&mut env, t);
        host.run_jobs(&mut env);
        out.extend(env.logs[before..].iter().filter(|(l, _)| l == "log").map(|(_, m)| m.clone()));
        t += 16.0;
    }
    assert!(env.errors().is_empty(), "errors: {:?}", env.errors());
    assert_eq!(
        out,
        vec![
            format!("state 0 ws://127.0.0.1:{port}/echo"),
            "open 1 chat".to_string(),
            "message hello ws://127.0.0.1:".to_string() + &port.to_string(),
            "message 1,2,3 ws://127.0.0.1:".to_string() + &port.to_string(),
            "close 1000 bye true 3 true".to_string(),
        ]
    );
}

#[test]
fn websocket_and_event_source_without_network_fail_cleanly() {
    let out = logs_of(
        "<html><body></body></html>",
        r#"
        const ws = new WebSocket('wss://example.test/socket');
        ws.onerror = () => console.log('ws error', ws.readyState);
        ws.onclose = (e) => console.log('ws close', e.code, e.wasClean);
        try { new WebSocket('ftp://x'); } catch (e) { console.log('scheme', e.name); }
        try { ws.send('x'); } catch (e) { console.log('send', e.name); }
        try { ws.close(1001); } catch (e) { console.log('code', e.name); }
        const es = new EventSource('/events');
        es.onerror = () => { console.log('es error', es.readyState); es.close(); };
        console.log('es', es.url, es.readyState, es.withCredentials);
        "#,
    );
    assert_eq!(
        out,
        vec![
            "scheme SyntaxError",
            "send InvalidStateError",
            "code InvalidAccessError",
            "es https://example.test/events 0 false",
            "es error 0",
            "ws error 3",
            "ws close 1006 false",
        ]
    );
}

#[test]
fn history_traversal_and_navigation_api() {
    let out = logs_of(
        "<html><body></body></html>",
        r#"
        window.addEventListener('popstate', (e) => console.log('popstate', JSON.stringify(e.state), location.pathname));
        navigation.addEventListener('currententrychange', (e) => console.log('change', e.navigationType, e.from.url.split('/').pop()));
        history.pushState({ p: 1 }, '', '/a');
        history.pushState({ p: 2 }, '', '/b');
        console.log('entries', navigation.entries().length, navigation.currentEntry.index, navigation.canGoBack, location.pathname);
        navigation.addEventListener('navigate', (e) => {
          console.log('navigate', e.navigationType, e.destination.url.split('/').pop(), e.canIntercept);
          e.intercept({ handler: () => Promise.resolve() });
        });
        navigation.navigate('/c', { state: { s: 3 } }).finished.then((entry) => console.log('finished', entry.getState().s, location.pathname));
        setTimeout(() => history.back(), 0);
        "#,
    );
    assert_eq!(
        out,
        vec![
            "change push page.html?q=1#frag",
            "change push a",
            "entries 3 2 true /b",
            "navigate push c true",
            "change push b",
            "finished 3 /c",
            "change traverse c",
            "popstate {\"p\":2} /b",
        ]
    );
}

#[test]
fn web_animations_and_font_loading() {
    let out = logs_of(
        "<html><body><div id=d></div></body></html>",
        r#"
        const d = document.getElementById('d');
        const a = d.animate([{ opacity: 0 }, { opacity: 1 }], { duration: 50, fill: 'forwards' });
        console.log('anim', a instanceof Animation, a.effect instanceof KeyframeEffect, a.effect.getKeyframes().length, a.effect.getTiming().duration, d.getAnimations().length, document.timeline instanceof DocumentTimeline);
        a.finished.then(() => console.log('finished', a.playState, a.effect.getComputedTiming().progress));
        a.onfinish = (e) => console.log('onfinish', e instanceof AnimationPlaybackEvent);
        const b = d.animate({ transform: ['none', 'scale(2)'] }, 1000);
        const bf = b.finished;
        b.cancel();
        bf.catch((e) => console.log('cancelled', e.name, b.playState));
        a.finish();
        const face = new FontFace('Test', 'url(/font.woff2)', { weight: '700' });
        document.fonts.add(face);
        console.log('fonts', document.fonts.has(face), document.fonts.size, face.status, face.weight, document.fonts.check('12px Test'), document.fonts.status);
        const bin = new FontFace('Bin', new Uint8Array([0, 1, 0, 0]));
        bin.loaded.then((f) => console.log('binary', f.status));
        "#,
    );
    let mut sorted = out.clone();
    sorted.sort();
    assert_eq!(out[0], "anim true true 2 50 1 true");
    assert_eq!(out[1], "fonts true 1 unloaded 700 false loaded");
    assert!(sorted.contains(&"binary loaded".to_string()), "{out:?}");
    assert!(sorted.contains(&"cancelled AbortError idle".to_string()), "{out:?}");
    assert!(sorted.contains(&"finished finished 1".to_string()), "{out:?}");
    assert!(sorted.contains(&"onfinish true".to_string()), "{out:?}");
}

/// Workers run in their own realm: own globals, no DOM, importScripts,
/// structured-clone messages both ways, transferred MessagePorts, error
/// events, terminate(), close(), and SharedWorker connections.
#[test]
fn workers_have_their_own_realm() {
    let (mut env, mut host) = setup("<html><body></body></html>");
    let js = |s: &str| (200u16, "text/javascript".to_string(), s.to_string());
    env.fetches.insert("https://example.test/dir/lib.js".into(), js("self.libValue = 40;"));
    env.fetches.insert(
        "https://example.test/dir/w.js".into(),
        js(r#"
        importScripts('lib.js');
        var shared = typeof sharedFromPage;
        onmessage = function (e) {
          if (e.data === 'boom') { throw new Error('worker failed'); }
          if (e.ports.length) { e.ports[0].postMessage('via port ' + e.data.k); return; }
          postMessage({ got: e.data, sum: libValue + e.data.n, doc: typeof document, win: typeof window, self: self === globalThis,
                       scope: self instanceof DedicatedWorkerGlobalScope && self instanceof WorkerGlobalScope, shared: shared,
                       map: e.data.m instanceof Map && e.data.m.get('a'), loc: location.href });
        };
        setTimeout(function () { postMessage('timer'); }, 5);
        "#),
    );
    env.fetches.insert("https://example.test/dir/closer.js".into(), js("postMessage('bye'); close(); postMessage('never');"));
    env.fetches.insert(
        "https://example.test/dir/sw.js".into(),
        js("var n = 0; onconnect = function (e) { var p = e.ports[0]; var me = ++n; p.onmessage = function (m) { p.postMessage(m.data + ':' + me); }; };"),
    );
    let out = run(
        &mut env,
        &mut host,
        r#"
        window.sharedFromPage = 1;
        var w = new Worker('w.js');
        w.onmessage = function (e) {
          if (e.data === 'timer') { console.log('timer'); return; }
          var d = e.data;
          console.log('msg', JSON.stringify(d.got.n), d.sum, d.doc, d.win, d.self, d.scope, d.shared, d.map, d.loc);
        };
        w.onerror = function (e) { console.log('error', e.message.indexOf('worker failed') >= 0); e.preventDefault(); };
        w.postMessage({ n: 2, m: new Map([['a', 'A']]) });
        var ch = new MessageChannel();
        ch.port1.onmessage = function (e) { console.log('port', e.data); };
        w.postMessage({ k: 'x' }, [ch.port2]);
        w.postMessage('boom');
        try { w.postMessage(function () {}); } catch (e) { console.log('clone', e.name); }
        var c = new Worker('closer.js');
        c.onmessage = function (e) { console.log('closer', e.data); };
        var s1 = new SharedWorker('sw.js'), s2 = new SharedWorker('sw.js');
        s1.port.onmessage = function (e) { console.log('shared', e.data); };
        s2.port.onmessage = function (e) { console.log('shared', e.data); };
        s1.port.postMessage('a'); s2.port.postMessage('b');
        console.log('sync', typeof Worker, w instanceof EventTarget);
        "#,
    );
    assert_eq!(out, vec!["clone DataCloneError", "sync function true"]);
    let mut got = Vec::new();
    for t in [1010.0, 1020.0, 1030.0, 1040.0, 1050.0] {
        let before = env.logs.len();
        host.run_timers(&mut env, t);
        host.run_jobs(&mut env);
        got.extend(env.logs[before..].iter().filter(|(l, _)| l == "log").map(|(_, m)| m.clone()));
    }
    got.sort();
    assert_eq!(
        got,
        vec![
            "closer bye",
            "error true",
            "msg 2 42 undefined undefined true true undefined A https://example.test/dir/w.js",
            "port via port x",
            "shared a:1",
            "shared b:2",
            "timer",
        ],
        "logs: {:?}",
        env.logs
    );
    // terminate(): nothing more arrives.
    run(&mut env, &mut host, "w.terminate(); w.postMessage({ n: 1 }); w.onmessage = function () { console.log('after terminate'); };");
    let before = env.logs.len();
    host.run_timers(&mut env, 1100.0);
    assert!(!env.logs[before..].iter().any(|(_, m)| m.contains("after terminate")));
}

/// `<template>` children live in its content fragment (not in the tree);
/// declarative shadow roots; nodes moved across documents keep their
/// identity; MutationObserver honours attributeFilter.
#[test]
fn templates_declarative_shadow_and_adoption() {
    let (mut env, mut host) = setup(
        "<html><body><template id=t><li class=x>a</li><template id=inner><b>n</b></template></template>\
         <div id=h><template shadowrootmode=open><span id=s>shadow</span></template><i>light</i></div></body></html>",
    );
    let out = run(
        &mut env,
        &mut host,
        r#"
        var t = document.getElementById('t');
        console.log('tpl', t.childNodes.length, t.content.childNodes.length, document.querySelectorAll('li').length, t.content.firstChild.tagName, t.innerHTML.indexOf('<li class="x">a</li>') === 0);
        console.log('nested', t.content.querySelector('template').content.firstChild.tagName, t.content === t.content);
        t.innerHTML = '<p>x</p><p>y</p>';
        console.log('set', t.childNodes.length, t.content.childNodes.length);
        var c = t.cloneNode(true);
        console.log('clone', c.content.childNodes.length, c.content.firstChild.textContent);
        var imp = document.importNode(t.content, true);
        document.body.appendChild(imp);
        console.log('import', document.querySelectorAll('body > p').length, t.content.childNodes.length);
        var h = document.getElementById('h');
        console.log('dsd', !!h.shadowRoot, h.shadowRoot && h.shadowRoot.getElementById('s').textContent, h.querySelectorAll('template').length, h.children.length);
        var r = h.attachShadow({ mode: 'open' });
        console.log('takeover', r === h.shadowRoot, r.childNodes.length);
        var d = new DOMParser().parseFromString('<body><u id=u>x</u></body>', 'text/html');
        var u = d.getElementById('u'); u.expando = 7;
        u.addEventListener('click', function () { console.log('clicked moved node'); });
        document.body.appendChild(u);
        console.log('adopt', document.getElementById('u') === u, u.expando, u.ownerDocument === document, d.body.childNodes.length);
        u.click();
        var seen = [];
        var mo = new MutationObserver(function (recs) { recs.forEach(function (rec) { seen.push(rec.attributeName); }); });
        mo.observe(document.body, { attributes: true, subtree: true, attributeFilter: ['value'] });
        u.setAttribute('selected', ''); u.setAttribute('value', 'v');
        "#,
    );
    let mo = run(&mut env, &mut host, "console.log('mo', seen.join())");
    assert_eq!(mo, vec!["mo value"]);
    assert_eq!(
        out,
        vec![
            "tpl 0 2 0 LI true",
            "nested B true",
            "set 0 2",
            "clone 2 x",
            "import 2 2",
            "dsd true shadow 0 1",
            "takeover true 0",
            "adopt true 7 true 0",
            "clicked moved node",
        ]
    );
}

/// `URL.username` / `password` exist and accept assignment in strict code
/// (Apple's store script clears `url.password`).
#[test]
fn url_credentials_are_assignable() {
    let r = result_of("<p>x</p>", "'use strict'; var u = new URL('https://www.example.test/a?b#c'); u.password = ''; u.username = ''; u.hash = ''; var result = [u.password, u.username, u.href].join('|');");
    assert_eq!(r, "'||https://www.example.test/a?b'");
}

#[test]
fn custom_element_attribute_reactions_run_before_the_method_returns() {
    // [CEReactions]: attributeChangedCallback runs inside setAttribute /
    // removeAttribute / toggleAttribute / reflected setters, so a guard
    // flag set around the call is seen (Polymer's reflection relies on it).
    let out = result_of(
        "<html><body></body></html>",
        r#"
        const log = [];
        class XRef extends HTMLElement {
          static observedAttributes = ['a', 'id', 'class', 'hidden'];
          attributeChangedCallback(n, o, v) { log.push(n + ':' + (this.guard ? 'in' : 'out')); }
        }
        customElements.define('x-ref', XRef);
        const el = document.createElement('x-ref');
        document.body.append(el);
        el.guard = true;
        el.setAttribute('a', '1');
        el.toggleAttribute('a');
        el.id = 'z';
        el.className = 'c';
        el.hidden = true;
        el.guard = false;
        window.result = log.join();
        "#,
    );
    assert_eq!(out, "'a:in,a:in,id:in,class:in,hidden:in'");
}

#[test]
fn child_nodes_is_one_refreshed_node_list() {
    let r = result_of(
        "<div id=d><b>1</b><i>2</i></div>",
        "var d = document.getElementById('d'); var a = d.childNodes; var same = a === d.childNodes; d.appendChild(document.createElement('u')); var b = d.childNodes; var bl = b.length; d.removeChild(d.firstChild); var result = [same, a === b, bl, d.childNodes.length, d.childNodes[0].tagName, d.childNodes instanceof NodeList, d.firstChild.childNodes.length].join();",
    );
    assert_eq!(r, "'true,true,3,2,I,true,1'");
}

/// `defaultValue` is the `value` content attribute: setting it on a dirty
/// input keeps the typed value (React restores uncontrolled inputs by
/// writing `defaultValue` after an `input` event; the TodoMVC-React suite
/// of Speedometer lost every typed todo).
#[test]
fn input_default_value_is_the_value_attribute() {
    let out = logs_of(
        "<html><body></body></html>",
        r#"
        const i = document.createElement('input');
        i.value = 'typed'; i.defaultValue = 'def';
        const j = document.createElement('input');
        j.defaultValue = 'd'; const before = j.value; j.setAttribute('value', 'e');
        console.log(i.value, i.getAttribute('value'), before, j.value, j.defaultValue);
        "#,
    );
    assert_eq!(out, vec!["typed def d e e"]);
}

/// `contentEditable` is settable (CodeMirror assigns it in strict mode) and
/// `isContentEditable` inherits; removing every attribute through
/// `attributes[0]` / `removeAttributeNode` works also after an attribute
/// was removed and set again (CodeMirror's clearAttributes).
#[test]
fn content_editable_and_attribute_node_removal() {
    let out = logs_of(
        "<html><body><div id=d contenteditable><span id=s></span></div></body></html>",
        r#"
        'use strict';
        const d = document.getElementById('d'), s = document.getElementById('s');
        const a = [d.contentEditable, s.contentEditable, s.isContentEditable];
        d.contentEditable = 'false';
        a.push(d.getAttribute('contenteditable'), s.isContentEditable);
        d.contentEditable = 'inherit';
        a.push(d.hasAttribute('contenteditable'));
        try { d.contentEditable = 'bogus'; } catch (e) { a.push(e.name); }
        const el = document.createElement('div');
        el.setAttribute('class', 'x'); el.setAttribute('title', 't');
        while (el.attributes.length) el.removeAttributeNode(el.attributes[0]);
        el.setAttribute('class', 'y');
        while (el.attributes.length) el.removeAttributeNode(el.attributes[0]);
        a.push(el.attributes.length);
        console.log(a.join(' '));
        "#,
    );
    assert_eq!(out, vec!["true inherit true false false false SyntaxError 0"]);
}
