//! Web API completeness: the inventory coverage check and tests for the
//! interfaces added to make the DOM / Web API surface complete.

use crate::tests::{run, setup};

/// Floor for `webapi_coverage` (raise it as coverage improves).
const MIN_COVERAGE: f64 = 0.65;

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
