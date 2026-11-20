//! Vertical font metrics: which tables they come from, the `@font-face`
//! metric overrides and the used `line-height: normal`.
use crate::font::{normal_line_height, Family, FontContext, LineMetrics};
use crate::MetricOverrides;

const SANS: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSans.ttf");

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

/// DejaVu Sans (2048 units per em) with the OS/2 `USE_TYPO_METRICS` bit set.
fn sans_with_typo_metrics() -> Vec<u8> {
    let mut data = SANS.to_vec();
    let n = u16::from_be_bytes([data[4], data[5]]) as usize;
    let os2 = (0..n)
        .map(|i| 12 + 16 * i)
        .find(|&r| &data[r..r + 4] == b"OS/2")
        .map(|r| u32::from_be_bytes([data[r + 8], data[r + 9], data[r + 10], data[r + 11]]) as usize)
        .expect("OS/2 table");
    // fsSelection, bit 7.
    data[os2 + 63] |= 0x80;
    data
}

/// Without `USE_TYPO_METRICS` the `hhea` metrics apply (DejaVu Sans:
/// ascender 1901, descender -483, no line gap), as in Chrome on Linux.
#[test]
fn hhea_metrics_without_use_typo_metrics() {
    let fc = FontContext::new();
    let font = fc.select(Family::Sans, 400, false);
    let m = fc.metrics(font, 2048.0);
    assert!(approx(m.ascent, 1901.0), "{m:?}");
    assert!(approx(m.descent, 483.0), "{m:?}");
    assert!(approx(m.line_gap, 0.0), "{m:?}");
}

/// With `USE_TYPO_METRICS` the OS/2 typographic metrics apply (1556,
/// -492, line gap 410).
#[test]
fn typo_metrics_with_use_typo_metrics() {
    let fc = FontContext::new();
    let font = fc.add_font_data("Typo Sans", 400, false, &sans_with_typo_metrics()).unwrap();
    let m = fc.metrics(font, 2048.0);
    assert!(approx(m.ascent, 1556.0), "{m:?}");
    assert!(approx(m.descent, 492.0), "{m:?}");
    assert!(approx(m.line_gap, 410.0), "{m:?}");
    assert_eq!(fc.normal_line_height(font, 2048.0), 1556.0 + 492.0 + 410.0);
}

/// `line-height: normal` is Blink's line spacing: the rounded ascent,
/// descent and line gap (DejaVu Sans at 16px: 14.85 + 3.77 → 15 + 4).
#[test]
fn normal_line_height_rounds_each_metric() {
    let fc = FontContext::new();
    let font = fc.select(Family::Sans, 400, false);
    assert_eq!(fc.normal_line_height(font, 16.0), 19.0);
    assert_eq!(fc.normal_line_height(font, 13.0), 15.0);
    assert_eq!(fc.normal_line_height(font, 100.0), 93.0 + 24.0);
    // Ahem-like metrics (0.8 / 0.2 em) keep the font size at sizes that
    // give whole pixels, and round each part otherwise.
    let ahem = |s: f32| normal_line_height(LineMetrics { ascent: 0.8 * s, descent: 0.2 * s, line_gap: 0.0 });
    assert_eq!(ahem(20.0), 20.0);
    assert_eq!(ahem(50.0), 50.0);
    assert_eq!(ahem(16.0), 16.0);
    assert_eq!(normal_line_height(LineMetrics { ascent: 10.4, descent: 2.4, line_gap: 1.5 }), 10.0 + 2.0 + 2.0);
}

/// The `@font-face` metric overrides replace the font's metrics, as
/// fractions of the font size; faces that differ in them are distinct even
/// with the same data and descriptors.
#[test]
fn metric_overrides_replace_font_metrics() {
    let fc = FontContext::new();
    let plain = fc.add_font_data("Override Sans", 400, false, SANS).unwrap();
    let overrides = MetricOverrides { ascent: Some(1.0), descent: Some(0.5), line_gap: None };
    let doc = fc.document_scope();
    let ids = doc.add_font_face_with_metrics("Override Sans", (400, 400), false, (100, 100), None, overrides, SANS);
    assert_eq!(ids.len(), 1);
    let overridden = ids[0];
    assert_ne!(overridden, plain);
    let m = doc.metrics(overridden, 20.0);
    assert_eq!((m.ascent, m.descent, m.line_gap), (20.0, 10.0, 0.0));
    assert_eq!(doc.normal_line_height(overridden, 20.0), 30.0);
    // The face registered without overrides keeps the font's metrics.
    assert!(approx(doc.metrics(plain, 2048.0).ascent, 1901.0));
    // A line gap override alone.
    let gap = MetricOverrides { line_gap: Some(1.0), ..Default::default() };
    let g = doc.add_font_face_with_metrics("Gap Sans", (400, 400), false, (100, 100), None, gap, SANS)[0];
    assert_eq!(doc.normal_line_height(g, 20.0), 19.0 + 5.0 + 20.0);
    // Feature variants of the face keep its overrides.
    let liga = doc.with_features(overridden, &[(*b"liga", 0)]);
    assert_eq!(doc.metrics(liga, 20.0).ascent, 20.0);
}

/// The style system's `line-height: normal` reads the first available
/// font of the family list (with its overrides).
#[test]
fn unit_line_metrics_use_first_available_font() {
    let fc = FontContext::new();
    let overrides = MetricOverrides { ascent: Some(0.25), descent: Some(0.125), line_gap: Some(0.125) };
    fc.add_font_face_with_metrics("Small Lines", (400, 400), false, (100, 100), None, overrides, SANS);
    let m = fc.unit_line_metrics(&["Missing".into(), "Small Lines".into()], Family::Sans, 400, false);
    assert_eq!((m.ascent, m.descent, m.line_gap), (0.25, 0.125, 0.125));
    let generic = fc.unit_line_metrics(&[], Family::Sans, 400, false);
    assert!(approx(generic.ascent * 2048.0, 1901.0));
}
