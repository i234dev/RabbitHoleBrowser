//! Several realms on one thread (a page and its frames, or the previous
//! and the next page of a tab): document numbers are per realm (0 = the
//! realm's own document, 1, 2, ... = its `DOMParser` / `createHTMLDocument`
//! documents), so no state keyed by them may be shared between realms.
//! And a Rust panic in a binding is a JS exception, not the end of the tab.

use crate::tests::{run, setup};

/// Ranges of a realm that goes away (an iframe removed, the previous page
/// of the tab) released their live slots through a thread-wide list: the
/// next realm to create a Range "released" them in its own documents —
/// another realm's slots in its main document (breaking its live ranges),
/// and slots of `DOMParser` documents it doesn't have (index out of
/// bounds: the tab crashed).
#[test]
fn dropped_realm_does_not_release_live_ranges_of_another() {
    let (mut env_b, mut host_b) = setup("<p id=x>hello</p>");
    run(&mut env_b, &mut host_b, "globalThis.r1 = document.createRange(); const t = document.getElementById('x').firstChild; r1.setStart(t, 1); r1.setEnd(t, 3);");
    {
        let (mut env_a, mut host_a) = setup("<p>a</p>");
        run(
            &mut env_a,
            &mut host_a,
            r#"
            for (let i = 0; i < 4; i++) { const r = document.createRange(); r.selectNodeContents(document.body); }
            const d = new DOMParser().parseFromString('<p>x</p><p>y</p>', 'text/html');
            const d2 = document.implementation.createHTMLDocument('t');
            for (let i = 0; i < 4; i++) { const r = d.createRange(); r.selectNodeContents(d.body); const r2 = d2.createRange(); r2.selectNodeContents(d2.body); }
            const it = d.createNodeIterator(d.body);
            it.nextNode();
            "#,
        );
        drop(host_a);
    }
    let out = run(
        &mut env_b,
        &mut host_b,
        r#"
        const r2 = document.createRange();
        r2.selectNodeContents(document.body);
        const it = document.createNodeIterator(document.body);
        console.log(r1.startContainer.nodeName, r1.startOffset, r1.endOffset, r1.toString());
        "#,
    );
    assert_eq!(out, vec!["#text 1 3 el".to_string()]);
    assert!(env_b.errors().is_empty(), "{:?}", env_b.errors());
}

/// `document.open()` without `close()` in one realm left the document
/// "open" for every realm of the thread: another realm's `write` to its
/// document with the same number replaced it with the first realm's markup.
#[test]
fn open_documents_are_per_realm() {
    let (mut env_a, mut host_a) = setup("<p>a</p>");
    run(&mut env_a, &mut host_a, "const d = document.implementation.createHTMLDocument('t'); d.open(); d.write('<p>from a</p>');");
    let (mut env_b, mut host_b) = setup("<p>b</p>");
    let out = run(
        &mut env_b,
        &mut host_b,
        "const d = document.implementation.createHTMLDocument('t'); d.write('<i>from b</i>'); console.log(d.body.innerHTML);",
    );
    assert_eq!(out, vec!["<i>from b</i>".to_string()]);
    // The first realm's open document still collects its own writes.
    let out = run(&mut env_a, &mut host_a, "const e = document.implementation.createHTMLDocument('u'); e.write('<b>e</b>'); console.log(e.body.innerHTML);");
    assert_eq!(out, vec!["<b>e</b>".to_string()]);
}

fn panic_native(_: &mut rabbithole_js::Realm, _: rabbithole_js::Value, args: &[rabbithole_js::Value]) -> rabbithole_js::JsResult<rabbithole_js::Value> {
    let v: Vec<u8> = Vec::new();
    Ok(rabbithole_js::Value::Number(v[args.len() + 7] as f64))
}

/// A panicking host function (a binding bug) throws a JS `Error` that the
/// page can catch; uncaught, it is reported like any script error, and the
/// page's scripts, listeners and timers keep working.
#[test]
fn panicking_binding_becomes_a_js_exception() {
    let (mut env, mut host) = setup("<button id=b>go</button>");
    {
        let realm = host.realm();
        let g = realm.global();
        realm.define_method(g, "__testPanic", 0, panic_native);
    }
    let out = run(
        &mut env,
        &mut host,
        r#"
        let caught = '';
        try { __testPanic(); } catch (e) { caught = e.message; }
        console.log(caught.startsWith('internal error: index out of bounds'));
        document.getElementById('b').addEventListener('click', () => { __testPanic(); });
        document.getElementById('b').addEventListener('click', () => { console.log('second listener ran'); });
        setTimeout(() => console.log('timer ran', document.querySelectorAll('button').length), 0);
        "#,
    );
    assert_eq!(out, vec!["true".to_string()]);
    let errors = env.errors();
    assert!(errors.iter().any(|e| e.contains("internal error in __testPanic()") && e.contains("tests_realms.rs")), "{errors:?}");
    let before = env.logs.len();
    let b = env.doc.find_element("button").unwrap();
    host.dispatch_event(&mut env, b, "click", crate::EventInit::simple(true, true));
    env.now += 10.0;
    let now = env.now;
    host.run_timers(&mut env, now);
    let logs: Vec<String> = env.logs[before..].iter().map(|(l, m)| format!("{l}: {m}")).collect();
    assert!(logs.iter().any(|l| l == "log: second listener ran"), "{logs:?}");
    assert!(logs.iter().any(|l| l == "log: timer ran 1"), "{logs:?}");
    assert!(logs.iter().any(|l| l.starts_with("error:") && l.contains("internal error")), "{logs:?}");
}
