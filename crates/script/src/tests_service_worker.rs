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
        navigator.serviceWorker.register('sw.js').then(async r => {
            registration = r;
            worker = r.installing;
            order.push('registered');
            await Promise.resolve();
            await Promise.resolve();
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

fn drive_fetch(env: &mut MockEnv, host: &mut ScriptHost, turns: usize) {
    for _ in 0..turns { env.now += 10.0; let now = env.now; host.run_timers(env, now); host.run_jobs(env); }
}
fn controlled_worker(source: &str) -> (MockEnv, ScriptHost) {
    let (mut env, mut host) = setup("<body></body>");
    env.fetches.insert("https://example.test/dir/sw.js".into(), (200, "text/javascript".into(), format!("onactivate = e => e.waitUntil(clients.claim());\n{source}")));
    run(&mut env, &mut host, "var registration; navigator.serviceWorker.register('sw.js').then(r => registration = r);");
    drive_fetch(&mut env, &mut host, 15);
    assert_eq!(run(&mut env, &mut host, "console.log(navigator.serviceWorker.controller.state);"), ["activated"]);
    (env, host)
}

#[test]
fn service_worker_fetch_cache_and_wait_until_have_independent_lifetimes() {
    let (mut env, mut host) = controlled_worker(r#"
        oninstall = e => e.waitUntil(caches.open('assets').then(c => c.put('cached.bin', new Response(new Uint8Array([0,128,255]), {status:201, statusText:'Cached', headers:{'x-sw':'cache'}}))));
        onfetch = e => {
            e.respondWith(caches.match(e.request));
            e.waitUntil(new Promise(resolve => setTimeout(resolve, 200)).then(() => caches.open('assets').then(c => c.put('later', new Response('lifetime complete')))));
        };
    "#);
    run(&mut env, &mut host, "var result; fetch('cached.bin').then(async r => result = [r.status,r.statusText,r.headers.get('x-sw'),Array.from(new Uint8Array(await r.arrayBuffer())).join(',')].join('|'));");
    drive_fetch(&mut env, &mut host, 3);
    assert_eq!(run(&mut env, &mut host, "console.log(result); caches.match('later').then(r => console.log(r === undefined));"), ["201|Cached|cache|0,128,255", "true"]);
    drive_fetch(&mut env, &mut host, 30);
    assert_eq!(run(&mut env, &mut host, "caches.match('later').then(r => r.text()).then(console.log);"), ["lifetime complete"]);
}

#[test]
fn service_worker_fetch_fallback_async_errors_and_dispatch_contract() {
    let (mut env, mut host) = controlled_worker(r#"
        addEventListener('fetch', e => {
            const path = new URL(e.request.url).pathname;
            e.handled.catch(error => console.log('handled-error', path, error.name));
            if (path.endsWith('/reject')) e.respondWith(Promise.reject(new Error('no')));
            else if (path.endsWith('/invalid')) e.respondWith(Promise.resolve({status:200}));
            else if (path.endsWith('/cancel')) e.preventDefault();
            else if (path.endsWith('/async')) e.respondWith(new Promise(resolve => setTimeout(() => resolve(new Response('async')), 30)));
            else if (path.endsWith('/once')) {
                e.respondWith(new Response('first'));
                try { e.respondWith(new Response('second')); } catch(error) { console.log('once', error.name); }
            } else if (path.endsWith('/late')) {
                e.waitUntil(new Promise(resolve => setTimeout(resolve, 20)));
                queueMicrotask(() => { try { e.respondWith(new Response('too late')); } catch(error) { console.log('late', error.name); } });
            }
        });
        addEventListener('fetch', e => console.log('second-listener', new URL(e.request.url).pathname));
    "#);
    for path in ["fallback", "late"] { env.fetches.insert(format!("https://example.test/dir/{path}"), (200, "text/plain".into(), "network".into())); }
    run(&mut env, &mut host, "var results; Promise.all(['fallback','reject','invalid','async','once','late','cancel'].map(path => fetch(path).then(r => r.text(), e => e.name))).then(r => results = r.join('|'));");
    drive_fetch(&mut env, &mut host, 30);
    assert_eq!(run(&mut env, &mut host, "console.log(results);"), ["network|TypeError|TypeError|async|first|network|TypeError"]);
    let output = logs(&env);
    assert!(output.iter().any(|s| s.ends_with("once InvalidStateError")));
    assert!(output.iter().any(|s| s.ends_with("late InvalidStateError")));
    assert!(output.iter().any(|s| s.ends_with("handled-error /dir/reject NetworkError")));
    assert!(output.iter().any(|s| s.ends_with("handled-error /dir/cancel NetworkError")));
    assert!(!output.iter().any(|s| s.ends_with("second-listener /dir/once")));
    assert!(output.iter().any(|s| s.ends_with("second-listener /dir/fallback")));
}

#[test]
fn service_worker_fetch_abort_preserves_reason_and_drops_unconsumed_interceptions() {
    let (mut env, mut host) = controlled_worker("onfetch = e => { console.log('fetch-dispatched'); e.respondWith(new Promise(() => {})); };");
    let output = run(&mut env, &mut host, r#"
        var initial = new AbortController(), later = new AbortController(), during = new AbortController(), reason = {why:'stop'}, results = [];
        initial.abort(reason);
        fetch('never-initial', {signal: initial.signal}).catch(e => results.push(e === reason));
        fetch('never-later', {signal: later.signal}).catch(e => results.push(e === reason));
        fetch('never-getter', {signal:during.signal,get mode(){during.abort(reason);return 'cors';}}).catch(e => results.push(e === reason));
    "#);
    assert!(output.is_empty());
    drive_fetch(&mut env, &mut host, 2);
    run(&mut env, &mut host, "later.abort(reason);");
    drive_fetch(&mut env, &mut host, 3);
    assert_eq!(run(&mut env, &mut host, "console.log(results.join(','));"), ["true,true,true"]);
    assert_eq!(logs(&env).iter().filter(|s| s.ends_with("fetch-dispatched")).count(), 1);
    assert!(host.next_due().is_none(), "aborted request must not retain a busy pending channel");
}

#[test]
fn service_worker_fetch_policy_getters_are_rooted_and_roundtrip_through_clone() {
    let (mut env, mut host) = controlled_worker(r#"
        onfetch = e => e.respondWith(Promise.resolve().then(() => {
            const request = e.request.clone();
            return new Response([request.mode,request.credentials,request.redirect].join('|'));
        }));
    "#);
    run(&mut env, &mut host, r#"
        var effects = [], result;
        const options = {
            get mode() { effects.push('mode'); for(let i=0;i<500;i++) ({value:new Array(8)}); return 'same-origin'; },
            get credentials() { effects.push('credentials'); for(let i=0;i<500;i++) ({value:new Array(8)}); return 'omit'; },
            redirect:'error'
        };
        const request = new Request('policy', options);
        fetch(request).then(r => r.text()).then(t => result = t);
    "#);
    drive_fetch(&mut env, &mut host, 15);
    assert_eq!(run(&mut env, &mut host, "console.log(result, effects.join(','));"), ["same-origin|omit|error mode,credentials"]);
}

#[test]
fn service_worker_old_fetch_and_wait_until_survive_replacement_and_unregister() {
    let (mut env, mut host) = controlled_worker(r#"
        onfetch = e => {
            e.respondWith(new Promise(resolve => setTimeout(() => resolve(new Response('old')), 200)));
            e.waitUntil(new Promise(resolve => setTimeout(resolve, 400)).then(() => caches.open('old').then(c => c.put('finished', new Response('old-event-complete')))));
        };
    "#);
    run(&mut env, &mut host, "var old = navigator.serviceWorker.controller, result; fetch('slow').then(r => r.text()).then(t => result = t);");
    drive_fetch(&mut env, &mut host, 2);
    env.fetches.insert("https://example.test/dir/sw.js".into(), (200, "text/javascript".into(), "skipWaiting(); onfetch = e => e.respondWith(new Response('new'));".into()));
    run(&mut env, &mut host, "registration.update();");
    drive_fetch(&mut env, &mut host, 12);
    assert_eq!(run(&mut env, &mut host, "console.log(old.state, typeof result); registration.unregister();"), ["redundant undefined"]);
    drive_fetch(&mut env, &mut host, 40);
    assert_eq!(run(&mut env, &mut host, "console.log(result); caches.match('finished').then(r => r.text()).then(console.log);"), ["old", "old-event-complete"]);
    run(&mut env, &mut host, "fetch('next').then(r => r.text()).then(console.log);");
    drive_fetch(&mut env, &mut host, 3);
    assert!(logs(&env).iter().any(|s| s == "new"));
}

#[test]
fn service_worker_fetch_waits_for_activation_without_falling_through_to_network() {
    let (mut env, mut host) = setup("<body></body>");
    env.fetches.insert("https://example.test/dir/sw.js".into(), (200, "text/javascript".into(), r#"
        onactivate = e => { e.waitUntil(clients.claim()); e.waitUntil(new Promise(resolve => setTimeout(resolve, 200))); };
        onfetch = e => e.respondWith(new Response('after-activation'));
    "#.into()));
    run(&mut env, &mut host, "var response; navigator.serviceWorker.oncontrollerchange = () => fetch('activation-pending').then(r => r.text()).then(t => response = t); navigator.serviceWorker.register('sw.js');");
    drive_fetch(&mut env, &mut host, 15);
    assert_eq!(run(&mut env, &mut host, "console.log(navigator.serviceWorker.controller.state, typeof response);"), ["activating undefined"]);
    drive_fetch(&mut env, &mut host, 20);
    assert_eq!(run(&mut env, &mut host, "console.log(response);"), ["after-activation"]);
}

#[test]
fn service_worker_abort_signal_reaches_request_clone_and_delayed_forwarding() {
    let (mut env, mut host) = controlled_worker(r#"
        onfetch = event => {
            const requests = [event.request, event.request.clone(), new Request(event.request)];
            event.request.signal.addEventListener('abort', () => console.log('worker-abort', event.request.signal.aborted, event.request.signal.reason.tag));
            event.respondWith(new Promise(() => {}));
            setTimeout(() => {
                for (const request of requests) fetch(request).then(() => console.log('must not forward'), error => console.log('forward-aborted', error === event.request.signal.reason, error.tag));
            }, 100);
        };
    "#);
    run(&mut env, &mut host, "var controller = new AbortController(), reason = {tag:'original', nested:[1,2]}, preserved; fetch('no-network', {signal:controller.signal}).catch(error => preserved = error === reason);");
    drive_fetch(&mut env, &mut host, 2);
    run(&mut env, &mut host, "controller.abort(reason);");
    drive_fetch(&mut env, &mut host, 30);
    assert_eq!(run(&mut env, &mut host, "console.log(preserved);"), ["true"]);
    let output = logs(&env);
    assert!(output.iter().any(|s| s.ends_with("worker-abort true original")), "{output:?}");
    assert_eq!(output.iter().filter(|s| s.ends_with("forward-aborted true original")).count(), 3, "{output:?}");
    assert!(!output.iter().any(|s| s.contains("must not forward")));
    assert!(host.next_due().is_none());
}

#[test]
fn aborted_fetch_wait_until_survives_skip_waiting_and_unregister() {
    let (mut env, mut host) = controlled_worker(r#"
        onfetch = event => {
            event.respondWith(new Promise((resolve,reject) => event.request.signal.addEventListener('abort',()=>reject(event.request.signal.reason))));
            event.waitUntil(new Promise(resolve => {
                function poll() { caches.match('release-gate').then(response => { if(response)resolve();else setTimeout(poll,10); }); }
                poll();
            }).then(()=>caches.open('lifetime').then(cache=>cache.put('after-abort',new Response('completed old event')))));
        };
    "#);
    run(&mut env,&mut host,"var controller=new AbortController(), reason={stop:true};fetch('old-event',{signal:controller.signal}).catch(()=>{});var old=navigator.serviceWorker.controller;");
    drive_fetch(&mut env,&mut host,2);
    run(&mut env,&mut host,"controller.abort(reason);");
    drive_fetch(&mut env,&mut host,2);
    env.fetches.insert("https://example.test/dir/sw.js".into(),(200,"text/javascript".into(),"skipWaiting();onfetch=e=>e.respondWith(new Response('new'));".into()));
    run(&mut env,&mut host,"registration.update();");drive_fetch(&mut env,&mut host,15);
    assert_eq!(run(&mut env,&mut host,"console.log(old.state, navigator.serviceWorker.controller===old);registration.unregister();caches.open('lifetime').then(cache=>cache.put('release-gate',new Response('release')));"),["redundant false"]);
    drive_fetch(&mut env,&mut host,25);
    assert_eq!(run(&mut env,&mut host,"caches.match('after-abort').then(response=>response ? response.text() : 'missing').then(console.log);"),["completed old event"]);
}

#[test]
fn navigation_mode_cannot_be_acquired_through_public_request_properties_or_init() {
    let (mut env, mut host) = setup("<body></body>");
    env.fetches.insert("https://example.test/dir/forbidden".into(), (200, "text/plain".into(), "unexpected network".into()));
    let output = run(&mut env, &mut host, r#"
        try { new Request('forbidden', {mode:'navigate'}); } catch(error) { console.log('constructor', error.name); }
        const forged=new Request('forbidden');
        Object.defineProperty(forged,'mode',{value:'navigate',configurable:true});
        fetch(forged).then(()=>console.log('unexpected success'),error=>console.log('forged',error.name));
        fetch('forbidden',{mode:'navigate'}).then(()=>console.log('unexpected success'),error=>console.log('init',error.name));
    "#);
    assert_eq!(output, ["constructor TypeError", "init TypeError", "forged TypeError"]);
}

#[test]
fn cache_quota_replacement_rejects_without_losing_binary_response() {
    let (mut env, mut host) = setup("<body></body>");
    host.service_workers.as_ref().unwrap().set_cache_storage_limits(crate::CacheStorageLimits { max_total_bytes: 8192, max_origin_bytes: 2048, max_entries: 8, max_caches: 4 }).unwrap();
    let output = run(&mut env, &mut host, r#"
      let cache, failure;
      caches.open('quota').then(c=>{cache=c;return c.put('entry',new Response(new Uint8Array([0,128,255]),{status:203,statusText:'Stored',headers:{'x-version':'old'}}));})
        .then(()=>cache.put('entry',new Response(new Uint8Array(3000))))
        .then(()=>console.log('unexpected success'),error=>{failure=error;return cache.match('entry');})
        .then(response=>Promise.all([response.status,response.statusText,response.headers.get('x-version'),response.arrayBuffer()]))
        .then(values=>console.log(failure.name,failure.code,failure instanceof DOMException,values[0],values[1],values[2],Array.from(new Uint8Array(values[3])).join(',')));
    "#);
    assert_eq!(output,["QuotaExceededError 22 true 203 Stored old 0,128,255"]);
}

#[test]
fn cache_add_all_over_quota_is_atomic_and_deletion_reclaims_entry_budget() {
    let (mut env, mut host) = setup("<body></body>");
    env.fetches.insert("https://example.test/dir/first".into(),(200,"text/plain".into(),"new first".into()));
    env.fetches.insert("https://example.test/dir/second".into(),(200,"text/plain".into(),"z".repeat(3000)));
    let registry=host.service_workers.as_ref().unwrap().clone();
    registry.set_cache_storage_limits(crate::CacheStorageLimits { max_total_bytes:8192,max_origin_bytes:2048,max_entries:8,max_caches:4 }).unwrap();
    let output=run(&mut env,&mut host,r#"
      let quotaCache;
      caches.open('batch').then(cache=>{quotaCache=cache;return cache.put('first',new Response('old first'));})
        .then(()=>quotaCache.addAll(['first','second']))
        .then(()=>console.log('unexpected success'),error=>{
          console.log(error.name);
          return Promise.all([quotaCache.match('first').then(r=>r.text()),quotaCache.match('second')]);
        }).then(values=>console.log(values[0],values[1]===undefined));
    "#);
    assert_eq!(output,["QuotaExceededError","old first true"]);
    let before=registry.cache_storage_usage(&env.url);
    assert!(registry.set_cache_storage_limits(crate::CacheStorageLimits { max_total_bytes:0,max_origin_bytes:0,max_entries:0,max_caches:0 }).is_err());
    assert_eq!(registry.cache_storage_usage(&env.url),before,"failed reconfiguration preserves data and budgets");
    assert_eq!(run(&mut env,&mut host,"quotaCache.delete('first').then(console.log);"),["true"]);
    let after=registry.cache_storage_usage(&env.url);
    assert_eq!(after.entries,0);assert!(after.total_bytes<before.total_bytes);
    assert_eq!(run(&mut env,&mut host,"quotaCache.put('small',new Response('fits')).then(()=>quotaCache.match('small')).then(r=>r.text()).then(console.log);"),["fits"]);
}
