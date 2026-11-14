//! Canvas retains CSS font family order and literal quoted names.
use crate::canvas2d::resolve_font;
use rabbithole_gfx::{Family, FontContext};

const SANS: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSans.ttf");
const MONO: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSansMono.ttf");

#[test]
fn canvas_named_families_are_distinct_from_generics_and_keep_order() {
    let base = FontContext::new();
    let a = base.document_scope();
    let b = base.document_scope();
    let alias_a = a.add_font_data("Scoped", 400, false, SANS).unwrap();
    let alias_b = b.add_font_data("Scoped", 400, false, MONO).unwrap();
    assert_eq!(resolve_font(&a, "20px Scoped"), Some((20.0, alias_a)));
    assert_eq!(resolve_font(&b, "20px Scoped"), Some((20.0, alias_b)));
    assert_ne!(a.measure(alias_a, 20.0, "WWWiii"), b.measure(alias_b, 20.0, "WWWiii"));
    assert_eq!(resolve_font(&b, "20px Missing, Scoped, serif"), Some((20.0, alias_b)));
    let serif = b.select(Family::Serif, 400, false);
    assert_eq!(resolve_font(&b, "20px Missing, serif, Scoped"), Some((20.0, serif)));
    let named_serif = b.add_font_data("serif", 400, false, MONO).unwrap();
    assert_eq!(resolve_font(&b, "20px 'serif'"), Some((20.0, named_serif)));
    assert_eq!(resolve_font(&b, "20px serif"), Some((20.0, serif)));
    assert_eq!(resolve_font(&a, "20px Scoped"), Some((20.0, alias_a)));
    let rounded = b.add_font_data("ui-rounded", 400, false, MONO).unwrap();
    assert_eq!(resolve_font(&b, "20px 'ui-rounded'"), Some((20.0, rounded)));
    assert_eq!(resolve_font(&b, "20px ui-rounded"), Some((20.0, b.select(Family::Sans, 400, false))));
    // A downloaded name cannot hijack a host generic's platform candidates.
    let arial = b.add_font_data("Arial", 400, false, MONO).unwrap();
    let consolas = b.add_font_data("Consolas", 400, false, SANS).unwrap();
    assert_eq!(resolve_font(&b, "20px Arial"), Some((20.0, arial)));
    assert_eq!(resolve_font(&b, "20px Consolas"), Some((20.0, consolas)));
    assert_eq!(resolve_font(&b, "20px sans-serif"), Some((20.0, b.select(Family::Sans, 400, false))));
    assert_eq!(resolve_font(&b, "20px monospace"), Some((20.0, b.select(Family::Mono, 400, false))));
}

#[test]
fn canvas_font_uses_css_tokenization_for_quotes_escapes_and_shorthand() {
    let fonts = FontContext::new();
    let quoted = fonts.add_font_data("A, B Family", 700, true, MONO).unwrap();
    assert_eq!(resolve_font(&fonts, r#"italic 700 15pt/1.2 "A, B Family", sans-serif"#), Some((20.0, quoted)));
    let escaped = fonts.add_font_data("Quoted Family", 400, false, SANS).unwrap();
    assert_eq!(resolve_font(&fonts, r#"20px "Quoted\20 Family""#), Some((20.0, escaped)));
    assert_eq!(resolve_font(&fonts, "1.25em 'Quoted Family'"), Some((20.0, escaped)));
    assert_eq!(resolve_font(&fonts, "1.25rem 'Quoted Family'"), Some((20.0, escaped)));
    assert_eq!(resolve_font(&fonts, "20px monospace"), Some((20.0, fonts.select(Family::Mono, 400, false))));
    for invalid in ["20px", "20px Arial,", "20px ,Arial", "20px 'two' names", "-1px Arial", "calc(-1px) Arial", "20px/-1 Arial", "20px/calc(-1px) Arial", "20bogus Arial", "20px/garbage Arial", "20px Arial; color:red"] {
        assert!(resolve_font(&fonts, invalid).is_none(), "invalid font accepted: {invalid}");
    }
}

#[test]
fn canvas_font_setter_preserves_previous_value_for_invalid_shorthands() {
    let (mut env, mut host) = crate::tests::setup("<canvas id=c></canvas>");
    let out = crate::tests::run(&mut env, &mut host, r#"
        const context = document.getElementById('c').getContext('2d');
        context.font = 'italic 700 20px "Quoted Family", monospace';
        const saved = context.font;
        const width = context.measureText('WWWiii').width;
        for (const invalid of ['20px', '20px Arial,', '20px/garbage Arial', '20px/calc(-1px) Arial', '-1px Arial', '20px Arial; color:red']) {
            context.font = invalid;
            if (context.font !== saved || context.measureText('WWWiii').width !== width) throw new Error('invalid font changed Canvas state');
        }
        console.log(context.font === saved, width > 0);
    "#);
    assert_eq!(out, ["true true"]);
}
