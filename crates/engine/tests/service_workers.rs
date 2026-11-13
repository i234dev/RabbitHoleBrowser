//! Actual loopback HTTP scripts, not stubbed registrations or fake activation.
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::time::{Duration, Instant};
use rabbithole_engine::{Engine, Page, Viewport};

struct Server {
    origin: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    requests: Arc<Mutex<Vec<(String, String)>>>,
}
impl Server {
    fn new(routes: &[(&str, &str, &str, &[(&str, &str)])]) -> Self {
        let routes: BTreeMap<String, (String, String, Vec<(String, String)>)> = routes.iter().map(|(path, mime, body, headers)| ((*path).into(), ((*mime).into(), (*body).into(), headers.iter().map(|(k,v)| ((*k).into(),(*v).into())).collect()))).collect();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let halt = stop.clone(); let seen = requests.clone();
        let thread = std::thread::spawn(move || {
            while !halt.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(v) => v,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => { std::thread::sleep(Duration::from_millis(1)); continue; }
                    Err(_) => break,
                };
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() { continue; }
                let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
                let mut headers = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" { break; }
                    headers.push_str(&line);
                }
                seen.lock().unwrap().push((path.clone(), headers));
                let (status, mime, body, extra) = match routes.get(path.split('?').next().unwrap()) {
                    Some((mime, body, extra)) => (200, mime.as_str(), body.as_str(), extra.as_slice()),
                    None => (404, "text/plain", "not found", &[][..]),
                };
                let _ = write!(stream, "HTTP/1.1 {status} OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
                for (name, value) in extra { let _ = write!(stream, "{name}: {value}\r\n"); }
                let _ = write!(stream, "\r\n{body}");
            }
        });
        Self { origin, stop, requests, thread: Some(thread) }
    }
}
impl Drop for Server {
    fn drop(&mut self) { self.stop.store(true, Ordering::Relaxed); if let Some(t) = self.thread.take() { let _ = t.join(); } }
}
fn engine() -> Engine {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.scripting = true; engine.loader.load_scripts = true; engine
}
fn wait(engine: &Engine, page: &mut Page, condition: &str) {
    let end = Instant::now() + Duration::from_secs(8);
    while Instant::now() < end {
        if engine.eval_in_page(page, condition) == "true" { return; }
        engine.tick(page); std::thread::sleep(Duration::from_millis(2));
    }
    panic!("condition did not settle: {condition}; console={:?}", page.take_console());
}

#[test]
fn real_service_worker_script_activates_and_survives_document_navigation() {
    let first = r#"<!doctype html><script>
      var installed = false, states = [];
      navigator.serviceWorker.register('sw.js').then(r => {
        const worker = r.installing;
        if(worker) worker.onstatechange = () => states.push(worker.state);
      });
      navigator.serviceWorker.ready.then(r => installed = r.active.state === 'activated' && navigator.serviceWorker.controller === r.active);
    </script>"#;
    let worker = r#"self.addEventListener('install', e => {
      e.waitUntil(fetch('install.txt').then(r => r.text()).then(text => {
        console.log('install-data', text, typeof document, typeof window);
        return caches.open('install-cache').then(cache => cache.put('cached.txt', new Response(text))).then(() => new Promise(resolve => setTimeout(resolve, 25)));
      }));
    });
    self.addEventListener('activate', e => e.waitUntil(clients.claim()));"#;
    let next = r#"<!doctype html><script>
      var controlled = false;
      navigator.serviceWorker.ready.then(r => controlled = navigator.serviceWorker.controller === r.active);
    </script>"#;
    let server = Server::new(&[
        ("/app/index.html", "text/html", first, &[]),
        ("/app/sw.js", "text/javascript", worker, &[]),
        ("/app/install.txt", "text/plain", "local-install-response", &[]),
        ("/app/next.html", "text/html", next, &[]),
    ]);
    let mut engine = engine();
    let vp = Viewport { width: 640.0, height: 480.0 };
    let mut page = engine.load(&format!("{}/app/index.html", server.origin), vp).unwrap();
    wait(&engine, &mut page, "installed === true");
    assert!(page.take_console().iter().any(|(_, text)| text.contains("install-data local-install-response undefined undefined")));
    assert_eq!(engine.eval_in_page(&mut page, "states.join(',')"), "'installed,activating,activated'");
    engine.eval_in_page(&mut page, "var cached; caches.match('cached.txt').then(r => r.text()).then(t => cached = t);");
    wait(&engine, &mut page, "cached === 'local-install-response'");
    drop(page);
    let mut page = engine.load(&format!("{}/app/next.html", server.origin), vp).unwrap();
    wait(&engine, &mut page, "controlled === true");
    engine.eval_in_page(&mut page, "var cached; caches.match('cached.txt').then(r => r.text()).then(t => cached = t);");
    wait(&engine, &mut page, "cached === 'local-install-response'");
    engine.eval_in_page(&mut page, "var removed; navigator.serviceWorker.getRegistration().then(async r => removed = [await r.unregister(), await r.unregister(), navigator.serviceWorker.controller !== null].join(','));");
    wait(&engine, &mut page, "typeof removed === 'string'");
    assert_eq!(engine.eval_in_page(&mut page, "removed"), "'true,false,true'");
    drop(page);
    let mut page = engine.load(&format!("{}/app/next.html", server.origin), vp).unwrap();
    assert_eq!(engine.eval_in_page(&mut page, "navigator.serviceWorker.controller === null"), "true");
    engine.eval_in_page(&mut page, "var registrations; navigator.serviceWorker.getRegistrations().then(r => registrations = r.length);");
    wait(&engine, &mut page, "registrations === 0");
    let requests = server.requests.lock().unwrap();
    let script: Vec<_> = requests.iter().filter(|(path, _)| path == "/app/sw.js").collect();
    assert_eq!(script.len(), 1, "registration was retained across navigation");
    assert!(script[0].1.to_ascii_lowercase().contains("service-worker: script"));
}

