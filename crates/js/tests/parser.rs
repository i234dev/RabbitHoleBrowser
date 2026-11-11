//! Parser corpus tests.

use rabbithole_js::ast::*;
use rabbithole_js::parser::{parse_expression, parse_program};

fn parse(src: &str) -> Program {
    match parse_program(src, false) {
        Ok(p) => p,
        Err(e) => panic!("failed to parse {src:?}: {e}"),
    }
}

fn parse_module(src: &str) -> Program {
    match parse_program(src, true) {
        Ok(p) => p,
        Err(e) => panic!("failed to parse module {src:?}: {e}"),
    }
}

fn err(src: &str) -> rabbithole_js::ParseError {
    match parse_program(src, false) {
        Ok(_) => panic!("expected a syntax error for {src:?}"),
        Err(e) => e,
    }
}

fn expr(src: &str) -> Expr {
    parse_expression(src).unwrap_or_else(|e| panic!("failed to parse expression {src:?}: {e}"))
}

fn stmt_expr(p: &Program, i: usize) -> &Expr {
    match &p.body[i].kind {
        StmtKind::Expr(e) => e,
        other => panic!("statement {i} is not an expression: {other:?}"),
    }
}

#[test]
fn asi_rules() {
    // `a \n ++b` → a; ++b
    let p = parse("a\n++b");
    assert_eq!(p.body.len(), 2);
    assert!(matches!(stmt_expr(&p, 1).kind, ExprKind::Update { prefix: true, .. }));
    // return \n x → return; x
    let p = parse("function f(){ return\nx }");
    let StmtKind::FunctionDecl(f) = &p.body[0].kind else { panic!() };
    let FunctionBody::Block(b) = &f.body else { panic!() };
    assert!(matches!(b[0].kind, StmtKind::Return(None)));
    assert_eq!(b.len(), 2);
    // No ASI before `(`: `let x = 1\n(f())` is a call of 1
    let p = parse("let x = 1\n(f())");
    let StmtKind::VarDecl(d) = &p.body[0].kind else { panic!() };
    assert!(matches!(d.declarations[0].init.as_ref().unwrap().kind, ExprKind::Call { .. }));
    // ASI at EOF and before `}`
    parse("x = 1");
    parse("{ x = 1 }");
    // postfix ++ on next line is an error for `a\n++` alone
    assert!(parse_program("a\n++", false).is_err());
    // do-while without semicolon
    parse("do x(); while (y) z()");
    // throw with newline is an error
    assert!(parse_program("throw\nx", false).is_err());
}

#[test]
fn regex_vs_division() {
    let e = expr("a / b / c");
    assert!(matches!(e.kind, ExprKind::Binary { op: BinaryOp::Div, .. }));
    let e = expr("/re/g.test(s)");
    let ExprKind::Call { callee, .. } = e.kind else { panic!() };
    let ExprKind::Member { object, .. } = callee.kind else { panic!() };
    assert_eq!(object.kind, ExprKind::Regex { pattern: "re".into(), flags: "g".into() });
    let e = expr("(1) / 2");
    assert!(matches!(e.kind, ExprKind::Binary { op: BinaryOp::Div, .. }));
    let p = parse("{}\n/foo/.test(x)");
    assert_eq!(p.body.len(), 2);
    assert!(matches!(stmt_expr(&p, 1).kind, ExprKind::Call { .. }));
    let p = parse("x = a[0] / 2 / y");
    assert_eq!(p.body.len(), 1);
    let e = expr("x = /[/]+/.source");
    assert!(matches!(e.kind, ExprKind::Assign { .. }));
    assert!(parse_program("x = /abc", false).is_err());
    assert!(parse_program("x = /abc/gg", false).is_err());
}

