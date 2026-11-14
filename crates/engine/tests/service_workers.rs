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
        let routes: Vec<_> = routes.iter().map(|(path,mime,body,headers)| (*path,200,*mime,*body,*headers)).collect();
        Self::with_status(&routes)
    }
    fn with_status(routes: &[(&str,u16,&str,&str,&[(&str,&str)])]) -> Self {
        let routes: BTreeMap<String, (u16,String,String,Vec<(String,String)>)> = routes.iter().map(|(path,status,mime,body,headers)| ((*path).into(), (*status,(*mime).into(),(*body).into(),headers.iter().map(|(k,v)| ((*k).into(),(*v).into())).collect()))).collect();
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
                // Winsock accept inherits the nonblocking listener mode.
                // Read the complete HTTP request before replying on every platform.
                stream.set_nonblocking(false).unwrap();
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
                    Some((status,mime,body,extra)) => (*status,mime.as_str(),body.as_str(),extra.as_slice()),
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

#[test]
fn aborted_old_worker_wait_until_survives_replacement_unregister_and_client_drop() {
    let start="<!doctype html><script>var controlled=false;navigator.serviceWorker.register('sw.js');navigator.serviceWorker.ready.then(r=>controlled=navigator.serviceWorker.controller===r.active);</script>";
    let worker=r#"onactivate=e=>e.waitUntil(clients.claim());onfetch=e=>{
      e.respondWith(new Promise((resolve,reject)=>e.request.signal.addEventListener('abort',()=>reject(e.request.signal.reason))));
      e.waitUntil(caches.open('lifetime').then(cache=>cache.put('/event-seen',new Response('yes'))));
      e.waitUntil(new Promise(resolve=>{function poll(){caches.match('/release-gate').then(r=>{if(r)resolve();else setTimeout(poll,10);});}poll();}).then(()=>caches.open('lifetime').then(cache=>cache.put('/after-abort',new Response('completed old event')))));
    };"#;
    let replacement="skipWaiting();onactivate=e=>e.waitUntil(clients.claim());onfetch=e=>e.respondWith(new Response('new'));";
    let observer="<!doctype html><script>var completed; caches.open('lifetime').then(cache=>cache.put('/release-gate',new Response('release')));function poll(){caches.match('/after-abort').then(r=>{if(r)r.text().then(text=>completed=text);else setTimeout(poll,10);});}poll();</script>";
    let server=Server::new(&[("/app/start.html","text/html",start,&[]),("/app/sw.js","text/javascript",worker,&[]),("/app/new-sw.js","text/javascript",replacement,&[]),("/observer.html","text/html",observer,&[])]);
    let mut engine=engine();let vp=Viewport{width:320.0,height:240.0};
    let mut page=engine.load(&format!("{}/app/start.html",server.origin),vp).unwrap();wait(&engine,&mut page,"controlled===true");
    engine.eval_in_page(&mut page,"var controller=new AbortController(),eventSeen=false,replaced=false,removed=false;fetch('old-event',{signal:controller.signal}).catch(()=>{});function check(){caches.match('/event-seen').then(r=>{if(r)eventSeen=true;else setTimeout(check,5);});}check();");
    wait(&engine,&mut page,"eventSeen===true");
    engine.eval_in_page(&mut page,"controller.abort({stop:true});navigator.serviceWorker.register('new-sw.js').then(registration=>{function check(){if(registration.active&&registration.active.state==='activated'&&registration.active.scriptURL.endsWith('/new-sw.js')){replaced=true;registration.unregister().then(value=>removed=value);}else setTimeout(check,5);}check();});");
    wait(&engine,&mut page,"replaced===true&&removed===true");drop(page);
    let mut observer=engine.load(&format!("{}/observer.html",server.origin),vp).unwrap();
    wait(&engine,&mut observer,"completed==='completed old event'");
    assert!(!server.requests.lock().unwrap().iter().any(|(path,_)|path=="/app/old-event"));
}