#[test]
fn service_worker_allowed_header_extends_scope_without_controlling_first_page() {
    let page = r#"<!doctype html><script>
      var active = false;
      navigator.serviceWorker.register('/scripts/sw.js', {scope:'/'});
      navigator.serviceWorker.ready.then(r => active = r.active.state === 'activated' && navigator.serviceWorker.controller === null);
    </script>"#;
    let server = Server::new(&[
        ("/app/index.html", "text/html", page, &[]),
        ("/scripts/sw.js", "text/javascript", "oninstall = e => e.waitUntil(Promise.resolve());", &[("Service-Worker-Allowed", "/")]),
    ]);
    let mut engine = engine();
    let mut page = engine.load(&format!("{}/app/index.html", server.origin), Viewport { width: 640.0, height: 480.0 }).unwrap();
    wait(&engine, &mut page, "active === true");
}

#[test]
fn cache_storage_is_origin_partitioned_and_survives_navigation() {
    let html = "<!doctype html><body>local cache fixture";
    let server = Server::new(&[("/page.html", "text/html", html, &[])]);
    let other = Server::new(&[("/page.html", "text/html", html, &[])]);
    let mut engine = engine();
    let vp = Viewport { width: 320.0, height: 240.0 };
    let mut page = engine.load(&format!("{}/page.html", server.origin), vp).unwrap();
    engine.eval_in_page(&mut page, "var stored = false; caches.open('shared').then(c => c.put('/key', new Response('origin-one'))).then(() => stored = true);");
    wait(&engine, &mut page, "stored === true");
    drop(page);
    let mut page = engine.load(&format!("{}/page.html", other.origin), vp).unwrap();
    engine.eval_in_page(&mut page, "var isolated = false; caches.keys().then(keys => isolated = keys.length === 0);");
    wait(&engine, &mut page, "isolated === true");
    drop(page);
    let mut page = engine.load(&format!("{}/page.html", server.origin), vp).unwrap();
    engine.eval_in_page(&mut page, "var recovered; caches.match('/key').then(r => r.text()).then(text => recovered = text);");
    wait(&engine, &mut page, "recovered === 'origin-one'");
}