#[test]
fn templates() {
    let e = expr("`a${ {b: 1}.b }c${`d${e}`}`");
    let ExprKind::Template { quasis, raws, expressions } = e.kind else { panic!() };
    assert_eq!(quasis, vec![Some("a".into()), Some("c".into()), Some("".into())]);
    assert_eq!(raws.len(), 3);
    assert_eq!(expressions.len(), 2);
    let e = expr("tag`x\\ny${1}`");
    let ExprKind::TaggedTemplate { quasis, raws, .. } = e.kind else { panic!() };
    assert_eq!(quasis[0], Some("x\ny".into()));
    assert_eq!(raws[0], "x\\ny");
    // Invalid escapes are allowed (cooked = None) only in tagged templates.
    let e = expr("tag`\\unicode`");
    let ExprKind::TaggedTemplate { quasis, .. } = e.kind else { panic!() };
    assert_eq!(quasis[0], None);
    assert!(parse_expression("`\\unicode`").is_err());
}

#[test]
fn classes() {
    let p = parse(
        "class A extends B { #x = 1; static y; static #z = 2; constructor(a) { super(a); this.#x = a } \
         get x() { return this.#x } set x(v) { this.#x = v } static m() {} *gen() {} async am() {} \
         async *agen() {} [computed]() {} 'str'() {} 42() {} static { init(); } #priv() { return #x in this } }",
    );
    let StmtKind::ClassDecl(c) = &p.body[0].kind else { panic!() };
    assert_eq!(c.name.as_deref(), Some("A"));
    assert!(c.superclass.is_some());
    assert_eq!(c.members.len(), 15);
    assert!(matches!(c.members[0], ClassMember::Field { key: PropertyKey::Private(_), is_static: false, .. }));
    assert!(matches!(c.members[1], ClassMember::Field { is_static: true, value: None, .. }));
    assert!(matches!(c.members[3], ClassMember::Constructor(_)));
    assert!(matches!(c.members[4], ClassMember::Getter { .. }));
    assert!(matches!(c.members[5], ClassMember::Setter { .. }));
    assert!(matches!(c.members[6], ClassMember::Method { is_static: true, .. }));
    assert!(matches!(&c.members[7], ClassMember::Method { value, .. } if value.is_generator));
    assert!(matches!(&c.members[8], ClassMember::Method { value, .. } if value.is_async));
    assert!(matches!(&c.members[9], ClassMember::Method { value, .. } if value.is_async && value.is_generator));
    assert!(matches!(c.members[10], ClassMember::Method { key: PropertyKey::Computed(_), .. }));
    assert!(matches!(c.members[13], ClassMember::StaticBlock(_)));
    // class expression, anonymous
    let e = expr("class { static get [k]() {} }");
    assert!(matches!(e.kind, ExprKind::Class(_)));
    assert!(parse_program("class { }", false).is_err());
    assert!(parse_program("class A { constructor() {} constructor = 1 }", false).is_err());
    // `static` / `get` as plain names
    let p = parse("class C { static; get; set = 1; async; static static() {} get get() {} }");
    let StmtKind::ClassDecl(c) = &p.body[0].kind else { panic!() };
    assert_eq!(c.members.len(), 6);
}

#[test]
fn destructuring() {
    let p = parse("const { a, b: { c = 1 } = {}, ...rest } = obj; let [x, , y = 2, ...zs] = arr;");
    let StmtKind::VarDecl(d) = &p.body[0].kind else { panic!() };
    let Pattern::Object { properties, rest } = &d.declarations[0].target else { panic!() };
    assert_eq!(properties.len(), 2);
    assert!(properties[0].shorthand);
    assert!(matches!(properties[1].value, Pattern::Assign { .. }));
    assert!(rest.is_some());
    let StmtKind::VarDecl(d) = &p.body[1].kind else { panic!() };
    let Pattern::Array { elements, rest } = &d.declarations[0].target else { panic!() };
    assert_eq!(elements.len(), 3);
    assert!(elements[1].is_none());
    assert!(matches!(elements[2], Some(Pattern::Assign { .. })));
    assert!(rest.is_some());
    // assignment destructuring
    let e = expr("[a, b.c, {d = 3}] = arr");
    let ExprKind::Assign { target: Pattern::Array { elements, .. }, .. } = e.kind else { panic!() };
    assert!(matches!(elements[1], Some(Pattern::Expr(_))));
    assert!(matches!(elements[2], Some(Pattern::Object { .. })));
    let e = expr("({ a, b: [c] } = o)");
    assert!(matches!(e.kind, ExprKind::Assign { target: Pattern::Object { .. }, .. }));
    // swap idiom
    parse("[a, b] = [b, a];");
    // params
    let e = expr("function ({ a = 1 }, [b], ...c) {}");
    let ExprKind::Function(f) = e.kind else { panic!() };
    assert_eq!(f.params.len(), 3);
    assert!(f.params[2].rest);
    // errors
    assert!(parse_program("const x;", false).is_err());
    assert!(parse_program("let [a];", false).is_err());
    assert!(parse_program("1 = 2", false).is_err());
    assert!(parse_program("a + 1 = 2", false).is_err());
    assert!(parse_program("[...a, b] = c", false).is_err());
}