#[test]
fn engine_get_navigation_serves_precached_utf8_html_offline_with_real_client_identity() {
    let start="<!doctype html><script>var controlled=false;navigator.serviceWorker.register('sw.js');navigator.serviceWorker.ready.then(r=>controlled=navigator.serviceWorker.controller===r.active);</script>";
    let offline="<!doctype html><title>Offline café 🐇</title><main id=offline>Cache without server</main><script>var cachedScriptRan=true,actualClientId;fetch('client-id').then(r=>r.text()).then(id=>actualClientId=id);</script>";
    let worker=r#"oninstall=e=>e.waitUntil(fetch('offline.html').then(r=>r.arrayBuffer()).then(bytes=>caches.open('pages').then(c=>c.put('offline.html',new Response(bytes,{status:203,headers:{'content-type':'text/html; charset=UTF-8'}})))));
      onactivate=e=>e.waitUntil(clients.claim());
      onfetch=e=>{
        if(e.request.mode==='navigate'){
          console.log('navigation-client',e.resultingClientId);
          console.log('navigation-policy',e.request.mode,e.request.destination,e.clientId==='',e.resultingClientId!=='');
          e.respondWith(caches.match(e.request));
        }else if(new URL(e.request.url).pathname.endsWith('/client-id'))e.respondWith(new Response(e.clientId));
      };"#;
    let server=Server::new(&[("/app/start.html","text/html",start,&[]),("/app/sw.js","text/javascript",worker,&[]),("/app/offline.html","text/html",offline,&[("Cache-Control","no-store")])]);
    let origin=server.origin.clone(); let mut engine=engine(); let vp=Viewport{width:320.0,height:240.0};
    let mut page=engine.load(&format!("{origin}/app/start.html"),vp).unwrap();
    wait(&engine,&mut page,"controlled===true");
    assert_eq!(server.requests.lock().unwrap().iter().filter(|(path,_)|path=="/app/offline.html").count(),1);
    drop(page); drop(server);
    engine.loader.fetcher.timeout=Duration::from_millis(500);
    let target=format!("{origin}/app/offline.html#anchor");
    let mut page=engine.load(&target,vp).expect("pre-cached navigation must work with stopped HTTP server");
    assert_eq!(page.status,203);
    assert_eq!(page.title,"Offline café 🐇");
    assert_eq!(page.url.to_string(),target);
    assert_eq!(engine.eval_in_page(&mut page,"cachedScriptRan && document.querySelector('#offline').textContent==='Cache without server'"),"true");
    wait(&engine,&mut page,"typeof actualClientId==='string'");
    let client=engine.eval_in_page(&mut page,"actualClientId").trim_matches('\'').to_string();
    let console=page.take_console();
    assert!(console.iter().any(|(_,text)|text.ends_with(&format!("navigation-client {client}"))),"{console:?}");
    assert!(console.iter().any(|(_,text)|text.ends_with("navigation-policy navigate document true true")),"{console:?}");
    let mut separate=Engine::new_bundled_fonts_only(); separate.loader.fetcher.timeout=Duration::from_millis(100);
    assert!(separate.load(&target,vp).is_err(),"a new Engine must not inherit another Engine's registry or explicit Cache Storage");
}

