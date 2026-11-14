//! Independent integration controls for RH001v2/RH002.
//! Source-only until composed with their implementations and validated in CI.
use std::path::PathBuf;
use std::time::Duration;

use rabbithole_agent::{call, Agent, Config};
use serde_json::{json, Value};

fn fixture(name: &str) -> (Agent, String) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let base = rabbithole_engine::reftest::serve_dir(root).expect("fixture server");
    let url = format!("{base}{name}");
    let mut agent = Agent::new(Config { profile_dir: None, settle_cap: Duration::from_secs(3), ..Config::default() });
    ok(&mut agent, "open", json!({"url":url}));
    (agent, url)
}

fn ok(agent: &mut Agent, tool: &str, args: Value) -> String {
    call(agent, tool, &args).unwrap_or_else(|error| panic!("{tool} {args}: {error}")).text
}

fn result_lines(text: &str) -> Vec<&str> {
    text.lines().filter(|line| line.starts_with("@e")).collect()
}

fn excerpt(line: &str) -> &str {
    line.split('"').nth(1).expect("quoted search excerpt")
}

#[test]
fn term_priority_selects_windows_but_emission_keeps_textual_order_and_presence() {
    let (mut agent, _) = fixture("mcp-peer-facts.html");
    let output = ok(&mut agent, "find", json!({
        "text":["DELTA","BETA","GAMMA","ALPHA","ABSENT_TERM"],"window":40,"limit":1
    }));
    let lines = result_lines(&output);
    assert_eq!(lines.len(), 1, "{output}");
    let passage = excerpt(lines[0]);
    assert!(passage.chars().count() <= 120, "{output}");
    assert!(passage.contains("BETA") && passage.contains("GAMMA") && passage.contains("DELTA"), "{output}");
    assert!(!passage.contains("ALPHA"), "request priority determines admission: {output}");
    assert!(passage.find("BETA").unwrap() < passage.find("GAMMA").unwrap() && passage.find("GAMMA").unwrap() < passage.find("DELTA").unwrap(), "{output}");
    assert!(output.contains("(+1 more in this element)"), "{output}");
    assert_eq!(output.lines().find(|line| line.starts_with("No matches for:")), Some("No matches for: \"ABSENT_TERM\""));
}

#[test]
fn overlapping_windows_are_united_and_expanding_unicode_keeps_original_characters() {
    let (mut agent, _) = fixture("mcp-peer-facts.html");
    let output = ok(&mut agent, "find", json!({"text":["OVERLAPONE","OVERLAPTWO"],"window":60}));
    let lines = result_lines(&output);
    assert_eq!(lines.len(), 1, "{output}");
    let passage = excerpt(lines[0]);
    assert_eq!(passage.matches("OVERLAPONE").count(), 1, "{output}");
    assert_eq!(passage.matches("OVERLAPTWO").count(), 1, "{output}");
    assert!(!output.contains("more in this element"), "both hits are visible: {output}");
    let output = ok(&mut agent, "find", json!({"text":["\u{307}","ος","needle"],"window":30}));
    let lines = result_lines(&output);
    assert_eq!(lines.len(), 1, "{output}");
    let passage = excerpt(lines[0]);
    assert!(passage.contains("İx ΟΣ NEEDLE"), "{output}");
    assert!(passage.chars().count() <= 90, "{output}");
    assert!(!output.contains("more in this element"), "{output}");
    for window in [1, 2] {
        let output = ok(&mut agent, "find", json!({"text":"needle","window":window}));
        assert!(excerpt(result_lines(&output)[0]).chars().count() <= window * 3, "{output}");
        assert!(output.contains("(+1 more in this element)"), "a clipped hit is not fully shown: {output}");
    }
    let legacy = ok(&mut agent, "find", json!({"text":"DELTA","context":0}));
    let passage = excerpt(result_lines(&legacy)[0]);
    assert!(passage.starts_with("START ") && !passage.contains("DELTA"), "{legacy}");
    assert!(passage.chars().count() <= 120, "{legacy}");
}

#[test]
fn redundant_links_do_not_remove_controls_or_break_stable_link_refs() {
    let (mut agent, _) = fixture("mcp-peer-facts.html");
    let before = ok(&mut agent, "find", json!({"role":"link","name":"FACTTOKEN"}));
    let original = result_lines(&before)[0].split_whitespace().next().unwrap().to_string();
    let found = ok(&mut agent, "find", json!({"text":"FACTTOKEN"}));
    let lines = result_lines(&found);
    assert!(found.contains("redundant links omitted; use role=link to include"), "{found}");
    assert!(lines.iter().any(|line| line.contains(" button ")), "button remains actionable: {found}");
    assert!(lines.iter().any(|line| line.contains(" link ") && excerpt(line).contains("extended link")), "longer links remain: {found}");
    let after = ok(&mut agent, "find", json!({"role":"link","name":"FACTTOKEN"}));
    assert_eq!(result_lines(&after)[0].split_whitespace().next().unwrap(), original);
    assert!(ok(&mut agent, "read", json!({"ref":original})).contains("FACTTOKEN"));
    ok(&mut agent, "click", json!({"ref":original}));
    assert_eq!(agent.tabs[agent.active.unwrap()].page().unwrap().url.fragment.as_deref(), Some("destination"));
}