#[test]
fn functions_async_generators_arrows() {
    let p = parse(
        "async function f() { await x; for await (const y of z) {} } \
         function* g() { yield; yield 1; yield* h(); const v = yield } \
         async function* ag() { yield await 1 } \
         const a = async (x, {y}, ...z) => { await x }; \
         const b = async x => x; const c = (x = 1) => x; const d = x => ({ x }); \
         const e = () => () => 1; const f2 = async () => {}; \
         var arr = [async, async (), async(1)]",
    );
    assert_eq!(p.body.len(), 10);
    let StmtKind::FunctionDecl(f) = &p.body[0].kind else { panic!() };
    assert!(f.is_async && !f.is_generator);
    let StmtKind::FunctionDecl(g) = &p.body[1].kind else { panic!() };
    assert!(g.is_generator);
    let FunctionBody::Block(gb) = &g.body else { panic!() };
    assert!(matches!(stmt_of(&gb[0]).kind, ExprKind::Yield { argument: None, delegate: false }));
    assert!(matches!(stmt_of(&gb[2]).kind, ExprKind::Yield { delegate: true, .. }));
    let StmtKind::VarDecl(d) = &p.body[3].kind else { panic!() };
    let ExprKind::Arrow(a) = &d.declarations[0].init.as_ref().unwrap().kind else { panic!() };
    assert!(a.is_async && a.params.len() == 3 && a.params[2].rest);
    let StmtKind::VarDecl(d) = &p.body[6].kind else { panic!() };
    let ExprKind::Arrow(a) = &d.declarations[0].init.as_ref().unwrap().kind else { panic!() };
    assert!(matches!(a.body, FunctionBody::Expr(_)));
    // `async` as an identifier / call
    let StmtKind::VarDecl(d) = &p.body[9].kind else { panic!() };
    let ExprKind::Array(els) = &d.declarations[0].init.as_ref().unwrap().kind else { panic!() };
    assert!(matches!(els[0], ArrayElement::Expr(Expr { kind: ExprKind::Identifier(_), .. })));
    assert!(matches!(els[1], ArrayElement::Expr(Expr { kind: ExprKind::Call { .. }, .. })));
    // `await` as identifier in sloppy scripts
    parse("var await = 1; await + 1");
    // `yield` as identifier outside generators
    parse("var yield = 1");
    assert!(parse_program("function* g() { var yield = 1 }", false).is_err());
    // arrow newline restriction
    assert!(parse_program("x\n=> 1", false).is_err());
    // line break between async and `(` makes it a call (and `=>` after it is an error)
    let p = parse("async\n(x)");
    assert!(matches!(stmt_expr(&p, 0).kind, ExprKind::Call { .. }));
    assert!(parse_program("async\n(x) => x", false).is_err());
}

fn stmt_of(s: &Stmt) -> &Expr {
    match &s.kind {
        StmtKind::Expr(e) => e,
        StmtKind::VarDecl(d) => d.declarations[0].init.as_ref().unwrap(),
        _ => panic!("{s:?}"),
    }
}

