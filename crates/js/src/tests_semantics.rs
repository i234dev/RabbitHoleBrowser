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
