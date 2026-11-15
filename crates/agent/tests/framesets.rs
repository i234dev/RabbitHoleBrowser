use rabbithole_agent::{call, Agent, Config};
use serde_json::{json, Value};
use std::time::Duration;
#[path="../../../tests/support/nested_frames.rs"] mod fixture;

fn ok(agent:&mut Agent,name:&str,args:Value)->String {call(agent,name,&args).unwrap_or_else(|error|panic!("{name}: {error}")).text}
fn agent()->Agent {Agent::new(Config{profile_dir:None,settle_cap:Duration::from_secs(3),..Config::default()})}

#[test]
fn nested_frameset_panels_are_real_contexts_in_snapshot_find_read_and_summary() {
    let server=fixture::serve();let mut agent=agent();
    let snapshot=ok(&mut agent,"open",json!({"url":server.url("/nested_frames"),"snapshot":true}));
    // Neutral frame labels cannot satisfy these checks when child loading fails.
    for word in ["LEFT","MIDDLE","RIGHT","BOTTOM"] {assert!(snapshot.contains(&format!("heading(1) \"{word}\"")),"{snapshot}");}
    let found=ok(&mut agent,"find",json!({"text":["LEFT","MIDDLE","RIGHT","BOTTOM"]}));
    for word in ["LEFT","MIDDLE","RIGHT","BOTTOM"] {assert!(found.contains(word),"{found}");}
    let read=ok(&mut agent,"read",json!({}));
    let mut previous=0;
    for word in ["LEFT","MIDDLE","RIGHT","BOTTOM"] {let position=read.find(word).unwrap_or_else(||panic!("missing {word}: {read}"));assert!(position>=previous,"document order: {read}");previous=position;}
    let summary=ok(&mut agent,"open",json!({"url":server.url("/nested_frames")}));
    for word in ["LEFT","MIDDLE","RIGHT","BOTTOM"] {assert!(summary.contains(word),"{summary}");}
    let js=ok(&mut agent,"debug",json!({"eval":"[frames.length,frames[0].frames.length,frames[0][0].document.body.textContent.trim(),frames[0][1].document.body.textContent.trim(),frames[0][2].document.body.textContent.trim(),frames[1].document.body.textContent.trim()].join('|')"}));
    assert!(js.contains("2|3|LEFT|MIDDLE|RIGHT|BOTTOM"),"{js}");
    let found=ok(&mut agent,"open",json!({"url":server.url("/nested_frames"),"find":["LEFT","MIDDLE","RIGHT","BOTTOM"]}));
    for word in ["LEFT","MIDDLE","RIGHT","BOTTOM"] {assert!(found.contains(word),"{found}");}
}

#[test]
fn read_of_a_frame_ref_uses_the_hosted_context() {
    let server=fixture::serve();let mut agent=agent();
    let snapshot=ok(&mut agent,"open",json!({"url":server.url("/nested_frames"),"snapshot":true}));
    let line=snapshot.lines().find(|line|line.contains("Upper row")&&line.contains("@e")).unwrap_or_else(||panic!("{snapshot}"));
    let start=line.find("@e").unwrap();let reference:String=line[start..].chars().take_while(|c|*c=='@'||*c=='e'||c.is_ascii_digit()).collect();
    let text=ok(&mut agent,"read",json!({"ref":reference}));
    assert!(text.starts_with(&server.url("/top.html")),"child URL attribution: {text}");
    for word in ["LEFT","MIDDLE","RIGHT"] {assert!(text.contains(word),"{text}");}
    assert!(!text.contains("BOTTOM"),"frame subtree must not pull in its sibling: {text}");
}
