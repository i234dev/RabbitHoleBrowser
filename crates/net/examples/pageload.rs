//! Network benchmark: fetch a page and all of its static subresources
//! (stylesheets, scripts, images, icons) the way the engine does
//! (`fetch_many_for` per destination), and report timings.
//!
//! `cargo run --release -p rabbithole-net --example pageload -- <url> [runs]`

use rabbithole_net::{Destination, Fetcher, Url};
use std::time::Instant;

fn attr_values(html: &str, attr: &str) -> Vec<String> {
    let mut out = Vec::new();
    let lower = html.to_ascii_lowercase();
    let pat = format!(" {attr}=");
    let mut i = 0;
    while let Some(p) = lower[i..].find(&pat) {
        let start = i + p + pat.len();
        let rest = &html[start..];
        let (val, used) = match rest.chars().next() {
            Some(q @ ('"' | '\'')) => {
                let end = rest[1..].find(q).map(|e| e + 1).unwrap_or(rest.len());
                (rest[1..end].to_string(), end + 1)
            }
            _ => {
                let end = rest.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(rest.len());
                (rest[..end].to_string(), end)
            }
        };
        out.push(val.replace("&amp;", "&"));
        i = start + used.max(1);
        if i >= html.len() {
            break;
        }
    }
    out
}

fn classify(u: &str) -> Option<Destination> {
    let path = u.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
    if path.ends_with(".css") {
        Some(Destination::Style)
    } else if path.ends_with(".js") || path.ends_with(".mjs") {
        Some(Destination::Script)
    } else if [".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".ico", ".avif"].iter().any(|e| path.ends_with(e)) {
        Some(Destination::Image)
    } else if [".woff", ".woff2", ".ttf", ".otf"].iter().any(|e| path.ends_with(e)) {
        Some(Destination::Font)
    } else {
        None
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let url = Url::parse(args.get(1).map(|s| s.as_str()).unwrap_or("https://www.wikipedia.org/")).expect("url");
    let runs: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2);
    let fetcher = Fetcher::default();
    for run in 0..runs {
        let t0 = Instant::now();
        let doc = match fetcher.fetch(&url) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("document: {e}");
                return;
            }
        };
        let t_doc = t0.elapsed();
        let html = String::from_utf8_lossy(&doc.body).into_owned();
        let mut groups: Vec<(Destination, Vec<Url>)> = vec![
            (Destination::Style, vec![]),
            (Destination::Script, vec![]),
            (Destination::Font, vec![]),
            (Destination::Image, vec![]),
        ];
        let mut seen = std::collections::HashSet::new();
        for v in attr_values(&html, "src").into_iter().chain(attr_values(&html, "href")) {
            let Some(d) = classify(&v) else { continue };
            let Some(u) = doc.url.join(v.trim()) else { continue };
            if !u.is_http() || !seen.insert(u.to_string()) {
                continue;
            }
            groups.iter_mut().find(|g| g.0 == d).unwrap().1.push(u);
        }
        let n: usize = groups.iter().map(|g| g.1.len()).sum();
        let t1 = Instant::now();
        let mut bytes = doc.body.len();
        let mut errors = 0;
        // Like the engine: one batch per destination, sequentially.
        for (d, urls) in &groups {
            let par = if *d == Destination::Image { 8 } else { 6 };
            for r in fetcher.fetch_many_for(urls, par, *d, Some(&doc.url)) {
                match r {
                    Ok(r) => bytes += r.body.len(),
                    Err(_) => errors += 1,
                }
            }
        }
        let t_sub = t1.elapsed();
        println!(
            "run {run}: document {:?} ({} B), {n} subresources in {:?} ({errors} errors), total {:?}, {} KB",
            t_doc,
            doc.body.len(),
            t_sub,
            t0.elapsed(),
            bytes / 1024
        );
    }
    println!("{}", fetcher.stats());
}
