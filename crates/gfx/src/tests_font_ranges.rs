//! `unicode-range` subsets of one face form a composite font, and face
//! matching follows the CSS Fonts 4 weight rules.
use crate::font::{Family, FontContext, FontId};

const SANS: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSans.ttf");
const SANS_BOLD: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSans-Bold.ttf");
const MONO: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSansMono.ttf");
const SERIF: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSerif.ttf");

const LATIN: &[(u32, u32)] = &[(0x0000, 0x00FF), (0x2000, 0x206F)];
const CYRILLIC: &[(u32, u32)] = &[(0x0400, 0x045F)];
const LATIN_EXT: &[(u32, u32)] = &[(0x0100, 0x024F)];

fn fonts_of(fc: &FontContext, font: FontId, text: &str) -> Vec<FontId> {
    fc.shape(font, 20.0, text, 0.0).iter().map(|g| g.font).collect()
}

/// Font services list a family's subsets with the Latin one last; every
/// subset of the requested weight is used for the characters of its range,
/// even when another subset's file also maps them.
#[test]
fn unicode_range_subsets_form_one_composite_face() {
    let fc = FontContext::new();
    let regular = fc.add_font_data_ranged("Subset Web", 400, false, (100, 100), Some(LATIN), SERIF).unwrap();
    let cyrillic = fc.add_font_data_ranged("Subset Web", 700, false, (100, 100), Some(CYRILLIC), MONO).unwrap();
    let ext = fc.add_font_data_ranged("Subset Web", 700, false, (100, 100), Some(LATIN_EXT), SANS).unwrap();
    let latin = fc.add_font_data_ranged("Subset Web", 700, false, (100, 100), Some(LATIN), SANS_BOLD).unwrap();
    assert_ne!(cyrillic, ext);
    assert_ne!(ext, latin);

    // The first available face is the subset covering U+0020.
    let primary = fc.select_family(&["Subset Web".into()], Family::Sans, 700, false);
    assert_eq!(primary, latin);
    // Mono maps Latin letters too, but its range excludes them.
    assert_eq!(fonts_of(&fc, cyrillic, "WiЖ"), vec![latin, latin, cyrillic]);
    assert_eq!(fonts_of(&fc, primary, "Wi Жŝ"), vec![latin, latin, latin, cyrillic, ext]);
    // The regular weight keeps its own subset.
    assert_eq!(fc.select_family(&["Subset Web".into()], Family::Sans, 400, false), regular);
    // Bold text is not synthesized over a real bold subset.
    assert!(!fc.needs_synthetic_bold(primary, 700));
    // Shaped text measures as the bold Latin face.
    let direct = FontContext::new();
    let bold = direct.add_font_data("Whole", 700, false, SANS_BOLD).unwrap();
    assert_eq!(fc.measure(primary, 20.0, "ITALIA"), direct.measure(bold, 20.0, "ITALIA"));
}

/// A character outside every subset of the matched style uses the closest
/// style of the family before other families.
#[test]
fn subset_fallback_prefers_the_closest_style() {
    let fc = FontContext::new();
    fc.add_font_data_ranged("Styles", 400, false, (100, 100), Some(CYRILLIC), SERIF).unwrap();
    let bold_cyr = fc.add_font_data_ranged("Styles", 800, false, (100, 100), Some(CYRILLIC), MONO).unwrap();
    let latin = fc.add_font_data_ranged("Styles", 700, false, (100, 100), Some(LATIN), SANS_BOLD).unwrap();
    let primary = fc.select_family(&["Styles".into()], Family::Sans, 700, false);
    assert_eq!(primary, latin);
    assert_eq!(fonts_of(&fc, primary, "AЖ"), vec![latin, bold_cyr]);
}

/// Same bytes, same descriptors and a different range are distinct faces;
/// the same range is the same face.
#[test]
fn ranges_are_part_of_face_identity() {
    let fc = FontContext::new();
    let a = fc.add_font_data_ranged("Ident", 400, false, (100, 100), Some(LATIN), SANS).unwrap();
    let b = fc.add_font_data_ranged("Ident", 400, false, (100, 100), Some(CYRILLIC), SANS).unwrap();
    assert_ne!(a, b);
    assert_eq!(fc.add_font_data_ranged("Ident", 400, false, (100, 100), Some(LATIN), SANS), Some(a));
    // A full range is no restriction.
    let full = fc.add_font_data_ranged("Ident", 400, false, (100, 100), Some(&[(0, 0x10FFFF)]), MONO).unwrap();
    assert_eq!(fc.add_font_data("Ident", 400, false, MONO), Some(full));
}

/// CSS Fonts 4 §5.2: a desired weight from 400 to 500 tries heavier
/// weights up to 500 first, then lighter ones, then heavier than 500.
#[test]
fn weight_matching_between_400_and_500() {
    let fc = FontContext::new();
    let light = fc.add_font_data("Weights", 300, false, SANS).unwrap();
    let medium = fc.add_font_data("Weights", 500, false, MONO).unwrap();
    let bold = fc.add_font_data("Weights", 700, false, SANS_BOLD).unwrap();
    let pick = |w| fc.select_family(&["Weights".into()], Family::Sans, w, false);
    assert_eq!(pick(400), medium);
    assert_eq!(pick(450), medium);
    assert_eq!(pick(500), medium);
    assert_eq!(pick(550), bold);
    assert_eq!(pick(350), light);
    assert_eq!(pick(200), light);
    assert_eq!(pick(900), bold);
    let only_light_heavy = FontContext::new();
    let l = only_light_heavy.add_font_data("W2", 300, false, SANS).unwrap();
    only_light_heavy.add_font_data("W2", 600, false, SANS_BOLD).unwrap();
    assert_eq!(only_light_heavy.select_family(&["W2".into()], Family::Sans, 400, false), l);
}
