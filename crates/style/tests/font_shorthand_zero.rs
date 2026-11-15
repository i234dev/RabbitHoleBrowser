//! CSS Fonts 4 §2.5/2.7 and CSS Values 4 §6: a zero length may omit its unit.
//! Zero is not a valid font weight; after `/`, zero is a line-height number.
use rabbithole_css::{parse_stylesheet, MediaEnv, Tokenizer};
use rabbithole_style::cascade::{resolve_font_shorthand, FontFamilyName, ResolvedFontShorthand};
use rabbithole_style::values::LengthContext;
use rabbithole_style::{style_tree, ComputedStyle, Family, FontStyle};

fn computed(declarations: &str) -> ComputedStyle {
    let doc = rabbithole_html::parse("<div id=target>Text</div>");
    let sheet = parse_stylesheet(&format!("#target{{{declarations}}}"));
    let tree = style_tree(&doc, &[sheet], &MediaEnv {
        width: 800.0, height: 600.0, screen: true, scripting: true,
    });
    tree.get(doc.element_by_id("target").unwrap()).unwrap().as_ref().clone()
}

fn resolved(value: &str) -> Option<ResolvedFontShorthand> {
    resolve_font_shorthand(&Tokenizer::tokenize(value), &ComputedStyle::initial(), &LengthContext {
        font_size: 16.0, root_font_size: 16.0, viewport_w: 800.0, viewport_h: 600.0,
        ch: 0.5, ex: 0.5, ic: 1.0,
    })
}

#[test]
fn font_shorthand_accepts_unitless_zero_sizes() {
    for value in ["0 monospace", "0/0 monospace", "0 / 0 monospace", "+0/0 monospace", "-0/0 monospace", "0.0/0 monospace", "0e0/0 monospace"] {
        let style = computed(&format!("font:italic bold 19px/2 serif;font:{value}"));
        assert_eq!(style.font_size, 0.0, "{value}");
        assert_eq!(style.font_weight, 400, "{value}");
        assert_eq!(style.font_style, FontStyle::Normal, "{value}");
        assert_eq!(style.font_family, Family::Mono, "{value}");
        let font = resolved(value).unwrap_or_else(|| panic!("valid zero size rejected: {value}"));
        assert_eq!(font.size, 0.0, "{value}");
        assert_eq!(font.weight, 400, "{value}");
        assert_eq!(font.families, [FontFamilyName::Generic("monospace".into())], "{value}");
    }
}

#[test]
fn font_shorthand_zero_is_a_size_while_nonzero_prefixes_remain_weights() {
    for weight in [1, 400, 700, 1000] {
        let value = format!("{weight} 0/0 monospace");
        let style = computed(&format!("font:{value}"));
        assert_eq!((style.font_size, style.font_weight), (0.0, weight), "{value}");
        let font = resolved(&value).unwrap();
        assert_eq!((font.size, font.weight), (0.0, weight), "{value}");
    }
    let style = computed("font:italic 700 0/normal monospace");
    assert_eq!((style.font_size, style.font_weight, style.font_style), (0.0, 700, FontStyle::Italic));
    assert_eq!((style.line_height, style.line_height_number), (None, None));
    // Once the zero supplied the mandatory size, `bold` is a family name.
    let style = computed("font:0 bold");
    assert_eq!((style.font_size, style.font_weight), (0.0, 400));
    assert_eq!(style.font_family_list, ["bold"]);
    let style = computed("font:normal normal normal normal 0/0 monospace");
    assert_eq!((style.font_size, style.font_weight, style.font_style), (0.0, 400, FontStyle::Normal));
}

