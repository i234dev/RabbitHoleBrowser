//! The GUI's real ContentHandle -> content thread -> async Loader protocol.
use super::*;
use std::sync::atomic::AtomicBool;
use std::collections::BTreeMap;

struct Server {
    origin: String,
    stop: Arc<AtomicBool>,
    task: Option<std::thread::JoinHandle<()>>,
    requests: Arc<Mutex<Vec<(String,String)>>>,
    script_gate: Arc<AtomicBool>,
}
impl Server {
    fn new(routes: &[(&str,u16,&str,&str,&[(&str,&str)])]) -> Self {
        let routes: BTreeMap<_,_> = routes.iter().map(|(path,status,mime,body,headers)| (path.to_string(),(*status,mime.to_string(),body.to_string(),headers.iter().map(|(k,v)|(k.to_string(),v.to_string())).collect::<Vec<_>>()))).collect();
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();listener.set_nonblocking(true).unwrap();
        let origin=format!("http://{}",listener.local_addr().unwrap());let stop=Arc::new(AtomicBool::new(false));let halt=stop.clone();
        let requests=Arc::new(Mutex::new(Vec::new()));let seen=requests.clone();
        let script_gate=Arc::new(AtomicBool::new(false));let release_script=script_gate.clone();
        let task=std::thread::spawn(move || while !halt.load(Ordering::Relaxed) {
            let (mut stream,_)=match listener.accept(){Ok(s)=>s,Err(_)=>{std::thread::sleep(Duration::from_millis(1));continue;}};
            stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut reader=std::io::BufReader::new(stream.try_clone().unwrap());let mut first=String::new();
            if std::io::BufRead::read_line(&mut reader,&mut first).is_err(){continue;}
            let path=first.split_whitespace().nth(1).unwrap_or("/").to_string();let mut headers=String::new();
            loop{let mut line=String::new();if std::io::BufRead::read_line(&mut reader,&mut line).unwrap_or(0)==0||line=="\r\n"{break;}headers.push_str(&line);}
            seen.lock().unwrap().push((path.clone(),headers));
            let (status,mime,body,extra)=routes.get(path.split('?').next().unwrap()).map(|(s,m,b,h)|(*s,m.as_str(),b.as_str(),h.as_slice())).unwrap_or((404,"text/plain","not found",&[]));
            if path=="/app/slow.js" { while !release_script.load(Ordering::Relaxed) && !halt.load(Ordering::Relaxed) { std::thread::sleep(Duration::from_millis(1)); } }
            let _=write!(stream,"HTTP/1.1 {status} OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n",body.len());
            for(k,v)in extra{let _=write!(stream,"{k}: {v}\r\n");}let _=write!(stream,"\r\n{body}");
        });
        Self{origin,stop,task:Some(task),requests,script_gate}
    }
}
impl Drop for Server{fn drop(&mut self){self.stop.store(true,Ordering::Relaxed);if let Some(task)=self.task.take(){let _=task.join();}}}
fn eval(h:&ContentHandle,rx:&Received,tab:u64,source:&str)->String{
    h.send(ToContent::Eval(source.into()));let (_,result)=wait_for(rx,tab,Duration::from_secs(15),|m|matches!(m,FromContent::EvalResult(_))).expect("content thread must remain responsive");
    let FromContent::EvalResult(result)=result else{unreachable!()};result.trim().trim_matches('\'').to_string()
}
fn until(h:&ContentHandle,rx:&Received,tab:u64,source:&str){
    let deadline=Instant::now()+Duration::from_secs(15);
    loop{if eval(h,rx,tab,source)=="true"{return;}assert!(Instant::now()<deadline,"condition: {source}");std::thread::sleep(Duration::from_millis(10));}
}
fn navigate(h:&ContentHandle,target:&str){h.send(ToContent::Navigate{request:Request::get(url(target)),push_history:true});}
fn committed(rx:&Received,tab:u64,title:&str){
    wait_for(rx,tab,Duration::from_secs(15),|m|matches!(m,FromContent::Committed{title:t,..} if t==title)).unwrap_or_else(||panic!("missing GUI commit: {title}"));
}
const START:&str="<!doctype html><title>Start</title><script>var controlled=false;navigator.serviceWorker.register('sw.js');navigator.serviceWorker.ready.then(r=>controlled=navigator.serviceWorker.controller===r.active);</script>";