#[test]
fn navigation_passthrough_preserves_private_source_and_constructor_override_resets_origin() {
    let start="<!doctype html><script>var controlled=false;navigator.serviceWorker.register('sw.js');navigator.serviceWorker.ready.then(r=>controlled=navigator.serviceWorker.controller===r.active);</script>";
    let worker=r#"onactivate=e=>e.waitUntil(clients.claim());onfetch=e=>{
      if(e.request.mode!=='navigate')return;
      const cloned=e.request.clone(), copied=new Request(cloned,{});
      console.log('source-policies',cloned.mode,copied.mode,copied.redirect);
      let rejected=0;
      try{new Request('/app/pass',{mode:'navigate'});}catch(error){if(error instanceof TypeError)rejected++;}
      try{new Request(e.request,{mode:'navigate'});}catch(error){if(error instanceof TypeError)rejected++;}
      console.log('constructor-rejected',rejected);
      const overridden=new Request(e.request,{headers:{'x-override':'yes'},credentials:'omit',redirect:'follow'});
      console.log('override-policies',overridden.mode,overridden.credentials);
      const forged=overridden.clone();Object.defineProperty(forged,'mode',{value:'navigate',configurable:true});
      e.waitUntil(fetch(forged).then(()=>console.log('unexpected forged success'),error=>console.log('forged-rejected',error.name)));
      e.waitUntil(fetch(overridden).then(r=>r.text()));
      e.respondWith(fetch(copied.clone()));
    };"#;
    let server=Server::new(&[("/app/start.html","text/html",start,&[]),("/app/sw.js","text/javascript",worker,&[]),("/app/pass","text/html","<!doctype html><title>Network pass</title><main>real network body</main>",&[])]);
    let mut engine=engine();let vp=Viewport{width:320.0,height:240.0};
    let mut page=engine.load(&format!("{}/app/start.html",server.origin),vp).unwrap();wait(&engine,&mut page,"controlled===true");drop(page);
    let mut page=engine.load(&format!("{}/app/pass",server.origin),vp).unwrap();
    assert_eq!(page.title,"Network pass");
    let end=Instant::now()+Duration::from_secs(2);
    while Instant::now()<end && server.requests.lock().unwrap().iter().filter(|(path,_)|path=="/app/pass").count()<2 {engine.tick(&mut page);std::thread::sleep(Duration::from_millis(2));}
    let console=page.take_console();
    for expected in ["source-policies navigate navigate manual","constructor-rejected 2","override-policies same-origin omit","forged-rejected TypeError"] {assert!(console.iter().any(|(_,text)|text.ends_with(expected)),"missing {expected}: {console:?}");}
    let requests=server.requests.lock().unwrap();let pass:Vec<_>=requests.iter().filter(|(path,_)|path=="/app/pass").collect();
    assert_eq!(pass.len(),2,"original passthrough plus explicit override only: {requests:?}");
    let native=pass.iter().find(|(_,headers)|!headers.contains("x-override: yes")).unwrap().1.to_ascii_lowercase();
    let changed=pass.iter().find(|(_,headers)|headers.contains("x-override: yes")).unwrap().1.to_ascii_lowercase();
    assert!(native.contains("sec-fetch-mode: navigate")&&native.contains("sec-fetch-dest: document"),"{native}");
    assert!(!native.contains("referer:"),"typed navigation has no source client referrer: {native}");
    assert!(changed.contains("sec-fetch-mode: same-origin")&&changed.contains("sec-fetch-dest: empty"),"{changed}");
    assert!(changed.contains(&format!("referer: {}/app/sw.js",server.origin)),"override must use worker client origin/referrer: {changed}");
}

