use super::*;
use crate::Tokenizer;

fn parse(s: &str) -> Option<ClipPathValue> {
    parse_clip_path(&Tokenizer::tokenize(s))
}

fn parsed_shape(s: &str) -> BasicShape {
    match parse(s) {
        Some(ClipPathValue::Shape(shape)) => shape,
        other => panic!("expected shape for {s:?}, got {other:?}"),
    }
}

#[test]
fn defaults_and_css_wide_values_are_explicit() {
    assert_eq!(parse("none"), Some(ClipPathValue::None));
    for (text, wide) in [("inherit", CssWide::Inherit), ("initial", CssWide::Initial), ("unset", CssWide::Unset), ("revert", CssWide::Revert), ("revert-layer", CssWide::RevertLayer)] {
        assert_eq!(parse(text), Some(ClipPathValue::Wide(wide)));
    }
    assert_eq!(parsed_shape("circle()"), BasicShape::Circle { radius: ShapeRadius::ClosestSide, center: ShapePosition::CENTER });
    assert_eq!(parsed_shape("ellipse()"), BasicShape::Ellipse { x_radius: ShapeRadius::ClosestSide, y_radius: ShapeRadius::ClosestSide, center: ShapePosition::CENTER });
}

#[test]
fn reference_box_order_case_and_escaped_tokens_share_one_value() {
    let expected = parse("circle(25% at 10px 20%)");
    assert!(expected.is_some());
    assert_eq!(parse("border-box circle(25% at 10px 20%)"), expected);
    assert_eq!(parse("circle(25% at 10px 20%) border-box"), expected);
    assert_eq!(parse("BORDER-BOX CIRCLE(25% AT 10PX 20%)"), expected);
    assert_eq!(parse(r"\63 ircle(25% at 10px 20%)"), expected);
    assert_eq!(parse(" /* shape */ circle(25% at 10px 20%) /* end */ "), expected);
}

#[test]
fn lengths_and_percentage_radii_remain_unresolved() {
    assert_eq!(parsed_shape("circle(75% at -10px 120%)"), BasicShape::Circle {
        radius: ShapeRadius::Length(ShapeLength::Percent(75.0)),
        center: ShapePosition { x: ShapeLength::Px(-10.0), y: ShapeLength::Percent(120.0) },
    });
    assert_eq!(parsed_shape("ellipse(25% 7px at left bottom)"), BasicShape::Ellipse {
        x_radius: ShapeRadius::Length(ShapeLength::Percent(25.0)), y_radius: ShapeRadius::Length(ShapeLength::Px(7.0)),
        center: ShapePosition { x: ShapeLength::Percent(0.0), y: ShapeLength::Percent(100.0) },
    });
    assert_eq!(parsed_shape("ellipse(closest-side farthest-side)"), BasicShape::Ellipse {
        x_radius: ShapeRadius::ClosestSide, y_radius: ShapeRadius::FarthestSide, center: ShapePosition::CENTER,
    });
    assert_eq!(parse("circle(-0)"), parse("circle(0px)"));
}

#[test]
fn positions_validate_axes_without_reordering_numeric_components() {
    for (text, x, y) in [
        ("top", 50.0, 0.0), ("bottom", 50.0, 100.0), ("right", 100.0, 50.0),
        ("center", 50.0, 50.0), ("left top", 0.0, 0.0), ("top left", 0.0, 0.0),
        ("center right", 100.0, 50.0), ("bottom center", 50.0, 100.0),
    ] {
        let BasicShape::Circle { center, .. } = parsed_shape(&format!("circle(at {text})")) else { panic!() };
        assert_eq!(center, ShapePosition { x: ShapeLength::Percent(x), y: ShapeLength::Percent(y) }, "{text}");
    }
    let BasicShape::Circle { center, .. } = parsed_shape("circle(at right 7px)") else { panic!() };
    assert_eq!(center, ShapePosition { x: ShapeLength::Percent(100.0), y: ShapeLength::Px(7.0) });
    for value in ["circle(at top 7px)", "circle(at 7px left)", "circle(at top bottom)", "circle(at left right)", "circle(at left 2px top 3px)"] {
        assert_eq!(parse(value), None, "{value}");
    }
}