#[test]
fn gui_service_worker_navigation_really_loads_cached_html_after_server_stops(){
    let offline="<!doctype html><title>Offline café 🐇</title><main id=cached>cached bytes</main><script>var ran=true,id;fetch('client-id').then(r=>r.text()).then(value=>id=value);</script>";
    let worker=r#"oninstall=e=>e.waitUntil(fetch('offline.html').then(r=>r.arrayBuffer()).then(b=>caches.open('pages').then(c=>c.put('offline.html',new Response(b,{status:203,headers:{'content-type':'text/html; charset=UTF-8'}})))));onactivate=e=>e.waitUntil(clients.claim());onfetch=e=>{if(e.request.mode==='navigate'){e.respondWith(caches.match(e.request));e.waitUntil(caches.open('ids').then(c=>c.put('nav-id',new Response(e.resultingClientId))));}else if(e.request.url.endsWith('/client-id'))e.respondWith(new Response(e.clientId));};"#;
    let server=Server::new(&[("/app/start",200,"text/html",START,&[]),("/app/sw.js",200,"text/javascript",worker,&[]),("/app/offline.html",200,"text/html",offline,&[])]);
    let (sink,rx)=sink();let h=spawn(71,&sink);navigate(&h,&format!("{}/app/start",server.origin));committed(&rx,71,"Start");until(&h,&rx,71,"controlled===true");
    let origin=server.origin.clone();let seen=server.requests.clone();drop(server);
    navigate(&h,&format!("{origin}/app/offline.html#kept"));committed(&rx,71,"Offline café 🐇");
    until(&h,&rx,71,"typeof id==='string'");
    assert_eq!(eval(&h,&rx,71,"ran&&document.querySelector('#cached').textContent==='cached bytes'&&location.hash==='#kept'"),"true");
    eval(&h,&rx,71,"var sameClient=false;caches.match('nav-id').then(r=>r.text()).then(value=>sameClient=value===id);");until(&h,&rx,71,"sameClient===true");
    assert_eq!(seen.lock().unwrap().iter().filter(|(p,_)|p=="/app/offline.html").count(),1,"only installation's precache reached HTTP");
    let other=spawn(72,&sink);navigate(&other,&format!("{origin}/app/offline.html"));
    wait_for(&rx,72,Duration::from_secs(15),|m|matches!(m,FromContent::Committed{..})).expect("other tab error page");
    assert_ne!(eval(&other,&rx,72,"document.title"),"Offline café 🐇","tabs must not inherit another Engine's registry/cache");
}

#[test]
fn gui_service_worker_navigation_passes_live_headers_and_follows_http_redirects(){
    let worker=r#"onactivate=e=>e.waitUntil(clients.claim());onfetch=e=>{if(e.request.mode==='navigate'){e.request.headers.set('X-Live','from-worker');e.request.headers.set('Cookie','forged=bad');e.respondWith(fetch(e.request.clone()));}};"#;
    let done="<!doctype html><title>HTTP done</title><script>var uncontrolled=navigator.serviceWorker.controller===null;</script>";
    let server=Server::new(&[("/app/start",200,"text/html",START,&[]),("/app/sw.js",200,"text/javascript",worker,&[]),("/app/forward",302,"text/html","",&[("Location","/outside/done")]),("/outside/done",200,"text/html",done,&[])]);
    let (sink,rx)=sink();let h=spawn(73,&sink);navigate(&h,&format!("{}/app/start",server.origin));committed(&rx,73,"Start");until(&h,&rx,73,"controlled===true");
    let mut request=Request::get(url(&format!("{}/app/forward#kept",server.origin))).with_header("X-Client","gui");request.referer=Some(url(&format!("{}/app/start",server.origin)));
    h.send(ToContent::Navigate{request,push_history:true});committed(&rx,73,"HTTP done");until(&h,&rx,73,"typeof uncontrolled==='boolean'");assert_eq!(eval(&h,&rx,73,"uncontrolled&&location.hash==='#kept'"),"true");
    let seen=server.requests.lock().unwrap();let forwarded:Vec<_>=seen.iter().filter(|(p,_)|p=="/app/forward").collect();assert_eq!(forwarded.len(),1,"no recursive SW interception or duplicate loader fetch");
    let headers=forwarded[0].1.to_ascii_lowercase();assert!(headers.contains("x-live: from-worker")&&headers.contains("x-client: gui")&&headers.contains("sec-fetch-mode: navigate"),"{headers}");assert!(!headers.contains("forged=bad"),"{headers}");
    assert_eq!(seen.iter().filter(|(p,_)|p=="/outside/done").count(),1);
}