#[test]
fn font_shorthand_line_height_zero_stays_a_number_and_inherits_as_a_multiplier() {
    for (value, expected_size, expected_height, expected_number) in [
        ("0/0 monospace", 0.0, 0.0, Some(0.0)),
        ("400 16px/0 monospace", 16.0, 0.0, Some(0.0)),
        ("0/1.25 monospace", 0.0, 0.0, Some(1.25)),
        ("0/20px monospace", 0.0, 20.0, None),
    ] {
        let style = computed(&format!("font:{value}"));
        assert_eq!(style.font_size, expected_size, "{value}");
        assert_eq!(style.line_height, Some(expected_height), "{value}");
        assert_eq!(style.line_height_number, expected_number, "{value}");
    }
    let doc = rabbithole_html::parse("<div><span id=child>Text</span></div>");
    let sheet = parse_stylesheet("div{font:0/1.25 monospace}span{font-size:20px}");
    let tree = style_tree(&doc, &[sheet], &MediaEnv {
        width: 800.0, height: 600.0, screen: true, scripting: true,
    });
    let child = tree.get(doc.element_by_id("child").unwrap()).unwrap();
    assert_eq!((child.font_size, child.line_height, child.line_height_number), (20.0, Some(25.0), Some(1.25)));
}

fn assert_rejected_without_changing_previous(value: &str) {
    let style = computed(&format!("font:italic 600 18px/2 serif;font:{value}"));
    assert_eq!((style.font_size, style.font_weight, style.font_style), (18.0, 600, FontStyle::Italic), "{value}");
    assert_eq!((style.line_height, style.line_height_number, style.font_family), (Some(36.0), Some(2.0), Family::Serif), "{value}");
    assert!(resolved(value).is_none(), "invalid shorthand accepted by resolver: {value}");
}

#[test]
fn font_shorthand_nonzero_unitless_sizes_and_out_of_range_weights_still_reject() {
    for value in ["1/0 monospace", "12/0 monospace", "12 monospace", "-1/0 monospace", "1001 0/0 monospace", "-1 0/0 monospace", "0.5 0/0 monospace"] {
        assert_rejected_without_changing_previous(value);
    }
}

#[test]
fn font_shorthand_zero_rejects_missing_families_and_malformed_tails_atomically() {
    for value in [
        "0", "0/0", "0/", "0 16px serif", "0 0 monospace", "0 700 serif",
        "0/-1 monospace", "0/-1px monospace", "0/auto monospace", "0/foo monospace",
        "0 initial", "0 serif extra", "0 , monospace", "0 monospace,", "0 monospace,,serif",
        "italic italic 0 monospace", "bold 700 0 monospace", "700 600 0 monospace",
        "small-caps small-caps 0 monospace", "normal normal normal normal normal 0 monospace",
        "0 generic(unknown),serif", "0 generic(kai kai),serif", "0 generic(kai),",
    ] {
        assert_rejected_without_changing_previous(value);
    }
}

#[test]
fn font_shorthand_existing_dimensions_and_quoted_family_syntax_are_preserved() {
    let font = resolved("italic 700 12px/1.5 'serif', monospace").unwrap();
    assert_eq!((font.size, font.weight, font.italic), (12.0, 700, true));
    assert_eq!(font.families, [FontFamilyName::Named("serif".into()), FontFamilyName::Generic("monospace".into())]);
    let font = resolved("0 'serif', monospace").unwrap();
    assert_eq!(font.size, 0.0);
    assert_eq!(font.families, [FontFamilyName::Named("serif".into()), FontFamilyName::Generic("monospace".into())]);
    // The CSS <string> production permits an empty quoted name. Canvas's
    // existing additional empty-name restriction is outside this change.
    assert_eq!(computed("font:18px serif;font:0 ''").font_size, 0.0);
    assert!(resolved("0 ''").is_none());
    // Functional generics are valid document CSS. Preserve the previous
    // size and serif fallback behavior without claiming their selection is
    // implemented, or expanding the separate Canvas selection subset.
    for family in ["generic(fangsong)", "generic(kai)", "generic(khmer-mul)", "generic(nastaliq)", "GENERIC( KAI )"] {
        let style = computed(&format!("font:31px monospace;font:18px {family},serif"));
        assert_eq!((style.font_size, style.font_family), (18.0, Family::Serif), "{family}");
        let style = computed(&format!("font:31px monospace;font:0 {family},serif"));
        assert_eq!((style.font_size, style.font_family), (0.0, Family::Serif), "{family}");
        assert!(resolved(&format!("18px {family},serif")).is_none(), "{family}");
        assert!(resolved(&format!("0 {family},serif")).is_none(), "{family}");
    }
}
