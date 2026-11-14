//! Test-only regression checkpoint: transferred ports inside a Worker port
//! message must survive both directions. The current worker bridges drop them.
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
    // Expected RED: worker-stage 0 true undefined undefined. The Worker realm
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
    // Expected RED: owner-stage 0 true, independently of the opposite leg.
    assert!(observed.iter().any(|line| line=="owner-stage 1 true"), "{observed:?}");
    let observed = until_log(&mut env, &mut host, "worker-answer ");
    assert!(observed.iter().any(|line| line=="worker-answer owner-reply true"), "{observed:?}");
}
