//! Service worker lifecycle runs a real script in a separate worker Realm.
use crate::tests::{run, setup, MockEnv};
use crate::ScriptHost;

fn tick(env: &mut MockEnv, host: &mut ScriptHost, turns: usize) {
    for n in 0..turns {
        env.now = 1010.0 + n as f64 * 10.0;
        let now = env.now;
        host.run_timers(env, now);
        host.run_jobs(env);
    }
}
fn logs(env: &MockEnv) -> Vec<String> { env.logs.iter().filter(|(level, _)| level == "log").map(|(_, text)| text.clone()).collect() }

#[test]
fn service_worker_runs_install_and_activate_with_lifetime_promises() {
    let (mut env, mut host) = setup("<!doctype html><body></body>");
    env.fetches.insert("https://example.test/dir/sw.js".into(), (200, "text/javascript".into(), r#"
        console.log('isolated', typeof document, typeof window, typeof pageSecret);
        self.workerOnly = true;
        self.addEventListener('install', event => {
            console.log('install', event instanceof ExtendableEvent);
            event.waitUntil(new Promise(resolve => setTimeout(() => { console.log('install-work'); resolve(); }, 30)));
        });
        self.onactivate = event => {
            console.log('activate');
            event.waitUntil(clients.claim());
        };
    "#.into()));
    run(&mut env, &mut host, r#"
        var pageSecret = 17, states = [], registration, ready = false;
        navigator.serviceWorker.ready.then(r => { ready = true; console.log('ready', r.active.state, navigator.serviceWorker.controller === r.active); });
        navigator.serviceWorker.register('sw.js').then(r => {
            registration = r;
            console.log('registered', r.installing.state, r.scope);
            const w = r.installing;
            w.onstatechange = () => states.push(w.state);
        }).catch(e => console.log('error', e.name, e.message));
    "#);
    tick(&mut env, &mut host, 30);
    let output = logs(&env);
    assert!(output.iter().any(|s| s.ends_with("isolated undefined undefined undefined")), "{output:?}");
    assert!(output.iter().any(|s| s == "registered installing https://example.test/dir/"), "{output:?}");
    let install = output.iter().position(|s| s.ends_with("install true")).unwrap();
    let work = output.iter().position(|s| s.ends_with("install-work")).unwrap();
    let activate = output.iter().position(|s| s.ends_with("activate")).unwrap();
    assert!(install < work && work < activate, "{output:?}");
    assert!(output.iter().any(|s| s == "ready activated true"), "{output:?}");
    let result = run(&mut env, &mut host, "console.log(states.join(','), typeof workerOnly, registration.installing, registration.waiting);");
    assert_eq!(result[0], "installed,activating,activated undefined null null");
}

#[test]
fn service_worker_rejected_install_never_activates_or_controls_client() {
    let (mut env, mut host) = setup("<body></body>");
    env.fetches.insert("https://example.test/dir/bad.js".into(), (200, "text/javascript".into(), "oninstall = e => e.waitUntil(Promise.reject(new Error('install failed'))); onactivate = () => console.log('must not activate');".into()));
    run(&mut env, &mut host, r#"
        var failedWorker, ready = false;
        navigator.serviceWorker.ready.then(() => ready = true);
        navigator.serviceWorker.register('bad.js').then(r => failedWorker = r.installing);
    "#);
    tick(&mut env, &mut host, 15);
    let result = run(&mut env, &mut host, r#"
        console.log(failedWorker.state, ready, navigator.serviceWorker.controller);
        navigator.serviceWorker.getRegistrations().then(r => console.log('registrations', r.length));
    "#);
    assert_eq!(result, ["redundant false null", "registrations 0"]);
    assert!(!logs(&env).iter().any(|s| s.contains("must not activate")));
}

#[test]
fn service_worker_validates_secure_origin_scope_and_script_before_running() {
    let (mut env, mut host) = setup("<body></body>");
    env.fetches.insert("https://example.test/dir/sw.js".into(), (200, "text/javascript".into(), "console.log('must not run');".into()));
    env.fetches.insert("https://example.test/dir/text.js".into(), (200, "text/plain".into(), "console.log('must not run');".into()));
    run(&mut env, &mut host, r#"
        navigator.serviceWorker.register('https://other.test/sw.js').catch(e => console.log('origin', e.name));
        navigator.serviceWorker.register('sw.js', { scope: '/' }).catch(e => console.log('scope', e.name));
        navigator.serviceWorker.register('text.js').catch(e => console.log('mime', e.name));
        navigator.serviceWorker.register('encoded%2fscript.js').catch(e => console.log('encoded', e.name));
        navigator.serviceWorker.register('data:text/javascript,1').catch(e => console.log('scheme', e.name));
    "#);
    tick(&mut env, &mut host, 20);
    let output = logs(&env);
    for expected in ["origin SecurityError", "scope SecurityError", "mime SecurityError", "encoded TypeError", "scheme TypeError"] {
        assert!(output.iter().any(|s| s == expected), "missing {expected}: {output:?}");
    }
    assert!(!output.iter().any(|s| s.contains("must not run")));
}

#[test]
fn service_worker_secure_contexts_only_trust_real_loopback_addresses() {
    for (url, expected) in [
        ("https://example.test/", true), ("http://localhost/", true),
        ("http://127.0.0.1/", true), ("http://[::1]/", true),
        ("http://example.test/", false), ("http://127.example.test/", false),
    ] {
        assert_eq!(crate::service_worker::trustworthy(&rabbithole_net::Url::parse_absolute(url).unwrap()), expected, "{url}");
    }
    let mut env = MockEnv::new("<body></body>");
    env.url = rabbithole_net::Url::parse_absolute("http://example.test/dir/page.html").unwrap();
    let mut host = ScriptHost::new(&mut env);
    let output = run(&mut env, &mut host, "navigator.serviceWorker.register('sw.js').catch(e => console.log(e.name, isSecureContext));");
    assert_eq!(output, ["SecurityError false"]);
}

#[test]
fn service_worker_update_waits_for_clients_until_skip_waiting() {
    let (mut env, mut host) = setup("<body></body>");
    env.fetches.insert("https://example.test/dir/sw.js".into(), (200, "text/javascript".into(), "onactivate = e => e.waitUntil(clients.claim());".into()));
    run(&mut env, &mut host, "var reg, old; navigator.serviceWorker.register('sw.js').then(r => reg = r);");
    tick(&mut env, &mut host, 15);
    run(&mut env, &mut host, "old = navigator.serviceWorker.controller;");
    env.fetches.insert("https://example.test/dir/sw.js".into(), (200, "text/javascript".into(), "oninstall = e => { console.log('update-install'); setTimeout(() => skipWaiting(), 100); }; onactivate = e => e.waitUntil(clients.claim());".into()));
    run(&mut env, &mut host, "reg.update().then(() => console.log('updated')); ");
    tick(&mut env, &mut host, 5);
    assert_eq!(run(&mut env, &mut host, "console.log(reg.waiting.state, navigator.serviceWorker.controller === old, old.state);"), ["installed true activated"]);
    tick(&mut env, &mut host, 30);
    assert_eq!(run(&mut env, &mut host, "console.log(reg.waiting, reg.active.state, old.state, navigator.serviceWorker.controller === reg.active);"), ["null activated redundant true"]);
}

#[test]
fn service_worker_evaluation_errors_reject_registration_and_invalid_event_lifetime_throws() {
    let (mut env, mut host) = setup("<body></body>");
    env.fetches.insert("https://example.test/dir/error.js".into(), (200, "text/javascript".into(), "throw new Error('evaluation failed');".into()));
    env.fetches.insert("https://example.test/dir/sw.js".into(), (200, "text/javascript".into(), "try { new ExtendableEvent('install').waitUntil(Promise.resolve()); } catch (e) { console.log('lifetime', e.name); }".into()));
    run(&mut env, &mut host, "navigator.serviceWorker.register('error.js').catch(e => console.log('evaluation', e.name));");
    tick(&mut env, &mut host, 10);
    assert!(logs(&env).iter().any(|s| s == "evaluation TypeError"));
    assert_eq!(run(&mut env, &mut host, "navigator.serviceWorker.getRegistrations().then(r => console.log(r.length));"), ["0"]);
    run(&mut env, &mut host, "navigator.serviceWorker.register('sw.js');");
    tick(&mut env, &mut host, 15);
    assert!(logs(&env).iter().any(|s| s.ends_with("lifetime InvalidStateError")));
}

#[test]
fn service_worker_cache_bytes_are_shared_with_page_and_readers_are_independent() {
    let (mut env, mut host) = setup("<body></body>");
    env.fetches.insert("https://example.test/dir/sw.js".into(), (200, "text/javascript".into(), r#"
        oninstall = event => event.waitUntil(caches.open('worker-cache').then(cache => cache.put('asset.bin', new Response(new Uint8Array([0, 128, 255]), { status: 201, headers: { 'x-cache-test': 'worker' } }))));
    "#.into()));
    run(&mut env, &mut host, "navigator.serviceWorker.register('sw.js');");
    tick(&mut env, &mut host, 15);
    let out = run(&mut env, &mut host, r#"
        (async () => {
            const cache = await caches.open('worker-cache');
            const response = await cache.match('asset.bin');
            console.log('cache', response.status, response.headers.get('x-cache-test'), Array.from(new Uint8Array(await response.arrayBuffer())).join(','));
            console.log('independent', Array.from(new Uint8Array(await (await caches.match('asset.bin')).arrayBuffer())).join(','));
            console.log('deleted', await caches.delete('worker-cache'), await caches.has('worker-cache'), (await caches.open('worker-cache').then(c => c.keys())).length, (await cache.keys()).length);
        })().catch(e => console.log('error', e.name, e.message));
    "#);
    assert_eq!(out, ["cache 201 worker 0,128,255", "independent 0,128,255", "deleted true false 0 1"]);
}

#[test]
fn service_worker_cache_respects_vary_search_methods_and_rejected_writes() {
    let (mut env, mut host) = setup("<body></body>");
    let out = run(&mut env, &mut host, r#"
        (async () => {
            const cache = await caches.open('vary');
            const en = new Request('/asset?v=1', { headers: { 'accept-language': 'en' } });
            const es = new Request('/asset?v=1', { headers: { 'accept-language': 'es' } });
            const response = new Response('english', { headers: { vary: 'Accept-Language' } });
            await cache.put(en, response);
            console.log('consumed', response.bodyUsed);
            await cache.put(es, new Response('spanish', { headers: { vary: 'Accept-Language' } }));
            console.log('vary', await (await cache.match(en)).text(), await (await cache.match(es)).text(), (await cache.keys()).length);
            console.log('query', await cache.match('/asset?v=2') === undefined, await (await cache.match(new Request('/asset?v=2', {headers:{'accept-language':'en'}}), {ignoreSearch:true})).text());
            console.log('method', await cache.match(new Request('/asset?v=1', {method:'POST'})) === undefined);
            for (const [request, response] of [['/bad', new Response('x', {status:206})], ['/bad', new Response('x', {headers:{vary:'*'}})], [new Request('/bad', {method:'POST'}), new Response('x')]]) {
                try { await cache.put(request, response); } catch(e) { console.log('rejected', e.name); }
            }
            console.log('remaining', (await cache.keys()).length, await cache.delete(en), (await cache.keys()).length);
        })().catch(e => console.log('error', e.name, e.message));
    "#);
    assert_eq!(out, ["consumed true", "vary english spanish 2", "query true english", "method true", "rejected TypeError", "rejected TypeError", "rejected TypeError", "remaining 2 true 1"]);
}

#[test]
fn service_worker_updatefound_follows_register_reactions_before_installing_is_cleared() {
    let (mut env, mut host) = setup("<body></body>");
    env.fetches.insert("https://example.test/dir/sw.js".into(), (200, "text/javascript".into(), "oninstall = e => e.waitUntil(Promise.resolve());".into()));
    run(&mut env, &mut host, r#"
        var registration, worker, found = 0, order = [], observed = [];
        navigator.serviceWorker.register('sw.js').then(r => {
            registration = r;
            worker = r.installing;
            order.push('registered');
            r.onupdatefound = () => {
                found++;
                order.push('updatefound');
                console.log('identity', r.installing === worker, r.installing.state);
                worker.onstatechange = () => observed.push(worker.state);
            };
        });
    "#);
    tick(&mut env, &mut host, 15);
    assert!(logs(&env).iter().any(|s| s == "identity true installing"));
    assert_eq!(run(&mut env, &mut host, "console.log(found, order.join(','), observed.join(','), registration.installing);"), ["1 registered,updatefound installed,activating,activated null"]);
    run(&mut env, &mut host, "registration.update(); navigator.serviceWorker.register('sw.js');");
    tick(&mut env, &mut host, 15);
    assert_eq!(run(&mut env, &mut host, "console.log(found, worker === registration.active);"), ["1 true"]);
}

#[test]
fn service_worker_unregistered_during_install_finishes_lifetime_without_activating() {
    let (mut env, mut host) = setup("<body></body>");
    env.fetches.insert("https://example.test/dir/sw.js".into(), (200, "text/javascript".into(), "oninstall = e => e.waitUntil(new Promise(resolve => setTimeout(() => { console.log('install settled'); resolve(); }, 30))); onactivate = () => console.log('must not activate');".into()));
    run(&mut env, &mut host, r#"
        var worker, registration, ready = false;
        navigator.serviceWorker.ready.then(() => ready = true);
        navigator.serviceWorker.register('sw.js').then(r => {
            registration = r;
            worker = r.installing;
            r.unregister().then(value => console.log('removed', value));
        });
    "#);
    tick(&mut env, &mut host, 20);
    let output = logs(&env);
    assert!(output.iter().any(|s| s == "removed true"));
    assert!(output.iter().any(|s| s.ends_with("install settled")));
    assert!(!output.iter().any(|s| s.contains("must not activate")));
    assert_eq!(run(&mut env, &mut host, "console.log(worker.state, registration.installing, registration.waiting, registration.active, ready, navigator.serviceWorker.controller);"), ["redundant null null null false null"]);
}