#[test]
fn controlled_window_fetch_intercepts_real_cache_bytes_and_uses_worker_network_fallback() {
    let html = r#"<!doctype html><script>
      var controlled = false;
      navigator.serviceWorker.register('sw.js');
      navigator.serviceWorker.ready.then(r => controlled = navigator.serviceWorker.controller === r.active);
    </script>"#;
    let worker = r#"
      oninstall = e => e.waitUntil(caches.open('assets').then(c => c.put('cached', new Response('from-worker-cache'))));
      onactivate = e => e.waitUntil(clients.claim());
      onfetch = e => {
        const path = new URL(e.request.url).pathname;
        if(path.endsWith('/cached')) e.respondWith(caches.match(e.request));
        else if(path.endsWith('/worker-network')) e.respondWith(fetch(e.request));
        else if(path.endsWith('/reject')) e.respondWith(Promise.reject(new Error('no fallback')));
        else if(path.endsWith('/invalid')) e.respondWith(Promise.resolve({status:200}));
        else if(path.endsWith('/binary')) e.respondWith(new Response(new Uint8Array([0,128,255]), {status:202,statusText:'Worker bytes',headers:{'x-worker':'yes'}}));
        else if(path.endsWith('/async')) {
          e.respondWith(new Promise(resolve => setTimeout(() => resolve(new Response('async-response')), 30)));
          e.waitUntil(new Promise(resolve => setTimeout(resolve, 150)).then(() => caches.open('assets').then(c => c.put('later', new Response('done')))));
        }
      };
    "#;
    let server = Server::new(&[
        ("/app/page.html", "text/html", html, &[]),
        ("/app/sw.js", "text/javascript", worker, &[]),
        ("/app/cached", "text/plain", "uncontrolled-network", &[]),
        ("/app/fallback", "text/plain", "plain-network", &[]),
        ("/app/worker-network", "text/plain", "worker-network-response", &[]),
        ("/outside/page.html", "text/html", "<!doctype html><body>outside scope", &[]),
    ]);
    let mut engine = engine(); let vp = Viewport {width:320.0,height:240.0};
    let mut page = engine.load(&format!("{}/app/page.html", server.origin), vp).unwrap();
    wait(&engine, &mut page, "controlled === true");
    engine.eval_in_page(&mut page, r#"
      var result, bytes;
      Promise.all(['cached','fallback','worker-network','reject','invalid','async'].map(path => fetch(path).then(r => r.text(),e => e.name))).then(v => result = v.join('|'));
      fetch('binary').then(async r => bytes = [r.status,r.statusText,r.headers.get('x-worker'),Array.from(new Uint8Array(await r.arrayBuffer())).join(',')].join('|'));
    "#);
    wait(&engine, &mut page, "typeof result === 'string' && typeof bytes === 'string'");
    assert_eq!(engine.eval_in_page(&mut page, "result"), "'from-worker-cache|plain-network|worker-network-response|TypeError|TypeError|async-response'");
    assert_eq!(engine.eval_in_page(&mut page, "bytes"), "'202|Worker bytes|yes|0,128,255'");
    engine.eval_in_page(&mut page, "var lifetimeDone = false; function checkLifetime(){ caches.match('later').then(r => { if(r) lifetimeDone = true; else setTimeout(checkLifetime,10); }); } checkLifetime();");
    wait(&engine, &mut page, "lifetimeDone === true");
    {
        let requests = server.requests.lock().unwrap();
        assert!(!requests.iter().any(|(path,_)| ["/app/cached","/app/binary","/app/reject","/app/invalid","/app/async"].contains(&path.as_str())), "cached/synthetic/rejected requests must not reach network: {requests:?}");
        assert_eq!(requests.iter().filter(|(path,_)| path == "/app/worker-network").count(),1,"worker fetch must not recurse");
    }
    drop(page);
    let mut page = engine.load(&format!("{}/outside/page.html", server.origin),vp).unwrap();
    engine.eval_in_page(&mut page,"var outside; fetch('/app/cached').then(r=>r.text()).then(t=>outside=t);");
    wait(&engine,&mut page,"outside === 'uncontrolled-network'");
    assert_eq!(server.requests.lock().unwrap().iter().filter(|(path,_)| path == "/app/cached").count(),1);
}

#[test]
fn intercepted_fetch_preserves_credentials_and_rejects_unsafe_no_cors_before_network() {
    let cross = Server::new(&[("/unsafe","text/plain","must not reach",&[])]);
    let html = r#"<!doctype html><script>
      var controlled=false; navigator.serviceWorker.register('sw.js');
      navigator.serviceWorker.ready.then(r=>controlled=navigator.serviceWorker.controller===r.active);
    </script>"#;
    let worker = "onactivate=e=>e.waitUntil(clients.claim()); onfetch=e=>e.respondWith(fetch(e.request.clone()));";
    let server=Server::new(&[("/app/page.html","text/html",html,&[]),("/app/sw.js","text/javascript",worker,&[]),("/app/network","text/plain","ok",&[])]);
    let mut engine=engine();
    // This minimal Engine fixture disables cookies by default. Install an
    // isolated in-memory jar so include/omit assertions test actual credentials.
    engine.loader.fetcher.cookies=Some(Arc::new(Mutex::new(rabbithole_net::cookies::CookieJar::new())));
    let mut page=engine.load(&format!("{}/app/page.html",server.origin),Viewport{width:320.0,height:240.0}).unwrap();
    wait(&engine,&mut page,"controlled===true");
    engine.eval_in_page(&mut page,"document.cookie='sw-secret=present; Path=/'; var done=false; fetch(new Request('network?omit',{credentials:'omit'})).then(()=>fetch('network?include',{credentials:'include'})).then(()=>done=true);");
    wait(&engine,&mut page,"done===true");
    {
        let requests=server.requests.lock().unwrap();
        let omit=&requests.iter().find(|(path,_)|path=="/app/network?omit").unwrap().1;
        let include=&requests.iter().find(|(path,_)|path=="/app/network?include").unwrap().1;
        assert!(!omit.to_ascii_lowercase().contains("cookie:"),"{omit}");
        assert!(include.contains("sw-secret=present"),"{include}");
    }
    let source=format!(r#"var rejected; Promise.all([
      fetch('{}/unsafe',{{mode:'no-cors',headers:{{'x-custom':'unsafe'}}}}),
      fetch('{}/unsafe',{{mode:'no-cors',method:'POST',body:'{{}}',headers:{{'content-type':'application/json'}}}}),
      fetch('{}/unsafe',{{mode:'same-origin'}}),
      fetch('{}/unsafe')
    ].map(p=>p.then(()=>false,e=>e instanceof TypeError))).then(v=>rejected=v.every(Boolean));"#,cross.origin,cross.origin,cross.origin,cross.origin);
    engine.eval_in_page(&mut page,&source);
    wait(&engine,&mut page,"rejected===true");
    let requests=cross.requests.lock().unwrap();
    assert_eq!(requests.len(),1,"only the ordinary CORS fetch may reach the cross-origin server: {requests:?}");
    assert!(!requests[0].1.to_ascii_lowercase().contains("x-custom:"));
}

