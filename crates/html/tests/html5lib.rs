//! html5lib tree-construction tests (the `.dat` files shipped with
//! web-platform-tests in `html/syntax/parsing/resources/`), run directly
//! against the parser. Skipped when no WPT checkout is present (set
//! `RH_WPT_ROOT`); `RH_HTML5LIB_VERBOSE=1` prints every failure.

use rabbithole_dom::{Document, Namespace, NodeData, NodeId};

struct Case {
    data: String,
    document: String,
    fragment: Option<String>,
    script_off: bool,
}

/// Mirrors the `.dat` reader in the WPT runner (`resources/test.js`).
fn parse_dat(text: &str) -> Vec<Case> {
    let text = text.strip_suffix('\n').unwrap_or(text);
    let lines: Vec<&str> = text.split('\n').collect();
    let mut cases = Vec::new();
    let mut data: Option<Vec<(String, String)>> = None;
    let mut key: Option<String> = None;
    let finish = |d: Vec<(String, String)>, cases: &mut Vec<Case>| {
        let get = |k: &str| d.iter().find(|(n, _)| n == k).map(|(_, v)| v.strip_suffix('\n').unwrap_or(v).to_string());
        cases.push(Case {
            data: get("data").unwrap_or_default(),
            document: get("document").unwrap_or_default(),
            fragment: get("document-fragment"),
            script_off: d.iter().any(|(n, _)| n == "script-off"),
        });
    };
    for (i, l) in lines.iter().enumerate() {
        let line = if i == lines.len() - 1 { l.to_string() } else { format!("{l}\n") };
        let heading = line.strip_prefix('#').map(|h| h.trim().to_string()).filter(|h| !h.is_empty());
        if let Some(h) = heading {
            if let Some(d) = data.as_mut() {
                if h == "description" || (h == "data" && key.as_deref() != Some("description")) {
                    if let Some(k) = &key {
                        if let Some(e) = d.iter_mut().find(|(n, _)| n == k) {
                            e.1.pop();
                        }
                    }
                    finish(data.take().unwrap(), &mut cases);
                }
            }
            data.get_or_insert_with(Vec::new).push((h.clone(), String::new()));
            key = Some(h);
        } else if let (Some(d), Some(k)) = (data.as_mut(), &key) {
            if let Some(e) = d.iter_mut().find(|(n, _)| n == k) {
                e.1.push_str(&line);
            }
        }
    }
    if let Some(d) = data {
        finish(d, &mut cases);
    }
    cases
}

fn prefix(ns: &Namespace) -> &'static str {
    match ns {
        Namespace::Svg => "svg",
        Namespace::MathMl => "math",
        _ => "",
    }
}

fn serialize(doc: &Document, root: NodeId) -> String {
    let mut lines = Vec::new();
    fn walk(doc: &Document, n: NodeId, depth: usize, lines: &mut Vec<String>) {
        let pad = if depth > 0 { " ".repeat(2 * depth - 1) } else { String::new() };
        let inner = " ".repeat(2 * depth + 1);
        match &doc.node(n).data {
            NodeData::Doctype(d) => {
                if d.name.is_empty() && d.public_id.is_empty() && d.system_id.is_empty() {
                    lines.push(format!("|{pad}<!DOCTYPE >"));
                } else if !d.public_id.is_empty() || !d.system_id.is_empty() {
                    lines.push(format!("|{pad}<!DOCTYPE {} \"{}\" \"{}\">", d.name, d.public_id, d.system_id));
                } else {
                    lines.push(format!("|{pad}<!DOCTYPE {}>", d.name));
                }
            }
            NodeData::Comment(c) => match doc.pi_target(n) {
                Some(t) => lines.push(format!("|{pad}<?{t} {c}?>")),
                None => lines.push(format!("|{pad}<!-- {c} -->")),
            },
            NodeData::Text(t) => lines.push(format!("|{pad}\"{t}\"")),
            NodeData::Element(e) => {
                let tag = match e.ns {
                    Namespace::Html => e.tag.clone(),
                    _ => format!("{} {}", prefix(&e.ns), e.tag),
                };
                lines.push(format!("|{pad}<{tag}>"));
                let mut attrs: Vec<(String, String)> = e
                    .attrs
                    .iter()
                    .map(|(k, v)| {
                        let name = match doc.attr_namespace(n, k) {
                            Some("http://www.w3.org/1999/xlink") => format!("xlink {}", k.split_once(':').map(|x| x.1).unwrap_or(k)),
                            Some("http://www.w3.org/XML/1998/namespace") => format!("xml {}", k.split_once(':').map(|x| x.1).unwrap_or(k)),
                            Some("http://www.w3.org/2000/xmlns/") => format!("xmlns {}", k.split_once(':').map(|x| x.1).unwrap_or(k)),
                            _ => k.clone(),
                        };
                        (name, v.clone())
                    })
                    .collect();
                attrs.sort();
                for (k, v) in attrs {
                    lines.push(format!("|{inner}{k}=\"{v}\""));
                }
                if e.ns == Namespace::Html && e.tag == "template" {
                    lines.push(format!("|{inner}content"));
                    if let Some(&c) = doc.template_contents.get(&n) {
                        for k in doc.children(c) {
                            walk(doc, k, depth + 2, lines);
                        }
                    }
                }
            }
            NodeData::Document => {}
        }
        for k in doc.children(n) {
            walk(doc, k, depth + 1, lines);
        }
    }

    for k in doc.children(root) {
        walk(doc, k, 1, &mut lines);
    }
    lines.join("\n")
}

