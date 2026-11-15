//! Batch uses the real tool dispatcher and the parent loopback fixture server.
use super::*;

const PAGE: &str = r#"<!doctype html><html><head><style>
button,input { display:block; margin:8px; } iframe { width:400px; height:160px; }
</style></head><body>
<button id="first" onclick="globalThis.batchCount=(globalThis.batchCount||0)+1">First</button>
<button id="after" onclick="globalThis.batchAfter=true">After</button>
<input id="typed" aria-label="Typed">
<form id="form" onsubmit="event.preventDefault()">
<input id="filled" name="filled"><input id="locked" name="locked" readonly value="initial">
<button type="submit">Send</button></form>
<iframe src="/child"></iframe>
<p>Local batch fixture</p></body></html>"#;

fn setup() -> (Server, Agent) {
    let srv = serve(Box::new(|path| match path {
        "/" => html(PAGE),
        "/child" => html("<!doctype html><html><body><input id='child-field' value='before'></body></html>"),
        _ => None,
    }));
    let mut a = agent();
    ok(&mut a, "open", json!({"url":srv.url("/")}));
    (srv, a)
}

#[test]
fn batch_uses_sequential_click_type_and_fill_selectors_without_a_snapshot() {
    let (_srv, mut a) = setup();
    let result = call(&mut a, "batch", &json!({"steps":[
        {"tool":"click","args":{"selector":"#first"}},
        {"tool":"type","args":{"selector":"#typed","text":"á🧪"}},
        {"tool":"fill","args":{"selector":"#filled","value":"value"}},
        {"tool":"debug","args":{"eval":"batchCount+'|'+document.getElementById('typed').value+'|'+document.getElementById('filled').value"}}
    ]})).unwrap();
    assert!(!result.failed);
    assert!(result.text.contains("attempted 4/4, succeeded 4, failed 0, pending 0"), "{}", result.text);
    assert!(result.text.contains("1. click #first:"), "summary must include its explicit target: {}", result.text);
    assert!(result.text.contains("1|á🧪|value"), "{}", result.text);
}

#[test]
fn batch_stops_at_the_first_runtime_failure_and_counts_unattempted_steps() {
    let (_srv, mut a) = setup();
    let error = call(&mut a, "batch", &json!({"steps":[
        {"tool":"click","args":{"selector":"#first"}},
        {"tool":"click","args":{"selector":"#first"}},
        {"tool":"click","args":{"selector":"#missing"}},
        {"tool":"click","args":{"selector":"#after"}},
        {"tool":"type","args":{"selector":"#typed","text":"must not run"}}
    ]})).unwrap_err();
    assert!(error.contains("attempted 3/5, succeeded 2, failed 1, pending 2"), "{error}");
    assert!(error.contains("Step 3 (click, failed):\nselector matched 0"), "{error}");
    let state = ok(&mut a, "debug", json!({"eval":"batchCount+'|'+typeof batchAfter+'|'+document.getElementById('typed').value"}));
    assert!(state.contains("2|undefined|"), "{state}");
    assert!(!state.contains("must not run"), "{state}");
}

#[test]
fn batch_preflights_nesting_unknown_tools_and_shapes_before_any_action() {
    let (_srv, mut a) = setup();
    let first = json!({"tool":"click","args":{"selector":"#first"}});
    let invalid = [
        json!({"steps":[first.clone(),{"tool":"batch","args":{"steps":[]}}]}),
        json!({"steps":[first.clone(),{"tool":"not-a-tool"}]}),
        json!({"steps":[first.clone(),{"tool":"snapshot","args":null}]}),
        json!({"steps":[first.clone(),{"tool":"snapshot","ignored":true}]}),
        json!({"steps":[first.clone()],"full":"true"}),
        json!({"steps":[]}),
    ];
    for args in invalid {
        let error = call(&mut a, "batch", &args).unwrap_err();
        assert!(error.contains("no steps executed"), "{error}");
    }
    let mut too_many = vec![first];
    too_many.extend((0..20).map(|_| json!({"tool":"snapshot"})));
    assert!(call(&mut a, "batch", &json!({"steps":too_many})).unwrap_err().contains("1–20"));
    let state = ok(&mut a, "debug", json!({"eval":"typeof batchCount"}));
    assert!(state.contains("undefined"), "{state}");
}

