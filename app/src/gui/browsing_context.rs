//! Real top-level context ownership. Only owned messages cross content threads.
//! A reservation stays tied to a UI creation request; closed contexts never
//! reopen and commands queued before attachment preserve source order.
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc::Sender};
use rabbithole_engine::net::{Request, Url};
use rabbithole_engine::script::{WindowContext, WindowHost, WindowInfo, WindowMessage, WindowOpenRequest};
use super::content::{FromContent, Inbox, Sink, ToContent};

const MAX_CONTEXTS: usize = 256;
const MAX_PENDING: usize = 256;
const MAX_NAME_BYTES: usize = 8192;
const MAX_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_CONTEXT_BYTES: usize = 16 * 1024 * 1024;
const MAX_GLOBAL_BYTES: usize = 64 * 1024 * 1024;
const MAX_GLOBAL_COMMANDS: usize = 4096;
const MAX_URL_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub struct BlankOrigin {
    pub origin: String,
    pub key: String,
    pub base_url: Url,
}
#[derive(Clone)]
pub struct WindowBinding {
    pub registry: Arc<BrowsingContexts>,
    pub id: u64,
    pub initial_blank: bool,
    tab: u64,
    epoch: u64,
}
impl WindowBinding {
    pub fn context(&self) -> WindowContext { WindowContext { id: self.id, host: Arc::new(RendererWindowHost { registry:self.registry.clone(),id:self.id,tab:self.tab,epoch:self.epoch }) } }
}
struct Record {
    info: WindowInfo,
    group: u64,
    script_opened: bool,
    owner: Option<(u64,u64)>,
    creator_owner: (u64,u64),
    route: Option<Sender<Inbox>>,
    ready: bool,
    queued: VecDeque<ToContent>,
    focus_when_attached: bool,
    focus_notified: bool,
    creation_notified: bool,
    pending: BTreeMap<u64, Pending>,
    pending_bytes: usize,
}
struct Pending { bytes:usize, owner:Option<(u64,u64)> }
#[derive(Default)]
struct State { next:u64, opaque:u64, ticket:u64, pending_bytes:usize, pending_commands:usize, creations:usize, records:BTreeMap<u64,Record> }
pub struct BrowsingContexts { state: Mutex<State>, sink: Sink, nonce: u64 }
impl BrowsingContexts {
    pub fn new(sink: Sink) -> Arc<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Arc::new(Self { state: Mutex::new(State::default()), sink, nonce: NEXT.fetch_add(1,Ordering::Relaxed) })
    }
    fn opaque(&self, state: &mut State) -> String { state.opaque += 1; format!("opaque:{}:{}",self.nonce,state.opaque) }
    fn blank() -> Url { Url::parse_absolute("about:blank").unwrap() }
    pub fn reserve_tab(self: &Arc<Self>, tab: u64, epoch: u64) -> WindowBinding {
        let mut s=self.state.lock().unwrap();
        if let Some((&id,r))=s.records.iter_mut().find(|(_,r)|!r.info.closed&&r.owner.is_some_and(|(owner,_)|owner==tab)) {
            r.owner=Some((tab,epoch));r.route=None;r.ready=false;
            return WindowBinding{registry:self.clone(),id,initial_blank:false,tab,epoch};
        }
        if s.records.len()>4096{s.records.retain(|_,r|!r.info.closed||r.owner.is_some()||!r.pending.is_empty()||r.creation_notified);}
        s.next+=1;let id=s.next;let key=self.opaque(&mut s);
        s.records.insert(id,Record { info:WindowInfo { url:Self::blank(),base_url:Self::blank(),origin:"null".into(),origin_key:key,opener:None,closed:false,name:String::new() }, group:id,script_opened:false,owner:Some((tab,epoch)),creator_owner:(tab,epoch),route:None,ready:false,queued:VecDeque::new(),focus_when_attached:false,focus_notified:false,creation_notified:false,pending:BTreeMap::new(),pending_bytes:0 });
        WindowBinding { registry:self.clone(),id,initial_blank:false,tab,epoch }
    }
    /// UI adopts an already reserved popup, never a second context identity.
    pub fn popup_binding(self: &Arc<Self>, id:u64, tab:u64, epoch:u64) -> Option<WindowBinding> {
        let mut s=self.state.lock().unwrap();let r=s.records.get_mut(&id)?;
        if r.info.closed || !r.script_opened || r.owner.is_some() { return None; }
        r.owner=Some((tab,epoch));
        Some(WindowBinding {registry:self.clone(),id,initial_blank:true,tab,epoch})
    }
    pub(crate) fn attach(&self,id:u64,tab:u64,epoch:u64,tx:Sender<Inbox>) {
        let mut s=self.state.lock().unwrap();let Some(r)=s.records.get_mut(&id)else{return};
        if !r.info.closed&&r.owner==Some((tab,epoch)){r.route=Some(tx);r.ready=false;}
    }
    /// No external commands run until the initial/replacement document exists.
    pub(crate) fn ready(&self,id:u64,tab:u64,epoch:u64){
        let focus={let mut s=self.state.lock().unwrap();let Some(r)=s.records.get_mut(&id)else{return};
            if r.info.closed||r.owner!=Some((tab,epoch))||r.ready{return;}
            r.ready=true;
            if let Some(tx)=r.route.clone(){while let Some(command)=r.queued.pop_front(){
                let ticket=Self::ticket_of(&command);
                match tx.send(Inbox::Ui(command)) {
                    Ok(())=>{if let Some(p)=r.pending.get_mut(&ticket){p.owner=Some((tab,epoch));}},
                    Err(std::sync::mpsc::SendError(Inbox::Ui(command)))=>{r.queued.push_front(command);r.route=None;r.ready=false;break;},
                    Err(_)=>unreachable!(),
                }
            }}
            let notify=r.focus_when_attached&&!r.focus_notified;r.focus_notified|=notify;notify
        };
        if focus{(self.sink)(tab,epoch,FromContent::FocusContext(id));}
    }
    pub(crate) fn acknowledge(&self,id:u64,tab:u64,epoch:u64,ticket:u64)->bool{
        let mut s=self.state.lock().unwrap();let (bytes,current)={let Some(r)=s.records.get_mut(&id)else{return false};
            if r.pending.get(&ticket).is_none_or(|p|p.owner!=Some((tab,epoch))){return false;}
            let p=r.pending.remove(&ticket).unwrap();r.pending_bytes-=p.bytes;(p.bytes,!r.info.closed&&r.owner==Some((tab,epoch)))
        };s.pending_bytes-=bytes;s.pending_commands-=1;current
    }
    pub(crate) fn renderer_gone(&self,id:u64,tab:u64,epoch:u64){
        let mut s=self.state.lock().unwrap();let (bytes,count)={let Some(r)=s.records.get_mut(&id)else{return};
            let tickets:Vec<_>=r.pending.iter().filter(|(_,p)|p.owner==Some((tab,epoch))).map(|(&ticket,_)|ticket).collect();
            let mut bytes=0;let count=tickets.len();for ticket in tickets{bytes+=r.pending.remove(&ticket).unwrap().bytes;}
            r.pending_bytes-=bytes;
            if r.owner==Some((tab,epoch)){r.route=None;r.ready=false;}(bytes,count)
        };s.pending_bytes-=bytes;s.pending_commands-=count;
    }
    fn clear_unsent(s:&mut State,id:u64){
        let (bytes,count)={let Some(r)=s.records.get_mut(&id)else{return};
            let tickets:Vec<_>=r.pending.iter().filter(|(_,p)|p.owner.is_none()).map(|(&ticket,_)|ticket).collect();let count=tickets.len();let mut bytes=0;
            for ticket in tickets{bytes+=r.pending.remove(&ticket).unwrap().bytes;}r.pending_bytes-=bytes;r.queued.clear();(bytes,count)
        };s.pending_bytes-=bytes;s.pending_commands-=count;
    }
    pub(crate) fn detach(&self,id:u64,tab:u64,epoch:u64) {
        let mut s=self.state.lock().unwrap();let current=s.records.get(&id).is_some_and(|r|r.owner==Some((tab,epoch)));
        if current{Self::clear_unsent(&mut s,id);let r=s.records.get_mut(&id).unwrap();r.info.closed=true;r.route=None;r.ready=false;r.owner=None;}
    }
    pub fn creation_consumed(&self,id:u64){let mut s=self.state.lock().unwrap();let notify=s.records.get(&id).is_some_and(|r|r.creation_notified);if notify{s.records.get_mut(&id).unwrap().creation_notified=false;s.creations-=1;}}
    pub fn focus_consumed(&self,id:u64){if let Some(r)=self.state.lock().unwrap().records.get_mut(&id){r.focus_notified=false;}}
    /// Trusted UI inspection; script receives only a renderer-leased adapter.
    pub fn info(&self,id:u64)->Option<WindowInfo>{self.state.lock().unwrap().records.get(&id).map(|r|r.info.clone())}
    pub fn owner(&self,id:u64)->Option<(u64,u64)>{self.state.lock().unwrap().records.get(&id).and_then(|r|r.owner)}
    pub fn creator_tab(&self,id:u64)->Option<u64>{self.state.lock().unwrap().records.get(&id).map(|r|r.creator_owner.0)}
    pub(crate) fn prepare_blank(&self,id:u64,tab:u64,epoch:u64,seed:BlankOrigin)->bool{
        let mut s=self.state.lock().unwrap();let Some(r)=s.records.get_mut(&id)else{return false};
        if r.info.closed||r.owner!=Some((tab,epoch)){return false;}
        r.info.url=Self::blank();r.info.origin=seed.origin;r.info.origin_key=seed.key;r.info.base_url=seed.base_url;true
    }
    pub(crate) fn allow_javascript(&self,id:u64,tab:u64,epoch:u64,expected:&str)->bool{
        let s=self.state.lock().unwrap();!expected.is_empty()&&s.records.get(&id).is_some_and(|r|!r.info.closed&&r.owner==Some((tab,epoch))&&r.info.origin_key==expected)
    }
    pub fn same_document_url(&self,id:u64,url:Url){let mut s=self.state.lock().unwrap();if let Some(r)=s.records.get_mut(&id){if !r.info.closed{r.info.url=url;}}}
    fn related(s:&State,source:u64,target:u64)->bool{
        matches!((s.records.get(&source),s.records.get(&target)),(Some(a),Some(b)) if !a.info.closed&&!b.info.closed&&a.group==b.group)
    }
    fn url_bytes(url:&Url)->usize{url.scheme.len().saturating_add(url.host.len()).saturating_add(url.path.len()).saturating_add(url.query.as_ref().map_or(0,String::len)).saturating_add(url.fragment.as_ref().map_or(0,String::len)).saturating_add(url.username.len()).saturating_add(url.password.len())}
    fn allowed_url(url:&Url)->bool{Self::url_bytes(url)<=MAX_URL_BYTES&&(matches!(url.scheme.as_str(),"http"|"https"|"data"|"javascript")||(url.scheme=="about"&&url.path=="blank"))}
    fn ticket_of(command:&ToContent)->u64{match command{ToContent::ContextNavigate{ticket,..}|ToContent::ContextMessage{ticket,..}=>*ticket,_=>unreachable!("only context commands are queued")}}
    fn enqueue(s:&mut State,target:u64,bytes:usize,command:impl FnOnce(u64)->ToContent)->bool{
        let Some(r)=s.records.get(&target)else{return false};
        if r.info.closed||r.pending.len()>=MAX_PENDING||bytes>MAX_CONTEXT_BYTES.saturating_sub(r.pending_bytes)||bytes>MAX_GLOBAL_BYTES.saturating_sub(s.pending_bytes)||s.pending_commands>=MAX_GLOBAL_COMMANDS{return false;}
        let Some(ticket)=s.ticket.checked_add(1)else{return false};s.ticket=ticket;s.pending_bytes+=bytes;s.pending_commands+=1;
        let r=s.records.get_mut(&target).unwrap();r.pending_bytes+=bytes;r.pending.insert(ticket,Pending{bytes,owner:None});
        let command=command(ticket);
        let command=if r.ready {
            if let Some(tx)=&r.route{match tx.send(Inbox::Ui(command)){Ok(())=>{r.pending.get_mut(&ticket).unwrap().owner=r.owner;return true;},Err(std::sync::mpsc::SendError(Inbox::Ui(command)))=>{r.route=None;r.ready=false;command},Err(_)=>unreachable!()}}else{command}
        }else{command};
        r.queued.push_back(command);true
    }
    fn navigate_locked(s:&mut State,source:u64,target:u64,url:Url,replace:bool,noreferrer:bool,base_override:Option<&Url>)->bool{
        if !Self::related(s,source,target)||!Self::allowed_url(&url){return false;}
        let from=s.records[&source].info.clone();let to=&s.records[&target].info;
        if url.scheme=="javascript"&&from.origin_key!=to.origin_key{return false;}
        let blank=if url.scheme=="about"&&url.path=="blank"{Some(BlankOrigin{origin:from.origin.clone(),key:from.origin_key.clone(),base_url:base_override.unwrap_or(&from.base_url).clone()})}else{None};
        let javascript_origin=if url.scheme=="javascript"{Some(from.origin_key.clone())}else{None};
        let mut request=Request::get(url);
        // An inherited blank still has a real initiator even though its URL is
        // opaque. Keep that origin separate from the permitted referrer.
        request.initiator=Url::parse_absolute(&format!("{}/",from.origin));
        request.referer=if from.url.is_http(){Some(from.url)}else{request.initiator.clone()};
        if noreferrer {request.referrer_policy=rabbithole_engine::net::ReferrerPolicy::NoReferrer;}
        let bytes=Self::url_bytes(&request.url).saturating_add(blank.as_ref().map_or(0,|seed|Self::url_bytes(&seed.base_url)+seed.origin.len()+seed.key.len())).saturating_add(512);
        Self::enqueue(s,target,bytes,|ticket|ToContent::ContextNavigate{id:target,ticket,request,replace,blank,javascript_origin})
    }
}
struct RendererWindowHost { registry:Arc<BrowsingContexts>,id:u64,tab:u64,epoch:u64 }
impl RendererWindowHost {
    fn valid(&self,state:&State,source:u64)->bool {
        source==self.id&&state.records.get(&source).is_some_and(|r|!r.info.closed&&r.owner==Some((self.tab,self.epoch)))
    }
}
impl WindowHost for RendererWindowHost {
    fn open(&self,request:WindowOpenRequest)->Option<u64>{
        let mut s=self.registry.state.lock().unwrap();
        if s.records.len()>4096{s.records.retain(|_,r|!r.info.closed||r.owner.is_some()||!r.pending.is_empty()||r.creation_notified);}
        if !self.valid(&s,request.source){return None;}
        let source=s.records.get(&request.source)?;
        if source.info.closed||source.info.origin!=request.source_origin||request.name.len()>MAX_NAME_BYTES||BrowsingContexts::url_bytes(&request.source_base_url)>MAX_URL_BYTES||request.url.as_ref().is_some_and(|url|!BrowsingContexts::allowed_url(url)){return None;}
        let group=source.group;let owner=source.owner?;let source_info=source.info.clone();
        let special=request.name.to_ascii_lowercase();
        let existing=if matches!(special.as_str(),"_self"|"_top"|"_parent"){Some(request.source)}else if !request.noopener&&special!="_blank"&&!request.name.is_empty(){s.records.iter().find(|(_,r)|!r.info.closed&&r.group==group&&r.info.name==request.name).map(|(&id,_)|id)}else{None};
        if let Some(id)=existing{
            if let Some(url)=request.url{if !BrowsingContexts::navigate_locked(&mut s,request.source,id,url,false,request.noreferrer,Some(&request.source_base_url)){return None;}}
            return Some(id);
        }
        if s.creations>=MAX_CONTEXTS||s.records.values().filter(|r|!r.info.closed).count()>=MAX_CONTEXTS{return None;}
        s.next+=1;let id=s.next;
        let name=if special=="_blank"{String::new()}else{request.name};
        s.records.insert(id,Record{info:WindowInfo{url:BrowsingContexts::blank(),base_url:request.source_base_url.clone(),origin:source_info.origin,origin_key:source_info.origin_key,opener:if request.noopener{None}else{Some(request.source)},closed:false,name},group:if request.noopener{id}else{group},script_opened:true,owner:None,creator_owner:owner,route:None,ready:false,queued:VecDeque::new(),focus_when_attached:true,focus_notified:false,creation_notified:true,pending:BTreeMap::new(),pending_bytes:0});
        if let Some(url)=request.url.filter(|url|!(url.scheme=="about"&&url.path=="blank")){
            // noopener changes familiarity only after the creation navigation.
            let old=s.records[&id].group;s.records.get_mut(&id).unwrap().group=group;
            let success=BrowsingContexts::navigate_locked(&mut s,request.source,id,url,false,request.noreferrer,Some(&request.source_base_url));
            s.records.get_mut(&id).unwrap().group=old;
            if !success{s.records.remove(&id);return None;}
        }
        s.creations+=1;
        drop(s);(self.registry.sink)(owner.0,owner.1,FromContent::OpenContext(id));Some(id)
    }
    fn info(&self,id:u64)->Option<WindowInfo>{let s=self.registry.state.lock().unwrap();if !self.valid(&s,self.id){return None;}s.records.get(&id).map(|r|r.info.clone())}
    fn navigate(&self,source:u64,target:u64,url:Url,replace:bool)->bool{let mut s=self.registry.state.lock().unwrap();if !self.valid(&s,source){return false;}BrowsingContexts::navigate_locked(&mut s,source,target,url,replace,false,None)}
    fn focus(&self,source:u64,target:u64)->bool{
        let owner={let mut s=self.registry.state.lock().unwrap();if !self.valid(&s,source)||!BrowsingContexts::related(&s,source,target){return false;}let r=s.records.get_mut(&target).unwrap();r.focus_when_attached=true;if r.focus_notified{return true;}let owner=r.owner.filter(|_|r.ready);r.focus_notified=owner.is_some();owner};
        if let Some((tab,epoch))=owner{(self.registry.sink)(tab,epoch,FromContent::FocusContext(target));}true
    }
    fn close(&self,source:u64,target:u64)->bool{
        let owner={let mut s=self.registry.state.lock().unwrap();if !self.valid(&s,source)||!BrowsingContexts::related(&s,source,target)||!s.records[&target].script_opened{return false;}
            BrowsingContexts::clear_unsent(&mut s,target);
            let r=s.records.get_mut(&target).unwrap();r.info.closed=true;if let Some(tx)=&r.route{let _=tx.send(Inbox::Ui(ToContent::Shutdown));}r.owner};
        if let Some((tab,epoch))=owner{(self.registry.sink)(tab,epoch,FromContent::CloseContext(target));}true
    }
    fn set_name(&self,source:u64,target:u64,name:String)->bool{
        let mut s=self.registry.state.lock().unwrap();if !self.valid(&s,source)||name.len()>MAX_NAME_BYTES||!BrowsingContexts::related(&s,source,target)||s.records[&source].info.origin_key!=s.records[&target].info.origin_key{return false;}s.records.get_mut(&target).unwrap().info.name=name;true
    }
    fn post_message(&self,source:u64,target:u64,data:String,target_origin_key:Option<String>)->bool{
        let mut s=self.registry.state.lock().unwrap();if !self.valid(&s,source)||data.len()>MAX_MESSAGE_BYTES||!BrowsingContexts::related(&s,source,target){return false;}
        let origin=s.records[&source].info.origin.clone();
        let bytes=data.len().saturating_add(origin.len()).saturating_add(target_origin_key.as_ref().map_or(0,String::len)).saturating_add(256);
        BrowsingContexts::enqueue(&mut s,target,bytes,|ticket|ToContent::ContextMessage{id:target,ticket,message:WindowMessage{source,data,origin,target_origin_key}})
    }
    fn disown(&self,source:u64){let mut s=self.registry.state.lock().unwrap();if !self.valid(&s,source){return;}if let Some(r)=s.records.get_mut(&source){r.info.opener=None;}}
    fn document_changed(&self,id:u64,url:Url,origin:String){
        let mut s=self.registry.state.lock().unwrap();if !self.valid(&s,id){return;}let Some(r)=s.records.get(&id)else{return};if r.info.closed{return;}
        let blank=url.scheme=="about"&&url.path=="blank";
        let key=if blank{r.info.origin_key.clone()}else if origin=="null"{self.registry.opaque(&mut s)}else{origin.clone()};
        let r=s.records.get_mut(&id).unwrap();r.info.url=url.clone();
        if !blank{r.info.origin=origin;r.info.origin_key=key;r.info.base_url=url;}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn url(value:&str)->Url{Url::parse_absolute(value).unwrap()}
    fn registry()->Arc<BrowsingContexts>{BrowsingContexts::new(Arc::new(|_,_,_|{}))}
    fn request(source:u64,origin:&str,name:&str)->WindowOpenRequest{WindowOpenRequest{source,source_origin:origin.into(),source_url:url(&format!("{origin}/page")),source_base_url:url(&format!("{origin}/base/")),url:None,name:name.into(),noopener:false,noreferrer:false}}
    fn parent(registry:&Arc<BrowsingContexts>)->WindowBinding{
        let binding=registry.reserve_tab(1,1);binding.context().host.document_changed(binding.id,url("https://one.example.test/page"),"https://one.example.test".into());binding
    }
    #[test]
    fn renderer_rebind_keeps_identity_and_atomically_revokes_old_capability(){
        let registry=registry();let binding=parent(&registry);let old=binding.context();
        let child=old.host.open(request(old.id,"https://one.example.test","auth")).unwrap();
        let replacement=registry.reserve_tab(1,2);assert_eq!(replacement.id,binding.id);
        replacement.context().host.document_changed(binding.id,url("https://two.example.test/new"),"https://two.example.test".into());
        assert!(old.host.info(binding.id).is_none());assert!(old.host.open(request(old.id,"https://one.example.test","late")).is_none());
        assert!(!old.host.navigate(old.id,child,url("https://one.example.test/stale"),false));assert!(!old.host.focus(old.id,child));assert!(!old.host.close(old.id,child));assert!(!old.host.set_name(old.id,child,"stale".into()));
        assert!(!old.host.post_message(old.id,child,"payload".into(),None));
        old.host.document_changed(old.id,url("https://one.example.test/stale"),"https://one.example.test".into());
        assert!(!registry.prepare_blank(old.id,1,1,BlankOrigin{origin:"null".into(),key:"stale".into(),base_url:url("about:blank")}));
        registry.detach(old.id,1,1);assert_eq!(registry.info(old.id).unwrap().origin,"https://two.example.test");assert!(!registry.info(old.id).unwrap().closed);
    }
    #[test]
    fn commands_before_attachment_are_fifo_and_acknowledged_by_renderer_lease(){
        let registry=registry();let parent=parent(&registry);let host=parent.context().host;
        let child=host.open(request(parent.id,"https://one.example.test","auth")).unwrap();
        assert!(host.navigate(parent.id,child,url("https://one.example.test/callback"),false));assert!(host.post_message(parent.id,child,"owned-clone".into(),None));
        let binding=registry.popup_binding(child,2,1).unwrap();let(tx,rx)=std::sync::mpsc::channel();registry.attach(child,2,1,tx);assert!(rx.try_recv().is_err());registry.ready(child,2,1);
        let first=match rx.recv().unwrap(){Inbox::Ui(ToContent::ContextNavigate{id,ticket,request,..})=>{assert_eq!(id,child);assert_eq!(request.url.path,"/callback");ticket},_=>panic!("navigation first")};
        let second=match rx.recv().unwrap(){Inbox::Ui(ToContent::ContextMessage{id,ticket,message})=>{assert_eq!(id,child);assert_eq!(message.origin,"https://one.example.test");assert_eq!(message.data,"owned-clone");ticket},_=>panic!("message second")};
        assert!(first<second);assert!(!registry.acknowledge(child,2,99,first));assert!(registry.acknowledge(child,2,1,first));assert!(registry.acknowledge(child,2,1,second));assert_eq!(registry.state.lock().unwrap().pending_bytes,0);
        drop(binding);
    }
    #[test]
    fn active_inbox_bytes_remain_charged_until_consumed_or_receiver_drops(){
        let registry=registry();let parent=parent(&registry);let host=parent.context().host;
        let(tx,rx)=std::sync::mpsc::channel();registry.attach(parent.id,1,1,tx);registry.ready(parent.id,1,1);
        let payload="x".repeat(1024*1024);let mut accepted=0;
        while host.post_message(parent.id,parent.id,payload.clone(),None){accepted+=1;assert!(accepted<MAX_PENDING);}
        assert!(accepted>0);let bytes=registry.state.lock().unwrap().pending_bytes;assert!(bytes<=MAX_CONTEXT_BYTES);
        registry.detach(parent.id,1,1);assert_eq!(registry.state.lock().unwrap().pending_bytes,bytes,"closing does not release bytes still held by a live receiver");
        drop(rx);registry.renderer_gone(parent.id,1,1);assert_eq!(registry.state.lock().unwrap().pending_bytes,0);
    }
    #[test]
    fn post_message_captures_send_origin_and_survives_sender_close(){
        let registry=registry();let parent=parent(&registry);let parent_host=parent.context().host;
        let child=parent_host.open(request(parent.id,"https://one.example.test","auth")).unwrap();let child=registry.popup_binding(child,2,1).unwrap();let child_host=child.context().host;
        let(tx,rx)=std::sync::mpsc::channel();registry.attach(parent.id,1,1,tx);registry.ready(parent.id,1,1);
        assert!(child_host.post_message(child.id,parent.id,"callback".into(),Some("https://one.example.test".into())));
        child_host.document_changed(child.id,url("https://two.example.test/callback"),"https://two.example.test".into());assert!(child_host.close(child.id,child.id));
        match rx.recv().unwrap(){Inbox::Ui(ToContent::ContextMessage{message,..})=>{assert_eq!(message.origin,"https://one.example.test");assert_eq!(message.source,child.id);},_=>panic!("callback must survive close")};
        assert!(parent_host.info(child.id).unwrap().closed);
    }
    #[test]
    fn queued_javascript_cannot_cross_an_intervening_document_origin_change(){
        let registry=registry();let parent=parent(&registry);let host=parent.context().host;
        let child=host.open(request(parent.id,"https://one.example.test","auth")).unwrap();let child=registry.popup_binding(child,2,1).unwrap();let(tx,rx)=std::sync::mpsc::channel();registry.attach(child.id,2,1,tx);registry.ready(child.id,2,1);
        assert!(host.navigate(parent.id,child.id,url("javascript:document.title='injected'"),false));
        let key=match rx.recv().unwrap(){Inbox::Ui(ToContent::ContextNavigate{javascript_origin:Some(key),..})=>key,_=>panic!("guarded JS navigation")};
        child.context().host.document_changed(child.id,url("https://two.example.test/"),"https://two.example.test".into());
        assert!(!registry.allow_javascript(child.id,2,1,&key));
    }
    #[test]
    fn opaque_identities_and_noopener_are_not_familiar_named_targets(){
        let registry=registry();let source=registry.reserve_tab(1,1);let unrelated=registry.reserve_tab(2,1);
        let source_info=registry.info(source.id).unwrap();let other=registry.info(unrelated.id).unwrap();assert_eq!(source_info.origin,"null");assert_eq!(other.origin,"null");assert_ne!(source_info.origin_key,other.origin_key);
        let host=source.context().host;let make=|noopener|WindowOpenRequest{source:source.id,source_origin:"null".into(),source_url:url("about:blank"),source_base_url:url("https://one.example.test/base/"),url:None,name:"auth".into(),noopener,noreferrer:false};
        let child=host.open(make(false)).unwrap();assert_eq!(host.open(make(false)),Some(child));assert_eq!(registry.info(child).unwrap().origin_key,source_info.origin_key);
        let isolated=host.open(make(true)).unwrap();assert_ne!(child,isolated);assert_eq!(registry.info(isolated).unwrap().opener,None);assert!(!host.navigate(source.id,isolated,url("https://one.example.test/"),false));
        assert!(!host.close(source.id,source.id),"normal user tabs are not script-closable");
    }
}