#[test]
fn inset_expands_signed_offsets_and_independent_corner_axes() {
    let BasicShape::Inset { offsets, corners } = parsed_shape("inset(-1px 20% 3px round 2px 4% / 6% 8px 10px)") else { panic!() };
    assert_eq!(offsets, [ShapeLength::Px(-1.0), ShapeLength::Percent(20.0), ShapeLength::Px(3.0), ShapeLength::Percent(20.0)]);
    assert_eq!(corners, [
        (ShapeLength::Px(2.0), ShapeLength::Percent(6.0)),
        (ShapeLength::Percent(4.0), ShapeLength::Px(8.0)),
        (ShapeLength::Px(2.0), ShapeLength::Px(10.0)),
        (ShapeLength::Percent(4.0), ShapeLength::Px(8.0)),
    ]);
    for (text, expected) in [("1px", [1.0, 1.0, 1.0, 1.0]), ("1px 2px", [1.0, 2.0, 1.0, 2.0]), ("1px 2px 3px", [1.0, 2.0, 3.0, 2.0]), ("1px 2px 3px 4px", [1.0, 2.0, 3.0, 4.0])] {
        let BasicShape::Inset { offsets, corners } = parsed_shape(&format!("inset({text})")) else { panic!() };
        assert_eq!(offsets, expected.map(ShapeLength::Px));
        assert_eq!(corners, [(ShapeLength::Px(0.0), ShapeLength::Px(0.0)); 4]);
    }
    let BasicShape::Inset { corners, .. } = parsed_shape("inset(0 round 5%)") else { panic!() };
    assert_eq!(corners, [(ShapeLength::Percent(5.0), ShapeLength::Percent(5.0)); 4]);
    // The longest accepted grammar must fit the parser's derived token bound.
    assert!(parse("border-box inset(1px 2px 3px 4px round 5px 6px 7px 8px / 9px 10px 11px 12px)").is_some());
}

#[test]
fn unsupported_and_malformed_values_never_accept_a_prefix() {
    for value in [
        "", "auto", "border-box", "circle", "circle(", "circle())", "circle() junk", "circle() circle()",
        "border-box circle() border-box", "circle() border-box border-box", "padding-box circle()", "circle() content-box",
        "none border-box", "inherit circle()", "initial border-box", "url(#clip)", "polygon(0 0, 100% 0, 0 100%)", "path('M0 0')",
        "circle(-1px)", "circle(-1%)", "circle(2)", "circle(1px 2px)", "circle(closest-corner)",
        "circle(1em)", "circle(1vw)", "circle(calc(50% - 1px))", "circle(10px, at center)",
        "circle(at)", "circle(at center at center)", "ellipse(1px)", "ellipse(1px 2px 3px)", "ellipse(1px -1%)",
        "inset()", "inset(1px 2px 3px 4px 5px)", "inset(0 at center)", "inset(0 round)",
        "inset(0 round -1px)", "inset(0 round 1px /)", "inset(0 round / 1px)", "inset(0 round 1px / 2px / 3px)",
        "inset(0 round 1px round 2px)", "inset(0) round 1px", "inset(0 round 1px 2px 3px 4px 5px)",
        "circle(1e999px)", "ellipse(1e999% 1px)", "inset(-1e999px)",
    ] {
        assert_eq!(parse(value), None, "{value}");
    }
}

#[test]
fn injected_nonfinite_tokens_are_rejected_in_every_length_role() {
    for n in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for token in [Token::Number(n), Token::Percentage(n), Token::Dimension(n, "px".into())] {
            for (name, inner) in [
                ("circle", vec![token.clone()]),
                ("ellipse", vec![token.clone(), Token::Number(0.0)]),
                ("circle", vec![Token::Ident("at".into()), token.clone(), Token::Number(0.0)]),
                ("inset", vec![token.clone()]),
                ("inset", vec![Token::Number(0.0), Token::Ident("round".into()), token.clone()]),
            ] {
                let mut tokens = vec![Token::Function(name.into())];
                tokens.extend(inner);
                tokens.push(Token::CloseParen);
                assert_eq!(parse_clip_path(&tokens), None, "{tokens:?}");
            }
        }
    }
}
