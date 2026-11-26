use rabbithole_css::parse_stylesheet;
use rabbithole_style::{MatchState, MediaEnv, StyleEngine, StyleOptions};

fn authored(markup: &str, css: &str) -> bool {
    let doc = rabbithole_html::parse(markup);
    let sheets = [parse_stylesheet(css)];
    let env = MediaEnv { width: 800., height: 600., screen: true, scripting: true };
    let mut engine = StyleEngine::new(&sheets, &env);
    let opts = StyleOptions { state: &MatchState::default(), resolve_url: &|_, _| None, document_base: "" };
    let tree = engine.full(&doc, &sheets, &opts);
    engine.author_white_space(&doc, &opts, &tree, doc.element_by_id("target").unwrap())
}

#[test]
fn whitespace_origin_for_editing() {
    let nested = "<pre><b id=target>abc</b></pre>";
    assert!(!authored(nested, ""));
    assert!(authored(nested, "pre {white-space:pre}"));
    assert!(authored("<pre style='white-space:pre'><b id=target></b></pre>", ""));
    assert!(!authored(nested, "pre::before {white-space:pre}"));
    assert!(!authored(nested, "pre {white-space:pre; white-space:revert}"));
    let inline = "<pre id=target style='white-space:pre'></pre>";
    assert!(!authored(inline, "pre {white-space:revert !important}"));
    assert!(authored(inline, "pre {white-space:revert}"));
    assert!(authored(nested, "@layer a {pre {white-space:pre}} @layer b {pre {white-space:revert-layer}}"));
    assert!(authored(nested, "b {white-space-collapse:preserve}"));
}
