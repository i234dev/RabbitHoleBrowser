//! Keep the built-in status page synchronized with committed measurements.
//! Only the small rendered table is embedded in the browser, not the baselines.
use std::{env, fmt::Write, fs, path::Path};

#[derive(Clone, Copy)]
struct Counts {
    passed: u64,
    total: u64,
    pages: u64,
}

fn number(value: &str) -> u64 {
    value
        .parse()
        .expect("invalid count in home-page measurement source")
}

fn read(root: &Path, relative: &str) -> String {
    let path = root.join(relative);
    println!("cargo:rerun-if-changed={}", path.display());
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {relative}: {e}"))
}

fn escaped(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut result = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    result
}

fn main() {
    let manifest = env::var_os("CARGO_MANIFEST_DIR").unwrap();
    let root = Path::new(&manifest).join("../..");
    let js = read(&root, "crates/js/tests/test262_baseline.txt");
    let fields: Vec<_> = js
        .lines()
        .find(|line| line.starts_with("total "))
        .expect("test262 baseline has no total")
        .split_whitespace()
        .collect();
    let test262 = Counts {
        passed: number(fields[1]),
        total: number(fields[1]) + number(fields[2]),
        pages: 0,
    };
    let test262_skipped = number(fields[3]);
    let css = read(&root, "crates/engine/tests/wpt-css-baseline.txt");
    let mut reftests = Counts {
        passed: 0,
        total: 0,
        pages: 0,
    };
    for line in css.lines().filter(|line| line.starts_with("dir ")) {
        let fields: Vec<_> = line.split_whitespace().collect();
        reftests.passed += number(fields[2]);
        reftests.total += number(fields[3]);
    }
    let summary = read(&root, "tools/wpt/summary.txt");
    let mut wpt = None;
    let mut dom = Counts {
        passed: 0,
        total: 0,
        pages: 0,
    };
    for line in summary.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        let Some(&name) = fields.first() else {
            continue;
        };
        if name == "TOTAL" || name == "dom" || name.starts_with("dom/") {
            let counts = Counts {
                pages: number(fields[1]),
                passed: number(fields[2]),
                total: number(fields[3]),
            };
            if name == "TOTAL" {
                wpt = Some(counts);
            } else {
                dom.pages += counts.pages;
                dom.passed += counts.passed;
                dom.total += counts.total;
            }
        }
    }
    let wpt = wpt.expect("WPT summary has no TOTAL");
    let measurements = read(&root, "tools/metrics/measurements.tsv");
    let mut html = String::from("<table class=\"measurements\"><thead><tr><th>Suite</th><th>Latest result</th><th>Measured</th><th>Scope</th></tr></thead><tbody>\n");
    for line in measurements
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
    {
        let fields: Vec<_> = line.split('\t').collect();
        assert_eq!(
            fields.len(),
            7,
            "measurement records need seven tab-separated fields"
        );
        let (id, label, recorded, date, scope, source, precision) = (
            fields[0], fields[1], fields[2], fields[3], fields[4], fields[5], fields[6],
        );
        let computed = match id {
            "test262" => Some(test262),
            "css" => Some(reftests),
            "wpt" => Some(wpt),
            "dom" => Some(dom),
            _ => None,
        };
        let (result, measured, scope) = if let Some(counts) = computed {
            assert!(
                counts.total > 0 && counts.passed <= counts.total,
                "invalid counts for {id}"
            );
            let percent = counts.passed as f64 * 100. / counts.total as f64;
            let result = format!(
                "{} / {} ({percent:.1}%)",
                grouped(counts.passed),
                grouped(counts.total)
            );
            // A new baseline updates the result automatically. Until its date
            // is recorded, do not assign the old run's date to the new counts.
            let stamp = format!("{}/{}", counts.passed, counts.total);
            let measured = if stamp == recorded {
                date
            } else {
                "Date not recorded"
            };
            let scope = if id == "test262" {
                format!("{scope}; {} skipped", grouped(test262_skipped))
            } else if counts.pages > 0 {
                format!("{scope}; {} pages", grouped(counts.pages))
            } else {
                scope.into()
            };
            (result, measured, scope)
        } else {
            assert_eq!(precision, "recorded", "unknown metric source {id}");
            (recorded.to_string(), date, scope.to_string())
        };
        writeln!(html, "<tr id=\"metric-{}\"><th scope=\"row\">{}</th><td class=\"result\">{}</td><td>{}</td><td>{}<span class=\"source\">Source: {}</span></td></tr>", escaped(id), escaped(label), escaped(&result), escaped(measured), escaped(&scope), escaped(source)).unwrap();
    }
    html.push_str("</tbody></table>\n");
    let output = env::var_os("OUT_DIR").unwrap();
    fs::write(Path::new(&output).join("home-metrics.html"), html)
        .expect("cannot write home metrics");
}
