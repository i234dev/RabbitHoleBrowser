//! The shipped home page reports its build version and committed measurements.
use rabbithole_dom::Document;
use rabbithole_engine::{Engine, Viewport};

fn row(page: &rabbithole_engine::Page, name: &str) -> String {
    let id = page
        .doc
        .element_by_id(name)
        .unwrap_or_else(|| panic!("missing home-page row {name}"));
    page.doc.text_content(id)
}

fn grouped(value: u64) -> String {
    let digits = value.to_string();
    digits
        .chars()
        .enumerate()
        .fold(String::new(), |mut out, (i, ch)| {
            if i > 0 && (digits.len() - i) % 3 == 0 {
                out.push(',');
            }
            out.push(ch);
            out
        })
}

#[test]
fn home_renders_real_version_and_current_baseline_results() {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.scripting = false;
    engine.loader.load_images = false;
    let page = engine
        .load(
            "about:home",
            Viewport {
                width: 1200.,
                height: 1100.,
            },
        )
        .unwrap();
    assert!(row(&page, "build-version").starts_with(&format!("v{} ", env!("CARGO_PKG_VERSION"))));

    let js = include_str!("../../js/tests/test262_baseline.txt");
    let totals: Vec<_> = js
        .lines()
        .find(|line| line.starts_with("total "))
        .unwrap()
        .split_whitespace()
        .collect();
    let passed: u64 = totals[1].parse().unwrap();
    let failed: u64 = totals[2].parse().unwrap();
    assert!(row(&page, "metric-test262").contains(&format!(
        "{} / {}",
        grouped(passed),
        grouped(passed + failed)
    )));

    let css = include_str!("../tests/wpt-css-baseline.txt");
    let (mut passed, mut total) = (0u64, 0u64);
    for line in css.lines().filter(|line| line.starts_with("dir ")) {
        let fields: Vec<_> = line.split_whitespace().collect();
        passed += fields[2].parse::<u64>().unwrap();
        total += fields[3].parse::<u64>().unwrap();
    }
    assert!(row(&page, "metric-css").contains(&format!(
        "{} / {}",
        grouped(passed),
        grouped(total)
    )));

    let wpt = include_str!("../../../tools/wpt/summary.txt");
    let totals: Vec<_> = wpt
        .lines()
        .find(|line| line.starts_with("TOTAL "))
        .unwrap()
        .split_whitespace()
        .collect();
    assert!(row(&page, "metric-wpt").contains(&format!(
        "{} / {}",
        grouped(totals[2].parse().unwrap()),
        grouped(totals[3].parse().unwrap())
    )));
    let (mut passed, mut total) = (0u64, 0u64);
    for line in wpt.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields
            .first()
            .is_some_and(|name| *name == "dom" || name.starts_with("dom/"))
        {
            passed += fields[2].parse::<u64>().unwrap();
            total += fields[3].parse::<u64>().unwrap();
        }
    }
    let dom = row(&page, "metric-dom");
    assert!(dom.contains("DOM / WPT dom/*"));
    assert!(dom.contains(&format!("{} / {}", grouped(passed), grouped(total))));
    // Recorded results must follow the supplied value, date and conditions
    // when a new measurement replaces a historical one.
    for line in include_str!("../../../tools/metrics/measurements.tsv").lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let fields: Vec<_> = line.split('\t').collect();
        assert_eq!(fields.len(), 7, "invalid measurement record: {line}");
        if fields[6] != "recorded" {
            continue;
        }
        let shown = row(&page, &format!("metric-{}", fields[0]));
        for field in &fields[1..6] {
            assert!(!field.is_empty(), "empty field in measurement {}", fields[0]);
            assert!(shown.contains(field), "measurement {} is missing {field:?}: {shown}", fields[0]);
        }
    }
    let frame = engine.snapshot(&page);
    assert_eq!((frame.width, frame.height), (1200, 1100));
    assert!(!frame.rgba.is_empty());
    assert!(page
        .doc
        .text_content(Document::ROOT)
        .contains("Compatibility and performance"));
    if let Ok(path) = std::env::var("RH_HOME_METRICS_SCREENSHOT") {
        frame.save_png(std::path::Path::new(&path)).unwrap();
    }
}
