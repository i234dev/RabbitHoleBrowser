//! Transferred reply ports must survive messages sent over an already
//! transferred iframe port. These fixtures use the real frame bridge.
use rabbithole_engine::{Engine, Page, Viewport};
use rabbithole_engine::net::Url;
use std::time::{Duration, Instant};

fn engine() -> Engine {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.scripting = true;
    engine.loader.load_scripts = true;
    engine
}
fn eval(engine: &Engine, page: &mut Page, source: &str) -> String {
    engine.eval_in_page(page, source).trim_matches(|c| c == '\'' || c == '"').to_string()
}
fn escape_attr(source: &str) -> String { source.replace('&', "&amp;").replace('"', "&quot;") }
fn until(engine: &Engine, page: &mut Page, condition: &str) {
    let deadline = Instant::now()+Duration::from_secs(10);
    while Instant::now()<deadline {
        engine.tick(page);
        if eval(engine, page, condition)=="true" { return; }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("protocol did not complete: {}", eval(engine, page, "JSON.stringify({trace:trace,errors:errors})"));
}

#[test]
fn nested_message_ports_survive_three_request_reply_hops_between_frames() {
    let mut engine = engine();
    // These are the same private-loopback fixture bytes that reproduced
    // undefined.postMessage on the published14 ELF; only the child transport
    // URL is replaced with srcdoc for a deterministic network-free test.
    let child = include_str!("../../../tests/pages/messageport-rpc-child.html");
    let parent = include_str!("../../../tests/pages/messageport-rpc-parent.html")
        .replace("src=/child.html", &format!("srcdoc=\"{}\"", escape_attr(child)));
    let mut page = engine.load_html(&parent, Url::parse("https://ports.test/root").unwrap(), Viewport { width: 800.0, height: 600.0 });
    until(&engine, &mut page, "errors.length>0 || (trace.includes('complete:done') && trace.includes('child-finished'))");
    assert_eq!(eval(&engine, &mut page, "JSON.stringify(errors)"), "[]");
    assert_eq!(eval(&engine, &mut page, "trace[0]"), "primary:ready");
    assert_eq!(eval(&engine, &mut page, "trace[1]"), "reply:answer:ports=1:source-null=true");
    assert_eq!(eval(&engine, &mut page, "trace.includes('complete:done') && trace.includes('child-finished')"), "true");
}

#[test]
fn multiple_nested_ports_keep_order_identity_and_empty_transfer_lists() {
    let mut engine = engine();
    let child = r#"<script>
        addEventListener('message',initial=>{
          var primary=initial.ports[0];
          primary.onmessage=event=>{
            if(event.data==='probe') {primary.postMessage({kind:'probe',ports:event.ports.length,sourceNull:event.source===null});return}
            parent.postMessage({kind:'report',count:event.ports.length,distinct:event.ports[0]!==event.ports[1],sourceNull:event.source===null},'*');
            event.ports[0].postMessage('left');event.ports[1].postMessage('right');
          };
          primary.postMessage('ready');
        });
    </script>"#;
    let parent = format!(r#"<script>
        var trace=[],errors=[];
        addEventListener('message',event=>{{if(event.data.kind==='report')trace.push('report:'+event.data.count+':'+event.data.distinct+':'+event.data.sourceNull+':'+(event.source===document.getElementById('f').contentWindow))}});
        function go(){{
          var primary=new MessageChannel();
          primary.port1.onmessage=event=>{{
            if(event.data==='ready'){{primary.port1.postMessage('probe');return}}
            trace.push('probe:'+event.data.ports+':'+event.data.sourceNull);
            var left=new MessageChannel(),right=new MessageChannel();
            left.port1.onmessage=event=>trace.push('first:'+event.data+':'+(event.source===null));
            right.port1.onmessage=event=>trace.push('second:'+event.data+':'+(event.source===null));
            primary.port1.postMessage('two',[left.port2,right.port2]);
          }};
          document.getElementById('f').contentWindow.postMessage('start','*',[primary.port2]);
        }}
        </script><iframe id=f onload=go() srcdoc="{}"></iframe>"#, escape_attr(child));
    let mut page = engine.load_html(&parent, Url::parse("https://ports.test/root").unwrap(), Viewport { width: 800.0, height: 600.0 });
    until(&engine, &mut page, "trace.length>=4");
    assert_eq!(eval(&engine, &mut page, "trace.slice().sort().join('|')"), "first:left:true|probe:0:true|report:2:true:true:true|second:right:true");
}