#[test]
fn navigation_fallback_rejections_redirects_timeout_and_disabled_scripts_are_bounded() {
    let start="<!doctype html><script>var controlled=false;navigator.serviceWorker.register('sw.js');navigator.serviceWorker.ready.then(r=>controlled=navigator.serviceWorker.controller===r.active);</script>";
    let worker=r#"onactivate=e=>e.waitUntil(clients.claim());onfetch=e=>{
      const path=new URL(e.request.url).pathname;
      if(e.request.mode!=='navigate'){
        if(path.endsWith('/timed-out'))e.respondWith(caches.match('/timed-out'));
        return;
      }
      if(path.endsWith('/reject'))e.respondWith(Promise.reject(new Error('navigation rejected')));
      else if(path.endsWith('/false'))e.respondWith(false);
      else if(path.endsWith('/hang')){
        e.request.signal.addEventListener('abort',()=>caches.open('flags').then(c=>c.put('/timed-out',new Response('aborted'))));
        e.respondWith(new Promise(()=>{}));
      }else if(path.endsWith('/redirect'))e.respondWith(Response.redirect('/app/done',302));
      else if(path.endsWith('/loop'))e.respondWith(Response.redirect('/app/loop',302));
      else if(path.endsWith('/done'))e.respondWith(new Response('<!doctype html><title>Redirect done</title><main>synthetic</main>',{status:201,headers:{'content-type':'text/html; charset=UTF-8'}}));
    };"#;
    let server=Server::new(&[("/app/start.html","text/html",start,&[]),("/app/sw.js","text/javascript",worker,&[]),("/app/fallback","text/html","<!doctype html><title>Network fallback</title>",&[]),("/outside/page","text/html","<!doctype html><title>Outside scope</title>",&[]),("/app/disabled","text/html","<!doctype html><title>Scripts disabled</title>",&[]),("/app/reject","text/plain","must not fetch",&[]),("/app/false","text/plain","must not fetch",&[]),("/app/hang","text/plain","must not fetch",&[])]);
    let origin=server.origin.clone();let mut engine=engine();let vp=Viewport{width:320.0,height:240.0};
    let mut page=engine.load(&format!("{origin}/app/start.html"),vp).unwrap();wait(&engine,&mut page,"controlled===true");drop(page);
    assert_eq!(engine.load(&format!("{origin}/outside/page"),vp).unwrap().title,"Outside scope");
    assert_eq!(engine.load(&format!("{origin}/app/fallback"),vp).unwrap().title,"Network fallback");
    engine.scripting=false;
    assert_eq!(engine.load(&format!("{origin}/app/disabled"),vp).unwrap().title,"Scripts disabled");engine.scripting=true;
    for path in ["reject","false"] {assert!(engine.load(&format!("{origin}/app/{path}"),vp).is_err());}
    let redirected=engine.load(&format!("{origin}/app/redirect"),vp).unwrap();
    assert_eq!(redirected.status,201);assert_eq!(redirected.title,"Redirect done");assert_eq!(redirected.url.to_string(),format!("{origin}/app/done"));drop(redirected);
    assert!(engine.load(&format!("{origin}/app/loop"),vp).err().unwrap().to_string().contains("Too many"));
    engine.loader.fetcher.timeout=Duration::from_millis(50);let started=Instant::now();
    let error=engine.load(&format!("{origin}/app/hang"),vp).err().unwrap();assert!(error.to_string().contains("timed out"),"{error}");assert!(started.elapsed()<Duration::from_secs(1));
    engine.loader.fetcher.timeout=Duration::from_secs(2);
    let mut page=engine.load(&format!("{origin}/app/done"),vp).unwrap();
    engine.eval_in_page(&mut page,"var abortObserved;fetch('timed-out').then(r=>r.text()).then(text=>abortObserved=text);");wait(&engine,&mut page,"abortObserved==='aborted'");
    let requests=server.requests.lock().unwrap();
    assert!(!requests.iter().any(|(path,_)|["/app/reject","/app/false","/app/hang","/app/redirect","/app/done","/app/loop"].contains(&path.as_str())),"rejected/cached/redirected requests must not fall through: {requests:?}");
}

#[test]
fn installing_worker_is_not_forced_active_by_navigation() {
    let start="<!doctype html><script>var registered=false;navigator.serviceWorker.register('sw.js').then(()=>registered=true);</script>";
    let worker="oninstall=e=>e.waitUntil(new Promise(()=>{}));onfetch=e=>e.respondWith(new Response('must not intercept an installing worker'));";
    let server=Server::new(&[("/app/start.html","text/html",start,&[]),("/app/sw.js","text/javascript",worker,&[]),("/app/network","text/html","<!doctype html><title>Network while installing</title>",&[])]);
    let mut engine=engine();let vp=Viewport{width:320.0,height:240.0};
    let mut first=engine.load(&format!("{}/app/start.html",server.origin),vp).unwrap();wait(&engine,&mut first,"registered===true");
    let second=engine.load(&format!("{}/app/network",server.origin),vp).unwrap();
    assert_eq!(second.title,"Network while installing");
    assert_eq!(server.requests.lock().unwrap().iter().filter(|(path,_)|path=="/app/network").count(),1);
}

