//! Main-content selection through the real tool dispatcher; local fixtures only.
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::Duration;

use rabbithole_agent::{ax, call, read, refs::RefMap, summary, Agent, Config};
use rabbithole_engine::{net::Url, Engine, Page, Viewport};
use serde_json::{json, Value};

const DYNAMIC: &str = include_str!("fixtures/reading-dynamic-cards.html");

fn serve(html: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}/", listener.local_addr().unwrap().port());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 { return; }
                loop {
                    line.clear();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 { return; }
                    if line == "\r\n" || line == "\n" { break; }
                }
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}", html.len());
                let _ = stream.write_all(response.as_bytes());
            });
        }
    });
    url
}

fn ok(agent: &mut Agent, tool: &str, args: Value) -> String {
    call(agent, tool, &args).unwrap_or_else(|e| panic!("{tool}: {e}")).text
}

fn body(output: &str) -> &str {
    output.split_once("⟧\n").unwrap().1.rsplit_once("\n⟦/page ").unwrap().0
}

fn reference(output: &str, name: &str) -> String {
    let rest = &output[output.find(name).unwrap_or_else(|| panic!("{name}: {output}")) + name.len()..];
    let rest = &rest[rest.find('@').unwrap()..];
    rest.chars().take_while(|c| *c == '@' || c.is_ascii_alphanumeric()).collect()
}

fn load_page(html: &str) -> Page {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.scripting = false;
    engine.load_html(html, Url::parse_absolute("https://reader.example.test/").unwrap(), Viewport { width: 1000.0, height: 800.0 })
}

fn overview(page: &Page) -> String {
    let mut refs = RefMap::default();
    let tree = ax::build(&[ax::FrameView { page, children: vec![] }], &mut refs, &ax::BuildOptions::default()).0;
    summary::render(page, &tree, &mut refs, &[]).join("\n")
}

#[test]
fn read_dynamic_inline_cards_keeps_document_write_load_and_later_append() {
    let url = serve(DYNAMIC);
    let mut agent = Agent::new(Config { profile_dir: None, settle_cap: Duration::from_secs(3), ..Config::default() });
    let opened = ok(&mut agent, "open", json!({"url": url}));
    assert!(opened.contains("chars in main (read shows it)"), "{opened}");
    assert!(opened.chars().count() < 900, "{opened}");
    assert!(!opened.contains("Loaded saying 2:"), "summary must not dump the article: {opened}");
    let plain = ok(&mut agent, "read", json!({"links": false}));
    let text = body(&plain);
    assert_eq!(text.matches("saying ").count(), 10, "{plain}");
    assert!(text.contains("Written saying 1:") && text.contains("Loaded saying 10:") && text.contains("pingüino 🐇"), "{plain}");
    assert!(!text.contains("Footer-only") && !text.contains("Navigation only") && !text.contains("Add another saying"), "{plain}");
    assert!(!text.contains("@e"), "{plain}");

    let linked = ok(&mut agent, "read", json!({}));
    let writer = reference(&linked, "[Writer 1]");
    assert!(body(&linked).contains("[Writer 1](@e"), "{linked}");
    let scoped = ok(&mut agent, "read", json!({"ref": writer, "links": false}));
    assert_eq!(body(&scoped), "Writer 1", "{scoped}");
    let limited = ok(&mut agent, "read", json!({"links": false, "max_tokens": 100}));
    assert_eq!(body(&limited), text.chars().take(400).collect::<String>());
    assert!(limited.contains("continues: read offset=400"), "{limited}");
    let continued = ok(&mut agent, "read", json!({"links": false, "offset": 400, "max_tokens": 100}));
    assert_eq!(body(&continued), text.chars().skip(400).take(400).collect::<String>());

    let snapshot = ok(&mut agent, "snapshot", json!({}));
    let more = reference(&snapshot, "button \"Add another saying\"");
    ok(&mut agent, "click", json!({"ref": more}));
    let changed = ok(&mut agent, "read", json!({"links": false}));
    assert!(body(&changed).contains("Loaded saying 11:"), "{changed}");
    assert_eq!(body(&changed).matches("saying ").count(), 11, "{changed}");
}

#[test]
fn read_main_and_article_markers_preserve_prose_and_citation_options() {
    let prose = "A substantive article explains its subject with complete sentences, enough context, and useful details for a careful reader. ".repeat(6);
    let page = load_page(&format!("<!doctype html><nav><p>Navigation noise</p></nav><main><article><h1>Article</h1><p>{prose}<a href='/guide'>guide</a><sup><a href='#note'>[1]</a></sup></p></article></main><footer><p>Footer noise</p></footer>"));
    let selected = read::main_content(&page).unwrap();
    assert!(page.doc.is_element(selected, "article"));
    let plain = read::read_with_options(&page, &mut RefMap::default(), 0, false, false);
    assert!(plain.contains(&prose) && plain.contains("guide"), "{plain}");
    assert!(!plain.contains("Navigation noise") && !plain.contains("Footer noise") && !plain.contains("[1]"), "{plain}");
    let cited = read::read_with_options(&page, &mut RefMap::default(), 0, true, true);
    assert!(cited.contains("[guide](@e") && cited.contains("[[1]](@e"), "{cited}");
}

#[test]
fn read_falls_back_to_filtered_body_when_no_prose_container_is_clear() {
    let page = load_page("<!doctype html><nav><p>Not the content, despite a paragraph long enough to attract a score.</p></nav><span>First independent fragment.</span><span>Second independent fragment.</span><footer><p>Not the content either, with a tempting paragraph and <a href='/'>a link</a>.</p></footer>");
    assert_eq!(read::main_content(&page), page.doc.body());
    let text = read::read_with_options(&page, &mut RefMap::default(), 0, false, false);
    assert!(text.contains("First independent fragment.") && text.contains("Second independent fragment."), "{text}");
    assert!(!text.contains("Not the content"), "{text}");
}

#[test]
fn read_small_semantic_main_does_not_displace_substantial_prose_outside() {
    let shell = "An introduction which does not contain the actual collection. ".repeat(5);
    let story = "The actual collection remains outside the shell, with enough detail to identify the page's substantial prose. ".repeat(10);
    let page = load_page(&format!("<!doctype html><main><p>{shell}</p><button>Open collection</button></main><article><p>{story}</p></article><footer><p>Footer information, not the collection.</p></footer>"));
    let text = read::read_with_options(&page, &mut RefMap::default(), 0, false, false);
    assert!(text.contains(story.trim()), "{text}");
    assert!(!text.contains("Footer information") && !text.contains("Open collection"), "{text}");
}

#[test]
fn summary_sparse_outline_counts_unicode_prose_without_markup_or_hidden_text() {
    let prose = "é🐇".repeat(110);
    let hidden = "INVISIBLE".repeat(2000);
    let page = load_page(&format!("<!doctype html><main><p>{prose}</p><p hidden>{hidden}</p><script>{hidden}</script><form><label>Secret form label<input value='hidden-value'></label></form></main><footer>{hidden}</footer>"));
    let summary = overview(&page);
    assert!(summary.contains("text: 220 chars in main (read shows it)"), "{summary}");
    assert!(!summary.contains("INVISIBLE") && !summary.contains("hidden-value"), "{summary}");
    let short = overview(&load_page("<!doctype html><main><p>A short introduction.</p></main>"));
    assert!(!short.contains("chars in main"), "{short}");
}