#[test]
fn gui_stop_and_supersede_abort_pending_worker_navigation_without_losing_old_page(){
    let worker=r#"onactivate=e=>e.waitUntil(clients.claim());onfetch=e=>{if(e.request.mode!=='navigate')return;const path=new URL(e.request.url).pathname;if(path.endsWith('/hang')){e.waitUntil(caches.open('flags').then(c=>c.put('seen',new Response('yes'))));e.request.signal.addEventListener('abort',()=>caches.open('flags').then(c=>c.put('aborted',new Response('yes'))));e.respondWith(new Promise(()=>{}));}else if(path.endsWith('/next'))e.respondWith(new Response('<!doctype html><title>Next cached</title>',{headers:{'content-type':'text/html'}}));};"#;
    let server=Server::new(&[("/app/start",200,"text/html",START,&[]),("/app/sw.js",200,"text/javascript",worker,&[])]);
    let(sink,rx)=sink();let h=spawn(74,&sink);navigate(&h,&format!("{}/app/start",server.origin));committed(&rx,74,"Start");until(&h,&rx,74,"controlled===true");
    navigate(&h,&format!("{}/app/hang",server.origin));
    eval(&h,&rx,74,"var seen=false;function poll(){caches.match('seen').then(r=>{if(r)seen=true;else setTimeout(poll,5);});}poll();");until(&h,&rx,74,"seen===true");
    let started=Instant::now();assert_eq!(eval(&h,&rx,74,"document.title"),"Start");assert!(started.elapsed()<Duration::from_secs(2),"pending respondWith must not block content input");
    h.send(ToContent::Stop);eval(&h,&rx,74,"var aborted=false;function checkAbort(){caches.match('aborted').then(r=>{if(r)aborted=true;else setTimeout(checkAbort,5);});}checkAbort();");until(&h,&rx,74,"aborted===true");assert_eq!(eval(&h,&rx,74,"document.title"),"Start");
    navigate(&h,&format!("{}/app/hang",server.origin));assert_eq!(eval(&h,&rx,74,"document.title"),"Start");navigate(&h,&format!("{}/app/next",server.origin));committed(&rx,74,"Next cached");
    assert!(!server.requests.lock().unwrap().iter().any(|(p,_)|p=="/app/hang"||p=="/app/next"));
}

#[test]
fn gui_navigation_keeps_reserved_client_and_realm_while_external_scripts_arrive(){
    let worker=r#"onactivate=e=>e.waitUntil(clients.claim());onfetch=e=>{if(e.request.mode==='navigate'){e.waitUntil(caches.open('ids').then(c=>c.put('nav-id',new Response(e.resultingClientId))));e.respondWith(new Response('<!doctype html><title>Deferred SW</title><script src="/app/slow.js"></script>',{headers:{'content-type':'text/html'}}));}else if(e.request.url.endsWith('/client-id'))e.respondWith(new Response(e.clientId));};"#;
    let server=Server::new(&[("/app/start",200,"text/html",START,&[]),("/app/sw.js",200,"text/javascript",worker,&[]),("/app/slow.js",200,"text/javascript","var deferredRan=true;",&[])]);
    let(sink,rx)=sink();let h=spawn(75,&sink);navigate(&h,&format!("{}/app/start",server.origin));committed(&rx,75,"Start");until(&h,&rx,75,"controlled===true");
    navigate(&h,&format!("{}/app/deferred",server.origin));committed(&rx,75,"Deferred SW");
    eval(&h,&rx,75,"var beforeExternal='preserved',initialClient;fetch('client-id').then(r=>r.text()).then(value=>initialClient=value);");
    until(&h,&rx,75,"typeof initialClient==='string'");
    assert_eq!(eval(&h,&rx,75,"beforeExternal==='preserved'&&typeof deferredRan==='undefined'"),"true","server must hold external script until explicit release");
    eval(&h,&rx,75,"var sameReserved=false;caches.match('nav-id').then(r=>r.text()).then(value=>sameReserved=value===initialClient);");until(&h,&rx,75,"sameReserved===true");
    server.script_gate.store(true,Ordering::Relaxed);
    until(&h,&rx,75,"typeof deferredRan==='boolean'");
    assert_eq!(eval(&h,&rx,75,"beforeExternal==='preserved'&&sameReserved"),"true","starting deferred scripts must reuse the committed realm/client");
    assert_eq!(server.requests.lock().unwrap().iter().filter(|(path,_)|path=="/app/slow.js").count(),1);
}