#[test]
fn optional_chains() {
    let e = expr("a?.b.c");
    let ExprKind::OptionalChain(inner) = e.kind else { panic!("{e:?}") };
    let ExprKind::Member { object, property: MemberProp::Identifier(c), optional: false } = inner.kind else { panic!() };
    assert_eq!(c, "c");
    assert!(matches!(object.kind, ExprKind::Member { optional: true, .. }));
    let e = expr("a?.[x]?.(1).d");
    assert!(matches!(e.kind, ExprKind::OptionalChain(_)));
    let e = expr("(a?.b).c");
    let ExprKind::Member { object, .. } = e.kind else { panic!() };
    assert!(matches!(object.kind, ExprKind::OptionalChain(_)));
    assert!(parse_expression("a?.b = 1").is_err());
    assert!(parse_expression("a?.b`t`").is_err());
    assert!(parse_expression("new a?.b()").is_err());
}

#[test]
fn loops_labels_switch() {
    let p = parse(
        "outer: for (let i = 0; i < 3; i++) { inner: while (true) { if (i) continue outer; else break inner } } \
         for (const [k, v] of Object.entries(o)) {} for (x in obj) ; for (var i = 0, j = 1;;) break; \
         for (a.b of c) {} for (let in x) {} for (;;) {} \
         switch (x) { case 1: case 2: f(); break; default: g() } \
         lbl: { break lbl; }",
    );
    assert_eq!(p.body.len(), 9);
    let StmtKind::ForOf { left, .. } = &p.body[1].kind else { panic!() };
    assert!(matches!(&**left, ForHead::VarDecl { kind: VarKind::Const, target: Pattern::Array { .. } }));
    assert!(matches!(&p.body[2].kind, StmtKind::ForIn { left, .. } if matches!(&**left, ForHead::Pattern(Pattern::Identifier(_)))));
    assert!(matches!(&p.body[4].kind, StmtKind::ForOf { left, .. } if matches!(&**left, ForHead::Pattern(Pattern::Expr(_)))));
    assert!(matches!(&p.body[5].kind, StmtKind::ForIn { left, .. } if matches!(&**left, ForHead::Pattern(Pattern::Identifier(_)))));
    let StmtKind::Switch { cases, .. } = &p.body[7].kind else { panic!() };
    assert_eq!(cases.len(), 3);
    assert!(cases[0].body.is_empty());
    assert!(cases[2].test.is_none());
    // `in` disabled in for-init but allowed inside parens / arrays
    let p = parse("for (var i = (a in b) ? 1 : 2; i;) {}");
    assert!(matches!(p.body[0].kind, StmtKind::For { .. }));
    parse("for (var i = [a in b]; false;) {}");
    // errors
    assert!(err("break").message.contains("break"));
    assert!(err("continue").message.contains("continue"));
    assert!(err("x: { continue x }").message.contains("continue"));
    assert!(err("a: a: 1").message.contains("already"));
    assert!(err("break foo").message.contains("label"));
    assert!(parse_program("switch (x) { default: default: }", false).is_err());
    assert!(parse_program("for (let x of y, z) {}", false).is_err());
    assert!(parse_program("for (let of x) {}", false).is_err());
}

#[test]
fn getters_setters_and_object_literals() {
    let e = expr(
        "({ a, b: 1, 'c': 2, 3: 4, [k]: 5, get x() { return 1 }, set x(v) {}, m() {}, async n() {}, *g() {}, async *ag() {}, \
          get: 1, set: 2, async: 3, ...spread, __proto__: null, get() {}, set() {}, async() {} })",
    );
    let ExprKind::Object(props) = e.kind else { panic!() };
    assert_eq!(props.len(), 19);
    assert!(matches!(props[0], ObjectProperty::Property { shorthand: true, .. }));
    assert!(matches!(props[5], ObjectProperty::Property { value: PropertyValue::Getter(_), .. }));
    assert!(matches!(props[6], ObjectProperty::Property { value: PropertyValue::Setter(_), .. }));
    assert!(matches!(&props[8], ObjectProperty::Property { value: PropertyValue::Method(f), .. } if f.is_async));
    assert!(matches!(&props[10], ObjectProperty::Property { value: PropertyValue::Method(f), .. } if f.is_async && f.is_generator));
    assert!(matches!(props[11], ObjectProperty::Property { key: PropertyKey::Identifier(ref k), value: PropertyValue::Init(_), .. } if k == "get"));
    assert!(matches!(props[14], ObjectProperty::Spread(_)));
    assert!(matches!(&props[16], ObjectProperty::Property { key: PropertyKey::Identifier(k), value: PropertyValue::Method(_), .. } if k == "get"));
    assert!(parse_expression("({ get x(a) {} })").is_err());
    assert!(parse_expression("({ set x() {} })").is_err());
}

