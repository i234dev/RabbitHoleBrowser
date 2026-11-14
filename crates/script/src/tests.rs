//! Tests with a mock environment (no network, no layout engine).

use crate::*;
use rabbithole_dom::{Document, NodeId};
use rabbithole_gfx::Rect;
use rabbithole_net::{Request, Response, Url};
use rabbithole_style::ComputedStyle;
use std::collections::HashMap;
use std::rc::Rc;

pub(crate) struct MockEnv {
    pub(crate) doc: Document,
    pub(crate) url: Url,
    pub(crate) dirty: u32,
    pub(crate) logs: Vec<(String, String)>,
    pub(crate) fetches: HashMap<String, (u16, String, String)>,
    pub(crate) navigations: Vec<(String, bool)>,
    pub(crate) storage: HashMap<(bool, String), String>,
    pub(crate) rects: HashMap<NodeId, Rect>,
    pub(crate) scroll: (f32, f32),
    pub(crate) cookies: String,
    pub(crate) title: Option<String>,
    pub(crate) submitted: Vec<(NodeId, Option<NodeId>)>,
    pub(crate) focused: Option<NodeId>,
    pub(crate) now: f64,
    pub(crate) opened: Vec<String>,
    /// Real network client for WebSocket / EventSource tests.
    pub(crate) net: Option<rabbithole_net::Fetcher>,
}

impl MockEnv {
    pub(crate) fn new(html: &str) -> MockEnv {
        MockEnv {
            doc: rabbithole_html::parse(html),
            url: Url::parse_absolute("https://example.test/dir/page.html?q=1#frag").unwrap(),
            dirty: 0,
            logs: Vec::new(),
            fetches: HashMap::new(),
            navigations: Vec::new(),
            storage: HashMap::new(),
            rects: HashMap::new(),
            scroll: (0.0, 0.0),
            cookies: String::new(),
            title: None,
            submitted: Vec::new(),
            focused: None,
            now: 1000.0,
            opened: Vec::new(),
            net: None,
        }
    }

    pub(crate) fn errors(&self) -> Vec<String> {
        self.logs.iter().filter(|(l, _)| l == "error").map(|(_, m)| m.clone()).collect()
    }

}

impl HostEnv for MockEnv {
    fn doc(&mut self) -> &mut Document {
        &mut self.doc
    }
    fn open_websocket(&mut self, url: Url, protocols: Vec<String>) -> Option<Result<rabbithole_net::WebSocket, String>> {
        let f = self.net.as_ref()?;
        Some(rabbithole_net::WebSocket::connect(f, &url, &protocols, Some(&self.url), None))
    }
    fn open_event_source(&mut self, url: Url, with_credentials: bool) -> Option<Result<rabbithole_net::EventSource, String>> {
        let f = self.net.as_ref()?;
        Some(rabbithole_net::EventSource::connect(f, &url, with_credentials, Some(&self.url), None))
    }
    fn document_url(&self) -> Url {
        self.url.clone()
    }
    fn base_url(&self) -> Url {
        self.url.clone()
    }
    fn mark_dom_dirty(&mut self) {
        self.dirty += 1;
    }
    fn ensure_layout(&mut self) {}
    fn node_rect(&mut self, node: NodeId) -> Option<Rect> {
        self.rects.get(&node).copied()
    }
    fn computed_style(&mut self, _node: NodeId) -> Option<Rc<ComputedStyle>> {
        let mut s = ComputedStyle::initial();
        s.display = rabbithole_style::Display::Block;
        s.color = rabbithole_gfx::Color::rgb(255, 0, 0);
        Some(Rc::new(s))
    }
    fn viewport(&self) -> (f32, f32) {
        (1000.0, 800.0)
    }
    fn scroll(&self) -> (f32, f32) {
        self.scroll
    }
    fn set_scroll(&mut self, x: f32, y: f32) {
        self.scroll = (x, y);
    }
    fn fetch(&mut self, req: Request) -> Result<Response, String> {
        let key = req.url.to_string();
        match self.fetches.get(&key) {
            Some((status, ct, body)) => Ok(Response {
                url: req.url.clone(),
                status: *status,
                content_type: ct.clone(),
                charset: Some("utf-8".into()),
                body: body.clone().into_bytes(),
                headers: vec![("content-type".into(), ct.clone()), ("x-test".into(), "yes".into())],
            }),
            None => Err(format!("no mock for {key}")),
        }
    }
    fn navigate(&mut self, url: Url, replace: bool) {
        self.navigations.push((url.to_string(), replace));
    }
    fn cookies(&mut self) -> String {
        self.cookies.clone()
    }
    fn set_cookie(&mut self, value: &str) {
        self.cookies = value.split(';').next().unwrap_or("").to_string();
    }
    fn storage_get(&mut self, local: bool, key: &str) -> Option<String> {
        self.storage.get(&(local, key.to_string())).cloned()
    }
    fn storage_set(&mut self, local: bool, key: &str, value: Option<&str>) {
        match value {
            Some(v) => {
                self.storage.insert((local, key.to_string()), v.to_string());
            }
            None => {
                self.storage.remove(&(local, key.to_string()));
            }
        }
    }
    fn storage_keys(&mut self, local: bool) -> Vec<String> {
        let mut k: Vec<String> = self.storage.keys().filter(|(l, _)| *l == local).map(|(_, k)| k.clone()).collect();
        k.sort();
        k
    }
    fn log(&mut self, level: &str, msg: &str) {
        self.logs.push((level.to_string(), msg.to_string()));
    }
    fn alert(&mut self, msg: &str) {
        self.logs.push(("alert".into(), msg.to_string()));
    }
    fn confirm(&mut self, _msg: &str) -> bool {
        true
    }
    fn prompt(&mut self, _msg: &str, default: &str) -> Option<String> {
        Some(default.to_string())
    }
    fn user_agent(&self) -> String {
        "RabbitHole/test".into()
    }
    fn title_changed(&mut self, title: &str) {
        self.title = Some(title.to_string());
    }
    fn now_ms(&self) -> f64 {
        self.now
    }
    fn random_bytes(&mut self, buf: &mut [u8]) {
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (i * 37 + 11) as u8;
        }
    }
    fn open_window(&mut self, url: Url) {
        self.opened.push(url.to_string());
    }
    fn focus_node(&mut self, node: Option<NodeId>) {
        self.focused = node;
    }
    fn submit_form(&mut self, form: NodeId, submitter: Option<NodeId>) {
        self.submitted.push((form, submitter));
    }
}

pub(crate) fn setup(html: &str) -> (MockEnv, ScriptHost) {
    let mut env = MockEnv::new(html);
    let host = ScriptHost::new(&mut env);
    (env, host)
}

/// Run a script and return the console.log lines it produced.
pub(crate) fn run(env: &mut MockEnv, host: &mut ScriptHost, src: &str) -> Vec<String> {
    let before = env.logs.len();
    let r = host.run_script(env, src, "test.js", false);
    if let Err(e) = &r {
        panic!("script error: {e}\nlogs: {:?}", env.logs);
    }
    host.run_jobs(env);
    env.logs[before..].iter().filter(|(l, _)| l == "log").map(|(_, m)| m.clone()).collect()
}

pub(crate) fn find(env: &MockEnv, tag: &str) -> NodeId {
    env.doc.find_element(tag).unwrap()
}