#[test]
fn gui_navigation_referrer_privacy_cannot_be_undone_by_clones_or_policy_override(){
    let worker=r#"onactivate=e=>e.waitUntil(clients.claim());onfetch=e=>{if(e.request.mode!=='navigate')return;
      const original=e.request,cloned=original.clone(),copied=new Request(original),unsafe=new Request(original,{referrerPolicy:'unsafe-url'});
      const changed=original.clone();Object.defineProperty(changed,'referrerPolicy',{value:'unsafe-url',configurable:true});
      const refs=[original.referrer,cloned.referrer,copied.referrer,unsafe.referrer,changed.referrer];
      const policyChanged=unsafe.referrerPolicy==='unsafe-url'&&changed.referrerPolicy==='unsafe-url';
      e.respondWith(Promise.all([original,cloned,copied,unsafe,changed].map(r=>fetch(r))).then(()=>new Response('<!doctype html><title>Referrer SW</title><script>var refs='+JSON.stringify(refs)+',policyChanged='+JSON.stringify(policyChanged)+';</script>',{headers:{'content-type':'text/html'}})));
    };"#;
    let source=Server::new(&[]);
    let destination=Server::new(&[("/app/start",200,"text/html",START,&[]),("/app/sw.js",200,"text/javascript",worker,&[]),("/app/referrer",200,"text/html","<!doctype html><title>Wire only</title>",&[])]);
    let(sink,rx)=sink();let h=spawn(76,&sink);navigate(&h,&format!("{}/app/start",destination.origin));committed(&rx,76,"Start");until(&h,&rx,76,"controlled===true");
    for (policy,expected) in [(rabbithole_engine::net::cors::ReferrerPolicy::StrictOriginWhenCrossOrigin,format!("{}/",source.origin)),(rabbithole_engine::net::cors::ReferrerPolicy::NoReferrer,String::new())] {
      let source_url=url(&format!("{}/private-path?query=secret-value#hidden",source.origin));
      h.send(ToContent::LoadHtml{html:"<!doctype html><title>Source origin</title>".into(),url:source_url.clone(),push_history:true});committed(&rx,76,"Source origin");
      let request=Request::get(url(&format!("{}/app/referrer",destination.origin))).with_referer(source_url).with_referrer_policy(policy);
      h.send(ToContent::Navigate{request,push_history:true});committed(&rx,76,"Referrer SW");until(&h,&rx,76,"typeof refs==='object'");
      assert_eq!(eval(&h,&rx,76,"policyChanged&&refs.every(r=>!r.includes('private-path')&&!r.includes('secret-value')&&!r.includes('hidden'))"),"true");
      assert_eq!(eval(&h,&rx,76,"refs.slice(0,3).every(r=>r===refs[0])"),"true");assert_eq!(eval(&h,&rx,76,"refs[0]"),expected);
    }
    let seen=destination.requests.lock().unwrap();let forwarded:Vec<_>=seen.iter().filter(|(path,_)|path=="/app/referrer").collect();assert_eq!(forwarded.len(),10,"5 forwards for each policy, no recursion");
    for(_,headers)in&forwarded{assert!(!headers.contains("private-path")&&!headers.contains("secret-value")&&!headers.contains("hidden"),"discarded referrer escaped to wire: {headers}");}
    assert!(forwarded.iter().any(|(_,headers)|headers.to_ascii_lowercase().contains("sec-fetch-site: same-site")),"referrer stripping must retain origin-only initiator metadata: {forwarded:?}");
}