#[test]
fn numeric_literals_and_operators() {
    let e = expr("[0x1F, 0o17, 0b101, 017, 1_000_000, 1e3, .5, 5., 10n, 0xffn]");
    let ExprKind::Array(els) = e.kind else { panic!() };
    let nums: Vec<f64> = els
        .iter()
        .filter_map(|e| match e {
            ArrayElement::Expr(Expr { kind: ExprKind::Number(n), .. }) => Some(*n),
            _ => None,
        })
        .collect();
    assert_eq!(nums, vec![31.0, 15.0, 5.0, 15.0, 1_000_000.0, 1000.0, 0.5, 5.0]);
    assert!(matches!(els[8], ArrayElement::Expr(Expr { kind: ExprKind::BigInt(ref s), .. }) if s == "10"));
    // exponent associativity: 2 ** 3 ** 2 = 2 ** (3 ** 2)
    let e = expr("2 ** 3 ** 2");
    let ExprKind::Binary { op: BinaryOp::Exp, left, right } = e.kind else { panic!() };
    assert!(matches!(left.kind, ExprKind::Number(n) if n == 2.0));
    assert!(matches!(right.kind, ExprKind::Binary { op: BinaryOp::Exp, .. }));
    assert!(parse_expression("-2 ** 2").is_err());
    parse_expression("(-2) ** 2").unwrap();
    parse_expression("2 ** -2").unwrap();
    // precedence: a + b * c
    let e = expr("a + b * c");
    let ExprKind::Binary { op: BinaryOp::Add, right, .. } = e.kind else { panic!() };
    assert!(matches!(right.kind, ExprKind::Binary { op: BinaryOp::Mul, .. }));
    // ?? mixing
    assert!(parse_expression("a ?? b || c").is_err());
    assert!(parse_expression("a || b ?? c").is_err());
    parse_expression("(a ?? b) || c").unwrap();
    parse_expression("a ?? (b || c)").unwrap();
    parse_expression("a ?? b ?? c").unwrap();
    // compound assignments
    for op in ["+=", "-=", "**=", "<<=", ">>>=", "&&=", "||=", "??="] {
        expr(&format!("a {op} 1"));
    }
    assert!(parse_expression("a + 1 += 2").is_err());
    // conditional and sequence
    let e = expr("a ? b : c, d");
    assert!(matches!(e.kind, ExprKind::Sequence(ref v) if v.len() == 2));
    // unary / update
    assert!(matches!(expr("typeof x").kind, ExprKind::Unary { op: UnaryOp::Typeof, .. }));
    assert!(matches!(expr("x++").kind, ExprKind::Update { prefix: false, .. }));
    assert!(parse_expression("1++").is_err());
    assert!(matches!(expr("void 0").kind, ExprKind::Unary { op: UnaryOp::Void, .. }));
    // in / instanceof
    assert!(matches!(expr("a in b").kind, ExprKind::Binary { op: BinaryOp::In, .. }));
    assert!(matches!(expr("a instanceof b").kind, ExprKind::Binary { op: BinaryOp::Instanceof, .. }));
    // new
    assert!(matches!(expr("new A").kind, ExprKind::New { ref args, .. } if args.is_empty()));
    let e = expr("new a.b.C(1)");
    assert!(matches!(e.kind, ExprKind::New { ref args, .. } if args.len() == 1));
    let e = expr("new new A()()");
    assert!(matches!(e.kind, ExprKind::New { .. }));
    let e = expr("new A().b");
    assert!(matches!(e.kind, ExprKind::Member { .. }));
    assert!(matches!(expr("function f() { new.target }").kind, ExprKind::Function(_)));
}