#[test]
fn service_worker_navigation_cross_origin_redirect_reselects_controller_and_rejects_other_schemes() {
    let destination=Server::new(&[("/landing","text/html","<!doctype html><title>Other origin</title><script>var uncontrolled=navigator.serviceWorker.controller===null;</script>",&[])]);
    let start="<!doctype html><script>var controlled=false;navigator.serviceWorker.register('sw.js');navigator.serviceWorker.ready.then(r=>controlled=navigator.serviceWorker.controller===r.active);</script>";
    let worker=format!("onactivate=e=>e.waitUntil(clients.claim());onfetch=e=>{{if(e.request.mode==='navigate')e.respondWith(Response.redirect(new URL(e.request.url).pathname.endsWith('/bad')?'data:text/html,blocked':'{}/landing',302));}};",destination.origin);
    let source=Server::new(&[("/app/start.html","text/html",start,&[]),("/app/sw.js","text/javascript",&worker,&[])]);
    let mut engine=engine();let vp=Viewport{width:320.0,height:240.0};
    let mut page=engine.load(&format!("{}/app/start.html",source.origin),vp).unwrap();wait(&engine,&mut page,"controlled===true");drop(page);
    let mut page=engine.load(&format!("{}/app/cross",source.origin),vp).unwrap();
    assert_eq!(page.url.to_string(),format!("{}/landing",destination.origin));assert_eq!(page.title,"Other origin");
    assert_eq!(engine.eval_in_page(&mut page,"uncontrolled"),"true");
    assert!(engine.load(&format!("{}/app/bad",source.origin),vp).is_err());
    assert_eq!(destination.requests.lock().unwrap().iter().filter(|(path,_)|path=="/landing").count(),1);
}

#[test]
fn navigation_network_redirect_is_followed_with_scope_reselection() {
    let start="<!doctype html><script>var controlled=false;navigator.serviceWorker.register('sw.js');navigator.serviceWorker.ready.then(r=>controlled=navigator.serviceWorker.controller===r.active);</script>";
    let worker="onactivate=e=>e.waitUntil(clients.claim());onfetch=e=>{if(e.request.mode==='navigate'&&new URL(e.request.url).pathname.endsWith('/forward'))e.respondWith(fetch(e.request));};";
    let server=Server::with_status(&[
      ("/app/start.html",200,"text/html",start,&[]),
      ("/app/sw.js",200,"text/javascript",worker,&[]),
      ("/app/forward",302,"text/html","",&[("Location","/outside/final")]),
      ("/app/fallback",302,"text/html","",&[("Location","/outside/final")]),
      ("/outside/final",200,"text/html","<!doctype html><title>HTTP redirect</title><script>var uncontrolled=navigator.serviceWorker.controller===null;</script>",&[]),
    ]);
    let mut engine=engine();let vp=Viewport{width:320.0,height:240.0};
    let mut page=engine.load(&format!("{}/app/start.html",server.origin),vp).unwrap();wait(&engine,&mut page,"controlled===true");drop(page);
    for path in ["forward","fallback"] {
      let mut page=engine.load(&format!("{}/app/{path}#kept",server.origin),vp).unwrap();
      assert_eq!(page.title,"HTTP redirect");assert_eq!(page.url.to_string(),format!("{}/outside/final#kept",server.origin));
      assert_eq!(engine.eval_in_page(&mut page,"uncontrolled"),"true");
    }
    let seen=server.requests.lock().unwrap();
    for path in ["/app/forward","/app/fallback"] {assert_eq!(seen.iter().filter(|(p,_)|p==path).count(),1,"{seen:?}");}
}

