//! Causal fixtures for the opt-in automation boundary; no external WPT edits.
use super::actions::{finish, VENDOR};
use crate::{Engine, Page, Viewport};
use rabbithole_net::Url;
use std::time::Duration;

fn fixture(body: &str, script: &str) -> (Engine, Page) {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.loader.load_images = false;
    let html = format!("<!doctype html><html class=reftest-wait><head><style>body{{margin:0}}#target{{position:absolute;left:0;top:0;width:100px;height:100px;background:red}}</style><script>window.test_driver_internal={{in_automation:false}};</script><script>{VENDOR}</script></head><body>{body}<script>{script}</script></body></html>");
    let page = engine.load_html(&html, Url::parse("https://example.test/actions").unwrap(), Viewport {width:800.0,height:600.0});
    (engine, page)
}

const SEND: &str = r#"
function send(actions, pause=0) {
  return test_driver_internal.action_sequence([
    {type:'none',id:'none',actions:actions.map(()=>({type:'pause',duration:pause}))},
    {type:'pointer',id:'mouse',parameters:{pointerType:'mouse'},actions}
  ]);
}
const root=document.documentElement;
const target=document.getElementById('target');
"#;

fn code(script: &str) -> String { format!("{SEND}\n{script}") }
fn attr<'a>(page: &'a Page, name: &str) -> Option<&'a str> { page.doc.attr(page.doc.html_element().unwrap(), name) }