#[test]
fn batch_accepts_twenty_steps_and_bounds_unicode_summaries_but_keeps_the_last_result() {
    let (_srv, mut a) = setup();
    let source = format!("'EARLY-{}'", "á𐐀🧪".repeat(150));
    let last = format!("LAST-{}", "🧪".repeat(260));
    let mut steps = vec![json!({"tool":"debug","args":{"eval":source}})];
    steps.extend((0..18).map(|_| json!({"tool":"debug","args":{"eval":"1"}})));
    steps.push(json!({"tool":"debug","args":{"eval":format!("'{last}'")}}));
    let result = call(&mut a, "batch", &json!({"steps":steps})).unwrap();
    assert!(result.text.contains("attempted 20/20, succeeded 20, failed 0, pending 0"), "{}", result.text);
    let summaries = result.text.split("Earlier steps:\n").nth(1).unwrap().split("\nStep 20").next().unwrap();
    for line in summaries.lines().filter(|line| line.chars().next().is_some_and(|c| c.is_ascii_digit())) {
        assert!(line.chars().count() < 200, "{} chars: {line}", line.chars().count());
    }
    assert!(result.text.contains(&last), "the final result was truncated: {}", result.text);
}

#[test]
fn batch_full_returns_all_text_including_long_earlier_results() {
    let (_srv, mut a) = setup();
    let early = format!("EARLY-{}", "é".repeat(320));
    let result = call(&mut a, "batch", &json!({"full":true,"steps":[
        {"tool":"debug","args":{"eval":format!("'{early}'")}},
        {"tool":"debug","args":{"eval":"'last'"}}
    ]})).unwrap();
    assert!(result.text.contains(&early), "{}", result.text);
    assert!(result.text.contains("Step 1 (debug):") && result.text.contains("Step 2 (debug):"), "{}", result.text);
    assert!(!result.text.contains("Earlier steps:"), "{}", result.text);
}

#[test]
fn batch_detects_partial_fill_failure_without_rolling_back_successful_fields() {
    let (_srv, mut a) = setup();
    let args = json!({"selector":"#form","values":{"filled":"kept","locked":"rejected"}});
    let error = call(&mut a, "batch", &json!({"steps":[
        {"tool":"fill","args":args},
        {"tool":"click","args":{"selector":"#after"}}
    ]})).unwrap_err();
    assert!(error.contains("attempted 1/2, succeeded 0, failed 1, pending 1"), "{error}");
    assert!(error.contains("Not filled:"), "{error}");
    let state = ok(&mut a, "debug", json!({"eval":"document.getElementById('filled').value+'|'+document.getElementById('locked').value+'|'+typeof batchAfter"}));
    assert!(state.contains("kept|initial|undefined"), "{state}");
    let standalone = call(&mut a, "fill", &json!({"selector":"#form","values":{"filled":"standalone","locked":"rejected"}})).unwrap();
    assert!(standalone.failed, "partial failure must be explicit without changing standalone Result");
}

#[test]
fn batch_debug_failure_is_structured_and_does_not_treat_error_like_values_as_errors() {
    let (_srv, mut a) = setup();
    let good = call(&mut a, "debug", &json!({"eval":"'Error: a legitimate returned value'"})).unwrap();
    assert!(!good.failed, "{}", good.text);
    for source in ["throw new Error('expected')", "const = invalid", "Promise.reject(new Error('expected rejection'))"] {
        let error = call(&mut a, "batch", &json!({"steps":[
            {"tool":"debug","args":{"eval":source}},
            {"tool":"click","args":{"selector":"#after"}}
        ]})).unwrap_err();
        assert!(error.contains("attempted 1/2, succeeded 0, failed 1, pending 1"), "{source}: {error}");
    }
    let state = ok(&mut a, "debug", json!({"eval":"typeof batchAfter"}));
    assert!(state.contains("undefined"), "{state}");
}