#[test]
fn navigation_forwarding_and_clones_use_live_headers_without_forging_metadata() {
    let start="<!doctype html><script>var controlled=false;navigator.serviceWorker.register('sw.js');navigator.serviceWorker.ready.then(r=>controlled=navigator.serviceWorker.controller===r.active);</script>";
    let worker=r#"onactivate=e=>e.waitUntil(clients.claim());onfetch=e=>{
      if(e.request.mode!=='navigate')return;
      const path=new URL(e.request.url).pathname, live=e.request.headers;
      live.set('X-Live',path);live.set('Cookie','forged-cookie=must-not-send');
      Object.defineProperty(e.request,'mode',{value:'cors',configurable:true});
      Object.defineProperty(e.request,'headers',{get(){const h=new Headers(live);for(let i=0;i<50;i++)new Array(20).fill({i});return h;},configurable:true});
      const forwarded=path.endsWith('/clone')?e.request.clone():path.endsWith('/copy')?new Request(e.request,{}):e.request;
      e.respondWith(fetch(forwarded));
    };"#;
    let html="<!doctype html><title>Live headers</title>";
    let server=Server::new(&[("/app/start.html","text/html",start,&[]),("/app/sw.js","text/javascript",worker,&[]),("/app/direct","text/html",html,&[]),("/app/clone","text/html",html,&[]),("/app/copy","text/html",html,&[])]);
    let mut engine=engine();let vp=Viewport{width:320.0,height:240.0};
    let mut page=engine.load(&format!("{}/app/start.html",server.origin),vp).unwrap();wait(&engine,&mut page,"controlled===true");drop(page);
    for kind in ["direct","clone","copy"] {assert_eq!(engine.load(&format!("{}/app/{kind}",server.origin),vp).unwrap().title,"Live headers");}
    let seen=server.requests.lock().unwrap();
    for kind in ["direct","clone","copy"] {
      let path=format!("/app/{kind}");let hits:Vec<_>=seen.iter().filter(|(p,_)|p==&path).collect();assert_eq!(hits.len(),1,"{seen:?}");
      let headers=hits[0].1.to_ascii_lowercase();
      assert!(headers.contains(&format!("x-live: {path}")),"live Headers lost: {headers}");
      assert!(headers.contains("sec-fetch-mode: navigate")&&headers.contains("sec-fetch-dest: document"),"metadata authority lost: {headers}");
      assert!(!headers.contains("forged-cookie"),"forbidden author header escaped guard: {headers}");
    }
}

#[test]
fn controlled_html_without_scripts_is_execution_ready_for_later_clients_claim() {
    let start="<!doctype html><script>var controlled=false;navigator.serviceWorker.register('sw.js');navigator.serviceWorker.ready.then(r=>controlled=navigator.serviceWorker.controller===r.active);</script>";
    let old="onactivate=e=>e.waitUntil(clients.claim());onfetch=e=>{if(e.request.mode==='navigate')e.respondWith(new Response('<!doctype html><title>Plain controlled HTML</title>',{headers:{'content-type':'text/html'}}));};";
    let new="skipWaiting();onactivate=e=>e.waitUntil(clients.claim());";
    let observer="<!doctype html><script>var updated=false;navigator.serviceWorker.register('/app/new.js',{scope:'/app/'}).then(r=>{function check(){if(r.active&&r.active.state==='activated'&&r.active.scriptURL.endsWith('/app/new.js'))updated=true;else setTimeout(check,5);}check();});</script>";
    let server=Server::new(&[("/app/start.html","text/html",start,&[]),("/app/sw.js","text/javascript",old,&[]),("/app/new.js","text/javascript",new,&[]),("/observer.html","text/html",observer,&[])]);
    let mut engine=engine();let vp=Viewport{width:320.0,height:240.0};
    let mut start=engine.load(&format!("{}/app/start.html",server.origin),vp).unwrap();wait(&engine,&mut start,"controlled===true");drop(start);
    let mut plain=engine.load(&format!("{}/app/plain",server.origin),vp).unwrap();assert_eq!(plain.title,"Plain controlled HTML");
    let mut observer=engine.load(&format!("{}/observer.html",server.origin),vp).unwrap();wait(&engine,&mut observer,"updated===true");
    // Creating a realm only here would adopt the stale reserved controller and
    // miss the claim performed while this script-free document was already live.
    assert_eq!(engine.eval_in_page(&mut plain,"navigator.serviceWorker.controller.scriptURL.endsWith('/app/new.js')"),"true");
}
