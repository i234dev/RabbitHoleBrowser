//! Scroll callbacks use the real content-thread protocol and existing harness.
//! Finite listeners expose host reentry without stack overflow or an infinite page.
use super::*;

const PAGE: &str = r#"<!doctype html><body style="margin:0"><div style="height:5000px">Scroll chain</div>
<script>
window.scrollSeen=[];
window.scrollDelay=0;
addEventListener('scroll',function(){
  if(scrollSeen.length>=4)return;
  scrollSeen.push(scrollY);
  console.log('scroll-chain-step:'+scrollSeen.length+':'+scrollY);
  var until=performance.now()+scrollDelay;
  while(performance.now()<until){}
  if(scrollSeen.length<4){scrollTo(0,scrollY===1?2:1);}
  else{console.log('scroll-chain-done');}
});
</script>"#;

fn run_chain(delay: u32, require_deferred: bool) {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    load(&h, PAGE);
    wait_for(&rx, 1, Duration::from_secs(10), is_frame).expect("initial frame");
    // The 25ms warmup ensures the initial request is beyond the 16ms scroll
    // throttle. The slow listener has a minimum wall duration, not a race
    // against a guessed sleep or an assertion about total test duration.
    // Multiple writes coalesce to the final coordinate; scripts see it now.
    h.send(ToContent::Eval(format!("scrollDelay={delay};var until=performance.now()+25;while(performance.now()<until){{}}scrollTo(0,5);scrollTo(0,9);scrollTo(0,1);'armed:'+scrollY+':'+scrollSeen.length")));
    let mut before_ack = Vec::new();
    let mut done = false;
    let (_, result) = wait_for(&rx, 1, Duration::from_secs(10), |msg| {
        match msg {
            FromContent::Console(lines) => {
                for (_, message) in lines {
                    if message.starts_with("scroll-chain-step:") { before_ack.push(message.clone()); }
                    done |= message == "scroll-chain-done";
                }
                false
            }
            FromContent::EvalResult(_) => true,
            _ => false,
        }
    }).expect("the arming evaluation replies");
    let FromContent::EvalResult(result) = result else { unreachable!() };
    assert_eq!(result.trim_matches(|c| c=='\'' || c=='"'), "armed:1:0", "scroll coordinates update synchronously");
    eprintln!("scroll-chain delay={delay} callbacks-before-eval-reply={}: {before_ack:?}", before_ack.len());
    if require_deferred {
        assert!(before_ack.is_empty(), "scroll listeners reentered apply_outcome before EvalResult: {before_ack:?}");
    }
    if !done {
        wait_for(&rx, 1, Duration::from_secs(10), |msg| match msg {
            FromContent::Console(lines) => lines.iter().any(|(_, message)| message == "scroll-chain-done"),
            _ => false,
        }).expect("all four finite callbacks finish through queued scroll events");
    }
    h.send(ToContent::Eval("scrollSeen.length+'|'+scrollSeen.join(',')+'|'+scrollY".into()));
    let (_, result) = wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::EvalResult(_))).expect("final state");
    let FromContent::EvalResult(result) = result else { unreachable!() };
    assert_eq!(result.trim_matches(|c| c=='\'' || c=='"'), "4|1,2,1,2|2", "latest scroll position and every callback survive");
}

#[test]
fn slow_scroll_listener_cannot_reenter_before_the_arming_evaluation_replies() {
    run_chain(25, true);
}

#[test]
fn fast_scroll_listener_coalesces_coordinates_and_delivers_the_final_event() {
    run_chain(0, false);
}