#[test]
fn selectors_reject_ambiguous_conflicting_and_invalid_targets_and_reach_child_frames() {
    let (_srv, mut a) = setup();
    assert!(call(&mut a, "click", &json!({"selector":"button"})).unwrap_err().contains("expected exactly one"));
    assert!(call(&mut a, "click", &json!({"ref":"cookies","selector":"#first"})).unwrap_err().contains("not both"));
    assert!(call(&mut a, "type", &json!({"selector":"[","text":"x"})).unwrap_err().contains("invalid selector"));
    ok(&mut a, "type", json!({"selector":"#child-field","text":"after","clear":true}));
    let state = ok(&mut a, "debug", json!({"eval":"document.querySelector('iframe').contentDocument.getElementById('child-field').value"}));
    assert!(state.contains("after"), "{state}");
    let state = ok(&mut a, "debug", json!({"eval":"typeof batchCount"}));
    assert!(state.contains("undefined"), "{state}");
}

fn coherence_setup() -> (Server, Agent) {
    let srv = serve(Box::new(|path| match path {
        "/" => html(r#"<!doctype html><html><body>
            <p id="parent-status">none</p>
            <iframe id="child" src="/child" style="width:420px;height:180px"></iframe>
            <iframe id="outer" src="/nested" style="width:420px;height:260px"></iframe>
            </body></html>"#),
        "/child" => html(r#"<!doctype html><html><body>
            <input id="coherent" aria-label="Coherent field" value="initial"
             oninput="parent.document.getElementById('parent-status').textContent=this.value">
            </body></html>"#),
        "/nested" => html(r#"<!doctype html><html><body>
            <iframe id="inner" src="/leaf" style="width:350px;height:160px"></iframe>
            </body></html>"#),
        "/leaf" => html("<!doctype html><html><body><input id='grand' aria-label='Grandchild field' value='initial-grand'></body></html>"),
        _ => None,
    }));
    let mut a = agent();
    ok(&mut a, "open", json!({"url":srv.url("/")}));
    (srv, a)
}

#[test]
fn frame_dom_is_shared_between_parent_script_snapshots_and_ref_typing() {
    let (_srv, mut a) = coherence_setup();
    ok(&mut a, "debug", json!({"eval":"document.getElementById('child').contentDocument.getElementById('coherent').value='from parent script'"}));
    let view = ok(&mut a, "snapshot", json!({}));
    assert!(view.contains("value=\"from parent script\""), "parent JS must update the document exposed by the agent: {view}");
    let reference = ref_of(&view, "textbox \"Coherent field\"");
    ok(&mut a, "type", json!({"ref":reference,"text":"from ref action","clear":true}));
    let state = ok(&mut a, "debug", json!({"eval":"document.getElementById('child').contentDocument.getElementById('coherent').value"}));
    assert!(state.contains("from ref action"), "ref typing must update the same document parent JS reads: {state}");
}

#[test]
fn selector_typing_in_a_child_runs_events_against_the_real_parent_context() {
    let (_srv, mut a) = coherence_setup();
    ok(&mut a, "type", json!({"selector":"#coherent","text":"event-value","clear":true}));
    let state = ok(&mut a, "debug", json!({"eval":"document.getElementById('parent-status').textContent+'|'+document.getElementById('child').contentDocument.getElementById('coherent').value"}));
    assert!(state.contains("event-value|event-value"), "child input and its parent event effect must share the canonical frame tree: {state}");
}

#[test]
fn nested_frame_selectors_and_parent_script_reads_use_the_same_grandchild() {
    let (_srv, mut a) = coherence_setup();
    let path = "document.getElementById('outer').contentDocument.getElementById('inner').contentDocument.getElementById('grand')";
    ok(&mut a, "debug", json!({"eval":format!("{path}.value='from nested parent'")}));
    let view = ok(&mut a, "snapshot", json!({}));
    assert!(view.contains("value=\"from nested parent\""), "nested parent mutation must reach the flattened grandchild: {view}");
    ok(&mut a, "fill", json!({"selector":"#grand","value":"from nested agent"}));
    let state = ok(&mut a, "debug", json!({"eval":format!("{path}.value")}));
    assert!(state.contains("from nested agent"), "nested selector fill must reach the parent's actual grandchild: {state}");
}
