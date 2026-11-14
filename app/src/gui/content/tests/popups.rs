//! Real content threads and the same context owner/transport used by the GUI.
//! The harness handles only native UI tab creation/focus/close notifications.
use super::*;
use crate::gui::browsing_context::{BrowsingContexts, WindowBinding};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

struct Server { origin:String, stop:Arc<AtomicBool>, task:Option<std::thread::JoinHandle<()>> }
impl Server {
    fn new() -> Self {
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();listener.set_nonblocking(true).unwrap();
        let origin=format!("http://{}",listener.local_addr().unwrap());let stop=Arc::new(AtomicBool::new(false));let halt=stop.clone();
        let task=std::thread::spawn(move || while !halt.load(Ordering::Relaxed) {
            let(mut stream,_)=match listener.accept(){Ok(value)=>value,Err(_)=>{std::thread::sleep(Duration::from_millis(1));continue;}};
            stream.set_nonblocking(false).unwrap();stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut reader=std::io::BufReader::new(stream.try_clone().unwrap());let mut line=String::new();
            if std::io::BufRead::read_line(&mut reader,&mut line).is_err(){continue;}
            let path=line.split_whitespace().nth(1).unwrap_or("/").to_owned();
            loop{line.clear();if std::io::BufRead::read_line(&mut reader,&mut line).unwrap_or(0)==0||line=="\r\n"{break;}}
            let body=if path=="/callback" {r#"<!doctype html><title>Callback</title><script>opener.postMessage({kind:'callback',value:42},location.origin);close();</script>"#}else{"<!doctype html><title>Popup parent</title><body>local fixture</body>"};
            let _=write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=UTF-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
        });Self{origin,stop,task:Some(task)}
    }
}
impl Drop for Server{fn drop(&mut self){self.stop.store(true,Ordering::Relaxed);if let Some(task)=self.task.take(){task.join().unwrap();}}}
struct Ui {
    registry:Arc<BrowsingContexts>,sink:Sink,rx:Received,
    handles:BTreeMap<u64,ContentHandle>,next_tab:u64,
    commits:Vec<(u64,String)>,focus:Vec<u64>,opened:Vec<(u64,u64)>,
}
impl Ui {
    fn new()->Self {let(sink,rx)=sink();let registry=BrowsingContexts::new(sink.clone());Self{registry,sink,rx,handles:BTreeMap::new(),next_tab:1,commits:Vec::new(),focus:Vec::new(),opened:Vec::new()}}
    fn spawn(&mut self,tab:u64,epoch:u64,binding:WindowBinding){
        let mut cfg=config(ViewSize::new(320,240,1.0));cfg.window_context=Some(binding);
        let handle=ContentHandle::spawn(tab,epoch,cfg,self.sink.clone());handle.send(ToContent::SetActive(true));self.handles.insert(tab,handle);
    }
    fn parent(&mut self,origin:&str)->u64 {
        let tab=self.next_tab;self.next_tab+=1;let binding=self.registry.reserve_tab(tab,1);self.spawn(tab,1,binding);
        self.handles[&tab].send(ToContent::Navigate{request:Request::get(url(&format!("{origin}/parent"))),push_history:true});
        self.until_commit(tab,"Popup parent");tab
    }
    fn pump(&mut self,timeout:Duration)->Option<(u64,FromContent)>{
        let(_,tab,event)=self.rx.recv_timeout(timeout).ok()?;
        match &event {
            FromContent::OpenContext(id)=>{
                self.registry.creation_consumed(*id);let next=self.next_tab;self.next_tab+=1;
                if let Some(binding)=self.registry.popup_binding(*id,next,1){self.opened.push((*id,next));self.spawn(next,1,binding);}
            }
            FromContent::FocusContext(id)=>{self.registry.focus_consumed(*id);self.focus.push(*id);}
            FromContent::CloseContext(id)=>{if let Some((tab,_))=self.registry.owner(*id){self.handles.remove(&tab);}}
            FromContent::Committed{title,..}=>self.commits.push((tab,title.clone())),
            FromContent::Crashed(reason)=>panic!("content thread {tab} crashed: {reason}"),
            _=>{}
        }
        Some((tab,event))
    }
    fn until_commit(&mut self,tab:u64,title:&str){let deadline=Instant::now()+Duration::from_secs(20);while !self.commits.iter().any(|(id,t)|*id==tab&&t==title){assert!(Instant::now()<deadline,"missing commit {tab}: {title}");self.pump(Duration::from_millis(50));}}
    fn eval(&mut self,tab:u64,source:&str)->String{
        self.handles[&tab].send(ToContent::Eval(source.into()));let deadline=Instant::now()+Duration::from_secs(15);
        loop{assert!(Instant::now()<deadline,"eval timeout tab {tab}: {source}");if let Some((id,FromContent::EvalResult(value)))=self.pump(Duration::from_millis(50)){if id==tab{return value.trim().trim_matches('\'').to_owned();}}}
    }
    fn until(&mut self,tab:u64,source:&str){let deadline=Instant::now()+Duration::from_secs(20);loop{if self.eval(tab,source)=="true"{return;}assert!(Instant::now()<deadline,"condition tab {tab}: {source}");self.pump(Duration::from_millis(5));}}
    fn opened_count(&mut self,count:usize){let deadline=Instant::now()+Duration::from_secs(15);while self.opened.len()<count{assert!(Instant::now()<deadline,"missing popup");self.pump(Duration::from_millis(20));}}
}

#[test]
fn gui_popup_blank_is_real_inherits_origin_and_supports_named_reuse_and_noopener(){
    let server=Server::new();let mut ui=Ui::new();let parent=ui.parent(&server.origin);
    assert_eq!(ui.eval(parent,"var p=open('', 'login');p!==null&&!p.closed&&p===open('about:blank','login')"),"true");
    ui.opened_count(1);let(context,child)=ui.opened[0];ui.until(child,"document.readyState!=='loading'");
    assert_eq!(ui.eval(child,"opener!==null&&document.URL==='about:blank'&&document.baseURI===opener.location.href"),"true");
    assert_eq!(ui.registry.info(context).unwrap().origin,server.origin);
    assert_eq!(ui.eval(parent,"open('', 'isolated', 'noopener')===null"),"true");ui.opened_count(2);let isolated=ui.opened[1].1;
    assert_eq!(ui.eval(isolated,"opener===null&&document.URL==='about:blank'"),"true");
    assert_eq!(ui.eval(parent,"p.focus();p.close();p.closed"),"true");
    ui.until(parent,"p.closed===true");
}

#[test]
fn gui_popup_deferred_location_callback_delivers_trusted_message_even_after_sender_close(){
    let server=Server::new();let mut ui=Ui::new();let parent=ui.parent(&server.origin);
    assert_eq!(ui.eval(parent,"var callback=false;var p=open('','callback');onmessage=e=>{callback=e.isTrusted&&e.source===p&&e.origin===location.origin&&e.data.kind==='callback'&&e.data.value===42;};p.location='/callback';p.focus();p!==null"),"true");
    ui.until(parent,"callback&&p.closed");assert_eq!(ui.opened.len(),1);
    assert!(ui.focus.contains(&ui.opened[0].0),"native focus notification reached the UI");
}

#[test]
fn gui_popup_message_origin_guard_and_structured_clone_cross_real_content_threads(){
    let server=Server::new();let mut ui=Ui::new();let parent=ui.parent(&server.origin);
    assert_eq!(ui.eval(parent,"var bad=false,good=false;var p=open('','messages');onmessage=e=>{if(e.data.bad)bad=true;if(e.data.good)good=e.isTrusted&&e.source===p&&e.origin===location.origin&&e.data.items[1]===2;};p!==null"),"true");
    ui.opened_count(1);let child=ui.opened[0].1;
    assert_eq!(ui.eval(child,"var payload={good:true,items:[1,2]};opener.postMessage({bad:true},'https://wrong.example.test');opener.postMessage(payload,'*');payload.items[1]=99;true"),"true");
    ui.until(parent,"good===true");assert_eq!(ui.eval(parent,"bad===false"),"true");
}

#[test]
fn gui_popup_renderer_replacement_preserves_proxy_identity_and_revokes_old_owner(){
    let server=Server::new();let mut ui=Ui::new();let parent=ui.parent(&server.origin);
    assert_eq!(ui.eval(parent,"var p=open('','replace');p!==null"),"true");ui.opened_count(1);let(context,child)=ui.opened[0];
    let binding=ui.registry.reserve_tab(child,2);assert_eq!(binding.id,context);let old=ui.handles.remove(&child).unwrap();
    ui.spawn(child,2,binding);drop(old);
    ui.handles[&child].send(ToContent::LoadHtml{html:"<!doctype html><title>Replacement</title>".into(),url:url(&format!("{}/replacement",server.origin)),push_history:false});ui.until_commit(child,"Replacement");
    assert_eq!(ui.eval(parent,"!p.closed&&p===open('', 'replace')&&p.location.href.endsWith('/replacement')"),"true");
    assert_eq!(ui.registry.owner(context),Some((child,2)));
}
