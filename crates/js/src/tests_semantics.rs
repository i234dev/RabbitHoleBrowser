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

#[test]
fn for_using_iterable_observes_head_binding_tdz() {
    for declaration in ["using", "await using"] {
        let source = format!(r#"
          var outcome='pending';
          async function check() {{
            let resource=[null];
            let error='none';
            try {{ for ({declaration} resource of resource) {{}} }}
            catch(e) {{ error=e.name; }}
            return error+':'+resource.length;
          }}
          check().then(value=>{{outcome=value;}});
          ({{toString(){{return outcome;}}}})
        "#);
        assert_eq!(evaluate(&source), "ReferenceError:1", "declaration={declaration}");
    }
}

#[test]
fn for_using_iterable_closure_keeps_the_uninitialized_head_binding() {
    for declaration in ["using", "await using"] {
        let source = format!(r#"
          var outcome='pending';
          async function check() {{
            let resource='outer';
            let read;
            for ({declaration} resource of (read=()=>resource,[null])) {{}}
            try {{ read(); return 'no error'; }}
            catch(e) {{ return e.name+':'+resource; }}
          }}
          check().then(value=>{{outcome=value;}});
          ({{toString(){{return outcome;}}}})
        "#);
        assert_eq!(evaluate(&source), "ReferenceError:outer", "declaration={declaration}");
    }
}

#[test]
fn for_using_disposes_once_and_preserves_iteration_closures_on_continue_and_break() {
    for (loop_kind, declaration) in [("for", "using"), ("for", "await using"), ("for await", "using"), ("for await", "await using")] {
        let source = format!(r#"
          var outcome='pending';
          async function check() {{
            const events=[], reads=[];
            const resources=[1,2,3].map(id=>({{
              id,
              [Symbol.dispose](){{events.push(this.id);}},
              [Symbol.asyncDispose](){{events.push(this.id);return Promise.resolve();}}
            }}));
            {loop_kind} ({declaration} resource of resources) {{
              reads.push(()=>resource.id);
              if (resource.id===1) continue;
              break;
            }}
            return events.join(',')+':'+reads.map(read=>read()).join(',');
          }}
          check().then(value=>{{outcome=value;}});
          ({{toString(){{return outcome;}}}})
        "#);
        assert_eq!(evaluate(&source), "1,2:1,2", "{loop_kind} ({declaration})");
    }
}

#[test]
fn duplicate_parameter_last_missing_argument_overwrites_earlier_value() {
    for (actual, expected) in [("1,2", "undefined"), ("1,2,3,4", "4"), ("1,2,3,undefined", "undefined"), ("", "undefined")] {
        assert_eq!(evaluate(&format!("(function(x,a,b,x){{return x;}})({actual})")), expected);
        assert_eq!(evaluate(&format!("(function(x,a,b,x){{return ()=>x;}})({actual})()")), expected);
    }
    assert_eq!(evaluate("(function(x,a,b,x){x=5;return [x,arguments[0],arguments.length].join(':');})(1,2)"), "5:1:2");
}

#[test]
fn anonymous_class_static_fields_resolve_the_outer_binding_in_its_tdz() {
    for reference in ["Outer", "eval('Outer')"] {
        let source = format!(r#"
          (function() {{
            try {{ let Outer=class {{static field={reference};}}; return 'no error'; }}
            catch(e) {{ return e.name; }}
          }})()
        "#);
        assert_eq!(evaluate(&source), "ReferenceError", "reference={reference}");
    }
}

#[test]
fn inferred_class_names_preserve_outer_closures_and_explicit_class_bindings() {
    for reference in ["Outer", "eval('Outer')"] {
        let source = format!(r#"
          (function() {{
            let Outer=class {{static read=()=>{reference};}};
            const Original=Outer, name=Outer.name;
            Outer=null;
            return [name,Original.read()===null].join(':');
          }})()
        "#);
        assert_eq!(evaluate(&source), "Outer:true", "reference={reference}");
    }
    assert_eq!(evaluate(r#"
      (function(){
        let Outer=class Inner {static read=()=>Inner;};
        const Original=Outer, name=Outer.name;
        Outer=null;
        return [name,Original.read()===Original].join(':');
      })()
    "#), "Inner:true");
    assert_eq!(evaluate(r#"
      (function(){
        class Outer {static read=()=>Outer;}
        const Original=Outer;
        Outer=null;
        return Original.read()===Original;
      })()
    "#), "true");
}

#[test]
fn var_arguments_reuses_the_implicit_object_before_its_initializer() {
    assert_eq!(evaluate("(function(){return typeof arguments;var arguments='later';})(1,2)"), "object");
    assert_eq!(evaluate("(function(){var arguments;return ()=>arguments[0];})(42)()"), "42");
    assert_eq!(evaluate("(function(a=1){var arguments;return arguments.length;})(1,2)"), "2");
    assert_eq!(evaluate("(function(a){var arguments;arguments[0]=7;return a;})(1)"), "7");
    assert_eq!(evaluate("(function(){var arguments=['replacement'];return arguments[0];})(42)"), "replacement");
}

#[test]
fn var_arguments_respects_explicit_parameter_and_body_bindings() {
    assert_eq!(evaluate("(function(arguments){var arguments;return arguments;})('parameter')"), "parameter");
    assert_eq!(evaluate("(function({arguments}){var arguments;return arguments;})({arguments:'pattern'})"), "pattern");
    assert_eq!(evaluate("(function(){var arguments;function arguments(){return 'function';}return arguments();})()"), "function");
    assert_eq!(evaluate("(function(){const original=arguments;return [((arguments)=>{var arguments;return arguments;})('arrow'),arguments===original].join(':');})(1)"), "arrow:true");
}