#[test]
fn mouse_actions_use_real_hover_click_and_finish_pending_ticks_after_wait_clears() {
    let (engine, mut page) = fixture(
        "<style>#target{background:green}#target:hover{background:red}#relative{position:relative;left:150px;width:100px;height:100px}</style><div id=target><div id=relative><div id=inner style='width:100px;height:100px'>click</div></div></div>",
        &code(r#"
          let events=[];
          for(const type of ['pointermove','pointerdown','mousedown','pointerup','mouseup','click']) {
            inner.addEventListener(type,event=>{events.push(type);if(!event.isTrusted)root.setAttribute('data-untrusted','yes');});
          }
          inner.addEventListener('click',()=>{
            root.setAttribute('data-hover-before',getComputedStyle(target).backgroundColor);
            relative.style.display='none';document.body.offsetTop;relative.remove();
            root.classList.remove('reftest-wait');
          });
          send([{type:'pointerMove',x:0,y:0,origin:inner},{type:'pointerDown',button:0},{type:'pointerUp',button:0},{type:'pause',duration:30}],16)
            .then(()=>{root.setAttribute('data-done','yes');root.setAttribute('data-events',events.join(','));});
        "#),
    );
    let frame = finish(&engine, &mut page, Duration::from_secs(2)).unwrap();
    assert_eq!(attr(&page,"data-hover-before"),Some("rgb(255, 0, 0)"));
    assert_eq!(attr(&page,"data-done"),Some("yes"));
    assert_eq!(attr(&page,"data-untrusted"),None);
    assert_eq!(attr(&page,"data-events"),Some("pointermove,pointerdown,mousedown,pointerup,mouseup,click"));
    let pixel = &frame.rgba[(10 * frame.width as usize + 10) * 4..][..4];
    assert_eq!(pixel, &[0,128,0,255]);
}

#[test]
fn mouse_state_and_source_are_persistent_across_queued_sends_and_repeats() {
    let (engine, mut page) = fixture("<div id=target></div>",&code(r#"
      let order=[],down=0,up=0,moves=[];
      target.addEventListener('pointerdown',()=>down++);
      target.addEventListener('pointerup',()=>up++);
      target.addEventListener('pointermove',event=>moves.push(event.clientX));
      const first=send([{type:'pointerMove',x:10,y:10},{type:'pointerDown',button:0},{type:'pointerDown',button:0}],1).then(()=>order.push(1));
      const second=send([{type:'pointerMove',x:30,y:0,origin:'pointer'},{type:'pointerUp',button:0},{type:'pointerUp',button:0}],1).then(()=>{
        order.push(2);root.setAttribute('data-state',[down,up,moves.join(':'),order.join(':')].join('|'));root.classList.remove('reftest-wait');
      });
    "#));
    finish(&engine,&mut page,Duration::from_secs(2)).unwrap();
    assert_eq!(attr(&page,"data-state"),Some("1|1|10:40|1:2"));
    assert_eq!(attr(&page,"data-untrusted"),None);
}

#[test]
fn element_origin_uses_current_rectangle_after_raf_mutation() {
    let (engine, mut page) = fixture("<div id=target></div>",&code(r#"
      requestAnimationFrame(()=>{target.style.left='200px';});
      target.addEventListener('click',event=>root.setAttribute('data-x',String(event.clientX)));
      send([{type:'pause',duration:35},{type:'pointerMove',x:0,y:0,origin:target},{type:'pointerDown',button:0},{type:'pointerUp',button:0}])
        .then(()=>root.classList.remove('reftest-wait'));
    "#));
    finish(&engine,&mut page,Duration::from_secs(2)).unwrap();
    assert_eq!(attr(&page,"data-x"),Some("250"));
}

#[test]
fn unsupported_late_action_is_rejected_before_any_input_and_cannot_fake_a_pass() {
    let (engine, mut page) = fixture("<div id=target></div>",&code(r#"
      root.setAttribute('data-moves','0');let count=0;
      target.addEventListener('pointermove',()=>root.setAttribute('data-moves',String(++count)));
      send([{type:'pointerMove',x:10,y:10},{type:'pointerMove',x:20,y:20,pressure:0.5}])
        .catch(()=>root.classList.remove('reftest-wait'));
    "#));
    let error=finish(&engine,&mut page,Duration::from_secs(2)).unwrap_err();
    assert!(error.contains("Unsupported action fields"),"{error}");
    assert_eq!(attr(&page,"data-moves"),Some("0"));
    assert!(page.pointer.hover.is_none());
}

#[test]
fn timer_navigation_during_pause_cancels_before_later_move() {
    let (engine, mut page) = fixture("<div id=target></div>",&code(r#"
      root.setAttribute('data-moves','0');
      target.addEventListener('pointermove',()=>root.setAttribute('data-moves','1'));
      send([{type:'pause',duration:80},{type:'pointerMove',x:10,y:10}]);
      setTimeout(()=>{location.href='https://example.test/next';},10);
    "#));
    let error=finish(&engine,&mut page,Duration::from_secs(2)).unwrap_err();
    assert!(error.contains("navigation"),"{error}");
    assert_eq!(attr(&page,"data-moves"),Some("0"));
}

#[test]
fn activation_navigation_cancels_remaining_actions() {
    let (engine, mut page) = fixture("<a id=target href='/next'></a>",&code(r#"
      root.setAttribute('data-moves','0');let moves=0;
      target.addEventListener('pointermove',()=>root.setAttribute('data-moves',String(++moves)));
      send([{type:'pointerMove',x:10,y:10},{type:'pointerDown',button:0},{type:'pointerUp',button:0},{type:'pointerMove',x:20,y:20}]);
    "#));
    let error=finish(&engine,&mut page,Duration::from_secs(2)).unwrap_err();
    assert!(error.contains("navigation"),"{error}");
    assert_eq!(attr(&page,"data-moves"),Some("1"));
}

#[test]
fn replacing_document_root_on_last_up_rejects_instead_of_resolving() {
    let (engine, mut page) = fixture("<div id=target></div>",&code(r#"
      target.addEventListener('click',()=>document.documentElement.replaceWith(document.createElement('html')));
      send([{type:'pointerMove',x:10,y:10},{type:'pointerDown',button:0},{type:'pointerUp',button:0}])
        .then(()=>document.documentElement.setAttribute('data-done','yes'));
    "#));
    let error=finish(&engine,&mut page,Duration::from_secs(2)).unwrap_err();
    assert!(error.contains("Document") || error.contains("document"),"{error}");
    assert_eq!(attr(&page,"data-done"),None);
}

#[test]
fn detached_element_origin_and_frame_pages_are_explicit_errors() {
    let (engine, mut page) = fixture("<div id=target></div>",&code(r#"
      send([{type:'pause',duration:50},{type:'pointerMove',x:0,y:0,origin:target}]);
      setTimeout(()=>target.remove(),10);
    "#));
    let error=finish(&engine,&mut page,Duration::from_secs(2)).unwrap_err();
    assert!(error.contains("detached"),"{error}");
    let (engine, mut frame_page)=fixture("<iframe></iframe>",&code("send([{type:'pointerMove',x:400,y:300}]);"));
    assert!(finish(&engine,&mut frame_page,Duration::from_secs(2)).unwrap_err().contains("frames"));
}

#[test]
fn wait_clear_still_waits_for_later_raf_pixels() {
    let (engine, mut page)=fixture("<div id=target></div>",&code(r#"
      target.addEventListener('click',()=>{
        root.classList.remove('reftest-wait');
        requestAnimationFrame(()=>requestAnimationFrame(()=>{target.style.background='lime';}));
      });
      send([{type:'pointerMove',x:10,y:10},{type:'pointerDown',button:0},{type:'pointerUp',button:0}]);
    "#));
    let started=std::time::Instant::now();
    let frame=finish(&engine,&mut page,Duration::from_secs(2)).unwrap();
    let pixel=&frame.rgba[(10*frame.width as usize+10)*4..][..4];
    if pixel!=[0,255,0,255] {
        // Intermittent on Windows CI only: report what the image area holds.
        let mut colors=std::collections::BTreeMap::<[u8;4],usize>::new();
        for y in 0..100usize { for x in 0..100usize {
            let o=(y*frame.width as usize+x)*4;
            *colors.entry(frame.rgba[o..o+4].try_into().unwrap()).or_default()+=1;
        }}
        let mut colors:Vec<_>=colors.into_iter().collect();
        colors.sort_by(|a,b|b.1.cmp(&a.1));
        colors.truncate(8);
        panic!("pixel {pixel:?} after {:?}; images in flight {}; image area colors {colors:?}",started.elapsed(),page.images_in_flight());
    }
}

#[test]
fn wait_clear_still_waits_for_delayed_image_pixels() {
    use image::ImageEncoder;
    use std::io::{Read,Write};
    let mut png=Vec::new();
    image::codecs::png::PngEncoder::new(&mut png).write_image(&[0,255,0,255],1,1,image::ExtendedColorType::Rgba8).unwrap();
    let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url=format!("http://{}/square.png",listener.local_addr().unwrap());
    std::thread::spawn(move||{
      let end=std::time::Instant::now()+Duration::from_secs(3);
      while std::time::Instant::now()<end {
        match listener.accept() {
          Ok((mut stream,_))=>{
            let _=stream.set_read_timeout(Some(Duration::from_secs(1)));
            let mut request=[0;2048];let _=stream.read(&mut request);
            std::thread::sleep(Duration::from_millis(60));
            let head=format!("HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",png.len());
            let _=stream.write_all(head.as_bytes());let _=stream.write_all(&png);break;
          }
          Err(error) if error.kind()==std::io::ErrorKind::WouldBlock=>std::thread::sleep(Duration::from_millis(5)),
          Err(_)=>break,
        }
      }
    });
    let (mut engine,mut page)=fixture("<div id=target><img id=image style='width:100px;height:100px'></div>",&code(&format!(r#"
      target.addEventListener('click',()=>{{image.src='{url}';root.classList.remove('reftest-wait');}});
      send([{{type:'pointerMove',x:10,y:10}},{{type:'pointerDown',button:0}},{{type:'pointerUp',button:0}}]);
    "#)));
    engine.loader.load_images=true;
    engine.loader.prefetch_images=false;
    let started=std::time::Instant::now();
    let frame=finish(&engine,&mut page,Duration::from_secs(2)).unwrap();
    let pixel=&frame.rgba[(10*frame.width as usize+10)*4..][..4];
    if pixel!=[0,255,0,255] {
        // Intermittent on Windows CI only: report what the image area holds.
        let mut colors=std::collections::BTreeMap::<[u8;4],usize>::new();
        for y in 0..100usize { for x in 0..100usize {
            let o=(y*frame.width as usize+x)*4;
            *colors.entry(frame.rgba[o..o+4].try_into().unwrap()).or_default()+=1;
        }}
        let mut colors:Vec<_>=colors.into_iter().collect();
        colors.sort_by(|a,b|b.1.cmp(&a.1));
        colors.truncate(8);
        panic!("pixel {pixel:?} after {:?}; images in flight {}; image area colors {colors:?}",started.elapsed(),page.images_in_flight());
    }
}

#[test]
fn label_activation_applies_real_focus_and_observes_its_events() {
    let (engine, mut page)=fixture("<label id=target for=control>focus</label><input id=control style='position:absolute;left:200px;top:0'>",&code(r#"
      control.addEventListener('focus',event=>root.setAttribute('data-focus',String(event.isTrusted)));
      send([{type:'pointerMove',x:10,y:10},{type:'pointerDown',button:0},{type:'pointerUp',button:0}])
        .then(()=>root.classList.remove('reftest-wait'));
    "#));
    finish(&engine,&mut page,Duration::from_secs(2)).unwrap();
    assert_eq!(page.state.focused,page.doc.element_by_id("control"));
    assert_eq!(attr(&page,"data-focus"),Some("true"));
}

#[test]
fn queued_promises_and_origins_survive_collection_and_watchdog_is_not_bypassed() {
    let (engine, mut page)=fixture("<div id=target></div>",&code(r#"
      send([{type:'pointerMove',x:0,y:0,origin:target},{type:'pointerDown',button:0},{type:'pointerUp',button:0}])
        .then(()=>{root.setAttribute('data-done','yes');root.classList.remove('reftest-wait');});
    "#));
    page.script.host.as_mut().unwrap().realm().gc();
    finish(&engine,&mut page,Duration::from_secs(2)).unwrap();
    assert_eq!(attr(&page,"data-done"),Some("yes"));
    let (engine, mut stalled)=fixture("<div id=target></div>",&code(r#"
      target.addEventListener('pointerdown',()=>{while(true){}});
      send([{type:'pointerMove',x:10,y:10},{type:'pointerDown',button:0},{type:'pointerUp',button:0}]);
    "#));
    stalled.script.host.as_mut().unwrap().realm().set_task_budget(Some(Duration::from_millis(25)));
    let error=finish(&engine,&mut stalled,Duration::from_secs(2)).unwrap_err();
    assert!(error.contains("watchdog"),"{error}");
}

#[test]
fn opt_in_vendor_is_served_without_modifying_the_corpus_and_disabled_mode_stays_empty() {
    use std::sync::atomic::{AtomicUsize,Ordering};
    static NEXT: AtomicUsize=AtomicUsize::new(0);
    let root=std::env::temp_dir().join(format!("rh-wpt-actions-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
    std::fs::create_dir_all(root.join("resources")).unwrap();
    let vendor=root.join("resources/testdriver-vendor.js");
    std::fs::write(&vendor,b"").unwrap();
    let plain_url=super::serve_dir(root.clone()).unwrap();
    let automated_url=super::serve_dir_with_options(root.clone(),super::Options{mouse_actions:true}).unwrap();
    let fetcher=rabbithole_net::Fetcher::default();
    let plain=fetcher.fetch(&Url::parse(&format!("{plain_url}resources/testdriver-vendor.js")).unwrap()).unwrap();
    let automated=fetcher.fetch(&Url::parse(&format!("{automated_url}resources/testdriver-vendor.js")).unwrap()).unwrap();
    assert!(plain.body.is_empty());
    assert_eq!(automated.body,VENDOR.as_bytes());
    assert!(std::fs::read(&vendor).unwrap().is_empty());
    let source=r#"<!doctype html><html class=reftest-wait><link rel=match href=ref.html><style>body{margin:0}#target{width:100px;height:100px;background:red}</style>
      <script>window.test_driver_internal={action_sequence(){return Promise.reject(new Error('NotImplemented'));}};</script>
      <script src='/resources/testdriver-vendor.js'></script><div id=target></div><script>
      target.addEventListener('click',()=>{target.style.background='lime';document.documentElement.classList.remove('reftest-wait');});
      test_driver_internal.action_sequence([{type:'none',id:'none',actions:[{type:'pause',duration:16},{type:'pause',duration:16},{type:'pause',duration:16}]},
        {type:'pointer',id:'mouse',parameters:{pointerType:'mouse'},actions:[{type:'pointerMove',x:50,y:50},{type:'pointerDown',button:0},{type:'pointerUp',button:0}]}]).catch(()=>{});
      </script>"#;
    std::fs::write(root.join("case.html"),source).unwrap();
    std::fs::write(root.join("ref.html"),"<!doctype html><style>body{margin:0}div{width:100px;height:100px;background:lime}</style><div></div>").unwrap();
    let mut engine=Engine::new_bundled_fonts_only();
    assert_eq!(super::run_test_with_options(&mut engine,&format!("{automated_url}case.html"),source,Duration::from_secs(2),None,super::Options{mouse_actions:true}),super::Outcome::Pass);
    let mut disabled=Engine::new_bundled_fonts_only();
    assert!(matches!(super::run_test(&mut disabled,&format!("{plain_url}case.html"),source,Duration::from_millis(30),None),super::Outcome::Error(_)));
    std::fs::write(&vendor,b"// a different vendor").unwrap();
    assert!(super::serve_dir_with_options(root.clone(),super::Options{mouse_actions:true}).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn queue_bounds_are_checked_before_any_input_is_published() {
    let (engine, mut page)=fixture("<div id=target></div>",&code(r#"
      root.setAttribute('data-moves','0');
      target.addEventListener('pointermove',()=>root.setAttribute('data-moves','1'));
      send(Array.from({length:129},()=>({type:'pointerMove',x:10,y:10}))).catch(()=>root.classList.remove('reftest-wait'));
    "#));
    let error=finish(&engine,&mut page,Duration::from_secs(2)).unwrap_err();
    assert!(error.contains("Too many action ticks"),"{error}");
    assert_eq!(attr(&page,"data-moves"),Some("0"));
}