#[test]
fn window_is_canonical_and_numeric_open_context_rejects_before_state_changes() {
    let (mut agent, url) = fixture("mcp-peer-facts.html");
    let canonical = ok(&mut agent, "find", json!({"text":"needle","window":30}));
    let alias = ok(&mut agent, "find", json!({"text":"needle","context":30}));
    assert_eq!(result_lines(&canonical), result_lines(&alias));
    let wins = ok(&mut agent, "find", json!({"text":"needle","window":30,"context":"ignored alias"}));
    assert_eq!(result_lines(&canonical), result_lines(&wins));
    let before = (agent.contexts.len(), agent.tabs.len(), agent.active, agent.cfg.bot, agent.engine.scripting, agent.engine.loader.load_scripts);
    for context in [json!(240), json!("240")] {
        assert!(call(&mut agent, "open", &json!({"url":url,"context":context,"find":"needle","window":30,"bot":true,"javascript":false})).is_err());
        assert_eq!((agent.contexts.len(), agent.tabs.len(), agent.active, agent.cfg.bot, agent.engine.scripting, agent.engine.loader.load_scripts), before);
        assert_eq!(agent.tabs[agent.active.unwrap()].page().unwrap().title, "Search contract fixture");
    }
}

#[test]
fn open_search_and_optional_summary_share_exactly_one_untrusted_fence() {
    let (mut agent, url) = fixture("mcp-peer-facts.html");
    let output = ok(&mut agent, "open", json!({"url":url,"find":["ALPHA","DELTA"],"window":40,"summary":true}));
    assert_eq!(output.matches("⟦page ").count(), 1, "{output}");
    assert_eq!(output.matches("⟦/page ").count(), 1, "{output}");
    assert!(output.contains("outline:") || output.contains("main"), "{output}");
    assert!(excerpt(result_lines(&output)[0]).contains("ALPHA"), "{output}");
    assert!(excerpt(result_lines(&output)[0]).contains("DELTA"), "{output}");
}

#[test]
fn read_after_dom_append_uses_live_content_but_explicit_main_ref_keeps_its_scope() {
    let (mut agent, _) = fixture("mcp-peer-dynamic.html");
    let shell = ok(&mut agent, "find", json!({"selector":"#shell"}));
    let reference = result_lines(&shell)[0].split_whitespace().next().unwrap().to_string();
    ok(&mut agent, "debug", json!({"eval":"appendLive(); 'done'"}));
    // Deliberately no manual restyle: ordinary debug already settles the page.
    // Keep that freshness contract while changing content selection.
    let output = ok(&mut agent, "read", json!({"links":false}));
    assert!(output.contains("LIVE_BODY_MARKER") && output.contains("LIVE_BODY_END"), "{output}");
    for hidden in ["HIDDEN_ONLY","SCRIPT_ONLY","FOOTER_ONLY"] { assert!(!output.contains(hidden), "{output}"); }
    let scoped = ok(&mut agent, "read", json!({"ref":reference,"links":false}));
    assert!(scoped.contains("Workspace shell"), "{scoped}");
    assert!(!scoped.contains("LIVE_BODY_MARKER"), "{scoped}");
}

#[test]
fn content_reading_does_not_remove_form_and_button_actions_from_summary_or_snapshot() {
    let (mut agent, _) = fixture("mcp-peer-dynamic.html");
    let button = ok(&mut agent, "find", json!({"role":"button","name":"Load details"}));
    let reference = result_lines(&button)[0].split_whitespace().next().unwrap().to_string();
    ok(&mut agent, "click", json!({"ref":reference}));
    let output = ok(&mut agent, "read", json!({"links":false}));
    assert!(output.contains("LIVE_BODY_MARKER"), "{output}");
    let ti = agent.active.unwrap();
    let summary = agent.summary_text(ti).unwrap();
    assert!(summary.contains("forms:") && summary.contains("Name") && summary.contains("Save"), "{summary}");
    let snapshot = ok(&mut agent, "snapshot", json!({"max_tokens":4000}));
    assert!(snapshot.contains("Load details") && snapshot.contains("Save") && snapshot.contains("textbox"), "{snapshot}");
    assert!(ok(&mut agent, "read", json!({"ref":reference})).contains("Load details"));
}

fn volume(summary: &str) -> usize {
    summary.lines().find_map(|line| line.strip_prefix("text: "))
        .expect("volume line for substantial prose with few headings")
        .split_whitespace().next().unwrap().parse().unwrap()
}

#[test]
fn volume_ignores_hidden_source_and_counts_visible_unicode_as_characters() {
    let (mut agent, _) = fixture("mcp-peer-dynamic.html");
    ok(&mut agent, "debug", json!({"eval":"appendLive(); 'done'"}));
    assert!(ok(&mut agent, "read", json!({"links":false})).contains("LIVE_BODY_END"));
    let ti = agent.active.unwrap();
    let initial = volume(&agent.summary_text(ti).unwrap());
    ok(&mut agent, "debug", json!({"eval":"document.getElementById('hidden-noise').textContent += 'HIDDEN_ONLY '.repeat(3000); document.getElementById('source-noise').textContent += 'SCRIPT_ONLY '.repeat(3000); 'done'"}));
    ok(&mut agent, "read", json!({"links":false}));
    assert_eq!(volume(&agent.summary_text(ti).unwrap()), initial);
    ok(&mut agent, "debug", json!({"eval":"document.querySelector('#feed p').appendChild(document.createTextNode('🙂'.repeat(300))); 'done'"}));
    ok(&mut agent, "read", json!({"links":false}));
    let enlarged = volume(&agent.summary_text(ti).unwrap());
    let added = enlarged.checked_sub(initial).expect("visible text must not reduce the volume");
    assert!((300..=302).contains(&added), "300 Unicode scalars, not 1200 UTF-8 bytes: {initial} -> {enlarged}");
}