#[test]
fn modules() {
    let p = parse_module(
        "import x from \"y\"; import * as ns from 'z'; import d, { a as b, c } from 'w'; import 'side';\n\
         export default async function () {} export const q = 1; export { a as b, c }; export * from 'm'; export * as n from 'm';\n\
         export { x as default }; export function f() {} export class K {} \
         await foo(); import.meta.url; import('dyn');",
    );
    assert!(p.is_module && p.strict);
    let StmtKind::Import(i) = &p.body[0].kind else { panic!() };
    assert_eq!(i.default.as_deref(), Some("x"));
    let StmtKind::Import(i) = &p.body[1].kind else { panic!() };
    assert_eq!(i.namespace.as_deref(), Some("ns"));
    let StmtKind::Import(i) = &p.body[2].kind else { panic!() };
    assert_eq!(i.named, vec![("a".to_string(), "b".to_string()), ("c".to_string(), "c".to_string())]);
    assert!(matches!(p.body[4].kind, StmtKind::Export(ExportDecl::DefaultDecl(_))));
    assert!(matches!(p.body[5].kind, StmtKind::Export(ExportDecl::Decl(_))));
    assert!(matches!(p.body[6].kind, StmtKind::Export(ExportDecl::Named { source: None, .. })));
    assert!(matches!(p.body[7].kind, StmtKind::Export(ExportDecl::All { namespace: None, .. })));
    assert!(matches!(p.body[8].kind, StmtKind::Export(ExportDecl::All { namespace: Some(_), .. })));
    assert!(matches!(stmt_expr(&p, 12).kind, ExprKind::Await(_)));
    assert!(matches!(stmt_expr(&p, 13).kind, ExprKind::Member { .. }));
    assert!(matches!(stmt_expr(&p, 14).kind, ExprKind::DynamicImport(_)));
    assert!(parse_program("import x from 'y'", false).is_err());
    assert!(parse_program("export default 1", false).is_err());
    // scripts still parse dynamic import
    parse("import('x').then(m => m)");
}

#[test]
fn comments_hashbang_html() {
    let p = parse("#!/usr/bin/env node\n<!-- html comment\nvar a = 1; /* block */ // line\n--> also a comment\nvar b = 2");
    assert_eq!(p.body.len(), 2);
    // `-->` not at line start is an operator sequence: a --> b is a-- > b
    let p = parse("a --> b");
    assert!(matches!(stmt_expr(&p, 0).kind, ExprKind::Binary { op: BinaryOp::Gt, .. }));
    assert!(parse_program("/* unterminated", false).is_err());
    // modules do not support html comments
    assert!(parse_program("<!-- x", true).is_err());
}

#[test]
fn strict_mode_and_directives() {
    let p = parse("'use strict'; var x = 1");
    assert!(p.strict);
    let p = parse("'not a directive' + 1; 'use strict'; with (a) {}");
    assert!(!p.strict);
    assert!(parse_program("'use strict'; with (a) {}", false).is_err());
    assert!(parse_program("'use strict'; var let = 1", false).is_err());
    assert!(parse_program("'use strict'; eval = 1", false).is_err());
    assert!(parse_program("'use strict'; delete x", false).is_err());
    parse("var let = 1; let = 2; var static = 3");
    let p = parse("function f() { 'use strict'; return 1 } function g() { return 2 }");
    let StmtKind::FunctionDecl(f) = &p.body[0].kind else { panic!() };
    assert!(f.strict);
    let StmtKind::FunctionDecl(g) = &p.body[1].kind else { panic!() };
    assert!(!g.strict);
    // class bodies are strict
    assert!(parse_program("class A { m() { with (x) {} } }", false).is_err());
}

