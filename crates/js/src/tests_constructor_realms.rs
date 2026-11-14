//! Constructor prototype fallback must preserve abrupt completions and realm selection.
use crate::Realm;

const CHECK: &str = r#"
function check(value, message) { if (!value) throw new Error(message); }
"#;

const CONSTRUCTORS: &str = r#"
function constructorCases() {
  const cases = [
    ['ordinary', function Plain() {}, []],
    ['Object', Object, []], ['Function', Function, []],
    ['Boolean', Boolean, []], ['Number', Number, []], ['String', String, []],
    ['Date', Date, []], ['RegExp', RegExp, []], ['Array', Array, []],
    ['Error', Error, []], ['TypeError', TypeError, []], ['RangeError', RangeError, []],
    ['EvalError', EvalError, []], ['ReferenceError', ReferenceError, []],
    ['SyntaxError', SyntaxError, []], ['URIError', URIError, []],
    ['Int8Array', Int8Array, []], ['ArrayBuffer', ArrayBuffer, []],
    ['DataView', DataView, [new ArrayBuffer(0)]],
    ['Map', Map, []], ['Set', Set, []], ['WeakMap', WeakMap, []], ['WeakSet', WeakSet, []],
    ['Promise', Promise, [function () {}]],
    ['GeneratorFunction', (function* () {}).constructor, []],
    ['AsyncFunction', (async function () {}).constructor, []],
    ['AsyncGeneratorFunction', (async function* () {}).constructor, []]
  ];
  if (typeof SharedArrayBuffer === 'function') cases.push(['SharedArrayBuffer', SharedArrayBuffer, []]);
  for (const name of ['Collator', 'NumberFormat', 'DateTimeFormat', 'PluralRules', 'RelativeTimeFormat', 'ListFormat', 'Segmenter']) {
    cases.push(['Intl.' + name, Intl[name], ['en']]);
  }
  cases.push(['Intl.Locale', Intl.Locale, ['en']]);
  cases.push(['Intl.DisplayNames', Intl.DisplayNames, ['en', {type:'language'}]]);
  return cases;
}
"#;

const REVOKED_FALLBACK: &str = r#"
(function () {
  for (const primitive of [undefined, null, 0, true, 'prototype', Symbol('prototype')]) {
    let bodyCalls=0, getterCalls=0, error;
    function Target() { bodyCalls++; }
    const handle=Proxy.revocable(function () {}, {
      get(target, key, receiver) {
        if (key === 'prototype') { getterCalls++; handle.revoke(); return primitive; }
        return Reflect.get(target, key, receiver);
      }
    });
    try { Reflect.construct(Target, [], handle.proxy); } catch (e) { error=e; }
    check(error instanceof TypeError, 'revoked fallback must throw TypeError');
    check(getterCalls===1 && bodyCalls===0, 'prototype getter precedes realm validation and constructor body');
  }
  return 'ok';
})()
"#;

const NATIVE_REVOKED_FALLBACK: &str = r#"
(function () {
  const failures=[];
  for (const [name, Constructor, args] of constructorCases()) {
    let getterCalls=0, error;
    const handle=Proxy.revocable(function () {}, {
      get(target, key, receiver) {
        if (key === 'prototype') { getterCalls++; handle.revoke(); return undefined; }
        return Reflect.get(target, key, receiver);
      }
    });
    try { Reflect.construct(Constructor, args, handle.proxy); } catch (e) { error=e; }
    if (!(error instanceof TypeError) || getterCalls!==1) failures.push(name);
  }
  check(failures.length===0, 'revoked constructor fallbacks: '+failures.join(','));
  return 'ok';
})()
"#;

const GETTER_THROW: &str = r#"
(function () {
  const failures=[];
  for (const [name, Constructor, args] of constructorCases()) {
    let getterCalls=0, error;
    const marker={name};
    const proxy=new Proxy(function () {}, {
      get(target, key, receiver) {
        if (key === 'prototype') { getterCalls++; throw marker; }
        return Reflect.get(target, key, receiver);
      }
    });
    try { Reflect.construct(Constructor, args, proxy); } catch (e) { error=e; }
    if (error!==marker || getterCalls!==1) failures.push(name);
  }
  check(failures.length===0, 'prototype getter abrupt completions: '+failures.join(','));
  return 'ok';
})()
"#;

const OBJECT_PROTOTYPE: &str = r#"
(function () {
  const failures=[];
  for (const [name, Constructor, args] of constructorCases()) {
    let getterCalls=0, error, instance;
    const prototype={name};
    const handle=Proxy.revocable(function () {}, {
      get(target, key, receiver) {
        if (key === 'prototype') { getterCalls++; handle.revoke(); return prototype; }
        return Reflect.get(target, key, receiver);
      }
    });
    try { instance=Reflect.construct(Constructor, args, handle.proxy); } catch (e) { error=e; }
    if (error || getterCalls!==1 || Object.getPrototypeOf(instance)!==prototype) failures.push(name);
  }
  check(failures.length===0, 'object prototypes must not request the revoked function realm: '+failures.join(','));
  return 'ok';
})()
"#;

fn evaluate_case(body: &str) {
    let mut realm = Realm::new();
    let source = format!("{CHECK}\n{CONSTRUCTORS}\n{body}");
    match realm.eval(&source, "constructor-realms.js") {
        Ok(value) => {
            let value = realm.root(value);
            assert_eq!(realm.to_rust_string(value.value()).unwrap(), "ok");
        }
        Err(error) => {
            let error = realm.root(error);
            panic!("{}", realm.error_to_string(&error.value()));
        }
    }
}

#[test]
fn revoked_fallback_validates_function_realm_before_running_the_body() {
    evaluate_case(REVOKED_FALLBACK);
}

#[test]
fn native_and_intl_fallbacks_propagate_revoked_new_target_errors() {
    evaluate_case(NATIVE_REVOKED_FALLBACK);
}

#[test]
fn native_and_intl_constructors_preserve_prototype_getter_abrupt_completions() {
    evaluate_case(GETTER_THROW);
}

#[test]
fn object_prototype_after_revocation_does_not_query_the_function_realm() {
    evaluate_case(OBJECT_PROTOTYPE);
}

#[test]
fn bound_constructor_fallback_uses_the_targets_foreign_intrinsics() {
    let mut realm = Realm::new();
    let other = realm.create_realm();
    let foreign_proto = realm.realm_intrinsics(other).object_proto;
    let target = realm.in_realm(other, |r| r.eval("var Target=function(){}; Target.prototype=null; Target.bind(null)", "foreign-constructor.js")).unwrap();
    let target = realm.root(target);
    let current_proto = realm.intrinsics.object_proto;
    assert_eq!(realm.proto_from_ctor(target.value(), current_proto).unwrap(), foreign_proto);
}