#[test]
fn aborted_fetch_propagates_worker_signal_and_never_forwards_delayed_request_to_http() {
    let html = "<!doctype html><script>var controlled=false;navigator.serviceWorker.register('sw.js');navigator.serviceWorker.ready.then(r=>controlled=navigator.serviceWorker.controller===r.active);</script>";
    let worker = r#"onactivate=e=>e.waitUntil(clients.claim());onfetch=e=>{
      const request=e.request.clone();
      e.waitUntil(caches.open('signals').then(c=>c.put('/event-seen',new Response('yes'))));
      e.respondWith(new Promise((resolve,reject)=>e.request.signal.addEventListener('abort',()=>reject(e.request.signal.reason))));
      setTimeout(()=>fetch(request).then(()=>console.log('unexpected-network'),error=>console.log('forward-aborted',error===e.request.signal.reason,error.name,error.message)),100);
    };"#;
    let server=Server::new(&[("/app/page.html","text/html",html,&[]),("/app/sw.js","text/javascript",worker,&[]),("/app/never","text/plain","must not reach HTTP",&[])]);
    let mut engine=engine();
    let mut page=engine.load(&format!("{}/app/page.html",server.origin),Viewport{width:320.0,height:240.0}).unwrap();
    wait(&engine,&mut page,"controlled===true");
    engine.eval_in_page(&mut page,"var controller=new AbortController(), reason=new DOMException('client stopped','AbortError'), preserved=false, eventSeen=false;fetch('never',{signal:controller.signal}).catch(e=>preserved=e===reason);function check(){caches.match('/event-seen').then(r=>{if(r)eventSeen=true;else setTimeout(check,5);});}check();");
    wait(&engine,&mut page,"eventSeen===true");
    engine.eval_in_page(&mut page,"controller.abort(reason);");
    wait(&engine,&mut page,"preserved===true");
    let end=Instant::now()+Duration::from_secs(2);
    let mut observed=false;
    while Instant::now()<end {
        engine.tick(&mut page);
        for (_,text) in page.take_console() { if text.contains("forward-aborted true AbortError client stopped") {observed=true;} assert!(!text.contains("unexpected-network")); }
        if observed {break;}
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(observed,"worker clone did not observe deserialized AbortError");
    assert!(!server.requests.lock().unwrap().iter().any(|(path,_)|path=="/app/never"));
}