#[test]
fn error_positions() {
    let e = err("var a = 1;\nvar b = ;");
    assert_eq!((e.line, e.column), (2, 9));
    let e = err("function f() {\n  return 1\n}\n}");
    assert_eq!(e.line, 4);
    let e = err("let s = 'abc");
    assert_eq!(e.line, 1);
    let e = err("x = `abc${1}");
    assert_eq!(e.line, 1);
    let e = err("class A extends {}");
    assert!(e.message.contains("Expected '{'"), "{}", e.message);
    let e = err("if (x) { } else else { }");
    assert_eq!((e.line, e.column), (1, 17));
}

#[test]
fn misc_syntax() {
    parse("try { } catch { } finally { }");
    parse("try { } catch ({ message }) { }");
    assert!(parse_program("try { }", false).is_err());
    parse("label: for (;;) { break label }");
    parse("debugger;");
    parse("if (a) function f() {}");
    parse("a = b ? c : d ? e : f");
    parse("x = { async *[Symbol.iterator]() {} }");
    parse("x = class extends (a, b) {}");
    parse("var { [key]: val = 1, 'quoted': q } = o");
    parse("o = { if: 1, class: 2, function: 3, null: 4, true: 5 }; o.if; o.class.function");
    parse("x = a?.b?.c?.(d)?.[e] ?? f");
    parse("x = (a, b) => (c, d)");
    parse("f(...args, last)");
    parse("new (foo())()");
    parse("(function () {})(); (() => {})(); !function () {}();");
    parse("x = y\n/re/g\n.z"); // division, since y / re / g . z
    parse("let async = 1; async = 2; async\n(1)");
    parse("for (const x of [1,2,3]) { }");
    parse("a = b\n++c");
    parse("'\\u{1F600}' + '\\u2028' + '\\0'");
    parse("x = +'1' + -2 + ~3 + !4 + ++a + --b + a++ + b--");
    parse("var \\u0061 = 1; a");
    // getter with keyword names and numeric keys in classes
    parse("class A { get 1() {} set 'a b'(v) {} static async *[x]() {} }");
    parse("obj = { 'use strict': 1 }");
    parse("x = 1 /* c */ /\n2");
    parse("var o = { x, y, z }; ({ x, y } = o)");
    parse("switch (a) {}");
    parse("if (a) ; else ;");
    parse("x = a => b => c");
    parse("async function f() { for await (x of y) {} }");
    parse("x = { get a() { return 1 }, set a(v) {} }.a");
    parse("x = typeof typeof x");
    parse("x = void (0, 1)");
    assert!(parse_program("x = new.target", false).is_err());
    parse("function f() { return new.target }");
}

