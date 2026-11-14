//! Transferred ports inside a Worker port message must survive both directions.
//! The isolated red checkpoint demonstrated their loss before the routing fix.
use crate::tests::{run, setup, MockEnv};
use crate::ScriptHost;
use std::time::{Duration, Instant};

fn logs(env: &MockEnv) -> Vec<String> {
    env.logs.iter().filter(|(level, _)| level=="log").map(|(_, text)| text.clone()).collect()
}
fn until_log(env: &mut MockEnv, host: &mut ScriptHost, prefix: &str) -> Vec<String> {
    let deadline = Instant::now()+Duration::from_secs(5);
    while Instant::now()<deadline {
        env.now += 1.0;
        let now = env.now;
        host.run_timers(env, now);
        host.run_jobs(env);
        let current = logs(env);
        if current.iter().any(|line| line.starts_with(prefix)) { return current; }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("worker protocol did not reach {prefix:?}: {:?}", env.logs);
}

#[test]
fn local_message_port_nested_transfer_is_a_positive_control() {
    let (mut env, mut host) = setup("");
    run(&mut env, &mut host, r#"
      var primary=new MessageChannel(),reply=new MessageChannel();
      primary.port2.onmessage=event=>{
        console.log('local-stage',event.ports.length,event.source===null);
        event.ports[0].postMessage('local-reply');
      };
      reply.port1.onmessage=event=>console.log('local-answer',event.data,event.source===null);
      primary.port1.postMessage('request',[reply.port2]);
    "#);
    let observed = until_log(&mut env, &mut host, "local-answer ");
    assert_eq!(observed, ["local-stage 1 true", "local-answer local-reply true"]);
}

#[test]
fn nested_reply_port_survives_owner_to_worker_port_message() {
    let (mut env, mut host) = setup("");
    env.fetches.insert("https://example.test/dir/nested-owner-to-worker.js".into(), (200, "text/javascript".into(), r#"
      onmessage=initial=>{
        var primary=initial.ports[0];
        primary.onmessage=event=>{
          postMessage({kind:'stage',count:event.ports.length,sourceNull:event.source===null,doc:typeof document,win:typeof window});
          if(event.ports.length)event.ports[0].postMessage('worker-reply');
        };
        primary.postMessage('ready');
      };
    "#.into()));
    run(&mut env, &mut host, r#"
      var worker=new Worker('nested-owner-to-worker.js');
      worker.onmessage=event=>{
        var value=event.data;
        console.log('worker-stage',value.count,value.sourceNull,value.doc,value.win);
      };
      var primary=new MessageChannel();
      primary.port1.onmessage=event=>{
        var reply=new MessageChannel();
        reply.port1.onmessage=event=>console.log('owner-answer',event.data,event.source===null);
        primary.port1.postMessage('request',[reply.port2]);
      };
      worker.postMessage('bootstrap',[primary.port2]);
    "#);
    let observed = until_log(&mut env, &mut host, "worker-stage ");
    // Observed on RED9cc7070: worker-stage 0 true undefined undefined. The Worker realm
    // and primary transfer work; only the nested response endpoint is lost.
    assert!(observed.iter().any(|line| line=="worker-stage 1 true undefined undefined"), "{observed:?}");
    let observed = until_log(&mut env, &mut host, "owner-answer ");
    assert!(observed.iter().any(|line| line=="owner-answer worker-reply true"), "{observed:?}");
}

#[test]
fn nested_reply_port_survives_worker_to_owner_port_message() {
    let (mut env, mut host) = setup("");
    env.fetches.insert("https://example.test/dir/nested-worker-to-owner.js".into(), (200, "text/javascript".into(), r#"
      onmessage=initial=>{
        var primary=initial.ports[0],reply=new MessageChannel();
        reply.port1.onmessage=event=>postMessage({kind:'answer',data:event.data,sourceNull:event.source===null});
        primary.postMessage('worker-request',[reply.port2]);
      };
    "#.into()));
    run(&mut env, &mut host, r#"
      var worker=new Worker('nested-worker-to-owner.js');
      worker.onmessage=event=>console.log('worker-answer',event.data.data,event.data.sourceNull);
      var primary=new MessageChannel();
      primary.port1.onmessage=event=>{
        console.log('owner-stage',event.ports.length,event.source===null);
        if(event.ports.length)event.ports[0].postMessage('owner-reply');
      };
      worker.postMessage('bootstrap',[primary.port2]);
    "#);
    let observed = until_log(&mut env, &mut host, "owner-stage ");
    // Observed on RED9cc7070: owner-stage 0 true, independently of the opposite leg.
    assert!(observed.iter().any(|line| line=="owner-stage 1 true"), "{observed:?}");
    let observed = until_log(&mut env, &mut host, "worker-answer ");
    assert!(observed.iter().any(|line| line=="worker-answer owner-reply true"), "{observed:?}");
}

#[test]
fn nested_worker_reply_ports_are_isolated_between_worker_instances() {
    let (mut env, mut host) = setup("");
    env.fetches.insert("https://example.test/dir/isolated-port-worker.js".into(), (200, "text/javascript".into(), r#"
      onmessage=initial=>{
        var primary=initial.ports[0],label=initial.data,reply=new MessageChannel();
        reply.port1.onmessage=event=>primary.postMessage({kind:'done',label:label,data:event.data});
        primary.postMessage({kind:'request',label:label},[reply.port2]);
      };
    "#.into()));
    run(&mut env, &mut host, r#"
      function start(label){
        var worker=new Worker('isolated-port-worker.js'),primary=new MessageChannel();
        primary.port1.onmessage=event=>{
          if(event.data.kind==='request'){
            if(event.ports.length!==1)throw Error('nested endpoint missing');
            event.ports[0].postMessage('owner-'+label);
          }else console.log('isolated',event.data.label,event.data.data);
        };
        worker.postMessage(label,[primary.port2]);
      }
      start('A');start('B');
    "#);
    until_log(&mut env, &mut host, "isolated A ");
    let mut observed = until_log(&mut env, &mut host, "isolated B ");
    observed.sort();
    assert_eq!(observed, ["isolated A owner-A", "isolated B owner-B"]);
}

#[test]
fn nested_shared_worker_ports_keep_each_connection_and_reply_separate() {
    let (mut env, mut host) = setup("");
    env.fetches.insert("https://example.test/dir/shared-nested-ports.js".into(), (200, "text/javascript".into(), r#"
      var connections=0;
      onconnect=initial=>{
        var primary=initial.ports[0],label=++connections;
        primary.onmessage=event=>{
          if(event.ports.length!==1)throw Error('shared nested endpoint missing');
          event.ports[0].postMessage('shared-'+label);
          var reply=new MessageChannel();
          reply.port1.onmessage=event=>primary.postMessage({kind:'ack',label:label,data:event.data});
          primary.postMessage({kind:'request',label:label},[reply.port2]);
        };
        primary.postMessage({kind:'ready',label:label});
      };
    "#.into()));
    run(&mut env, &mut host, r#"
      function connect(){
        var worker=new SharedWorker('shared-nested-ports.js');
        worker.port.onmessage=event=>{
          var value=event.data;
          if(value.kind==='ready'){
            var reply=new MessageChannel();
            reply.port1.onmessage=event=>console.log('shared-result',value.label,event.data);
            worker.port.postMessage('request',[reply.port2]);
          }else if(value.kind==='request'){
            if(event.ports.length!==1)throw Error('owner shared endpoint missing');
            event.ports[0].postMessage('owner-'+value.label);
          }else console.log('shared-ack',value.label,value.data);
        };
      }
      connect();connect();
    "#);
    until_log(&mut env, &mut host, "shared-ack 1 ");
    let mut observed = until_log(&mut env, &mut host, "shared-ack 2 ");
    observed.sort();
    assert_eq!(observed, ["shared-ack 1 owner-1", "shared-ack 2 owner-2", "shared-result 1 shared-1", "shared-result 2 shared-2"]);
}

#[test]
fn ports_from_a_terminated_worker_do_not_reconnect_to_a_replacement_worker() {
    let (mut env, mut host) = setup("");
    env.fetches.insert("https://example.test/dir/replace-port-worker.js".into(), (200, "text/javascript".into(), r#"
      onmessage=initial=>{
        var primary=initial.ports[0],label=initial.data;
        primary.onmessage=event=>postMessage({label:label,data:event.data});
        primary.postMessage('ready');
      };
    "#.into()));
    run(&mut env, &mut host, r#"
      var workers={},ports={};
      function start(label){
        var worker=new Worker('replace-port-worker.js'),primary=new MessageChannel();
        workers[label]=worker;ports[label]=primary.port1;
        primary.port1.onmessage=event=>console.log('ready',label);
        worker.onmessage=event=>console.log('echo',event.data.label,event.data.data);
        worker.postMessage(label,[primary.port2]);
      }
      start('A');
    "#);
    until_log(&mut env, &mut host, "ready A");
    run(&mut env, &mut host, "workers.A.terminate();start('B')");
    until_log(&mut env, &mut host, "ready B");
    run(&mut env, &mut host, "ports.A.postMessage('stale');ports.B.postMessage('fresh')");
    let observed = until_log(&mut env, &mut host, "echo B fresh");
    assert_eq!(observed, ["ready A", "ready B", "echo B fresh"]);
}

#[test]
fn nested_worker_port_can_be_relayed_from_one_worker_to_another() {
    let (mut env, mut host) = setup("");
    env.fetches.insert("https://example.test/dir/relay-a.js".into(), (200, "text/javascript".into(), r#"
      onmessage=initial=>{
        var channel=new MessageChannel();
        channel.port1.onmessage=event=>channel.port1.postMessage('A:'+event.data);
        postMessage('endpoint',[channel.port2]);
      };
    "#.into()));
    env.fetches.insert("https://example.test/dir/relay-b.js".into(), (200, "text/javascript".into(), r#"
      onmessage=initial=>{
        var primary=initial.ports[0];
        primary.onmessage=event=>{
          if(event.ports.length!==1)throw Error('relayed endpoint missing');
          var endpoint=event.ports[0];
          endpoint.onmessage=event=>postMessage(event.data);
          endpoint.postMessage('B-roundtrip');
        };
        primary.postMessage('ready');
      };
    "#.into()));
    run(&mut env, &mut host, r#"
      var workerA=new Worker('relay-a.js'),workerB=new Worker('relay-b.js');
      var primary=new MessageChannel(),endpointFromA=null,readyB=false,sent=false;
      function relay(){
        if(endpointFromA&&readyB&&!sent){sent=true;primary.port1.postMessage('relay',[endpointFromA])}
      }
      workerA.onmessage=event=>{endpointFromA=event.ports[0];relay()};
      primary.port1.onmessage=event=>{readyB=true;relay()};
      workerB.onmessage=event=>console.log('relay-answer',event.data,event.source===null);
      workerA.postMessage('start');
      workerB.postMessage('bootstrap',[primary.port2]);
    "#);
    let observed = until_log(&mut env, &mut host, "relay-answer ");
    assert_eq!(observed, ["relay-answer A:B-roundtrip true"]);
}
