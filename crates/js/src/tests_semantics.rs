//! Scope and expression regressions reduced from standard conformance cases.
use crate::Realm;

fn evaluate(source: &str) -> String {
    let mut realm = Realm::new();
    let result = realm.eval(source, "semantics.js");
    realm.run_jobs();
    match result {
        Ok(value) => realm.to_rust_string(value).unwrap(),
        Err(error) => panic!("{} threw: {}", source, realm.error_to_string(&error)),
    }
}

#[test]
fn annexb_block_functions_do_not_replace_implicit_arguments() {
    for parameters in ["", "x", "...rest", "x=1", "{x}={}"] {
        let source = format!(r#"
          (function({parameters}) {{
            const original=arguments;
            let inside;
            {{ inside=arguments(); function arguments(){{return 'block';}} }}
            return [Object.prototype.toString.call(arguments),arguments===original,inside].join(':');
          }})()
        "#);
        assert_eq!(evaluate(&source), "[object Arguments]:true:block", "params={parameters}");
    }
}

#[test]
fn annexb_arguments_binding_respects_parameter_expressions_and_body_functions() {
    for (parameters, actual, expected) in [
        ("", "", "block"),
        ("...rest", "", "block"),
        ("{x}", "{}", "block"),
        ("[x]", "[]", "block"),
        ("x=1", "", "top"),
        ("{x=1}", "{}", "top"),
        ("[x=1]", "[]", "top"),
        ("{['x']:x}", "{}", "top"),
    ] {
        let source = format!(r#"
          (function({parameters}) {{
            function arguments() {{return 'top';}}
            {{ function arguments(){{return 'block';}} }}
            return arguments();
          }})({actual})
        "#);
        assert_eq!(evaluate(&source), expected, "params={parameters}");
    }
    assert_eq!(evaluate("(function(arguments){ { function arguments(){return 'block';} } return arguments; })('parameter')"), "parameter");
}

#[test]
fn exponentiation_accepts_update_expressions_with_right_associativity() {
    for (expression, initial, expected) in [
        ("--base ** 2", 4, "9:3"),
        ("++base ** 2", 4, "25:5"),
        ("base++ ** 2", 4, "16:5"),
        ("base-- ** 2", 4, "16:3"),
        ("--base ** --base ** 2", 4, "81:2"),
        ("++base ** ++base ** 2", 2, "43046721:4"),
        ("base++ ** base++ ** 2", 4, "1125899906842624:6"),
    ] {
        let source = format!("var base={initial}; [({expression}),base].join(':')");
        assert_eq!(evaluate(&source), expected, "expression={expression}");
    }
}

#[test]
fn exponentiation_rejects_unparenthesized_unary_bases() {
    for expression in ["-base ** 2", "+base ** 2", "!base ** 2", "typeof base ** 2", "-++base ** 2", "void base++ ** 2"] {
        assert!(crate::parser::parse_program(expression, false).is_err(), "accepted {expression}");
    }
    assert_eq!(evaluate("var base=4; [(-base) ** 2, base ** -2, ++base ** -2].join(':')"), "16:0.0625:0.04");
}