#[test]
fn realistic_snippet() {
    let src = r#"
'use strict';
/**
 * A small module-ish snippet exercising everyday syntax.
 */
(function (global, factory) {
  typeof exports === 'object' && typeof module !== 'undefined' ? factory(exports) :
  typeof define === 'function' && define.amd ? define(['exports'], factory) :
  (global = typeof globalThis !== 'undefined' ? globalThis : global || self, factory(global.Lib = {}));
})(this, (function (exports) {
  const VERSION = "1.2.3";
  const DEFAULTS = Object.freeze({ retries: 3, timeout: 1_000, headers: { 'Content-Type': 'application/json' } });

  class EventEmitter {
    #listeners = new Map();
    on(name, fn) {
      if (!this.#listeners.has(name)) this.#listeners.set(name, new Set());
      this.#listeners.get(name).add(fn);
      return () => this.off(name, fn);
    }
    off(name, fn) { this.#listeners.get(name)?.delete(fn); }
    emit(name, ...args) {
      for (const fn of this.#listeners.get(name) ?? []) {
        try { fn.apply(this, args); } catch (err) { console.error(`listener for ${name} failed:`, err); }
      }
    }
    static create() { return new this(); }
  }

  class Client extends EventEmitter {
    constructor(options = {}) {
      super();
      this.options = { ...DEFAULTS, ...options };
      this.queue = [];
      this.running = false;
    }

    get size() { return this.queue.length; }

    async request(url, { method = 'GET', body, signal } = {}) {
      let attempt = 0, lastError;
      while (attempt++ < this.options.retries) {
        try {
          const res = await fetch(url, { method, body: body && JSON.stringify(body), headers: this.options.headers, signal });
          if (!res.ok) throw new Error(`HTTP ${res.status}`);
          const data = await res.json();
          this.emit('response', { url, data });
          return data;
        } catch (e) {
          lastError = e;
          await new Promise(r => setTimeout(r, 2 ** attempt * 100));
        }
      }
      throw lastError ?? new Error('unreachable');
    }

    *entries() {
      for (let i = 0; i < this.queue.length; i++) yield [i, this.queue[i]];
    }

    async *stream(urls) {
      for await (const u of urls) yield await this.request(u);
    }
  }

  function debounce(fn, wait = 100) {
    let t;
    return function (...args) {
      clearTimeout(t);
      t = setTimeout(() => fn.apply(this, args), wait);
    };
  }

  const pick = (obj, ...keys) => keys.reduce((acc, k) => (k in obj ? { ...acc, [k]: obj[k] } : acc), {});
  const range = (n) => Array.from({ length: n }, (_, i) => i);
  const sum = (...xs) => xs.reduce((a, b) => a + b, 0);
  const re = /^(?<year>\d{4})-(?<month>\d{2})-(?<day>\d{2})$/u;

  function parseDate(s) {
    const m = re.exec(s);
    if (!m) return null;
    const { year, month, day } = m.groups;
    return new Date(+year, month - 1, +day);
  }

  switch (typeof window) {
    case 'undefined':
      break;
    case 'object': {
      const w = window;
      w.__lib = VERSION;
      // fallthrough
    }
    default:
      void 0;
  }

  label: for (const x of range(10)) {
    if (x % 2) continue label;
    if (x > 6) break label;
  }

  let counter = 0;
  do { counter += 1 } while (counter < 3);

  const obj = {
    a: 1,
    'b-c': 2,
    [`computed${counter}`]: 3,
    get total() { return this.a + this['b-c']; },
    set total(v) { this.a = v - this['b-c']; },
    method() { return this.total; },
    async asyncMethod() { return await Promise.resolve(this.a); },
    *gen() { yield* [1, 2, 3]; },
  };

  var legacy = 017 + 0x1f + 0b11 + 0o7, str = 'it\'s' + "a \"quote\"" + `template ${obj.a + 1} ${`nested`}`;
  if (legacy > 0 && str) {
    exports.ready = true
  } else if (!str) exports.ready = false
  else exports.ready = null

  exports.Client = Client;
  exports.EventEmitter = EventEmitter;
  exports.debounce = debounce;
  exports.pick = pick;
  exports.sum = sum;
  exports.parseDate = parseDate;
  exports.VERSION = VERSION;
  Object.defineProperty(exports, '__esModule', { value: true });
}));
"#;
    let p = parse(src);
    assert!(p.strict);
    assert_eq!(p.body.len(), 2);
}

#[test]
fn large_input_is_fast() {
    // ~1 MB of generated code.
    let mut src = String::new();
    for i in 0..20000 {
        src.push_str(&format!(
            "function f{i}(a, b) {{ var x = a * {i} + b; if (x > 10) {{ return [x, 'str', {{ k: x }}]; }} return x ? f{i}(b, a) : null; }}\n"
        ));
    }
    let start = std::time::Instant::now();
    let p = parse(&src);
    assert_eq!(p.body.len(), 20000);
    assert!(start.elapsed().as_secs() < 10, "parsing took {:?}", start.elapsed());
}

/// Deeply nested `(a=(b=(...` must not retry every level as arrow
/// parameters (exponential time; seen in reCAPTCHA).
#[test]
fn nested_parens_parse_fast() {
    let depth = 60;
    let mut src = String::new();
    for i in 0..depth {
        src.push_str(&format!("(v{i}=", ));
    }
    src.push('1');
    for _ in 0..depth {
        src.push(')');
    }
    let t = std::time::Instant::now();
    parse(&src);
    assert!(t.elapsed() < std::time::Duration::from_secs(2), "{:?}", t.elapsed());
    parse("var f = (a, b = (c) => c) => a + b(1);");
}