fn run_case(c: &Case) -> String {
    match &c.fragment {
        Some(ctx) => {
            let (ns, name) = match ctx.split_once(' ') {
                Some(("svg", n)) => (Namespace::Svg, n),
                Some(("math", n)) => (Namespace::MathMl, n),
                _ => (Namespace::Html, ctx.as_str()),
            };
            let (doc, container) = rabbithole_html::parse_fragment_in(&c.data, name, ns, true, rabbithole_dom::QuirksMode::NoQuirks);
            serialize(&doc, container)
        }
        None => {
            let doc = rabbithole_html::parse_with_scripting(&c.data, true);
            serialize(&doc, Document::ROOT)
        }
    }
}

#[test]
fn html5lib_tree_construction() {
    let root = std::env::var_os("RH_WPT_ROOT").map(std::path::PathBuf::from).unwrap_or_else(|| "/mnt/hdd5tb/rabbithole/suites/wpt-dom".into());
    let dir = root.join("html/syntax/parsing/resources");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        eprintln!("skipping: no html5lib data at {}", dir.display());
        return;
    };
    let verbose = std::env::var_os("RH_HTML5LIB_VERBOSE").is_some();
    let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| e == "dat").unwrap_or(false)).collect();
    files.sort();
    let (mut pass, mut total) = (0, 0);
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        let (mut fp, mut ft) = (0, 0);
        for c in parse_dat(&text).iter().filter(|c| !c.script_off) {
            ft += 1;
            let got = std::panic::catch_unwind(|| run_case(c)).unwrap_or_else(|_| "PANIC".into());
            if got == c.document {
                fp += 1;
            } else if verbose {
                eprintln!("--- {} {:?}\n{}\nexpected:\n{}\ngot:\n{}\n", f.file_name().unwrap().to_string_lossy(), c.fragment, c.data, c.document, got);
            }
        }
        eprintln!("{:<45} {fp}/{ft}", f.file_name().unwrap().to_string_lossy());
        pass += fp;
        total += ft;
    }
    eprintln!("html5lib: {pass}/{total}");
    // Regression floor (raise as the parser improves).
    assert!(pass * 100 >= total * 94, "html5lib pass rate dropped: {pass}/{total}");
}

/// Parse a document through the incremental parser, `chunk` characters
/// per `write` (as `document.write` after `document.open()`).
fn run_incremental(data: &str, chunk: usize) -> String {
    let mut doc = Document::new();
    let mut p = rabbithole_html::Parser::new(true);
    p.set_insertion_point_at_end();
    let chars: Vec<char> = data.chars().collect();
    let drive = |p: &mut rabbithole_html::Parser, doc: &mut Document| loop {
        match p.run(doc) {
            rabbithole_html::ParseStatus::Script(_) => continue,
            _ => break,
        }
    };
    for piece in chars.chunks(chunk.clamp(1, chars.len().max(1))) {
        p.write(&piece.iter().collect::<String>());
        drive(&mut p, &mut doc);
    }
    p.close();
    drive(&mut p, &mut doc);
    assert!(p.is_finished());
    serialize(&doc, Document::ROOT)
}

/// The incremental parser builds the same trees as the one-shot parser,
/// whole and one character per write.
#[test]
fn html5lib_incremental_matches() {
    let root = std::env::var_os("RH_WPT_ROOT").map(std::path::PathBuf::from).unwrap_or_else(|| "/mnt/hdd5tb/rabbithole/suites/wpt-dom".into());
    let dir = root.join("html/syntax/parsing/resources");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        eprintln!("skipping: no html5lib data at {}", dir.display());
        return;
    };
    let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| e == "dat").unwrap_or(false)).collect();
    files.sort();
    let mut bad = Vec::new();
    let mut n = 0;
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        for c in parse_dat(&text).iter().filter(|c| !c.script_off && c.fragment.is_none()) {
            n += 1;
            let whole = serialize(&rabbithole_html::parse_with_scripting(&c.data, true), Document::ROOT);
            for chunk in [usize::MAX, 1, 3] {
                let got = std::panic::catch_unwind(|| run_incremental(&c.data, chunk)).unwrap_or_else(|_| "PANIC".into());
                if got != whole {
                    bad.push(format!("--- {} chunk {chunk}\n{}\nwhole:\n{}\nincremental:\n{}\n", f.file_name().unwrap().to_string_lossy(), c.data, whole, got));
                    break;
                }
            }
        }
    }
    for b in bad.iter().take(10) {
        eprintln!("{b}");
    }
    assert!(bad.is_empty(), "{} of {n} documents differ when parsed incrementally", bad.len());
}