#[test]
fn query_and_basic_properties() {
    let (mut env, mut host) = setup(r#"<div id="a" class="x y"><p class="p1">one</p><p class="p2" data-foo-bar="baz">two</p><span>three</span></div>"#);
    let out = run(
        &mut env,
        &mut host,
        r#"
        const a = document.getElementById('a');
        console.log(a.tagName, a.id, a.className, a.children.length, a.childElementCount);
        console.log(document.querySelectorAll('#a > p').length, document.querySelector('p.p2').textContent);
        console.log(document.querySelectorAll('p:not(.p1)').length, a.querySelector('span').nodeName);
        console.log(document.querySelector('[data-foo-bar="baz"]').dataset.fooBar);
        console.log(a.firstElementChild.nextElementSibling.className, a.lastChild.previousSibling.className);
        console.log(a.classList.contains('y'), a.matches('div.x'), a.querySelector('p').closest('#a') === a);
        console.log(a.textContent, document.body.innerText.includes('two'));
        console.log(a instanceof HTMLDivElement, a instanceof Element, a instanceof Node, a instanceof EventTarget);
        console.log(Object.prototype.toString.call(a), document.documentElement.tagName, document.head !== null);
    "#,
    );
    assert_eq!(out[0], "DIV a x y 3 3");
    assert_eq!(out[1], "2 two");
    assert_eq!(out[2], "1 SPAN");
    assert_eq!(out[3], "baz");
    assert_eq!(out[4], "p2 p2");
    assert_eq!(out[5], "true true true");
    assert_eq!(out[6], "onetwothree true");
    assert_eq!(out[7], "true true true true");
    assert_eq!(out[8], "[object HTMLDivElement] HTML true");
}

#[test]
fn mutation_and_inner_html() {
    let (mut env, mut host) = setup(r#"<div id="root"></div>"#);
    let out = run(
        &mut env,
        &mut host,
        r#"
        const root = document.getElementById('root');
        const el = document.createElement('p');
        el.textContent = 'hello';
        el.setAttribute('title', 't');
        root.appendChild(el);
        const t = document.createTextNode(' world');
        el.appendChild(t);
        console.log(root.innerHTML);
        root.innerHTML = '<ul><li>a</li><li>b</li></ul><script>window.ran = true</script>';
        console.log(root.querySelectorAll('li').length, typeof window.ran, root.firstChild.tagName);
        root.firstChild.insertAdjacentHTML('beforeend', '<li>c</li>');
        root.firstChild.firstChild.remove();
        console.log(root.innerHTML);
        const frag = document.createDocumentFragment();
        frag.appendChild(document.createElement('i'));
        frag.append('txt', document.createElement('b'));
        root.append(frag);
        console.log(root.innerHTML, frag.childNodes.length);
        root.replaceChildren();
        console.log(root.childNodes.length, root.outerHTML);
        const clone = el.cloneNode(true);
        console.log(clone.outerHTML, clone.isConnected, el.contains(t), clone.contains(t));
        console.log(document.body.contains(root), root.parentNode === document.body, root.ownerDocument === document);
    "#,
    );
    assert_eq!(out[0], r#"<p title="t">hello world</p>"#);
    assert_eq!(out[1], "2 undefined UL");
    assert_eq!(out[2], "<ul><li>b</li><li>c</li></ul><script>window.ran = true</script>");
    assert_eq!(out[3], "<ul><li>b</li><li>c</li></ul><script>window.ran = true</script><i></i>txt<b></b> 0");
    assert_eq!(out[4], r#"0 <div id="root"></div>"#);
    assert_eq!(out[5], r#"<p title="t">hello world</p> false true false"#);
    assert_eq!(out[6], "true true true");
    assert!(env.dirty > 0);
    assert!(host.take_dirty());
    assert!(!host.dirty());
}

#[test]
fn class_list_style_and_attributes() {
    let (mut env, mut host) = setup(r#"<div id="d" class="a" style="color: blue; margin-top: 2px"></div>"#);
    let out = run(
        &mut env,
        &mut host,
        r#"
        const d = document.getElementById('d');
        d.classList.add('b', 'c'); d.classList.remove('a'); d.classList.toggle('z'); d.classList.toggle('z');
        console.log(d.getAttribute('class'), d.classList.length, d.classList.replace('b', 'q'), d.className);
        d.style.color = 'red';
        d.style.backgroundColor = 'green';
        d.style.setProperty('--x', '1');
        d.style.removeProperty('margin-top');
        console.log(d.getAttribute('style'));
        console.log(d.style.color, d.style.backgroundColor, d.style.getPropertyValue('background-color'), d.style.cssText);
        d.hidden = true; d.tabIndex = 3; d.dataset.testValue = 'v';
        console.log(d.hasAttribute('hidden'), d.getAttribute('tabindex'), d.getAttribute('data-test-value'), Object.keys(d.dataset).join(','));
        delete d.dataset.testValue;
        d.toggleAttribute('hidden');
        console.log(d.hasAttribute('data-test-value'), d.hidden, d.getAttributeNames().join('|'));
        const cs = getComputedStyle(d);
        console.log(cs.color, cs.display, cs.getPropertyValue('display'));
    "#,
    );
    assert_eq!(out[0], "b c 2 true q c");
    assert_eq!(out[1], "color: red; background-color: green; --x: 1;");
    assert_eq!(out[2], "red green green color: red; background-color: green; --x: 1;");
    assert_eq!(out[3], "true 3 v testValue");
    assert_eq!(out[4], "false false id|class|style|tabindex");
    assert_eq!(out[5], "rgb(255, 0, 0) block block");
}

#[test]
fn events_bubbling_capture_and_default() {
    let (mut env, mut host) = setup(r#"<div id="outer"><div id="inner"><button id="btn" onclick="window.inlineRan = (window.inlineRan||0)+1">x</button></div></div>"#);
    let out = run(
        &mut env,
        &mut host,
        r#"
        window.order = [];
        const outer = document.getElementById('outer'), inner = document.getElementById('inner'), btn = document.getElementById('btn');
        outer.addEventListener('click', e => order.push('outer-capture:' + e.eventPhase), true);
        outer.addEventListener('click', e => order.push('outer-bubble:' + e.eventPhase));
        inner.addEventListener('click', e => { order.push('inner:' + e.eventPhase + ':' + (e.target === btn) + ':' + (e.currentTarget === inner)); });
        btn.addEventListener('click', e => { order.push('btn:' + e.eventPhase); e.preventDefault(); }, { once: true });
        btn.addEventListener('click', e => order.push('btn2'));
        document.addEventListener('click', () => order.push('document'));
        window.addEventListener('click', () => order.push('window'));
        console.log('ready');
    "#,
    );
    assert_eq!(out, vec!["ready"]);
    let btn = find(&env, "button");
    let r = host.dispatch_event(&mut env, btn, "click", EventInit { bubbles: true, cancelable: true, detail: Some(EventDetail::Mouse { x: 1.0, y: 2.0, button: 0, ctrl: false, shift: false, alt: false, meta: false }) });
    assert!(r.default_prevented);
    assert!(r.handled);
    let order = run(&mut env, &mut host, "console.log(order.join(','), window.inlineRan)");
    assert_eq!(order[0], "outer-capture:1,btn:2,btn2,inner:3:true:true,outer-bubble:3,document,window 1");
    // Second dispatch: `once` listener gone, no preventDefault.
    let r = host.dispatch_event(&mut env, btn, "click", EventInit::simple(true, true));
    assert!(!r.default_prevented);
    let order = run(&mut env, &mut host, "console.log(order.length, window.inlineRan)");
    assert_eq!(order[0], "13 2");
    // stopPropagation stops bubbling.
    run(&mut env, &mut host, "order = []; inner.addEventListener('click', e => e.stopPropagation());");
    host.dispatch_event(&mut env, btn, "click", EventInit::simple(true, true));
    let order = run(&mut env, &mut host, "console.log(order.join(','))");
    assert_eq!(order[0], "outer-capture:1,btn2,inner:3:true:true");
    assert!(host.has_listeners(btn, "click"));
    assert!(!host.has_listeners(btn, "keydown"));
    assert!(env.errors().is_empty(), "{:?}", env.errors());
}

#[test]
fn dispatch_event_from_script_and_custom_events() {
    let (mut env, mut host) = setup(r#"<div id="d"></div>"#);
    let out = run(
        &mut env,
        &mut host,
        r#"
        const d = document.getElementById('d');
        let got = null;
        d.addEventListener('custom', e => { got = e.detail.x + ':' + e.bubbles + ':' + e.type + ':' + (e instanceof CustomEvent) + ':' + e.isTrusted; });
        const r = d.dispatchEvent(new CustomEvent('custom', { detail: { x: 42 }, bubbles: true, cancelable: true }));
        console.log(got, r);
        const et = new EventTarget();
        et.addEventListener('ping', e => console.log('ping', e.target === et));
        et.dispatchEvent(new Event('ping'));
        d.onclick = function (e) { console.log('onclick', this === d, e.clientX); return false; };
        const me = new MouseEvent('click', { clientX: 7, bubbles: true, cancelable: true });
        console.log(d.dispatchEvent(me), me.defaultPrevented, typeof d.onclick);
        const ke = new KeyboardEvent('keydown', { key: 'Enter' });
        console.log(ke.keyCode, ke.which, ke.key);
    "#,
    );
    assert_eq!(out[0], "42:true:custom:true:false true");
    assert_eq!(out[1], "ping true");
    assert_eq!(out[2], "onclick true 7");
    assert_eq!(out[3], "false true function");
    assert_eq!(out[4], "13 13 Enter");
}

/// Listeners of `dispatchEvent` (a framework's whole work loop, posted
/// through a MessageChannel) run with the collector enabled: their garbage
/// is collected (a news site's render grew the heap to millions of objects
/// and the process aborted on a 370 MB heap reallocation), and the event,
/// its path and the remaining listeners survive the collections.
#[test]
fn collection_runs_inside_dispatch_event_listeners() {
    let (mut env, mut host) = setup(r#"<div id="d"><span id="s"></span></div>"#);
    host.realm().gc_threshold = 1000;
    let out = run(
        &mut env,
        &mut host,
        r#"
        const s = document.getElementById('s');
        const log = [];
        s.addEventListener('work', e => { let t = 0; for (let i = 0; i < 300000; i++) { const o = { a: [i] }; t += o.a[0] & 1; } log.push('target:' + t + ':' + e.detail.k); });
        document.getElementById('d').addEventListener('work', e => log.push('bubble:' + (e.target === s) + ':' + e.detail.k));
        const et = new EventTarget();
        et.addEventListener('work', function () { let t = 0; for (let i = 0; i < 300000; i++) t += [i].length; log.push('et:' + t); });
        const r1 = s.dispatchEvent(new CustomEvent('work', { detail: { k: 'x' }, bubbles: true }));
        const r2 = et.dispatchEvent(new Event('work'));
        console.log(log.join(), r1, r2);
    "#,
    );
    assert_eq!(out[0], "target:150000:x,bubble:true:x,et:300000 true true");
    let slots = host.realm().heap.len();
    assert!(slots < 200_000, "heap grew to {slots} slots");
}

#[test]
fn timers_intervals_and_microtasks() {
    let (mut env, mut host) = setup("<div></div>");
    run(
        &mut env,
        &mut host,
        r#"
        window.log = [];
        setTimeout(() => log.push('t20'), 20);
        const id = setTimeout(() => log.push('never'), 5);
        clearTimeout(id);
        setTimeout((a, b) => log.push('t0:' + a + b), 0, 'x', 'y');
        let n = 0;
        const iv = setInterval(() => { n++; log.push('iv' + n); if (n === 3) clearInterval(iv); }, 10);
        Promise.resolve().then(() => log.push('micro'));
        requestAnimationFrame(ts => log.push('raf:' + (ts >= 0)));
        log.push('sync');
    "#,
    );
    // After run (jobs drained): sync + micro.
    let out = run(&mut env, &mut host, "console.log(log.join(','))");
    assert_eq!(out[0], "sync,micro");
    assert!(host.next_due().is_some());
    env.now = 1000.0;
    assert!(host.run_timers(&mut env, 1000.0)); // t0
    env.now = 1016.0;
    host.run_timers(&mut env, 1016.0); // iv1 at 1010, raf
    env.now = 1030.0;
    host.run_timers(&mut env, 1030.0); // t20, iv2 (1020)
    env.now = 1050.0;
    host.run_timers(&mut env, 1050.0); // iv3 (1030→ re-armed at 1026+10=1036... )
    env.now = 1100.0;
    host.run_timers(&mut env, 1100.0);
    let out = run(&mut env, &mut host, "console.log(log.join(','))");
    assert_eq!(out[0], "sync,micro,t0:xy,iv1,raf:true,t20,iv2,iv3");
    assert!(host.next_due().is_none());
}

#[test]
fn fetch_and_xhr() {
    let (mut env, mut host) = setup("<div></div>");
    env.fetches.insert("https://example.test/api/data.json".into(), (200, "application/json".into(), r#"{"ok":true,"n":3}"#.into()));
    env.fetches.insert("https://example.test/dir/rel.txt".into(), (404, "text/plain".into(), "nope".into()));
    let out = run(
        &mut env,
        &mut host,
        r#"
        fetch('/api/data.json').then(r => { console.log('status', r.status, r.ok, r.headers.get('X-Test')); return r.json(); }).then(j => console.log('json', j.ok, j.n));
        fetch('rel.txt').then(r => r.text()).then(t => console.log('rel', t));
        fetch('https://nowhere.test/').catch(e => console.log('err', e instanceof TypeError));
        const xhr = new XMLHttpRequest();
        xhr.open('GET', '/api/data.json', false);
        xhr.send();
        console.log('sync', xhr.status, xhr.readyState, JSON.parse(xhr.responseText).n, xhr.getResponseHeader('x-test'));
        const ax = new XMLHttpRequest();
        ax.open('POST', '/api/data.json');
        ax.onload = () => console.log('async', ax.status, ax.response.length > 0);
        ax.addEventListener('readystatechange', () => { if (ax.readyState === 4) console.log('rs4'); });
        ax.send('a=1');
        console.log('sent');
        const u = new URL('../x/y?z=1#h', 'https://a.b/c/d/e');
        console.log(u.href, u.pathname, u.search, u.hash, u.host, u.origin);
        u.searchParams.set('q', 'a b'); u.hash = '';
        console.log(u.toString(), [...new URLSearchParams('a=1&b=2&a=3').getAll('a')].join('|'), new URLSearchParams({x: 'y z'}).toString());
        console.log(btoa('hi'), atob('aGk='), new TextDecoder().decode(new TextEncoder().encode('ñ')));
    "#,
    );
    assert!(out.contains(&"status 200 true yes".to_string()), "{out:?}");
    assert!(out.contains(&"json true 3".to_string()));
    assert!(out.contains(&"rel nope".to_string()));
    assert!(out.contains(&"err true".to_string()));
    assert!(out.contains(&"sync 200 4 3 yes".to_string()));
    let sent_pos = out.iter().position(|l| l == "sent").unwrap();
    let async_pos = out.iter().position(|l| l == "async 200 true").unwrap();
    assert!(async_pos > sent_pos);
    assert!(out.contains(&"rs4".to_string()));
    assert!(out.contains(&"https://a.b/c/x/y?z=1#h /c/x/y ?z=1 #h a.b https://a.b".to_string()));
    assert!(out.contains(&"https://a.b/c/x/y?z=1&q=a+b 1|3 x=y+z".to_string()));
    assert!(out.contains(&"aGk= hi ñ".to_string()));
}

#[test]
fn storage_cookie_location_and_navigation() {
    let (mut env, mut host) = setup(r#"<a id="l" href="/go?x=1">go</a><form id="f" action="/post" method="post"><input name="q" value="v"><button id="s">ok</button></form>"#);
    let out = run(
        &mut env,
        &mut host,
        r#"
        localStorage.setItem('k', 'v'); localStorage.other = 'w'; sessionStorage.setItem('s', '1');
        console.log(localStorage.getItem('k'), localStorage.k, localStorage.length, Object.keys(localStorage).join(','), sessionStorage.length);
        localStorage.removeItem('k'); delete localStorage.other;
        console.log(localStorage.length, localStorage.getItem('k'));
        document.cookie = 'a=1; path=/';
        console.log(document.cookie);
        console.log(location.href, location.pathname, location.search, location.hash, location.origin, location.hostname);
        history.pushState({p: 1}, '', '/pushed?y=2');
        console.log(location.pathname, history.state.p, history.length);
        document.getElementById('l').click();
        document.getElementById('s').click();
        document.title = 'New title';
        console.log(document.title, document.readyState, document.URL.includes('pushed'));
        window.open('/popup');
    "#,
    );
    assert_eq!(out[0], "v v 2 k,other 1");
    assert_eq!(out[1], "0 null");
    assert_eq!(out[2], "a=1");
    assert_eq!(out[3], "https://example.test/dir/page.html?q=1#frag /dir/page.html ?q=1 #frag https://example.test example.test");
    assert_eq!(out[4], "/pushed 1 2");
    assert_eq!(env.navigations, vec![("https://example.test/go?x=1".to_string(), false)]);
    assert_eq!(env.submitted.len(), 1);
    assert_eq!(out[5], "New title loading true");
    assert_eq!(env.title.as_deref(), Some("New title"));
    assert_eq!(env.opened, vec!["https://example.test/popup".to_string()]);
    assert_eq!(host.location_href(&mut env), "https://example.test/pushed?y=2");
}

#[test]
fn forms_controls_reflect() {
    let (mut env, mut host) = setup(r#"<form id="f"><input id="t" name="t" value="a"><input id="c" type="checkbox" name="c"><input type="radio" name="r" value="1" id="r1" checked><input type="radio" name="r" value="2" id="r2"><select id="sel"><option>one</option><option value="2" selected>two</option></select><textarea id="ta">hello</textarea></form>"#);
    let out = run(
        &mut env,
        &mut host,
        r#"
        const t = document.getElementById('t'), c = document.getElementById('c'), sel = document.getElementById('sel'), ta = document.getElementById('ta');
        t.value = 'b';
        c.checked = true;
        document.getElementById('r2').checked = true;
        console.log(t.value, t.getAttribute('value'), c.checked, c.hasAttribute('checked'), document.getElementById('r1').checked, t.type, c.type, t.form.id);
        console.log(sel.value, sel.selectedIndex, sel.options.length, sel.options[1].text, ta.value);
        sel.value = 'one'; ta.value = 'changed';
        console.log(sel.value, sel.selectedIndex, ta.value, ta.textContent);
        const fd = new FormData(document.getElementById('f'));
        console.log([...fd.entries()].map(e => e.join('=')).join('&'));
        const f = document.getElementById('f');
        console.log(f.elements.length, f.elements.t === t, f.method, f.action.endsWith('page.html?q=1#frag'));
    "#,
    );
    // The value / checkedness changed; the attributes keep the defaults.
    assert_eq!(out[0], "b a true false false text checkbox f");
    assert_eq!(out[1], "2 1 2 two hello");
    assert_eq!(out[2], "one 0 changed hello");
    assert_eq!(out[3], "t=b&c=on&r=2");
    assert_eq!(out[4], "6 true get true");
}

#[test]
fn mutation_observer_and_dynamic_scripts() {
    let (mut env, mut host) = setup(r#"<div id="d"></div>"#);
    env.fetches.insert("https://example.test/dir/ext.js".into(), (200, "application/javascript".into(), "window.ext = 'loaded'".into()));
    let out = run(
        &mut env,
        &mut host,
        r#"
        const d = document.getElementById('d');
        const mo = new MutationObserver(records => {
            console.log('mo', records.length, records[0].type, records[0].addedNodes.length, records[0].target === d, records.map(r => r.attributeName).join(','));
        });
        mo.observe(d, { childList: true, attributes: true, subtree: true });
        d.appendChild(document.createElement('span'));
        d.setAttribute('x', '1');
        const s = document.createElement('script');
        s.textContent = 'window.dyn = 5';
        document.body.appendChild(s);
        console.log('dyn', window.dyn);
        const ext = document.createElement('script');
        ext.src = 'ext.js';
        ext.onload = () => console.log('ext load', window.ext);
        document.head.appendChild(ext);
        console.log('ext sync', window.ext);
    "#,
    );
    assert_eq!(out[0], "dyn 5");
    assert_eq!(out[1], "ext sync loaded");
    assert!(out.contains(&"ext load loaded".to_string()), "{out:?}");
    assert!(out.iter().any(|l| l.starts_with("mo 2 childList 1 true ,x")), "{out:?}");
}

#[test]
fn document_ready_order_and_write() {
    let (mut env, mut host) = setup(r#"<p id="p">x</p><script id="s"></script>"#);
    let script_node = find(&env, "script");
    host.set_current_script(Some(script_node));
    run(
        &mut env,
        &mut host,
        r#"
        window.seq = [];
        document.addEventListener('DOMContentLoaded', () => seq.push('dcl:' + document.readyState));
        window.addEventListener('load', () => seq.push('load:' + document.readyState));
        window.onload = () => seq.push('onload');
        document.write('<b>written</b>');
        seq.push('state:' + document.readyState);
        console.log(document.currentScript.id);
    "#,
    );
    host.set_current_script(None);
    host.document_ready(&mut env);
    let out = run(&mut env, &mut host, "console.log(seq.join(','), document.body.innerHTML)");
    assert_eq!(out[0], r#"state:loading,dcl:interactive,load:complete,onload <p id="p">x</p><script id="s"></script><b>written</b>"#);
}

#[test]
fn geometry_match_media_and_scroll() {
    let (mut env, mut host) = setup(r#"<div id="d"></div><html>"#);
    let d = find(&env, "div");
    env.rects.insert(d, Rect::new(10.0, 500.0, 100.0, 50.0));
    env.scroll = (0.0, 100.0);
    let out = run(
        &mut env,
        &mut host,
        r#"
        const d = document.getElementById('d');
        const r = d.getBoundingClientRect();
        console.log(r.x, r.top, r.width, r.bottom, d.offsetWidth, d.offsetHeight, JSON.stringify(r.toJSON()).length > 10);
        console.log(window.innerWidth, innerHeight, scrollY, matchMedia('(max-width: 1200px)').matches, matchMedia('(min-width: 1200px)').matches);
        d.scrollIntoView();
        console.log(window.scrollY);
        window.scrollTo(0, 42); console.log(scrollY, document.documentElement.scrollTop, document.documentElement.clientWidth);
        window.scrollBy({ top: 8 }); console.log(pageYOffset);
    "#,
    );
    assert_eq!(out[0], "10 400 100 450 100 50 true");
    assert_eq!(out[1], "1000 800 100 true false");
    assert_eq!(out[2], "500");
    assert_eq!(out[3], "42 42 1000");
    assert_eq!(out[4], "50");
}

#[test]
fn errors_are_logged_not_fatal() {
    let (mut env, mut host) = setup("<div id='d'></div>");
    assert!(host.run_script(&mut env, "null.x", "bad.js", false).is_err());
    assert!(env.errors().iter().any(|e| e.contains("Cannot read properties of null")));
    run(&mut env, &mut host, "document.getElementById('d').addEventListener('click', () => { throw new Error('boom'); }); Promise.reject(new Error('rej'));");
    let div = find(&env, "div");
    host.dispatch_event(&mut env, div, "click", EventInit::simple(true, true));
    assert!(env.errors().iter().any(|e| e.contains("boom")));
    assert!(env.errors().iter().any(|e| e.contains("rej")));
    let out = host.eval_for_console(&mut env, "1 + 2");
    assert_eq!(out, "3");
    let out = host.eval_for_console(&mut env, "nope");
    assert!(out.contains("nope is not defined"));
}

#[test]
fn todo_app_end_to_end() {
    let html = r#"
    <div id="app">
      <form id="form"><input id="new" name="new" placeholder="todo"><button type="submit" id="add">Add</button></form>
      <ul id="list"></ul>
      <p id="count">0 items</p>
      <button id="clear">Clear done</button>
    </div>"#;
    let (mut env, mut host) = setup(html);
    run(
        &mut env,
        &mut host,
        r#"
        (function () {
          const form = document.getElementById('form');
          const input = document.getElementById('new');
          const list = document.getElementById('list');
          const count = document.getElementById('count');
          const todos = [];
          function render() {
            list.innerHTML = '';
            todos.forEach((t, i) => {
              const li = document.createElement('li');
              li.className = t.done ? 'done' : '';
              li.dataset.index = i;
              const cb = document.createElement('input');
              cb.type = 'checkbox'; cb.checked = t.done;
              cb.addEventListener('change', () => { t.done = cb.checked; render(); });
              li.appendChild(cb);
              li.appendChild(document.createTextNode(' ' + t.text));
              list.appendChild(li);
            });
            const n = todos.filter(t => !t.done).length;
            count.textContent = n + (n === 1 ? ' item' : ' items') + ' left';
          }
          form.addEventListener('submit', e => {
            e.preventDefault();
            const text = input.value.trim();
            if (!text) return;
            todos.push({ text, done: false });
            input.value = '';
            render();
          });
          document.getElementById('clear').addEventListener('click', () => {
            for (let i = todos.length - 1; i >= 0; i--) if (todos[i].done) todos.splice(i, 1);
            render();
          });
          list.addEventListener('click', e => {
            if (e.target.tagName === 'LI') { const t = todos[e.target.dataset.index]; t.done = !t.done; render(); }
          });
          render();
        })();
    "#,
    );
    let input = find(&env, "input");
    let form = find(&env, "form");
    for text in ["Buy carrots", "Feed rabbits", "Write browser"] {
        env.doc.set_control_value(input, Some(text)); // the user types
        let r = host.dispatch_event(&mut env, form, "submit", EventInit::simple(true, true));
        assert!(r.default_prevented);
    }
    let out = run(&mut env, &mut host, "console.log(document.querySelectorAll('#list li').length, document.getElementById('count').textContent, document.getElementById('new').value === '')");
    assert_eq!(out[0], "3 3 items left true");
    // Toggle the second one via a click on the li, then clear done.
    let lis: Vec<NodeId> = env.doc.descendants(0).filter(|&n| env.doc.is_element(n, "li")).collect();
    host.dispatch_event(&mut env, lis[1], "click", EventInit::simple(true, true));
    let out = run(&mut env, &mut host, "console.log(document.querySelector('#list li.done') !== null, document.getElementById('count').textContent)");
    assert_eq!(out[0], "true 2 items left");
    // Checkbox change on the first item through the checkbox element.
    let first_cb = env.doc.descendants(0).find(|&n| env.doc.is_element(n, "input") && env.doc.attr(n, "type") == Some("checkbox")).unwrap();
    env.doc.set_checked(first_cb, Some(true)); // the user checks it
    host.dispatch_event(&mut env, first_cb, "change", EventInit::simple(true, false));
    let out = run(&mut env, &mut host, "console.log(document.querySelectorAll('#list li.done').length)");
    assert_eq!(out[0], "2");
    let add_btn = find(&env, "button");
    host.dispatch_event(&mut env, add_btn, "click", EventInit::simple(true, true)); // "Add" button – no-op (submit handled above)
    let clear = env.doc.element_by_id("clear").unwrap();
    host.dispatch_event(&mut env, clear, "click", EventInit::simple(true, true));
    let out = run(&mut env, &mut host, "console.log(document.querySelectorAll('#list li').length, document.getElementById('count').textContent, document.getElementById('list').innerHTML)");
    assert_eq!(out[0], r#"1 1 item left <li class="" data-index="0"><input type="checkbox"> Write browser</li>"#);
    assert!(env.errors().is_empty(), "{:?}", env.errors());
}

#[test]
fn misc_apis() {
    let (mut env, mut host) = setup(r#"<div id="d"><p>a</p><!-- c --><p>b</p></div><template id="t"><span>tpl</span></template>"#);
    let out = run(
        &mut env,
        &mut host,
        r#"
        const parsed = new DOMParser().parseFromString('<html><body><p class="x">hi</p></body></html>', 'text/html');
        console.log(parsed.querySelector('p.x').textContent, parsed.body.children.length, new XMLSerializer().serializeToString(parsed.body));
        const imported = document.importNode(parsed.querySelector('p'), true);
        document.getElementById('d').appendChild(imported);
        console.log(document.querySelectorAll('#d p').length);
        const w = document.createTreeWalker(document.getElementById('d'), NodeFilter.SHOW_ELEMENT);
        const names = []; let n; while ((n = w.nextNode())) names.push(n.tagName);
        console.log(names.join(','));
        const it = document.createNodeIterator(document.getElementById('d'), NodeFilter.SHOW_COMMENT);
        console.log(it.nextNode().nodeValue.trim());
        const tpl = document.getElementById('t');
        console.log(tpl.content.firstChild.tagName, tpl.content.nodeType);
        const ac = new AbortController();
        document.addEventListener('x', () => console.log('never'), { signal: ac.signal });
        ac.abort();
        document.dispatchEvent(new Event('x'));
        console.log(ac.signal.aborted, ac.signal.reason.name);
        class MyEl extends HTMLElement { connectedCallback() { this.textContent = 'custom!'; } }
        customElements.define('my-el', MyEl);
        const el = document.createElement('my-el');
        document.body.appendChild(el);
        console.log(el.textContent, customElements.get('my-el') === MyEl);
        console.log(typeof performance.now(), crypto.randomUUID().length, navigator.userAgent, typeof structuredClone({a:1}).a);
        try { document.querySelector('p:::bad'); } catch (e) { console.log(e.name, e instanceof DOMException, e instanceof Error); }
        const io = new IntersectionObserver(entries => console.log('io', entries.length, entries[0].isIntersecting));
        io.observe(document.getElementById('d'));
        console.log(CSS.supports('display', 'flex'), CSS.escape('1a'), document.compatMode, document.characterSet);
    "#,
    );
    assert_eq!(out[0], r#"hi 1 <body xmlns="http://www.w3.org/1999/xhtml"><p class="x">hi</p></body>"#);
    assert_eq!(out[1], "3");
    assert_eq!(out[2], "P,P,P");
    assert_eq!(out[3], "c");
    assert_eq!(out[4], "SPAN 11");
    assert_eq!(out[5], "true AbortError");
    assert_eq!(out[6], "custom! true");
    assert_eq!(out[7], "number 36 RabbitHole/test number");
    assert_eq!(out[8], "SyntaxError true true");
    // IntersectionObserver delivers at the next rendering update; the mock
    // has no layout box for #d, so it is not intersecting.
    host.run_timers(&mut env, 1100.0);
    assert!(env.logs.iter().any(|(l, m)| l == "log" && m == "io 1 false"), "{:?}", env.logs);
    // No DOCTYPE: quirks mode.
    assert!(out.contains(&"true \\31 a BackCompat UTF-8".to_string()), "{out:?}");
}

#[test]
fn realistic_vanilla_script() {
    let html = r##"<nav id="nav"><a href="#home" class="active">Home</a><a href="#about">About</a></nav>
    <main><section id="home" class="page">Home page</section><section id="about" class="page" hidden>About page</section></main>
    <div id="counter"><button id="inc">+</button><span id="n">0</span></div>
    <ul id="items"></ul><input id="search" placeholder="search">"##;
    let (mut env, mut host) = setup(html);
    run(
        &mut env,
        &mut host,
        r#"
        'use strict';
        const $ = sel => document.querySelector(sel);
        const $$ = sel => Array.from(document.querySelectorAll(sel));
        // Router via hashchange
        function show(id) {
          $$('.page').forEach(p => p.hidden = p.id !== id);
          $$('#nav a').forEach(a => a.classList.toggle('active', a.getAttribute('href') === '#' + id));
        }
        window.addEventListener('hashchange', () => show(location.hash.slice(1) || 'home'));
        // Counter with closures and dataset
        let n = 0;
        $('#inc').addEventListener('click', () => { n++; $('#n').textContent = String(n); $('#inc').dataset.count = n; });
        // List rendering with template literals + event delegation
        const data = ['alpha', 'beta', 'gamma', 'delta'];
        const items = $('#items');
        function renderItems(filter = '') {
          items.innerHTML = data.filter(d => d.includes(filter)).map((d, i) => `<li data-i="${i}" class="item">${d.toUpperCase()}</li>`).join('');
        }
        renderItems();
        items.addEventListener('click', e => {
          const li = e.target.closest('li.item');
          if (li) li.classList.toggle('selected');
        });
        $('#search').addEventListener('input', e => renderItems(e.target.value));
        // Async flow
        window.asyncDone = false;
        (async () => { await new Promise(r => setTimeout(r, 5)); await null; window.asyncDone = true; })();
        // Map/Set/JSON/regex
        const m = new Map([['a', 1]]); m.set('b', 2);
        window.summary = JSON.stringify({ keys: [...m.keys()], ok: /^al/.test(data[0]), n: data.length });
    "#,
    );
    let inc = env.doc.element_by_id("inc").unwrap();
    for _ in 0..3 {
        host.dispatch_event(&mut env, inc, "click", EventInit::simple(true, true));
    }
    let out = run(&mut env, &mut host, "console.log($('#n').textContent, $('#inc').dataset.count, window.summary, document.querySelectorAll('li.item').length)");
    assert_eq!(out[0], r#"3 3 {"keys":["a","b"],"ok":true,"n":4} 4"#);
    // Hash navigation from script queues hashchange (a task, so a listener
    // that sets the hash again cannot recurse) and routes; setting the
    // current fragment again does not navigate.
    let out = run(&mut env, &mut host, "window.hc = 0; addEventListener('hashchange', () => { hc++; location.hash = location.hash; }); location.hash = '#about'; location.hash = '#about'; console.log(hc)");
    assert_eq!(out[0], "0");
    let now = env.now;
    host.run_timers(&mut env, now);
    let out = run(&mut env, &mut host, "console.log($('#about').hidden, $('#home').hidden, $('#nav a.active').textContent, hc)");
    assert_eq!(out[0], "false true About 1");
    // Delegated click.
    let lis: Vec<NodeId> = env.doc.descendants(0).filter(|&n| env.doc.is_element(n, "li")).collect();
    host.dispatch_event(&mut env, lis[2], "click", EventInit::simple(true, true));
    let out = run(&mut env, &mut host, "console.log($$('li.selected').map(l => l.textContent).join())");
    assert_eq!(out[0], "GAMMA");
    // Input event filtering.
    let search = env.doc.element_by_id("search").unwrap();
    env.doc.set_attr(search, "value", Some("a"));
    host.dispatch_event(&mut env, search, "input", EventInit { bubbles: true, cancelable: false, detail: Some(EventDetail::Input { data: Some("a".into()), input_type: "insertText".into(), is_composing: false }) });
    let out = run(&mut env, &mut host, "console.log($$('li.item').map(l => l.textContent).join())");
    assert_eq!(out[0], "ALPHA,BETA,GAMMA,DELTA");
    env.doc.set_attr(search, "value", Some("et"));
    host.dispatch_event(&mut env, search, "input", EventInit::simple(true, false));
    let out = run(&mut env, &mut host, "console.log($$('li.item').map(l => l.textContent).join())");
    assert_eq!(out[0], "BETA");
    env.now = 2000.0;
    host.run_timers(&mut env, 2000.0);
    let out = run(&mut env, &mut host, "console.log(window.asyncDone)");
    assert_eq!(out[0], "true");
    assert!(env.errors().is_empty(), "{:?}", env.errors());
}

/// Run a module script (inline, at the document URL) and return its
/// console.log lines plus the result.
fn run_module(env: &mut MockEnv, host: &mut ScriptHost, src: &str) -> (Vec<String>, Result<(), String>) {
    let before = env.logs.len();
    let url = env.url.to_string();
    let r = host.run_script(env, src, &url, true);
    host.run_jobs(env);
    let logs = env.logs[before..].iter().filter(|(l, _)| l == "log").map(|(_, m)| m.clone()).collect();
    (logs, r)
}

#[test]
fn module_script_with_relative_imports() {
    let (mut env, mut host) = setup("<div id=out></div>");
    env.fetches.insert("https://example.test/dir/lib/util.js".into(), (200, "text/javascript".into(), "import { x } from '../shared.js'; export const greet = n => 'hi ' + n + ' ' + x; export default 7;".into()));
    env.fetches.insert("https://example.test/dir/shared.js".into(), (200, "text/javascript".into(), "export let x = 'shared'; export function setX(v) { x = v; }".into()));
    let (out, r) = run_module(
        &mut env,
        &mut host,
        "import d, { greet } from './lib/util.js'; import { setX } from './shared.js';\n\
         console.log(greet('mod'), d, this === undefined, typeof window.greet);\n\
         setX('changed'); console.log(greet('again'));\n\
         console.log(import.meta.url, import.meta.resolve('./lib/x.js'));\n\
         import('./lib/util.js').then(ns => console.log('dynamic', ns.default));\n\
         import('./nope.js').catch(e => console.log(e.name + ': ' + e.message));",
    );
    assert!(r.is_ok(), "{r:?} {:?}", env.logs);
    assert_eq!(out[0], "hi mod shared 7 true undefined");
    assert_eq!(out[1], "hi again changed");
    assert_eq!(out[2], "https://example.test/dir/page.html?q=1#frag https://example.test/dir/lib/x.js");
    assert!(out.contains(&"dynamic 7".to_string()), "{out:?}");
    assert!(out.contains(&"TypeError: Failed to fetch dynamically imported module: https://example.test/dir/nope.js".to_string()), "{out:?}");

    // A failing static import is logged with the specifier and URL.
    let (_, r) = run_module(&mut env, &mut host, "import { y } from './missing.js';");
    assert!(r.is_err());
    let errors = env.errors();
    assert!(errors.iter().any(|e| e.contains("\"./missing.js\"") && e.contains("https://example.test/dir/missing.js")), "{errors:?}");
    // Missing export → SyntaxError naming the module and export.
    let (_, r) = run_module(&mut env, &mut host, "import { nothing } from './shared.js';");
    assert!(r.unwrap_err().contains("SyntaxError: The requested module './shared.js' does not provide an export named 'nothing'"));

    // Dynamically inserted module scripts run through the same loader (and cache).
    let out = run(
        &mut env,
        &mut host,
        "const s = document.createElement('script'); s.type = 'module'; s.textContent = \"import d from './lib/util.js'; console.log('inserted', d);\"; document.body.appendChild(s);",
    );
    assert_eq!(out, vec!["inserted 7".to_string()]);
}

#[test]
fn import_maps_resolve_bare_specifiers() {
    let html = r#"<script type="importmap">{"imports":{"lodash":"/vendor/lodash.js","lib/":"./libs/"},"scopes":{"/dir/scoped/":{"lodash":"/vendor/lodash2.js"}}}</script><p>x</p>"#;
    let (mut env, mut host) = setup(html);
    env.fetches.insert("https://example.test/vendor/lodash.js".into(), (200, "text/javascript".into(), "export default 'lodash1';".into()));
    env.fetches.insert("https://example.test/vendor/lodash2.js".into(), (200, "text/javascript".into(), "export default 'lodash2';".into()));
    env.fetches.insert("https://example.test/dir/libs/a.js".into(), (200, "text/javascript".into(), "export const a = 'A';".into()));
    env.fetches.insert("https://example.test/dir/scoped/m.js".into(), (200, "text/javascript".into(), "import l from 'lodash'; export default l;".into()));
    let (out, r) = run_module(&mut env, &mut host, "import l from 'lodash'; import { a } from 'lib/a.js'; import s from './scoped/m.js'; console.log(l, a, s, import.meta.resolve('lodash'));");
    assert!(r.is_ok(), "{r:?} {:?}", env.logs);
    assert_eq!(out, vec!["lodash1 A lodash2 https://example.test/vendor/lodash.js".to_string()]);

    let (_, r) = run_module(&mut env, &mut host, "import x from 'not-mapped';");
    let e = r.unwrap_err();
    assert!(e.contains(r#"TypeError: Failed to resolve module specifier "not-mapped". Relative references must start with either "/", "./", or "../"."#), "{e}");

    // The embedder can install a map explicitly.
    let (mut env, mut host) = setup("<p>x</p>");
    env.fetches.insert("https://example.test/pkg/index.js".into(), (200, "text/javascript".into(), "export const v = 'pkg';".into()));
    host.set_import_map(r#"{"imports":{"pkg":"https://example.test/pkg/index.js"}}"#).unwrap();
    let (out, r) = run_module(&mut env, &mut host, "import { v } from 'pkg'; console.log(v);");
    assert!(r.is_ok(), "{r:?}");
    assert_eq!(out, vec!["pkg".to_string()]);
    assert!(host.set_import_map("not json").is_err());
}

#[test]
fn json_and_css_modules() {
    let (mut env, mut host) = setup("<p>x</p>");
    env.fetches.insert("https://example.test/dir/data.json".into(), (200, "application/json".into(), r#"{"a":[1,2],"name":"data"}"#.into()));
    env.fetches.insert("https://example.test/dir/s.css".into(), (200, "text/css".into(), "p { color: red }".into()));
    env.fetches.insert("https://example.test/dir/bad.json".into(), (200, "application/json".into(), "{nope".into()));
    let (out, r) = run_module(
        &mut env,
        &mut host,
        "import data from './data.json' with { type: 'json' }; import sheet from './s.css' with { type: 'css' };\nconsole.log(data.a.length, data.name, typeof sheet);",
    );
    assert!(r.is_ok(), "{r:?} {:?}", env.logs);
    assert_eq!(out, vec!["2 data object".to_string()]);
    let (_, r) = run_module(&mut env, &mut host, "import bad from './bad.json' with { type: 'json' };");
    assert!(r.unwrap_err().contains("SyntaxError"));
}

#[test]
fn custom_element_constructors_run() {
    let mut env = MockEnv::new("<html><body><x-old></x-old></body></html>");
    let mut host = ScriptHost::new(&mut env);
    host.run_script(
        &mut env,
        r#"
        const log = [];
        const priv = new WeakMap();
        class XEl extends HTMLElement {
          static get observedAttributes() { return ['src']; }
          #secret = 41;
          constructor() { super(); this.delegate = { connect: () => log.push('connect:' + this.localName) }; priv.set(this, 1); }
          connectedCallback() { this.delegate.connect(); log.push('secret:' + (this.#secret + 1)); }
          attributeChangedCallback(n, o, v) { log.push('attr:' + n + '=' + v); }
        }
        document.querySelector('x-old').setAttribute('src', 'a.html');
        customElements.define('x-old', XEl);
        customElements.define('x-new', class extends XEl {});
        const made = document.createElement('x-new');
        log.push('made:' + (made instanceof XEl) + ':' + priv.has(made) + ':' + (typeof made.delegate));
        document.body.appendChild(made);
        const viaNew = new XEl();
        log.push('new:' + (viaNew instanceof HTMLElement) + ':' + viaNew.localName);
        window.result = log.join(',');
        "#,
        "t.js",
        false,
    )
    .unwrap();
    let out = host.eval_for_console(&mut env, "result");
    assert_eq!(out, "'attr:src=a.html,connect:x-old,secret:42,made:true:true:object,connect:x-new,secret:42,new:true:x-old'");
}

#[test]
fn dommatrix_meta_and_node_classes() {
    let mut env = MockEnv::new(r#"<html><head><meta name="theme" content="dark"></head><body></body></html>"#);
    let mut host = ScriptHost::new(&mut env);
    host.run_script(
        &mut env,
        r#"
        const m = new DOMMatrix('translate(10px, 20px) scale(2)');
        const p = m.transformPoint({ x: 1, y: 1 });
        const inv = m.inverse().multiply(m);
        const meta = document.querySelector('meta');
        window.result = [m.toString(), p.x + ',' + p.y, inv.isIdentity, new DOMMatrix().is2D, typeof CDATASection.prototype, typeof ProcessingInstruction, meta.name + '=' + meta.content].join('|');
        "#,
        "t.js",
        false,
    )
    .unwrap();
    assert_eq!(host.eval_for_console(&mut env, "result"), "'matrix(2, 0, 0, 2, 10, 20)|12,22|true|true|object|function|theme=dark'");
}

#[test]
fn inner_html_on_script_is_raw_text() {
    let mut env = MockEnv::new("<html><head></head><body></body></html>");
    let mut host = ScriptHost::new(&mut env);
    host.run_script(
        &mut env,
        r#"
        const s = document.createElement('script');
        s.innerHTML = 'for (var r = 0; r < 3; r++) { window.count = (window.count || 0) + 1 } window.amp = 1 && 2;';
        document.head.appendChild(s);
        const t = document.createElement('textarea');
        t.innerHTML = 'a &amp; <b>b</b>';
        window.result = [window.count, window.amp, t.value, typeof DocumentType].join('|');
        "#,
        "t.js",
        false,
    )
    .unwrap();
    assert_eq!(host.eval_for_console(&mut env, "result"), "'3|2|a & <b>b</b>|function'");
}

#[test]
fn body_onload_attribute_is_the_window_load_handler() {
    // Acid3 starts from `<body onload="update()">`.
    let (mut env, mut host) = setup(r#"<body onload="window.loaded = (window.loaded || 0) + 1; window.evType = event.type"><p>x</p></body>"#);
    run(&mut env, &mut host, "window.loaded = 0;");
    host.document_ready(&mut env);
    let out = run(&mut env, &mut host, "console.log(window.loaded, window.evType)");
    assert_eq!(out[0], "1 load");
}

#[test]
fn node_iterator_starts_at_root_and_table_dom_api() {
    let (mut env, mut host) = setup(r#"<div id="r"><b>x</b><i>y</i></div>"#);
    let out = run(&mut env, &mut host, r#"
        const it = document.createNodeIterator(document.getElementById('r'), NodeFilter.SHOW_ELEMENT, null, true);
        const seq = [it.nextNode(), it.nextNode(), it.nextNode(), it.nextNode(), it.previousNode(), it.previousNode()].map((n) => n ? n.tagName : 'null');
        const t = document.createElement('table');
        const cap = t.createCaption(), th = t.createTHead(), tf = t.createTFoot();
        const r1 = t.insertRow(0); const r2 = th.insertRow(0);
        console.log(seq.join(','), t.caption === cap, t.tHead === th, t.tFoot === tf, t.rows.length, t.rows[0] === r2, r1.rowIndex, r1.sectionRowIndex, t.tBodies.length);
        t.deleteCaption(); t.deleteTHead(); t.deleteTFoot();
        console.log(t.childNodes.length, !!t.caption);
    "#);
    assert_eq!(out[0], "DIV,B,I,null,I,B true true true 2 true 1 0 1");
    assert_eq!(out[1], "1 false");
}

fn sleep_ms(_realm: &mut rabbithole_js::Realm, _this: rabbithole_js::Value, args: &[rabbithole_js::Value]) -> rabbithole_js::JsResult<rabbithole_js::Value> {
    let ms = args.first().and_then(|v| v.as_number()).unwrap_or(0.0);
    std::thread::sleep(std::time::Duration::from_millis(ms as u64));
    Ok(rabbithole_js::Value::Undefined)
}

fn timeouts(env: &MockEnv) -> usize {
    env.errors().iter().filter(|e| e.contains("Script execution timed out")).count()
}

/// Run timers due at `now` until none is left due (yielded runs included).
fn drain_timers(env: &mut MockEnv, host: &mut ScriptHost, now: f64) -> usize {
    env.now = now;
    let mut calls = 0;
    while host.next_due().map(|d| d <= now).unwrap_or(false) && calls < 50 {
        host.run_timers(env, now);
        host.run_jobs(env);
        calls += 1;
    }
    calls
}

#[test]
fn runaway_timer_is_stopped_alone() {
    let (mut env, mut host) = setup("<div></div>");
    host.realm().set_task_budget(Some(std::time::Duration::from_millis(150)));
    run(&mut env, &mut host, r#"
        window.log = [];
        setTimeout(() => log.push('a'), 0);
        setTimeout(() => { while (true) {} }, 0);
        setTimeout(() => { Promise.resolve().then(() => log.push('micro')); log.push('c'); }, 0);
        setTimeout(() => { try { while (true) {} } finally { log.push('finally must not run'); } }, 0);
        setTimeout(() => log.push('d'), 0);
    "#);
    drain_timers(&mut env, &mut host, 2000.0);
    let out = run(&mut env, &mut host, "console.log(log.join(','))");
    assert_eq!(out[0], "a,c,micro,d");
    assert_eq!(timeouts(&env), 2, "{:?}", env.errors());
}

#[test]
fn runaway_listener_does_not_stop_the_next_one() {
    let (mut env, mut host) = setup("<div id=d></div>");
    host.realm().set_task_budget(Some(std::time::Duration::from_millis(150)));
    run(&mut env, &mut host, r#"
        window.log = [];
        const d = document.getElementById('d');
        d.addEventListener('click', () => { while (true) {} });
        d.addEventListener('click', () => log.push('second'));
        // A runaway loop of nested dispatches must not restart its budget.
        d.addEventListener('ping', () => {});
        setTimeout(() => { for (;;) d.dispatchEvent(new Event('ping')); }, 0);
        setTimeout(() => log.push('after'), 0);
    "#);
    let div = find(&env, "div");
    host.realm().begin_task();
    host.dispatch_event(&mut env, div, "click", EventInit::simple(true, true));
    host.run_jobs(&mut env);
    let t = std::time::Instant::now();
    drain_timers(&mut env, &mut host, 2000.0);
    assert!(t.elapsed() < std::time::Duration::from_secs(3));
    let out = run(&mut env, &mut host, "console.log(log.join(','))");
    assert_eq!(out[0], "second,after");
    assert_eq!(timeouts(&env), 2, "{:?}", env.errors());
}

#[test]
fn long_timer_runs_yield_and_resume() {
    let (mut env, mut host) = setup("<div></div>");
    {
        let realm = host.realm();
        let g = realm.global();
        realm.define_method(g, "sleepMs", 1, sleep_ms);
    }
    run(&mut env, &mut host, r#"
        window.done = 0;
        for (let i = 0; i < 6; i++) setTimeout(() => { sleepMs(40); done++; }, 0);
    "#);
    env.now = 2000.0;
    host.run_timers(&mut env, 2000.0);
    let first = run(&mut env, &mut host, "console.log(done)")[0].parse::<u32>().unwrap();
    assert!((1..6).contains(&first), "ran {first} timers in one slice");
    assert!(host.next_due().unwrap() <= 2000.0, "the rest stay due");
    drain_timers(&mut env, &mut host, 2000.0);
    let out = run(&mut env, &mut host, "console.log(done)");
    assert_eq!(out[0], "6");
    assert_eq!(timeouts(&env), 0);
}

#[test]
fn doctype_node_compat_mode_and_xml_documents() {
    let (mut env, mut host) = setup(r#"<!DOCTYPE HTML PUBLIC "-//W3C//DTD HTML 4.01//EN" "http://www.w3.org/TR/html4/strict.dtd"><p>x</p>"#);
    let out = run(&mut env, &mut host, r#"
        const dt = document.firstChild;
        console.log(dt.nodeType, dt.name, dt.publicId, dt === document.doctype, document.compatMode, dt.textContent, dt.nextSibling === document.documentElement);
        const doctype = document.implementation.createDocumentType('html', '-//W3C//DTD XHTML 1.0 Strict//EN', 'x');
        const doc = document.implementation.createDocument('http://www.w3.org/1999/xhtml', 'html', doctype);
        doc.documentElement.appendChild(doc.createElementNS('http://www.w3.org/1999/xhtml', 'head'));
        doc.documentElement.appendChild(doc.createElementNS('http://www.w3.org/1999/xhtml', 'body'));
        const t = doc.createElementNS('http://www.w3.org/1999/xhtml', 'title');
        doc.documentElement.firstChild.appendChild(t);
        t.textContent = 'Sparrow';
        doc.body.appendChild(doc.createElementNS('http://www.w3.org/1999/xhtml', 'form'));
        console.log(String(doc), doctype.ownerDocument === doc, doc.firstChild === doctype, doc.title, doc.forms.length, doc.documentElement.tagName);
        let code = 0;
        // Doctype names are only checked for whitespace, NUL and '>' (whatwg/dom#1079).
        document.implementation.createDocumentType('a:', '', '');
        try { document.implementation.createDocumentType('a:>', '', ''); } catch (e) { code = e.code === e.INVALID_CHARACTER_ERR && e.INVALID_ACCESS_ERR === 15 ? 5 : -1; }
        console.log(code);
    "#);
    assert_eq!(out[0], "10 html -//W3C//DTD HTML 4.01//EN true CSS1Compat null true");
    assert_eq!(out[1], "[object XMLDocument] true true Sparrow 1 html");
    assert_eq!(out[2], "5");
    let (mut env, mut host) = setup("<p>no doctype</p>");
    let out = run(&mut env, &mut host, "console.log(document.compatMode, document.doctype)");
    assert_eq!(out[0], "BackCompat null");
}

#[test]
fn document_children_are_validated_and_namespaces_kept() {
    let (mut env, mut host) = setup("<!doctype html><p>x</p>");
    let out = run(&mut env, &mut host, r#"
        const codes = [];
        const t = (f) => { try { f(); codes.push(0); } catch (e) { codes.push(e.code); } };
        t(() => document.appendChild(document.createElement('a')));
        t(() => document.appendChild(document.createTextNode('x')));
        t(() => document.body.appendChild(document.implementation.createDocumentType('html', '', '')));
        t(() => document.appendChild(document.createComment('ok')));
        t(() => document.createElementNS(null, 'd:iv'));
        t(() => document.createElementNS('http://example.com/', 'xmlns:test'));
        t(() => document.createElementNS('http://www.w3.org/2000/xmlns/', 'x:test'));
        t(() => document.createElementNS('http://www.w3.org/2000/xmlns/', 'xmlns:test'));
        t(() => document.createElementNS(null, ':div'));
        t(() => document.createElementNS(null, '0div'));
        const e = document.createElementNS('http://ns.example.com/', 'prefix:localName');
        console.log(codes.join(','), e.tagName, e.localName, e.prefix, e.namespaceURI, document.createElement('DIV').tagName);
    "#);
    assert_eq!(out[0], "3,3,3,0,14,14,14,0,5,5 prefix:localName localName prefix http://ns.example.com/ DIV");
}

#[test]
fn live_ranges_follow_mutations() {
    let (mut env, mut host) = setup("<body></body>");
    let out = run(&mut env, &mut host, r#"
        const p = document.createElement('p');
        p.appendChild(document.createTextNode('12345'));
        document.body.appendChild(p);
        const r = document.createRange();
        r.setEnd(document.body, 1);
        r.setStart(p.firstChild, 2);
        document.body.removeChild(p);
        const a = [r.collapsed, r.startContainer === document.body, r.startOffset, r.endOffset];
        // insertNode splits the text node; the range keeps the inserted node.
        const q = document.createElement('p');
        const t1 = document.createTextNode('12345'), t2 = document.createTextNode('ABCDE');
        q.appendChild(t1); q.appendChild(t2); document.body.appendChild(q);
        const r2 = document.createRange();
        r2.setStart(t1, 2); r2.setEnd(t1, 3);
        r2.insertNode(t2);
        const b = [q.childNodes.length, t1.data, q.childNodes[2].data, r2.toString()];
        // extractContents moves fully contained nodes.
        const h = document.createElement('h1');
        const x1 = document.createTextNode('Hello '), em = document.createElement('em'), x2 = document.createTextNode('Wonderful'), x3 = document.createTextNode(' Kitty');
        em.appendChild(x2); h.appendChild(x1); h.appendChild(em); h.appendChild(x3);
        document.body.appendChild(h);
        const r3 = document.createRange();
        r3.setStart(x2, 6); r3.setEnd(h, 3);
        const f = r3.extractContents();
        const c = [f.childNodes[0].firstChild.data, f.childNodes[1] === x3, h.textContent, r3.startContainer === h, r3.startOffset];
        let code = 0;
        try { r3.setEndBefore(document); } catch (e) { code = e.code === e.INVALID_NODE_TYPE_ERR ? 24 : -1; }
        console.log(a.join(','), b.join(','), c.join(','), code);
    "#);
    assert_eq!(out[0], "true,true,0,0 3,12,345,ABCDE3 ful,true,Hello Wonder,true,2 24");
}

#[test]
fn node_iterator_survives_removal_by_its_filter() {
    let (mut env, mut host) = setup("<body></body>");
    let out = run(&mut env, &mut host, r#"
        const b = document.body;
        const [t1, t2, t3, t4] = [1, 2, 3, 4].map((i) => b.appendChild(document.createElement('t' + i)));
        const seen = [];
        let removing = null;
        const it = document.createNodeIterator(b, NodeFilter.SHOW_ELEMENT, (n) => { seen.push(n.tagName); if (n === removing) { b.removeChild(n); removing = null; return 2; } return 1; });
        it.nextNode(); it.nextNode(); it.nextNode(); it.nextNode(); it.nextNode();
        removing = t4;
        const prev = it.previousNode();
        console.log(prev && prev.tagName, seen.join(','), it.referenceNode.tagName, it.pointerBeforeReferenceNode);
    "#);
    assert_eq!(out[0], "T3 BODY,T1,T2,T3,T4,T4,T3 T3 true");
}

#[test]
fn input_value_and_checkedness_are_separate_from_attributes() {
    let (mut env, mut host) = setup("<form><input name=a value=def><input type=checkbox id=c checked><input type=radio name=r id=r1><input type=radio name=r id=r2></form>");
    let out = run(&mut env, &mut host, r#"
        const i = document.createElement('input');
        i.type = 'text'; i.value = 'test';
        const a = document.querySelector('[name=a]');
        a.value = 'typed';
        const c = document.getElementById('c');
        c.checked = false;
        const r1 = document.getElementById('r1'), r2 = document.getElementById('r2');
        r1.checked = true;
        r2.setAttribute('checked', 'checked');
        console.log(i.hasAttribute('value'), i.value, a.value, a.defaultValue, a.getAttribute('value'), c.checked, c.hasAttribute('checked'), r1.checked, r2.checked,
            document.querySelectorAll(':checked').length);
        document.forms[0].reset();
        console.log(a.value, c.checked, r1.checked, r2.checked);
    "#);
    assert_eq!(out[0], "false test typed def def false true true false 1");
    assert_eq!(out[1], "def true false true");
}

#[test]
fn document_open_write_close_replaces_the_document() {
    let (mut env, mut host) = setup("<!doctype html><p>x</p>");
    let out = run(&mut env, &mut host, r#"
        const doc = document.implementation.createHTMLDocument('t');
        doc.open();
        doc.write('<!DOCTYPE HTML PUBLIC "-//W3C//DTD HTML 4.0 Transitional//EN"><title></title><span></span>');
        doc.close();
        console.log(doc.childNodes.length, doc.firstChild.publicId, doc.compatMode, doc.body.firstChild.tagName);
    "#);
    assert_eq!(out[0], "2 -//W3C//DTD HTML 4.0 Transitional//EN BackCompat SPAN");
}
