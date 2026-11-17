//! Exercise the generated completion store, including the helper boundary.
use crate::bigint::BigInt;
use crate::bytecode::Op;
use crate::realm::Realm;
use crate::vm::FrameExtra;
use crate::{JsResult, Value, V};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

thread_local! {
    static OBSERVATIONS: Cell<usize> = const { Cell::new(0) };
    // Keep replaced allocations alive until generated code has returned: a
    // stale destination becomes a detectable wrong write, rather than a UAF.
    static RETIRED: RefCell<Vec<Box<FrameExtra>>> = const { RefCell::new(Vec::new()) };
    static REENTERED: Cell<bool> = const { Cell::new(false) };
    static VERIFIED: Cell<bool> = const { Cell::new(false) };
}

fn realm(mode: Option<u32>) -> Realm {
    let mut realm = Realm::new();
    realm.set_jit(mode.is_some(), mode.unwrap_or(0));
    realm
}

fn evaluate(realm: &mut Realm, source: &str, mode: Option<u32>) -> Value {
    let code = realm.compile(source, "numeric-completion.js", false).unwrap();
    assert!(code.ops.iter().any(|op| matches!(op, Op::StoreCompletionR(_))));
    let before = realm.jit_entries();
    // Exercise loop OSR without depending on an ambient GC stress interval.
    // Entry threshold/defaults are unchanged; only this test's first safe
    // point is scheduled while its bounded loop is still running.
    if mode == Some(3) { realm.tick = 2; }
    let result = realm.run_script(code.clone()).unwrap();
    if mode.is_some() && rabbithole_jit::supported() {
        assert!(realm.jit_entries() > before, "this invocation must enter generated code");
        assert_ne!(code.jit.s.entry.get(), 0, "the actual script body was compiled");
    } else {
        assert_eq!(realm.jit_entries(), before);
    }
    assert!(realm.frames.is_empty() && realm.vstack.is_empty() && realm.new_target_stack.is_empty());
    assert_eq!(realm.jit.live_entries, 0);
    result
}

#[test]
fn numeric_completion_loop_last_statement_writes_numbers_and_preserves_bits() {
    for mode in [None, Some(0), Some(3)] {
        for (source, expected) in [
            ("let sum=0;for(let i=0;i<80;i++)sum+=i;", 3160.0),
            ("let v=0;for(let i=0;i<80;i++){v=i===79?-0:i;v;}", -0.0),
            ("let v=0;for(let i=0;i<80;i++){v=i===79?nanPayload:i;v;}", f64::from_bits(0xFFF8_0000_0000_0042)),
        ] {
            let mut r = realm(mode);
            if expected.is_nan() {
                // Explicit negative quiet-NaN payload: arithmetic such as
                // 0/0 has no portable sign/payload oracle. This tests copying
                // these exact Number bits through the completion fast store.
                r.define_global("nanPayload", Value::Number(expected));
            }
            let value = evaluate(&mut r, source, mode);
            if expected.is_nan() { assert!(value.as_number().unwrap().is_nan()); }
            // No final expression overwrites the loop completion. This also
            // compares -0/NaN bits, without a JS coercion masking a wrong store.
            assert_eq!(value.bits(), Value::Number(expected).bits(), "{source}");
            if mode.is_some() && rabbithole_jit::supported() {
                assert!(r.jit.completion_fallbacks <= 2, "80 iterations cannot all use the helper: {}", r.jit.completion_fallbacks);
            } else { assert_eq!(r.jit.completion_fallbacks, 0); }
            eprintln!("numeric-completion loop mode={mode:?} supported={} entries={} helper_fallbacks={} result_bits={:016x}", rabbithole_jit::supported(), r.jit_entries(), r.jit.completion_fallbacks, value.bits());
        }
    }
}

fn observe_completion(r: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    if r.jit_enabled() && rabbithole_jit::supported() { assert!(r.jit.live_entries > 0); }
    let kind = match args[0].v() { V::String(s) => s.to_rust_string(), _ => panic!("fixture kind") };
    let value = r.frames.last().unwrap().extra.as_ref().unwrap().completion.clone();
    match (kind.as_str(), value.v()) {
        ("string", V::String(s)) => assert_eq!(s.to_rust_string(), "café 🐇 e\u{301}"),
        ("bigint", V::BigInt(b)) => assert_eq!(b, &BigInt::from_i128(123456789012345678901234567890)),
        ("object", V::Object(id)) => assert_eq!(r.obj(id).props.get(&"tag".into()).unwrap().value, Value::Number(42.0)),
        _ => panic!("completion kind changed: {kind} {value:?}"),
    }
    let before = r.heap.collections;
    r.gc();
    assert!(r.heap.collections > before, "an actual collection ran with the completion live");
    if let V::Object(id) = value.v() { assert!(r.heap.try_get(id).is_some()); }
    OBSERVATIONS.with(|n| n.set(n.get() + 1));
    Ok(Value::Undefined)
}

fn make_object(r: &mut Realm, _: Value, _: &[Value]) -> JsResult<Value> {
    let object = r.new_object();
    r.define_property(object, "tag", Value::Number(42.0), crate::ALL);
    Ok(Value::Object(object))
}

#[test]
fn numeric_completion_counted_values_gc_and_abrupt_finally_keep_helper_semantics() {
    for mode in [None, Some(0), Some(3)] {
        let mut r = realm(mode);
        let big = Rc::new(BigInt::from_i128(123456789012345678901234567890));
        r.define_global("sharedBig", Value::BigInt(big.clone()));
        r.define_global("sharedString", Value::str("café 🐇 e\u{301}"));
        let observe = r.new_native_function("observeCompletion", 1, observe_completion);
        r.define_global("observeCompletion", Value::Object(observe));
        let make = r.new_native_function("makeObject", 0, make_object);
        r.define_global("makeObject", Value::Object(make));
        let references = Rc::strong_count(&big);
        OBSERVATIONS.with(|n| n.set(0));
        let result = evaluate(&mut r, r#"
            let sum=0;for(let i=0;i<32;i++)sum+=i;
            sharedString;var s=observeCompletion('string');1;
            sharedBig;var b=observeCompletion('bigint');2;
            makeObject();var o=observeCompletion('object');3;
        "#, mode);
        assert_eq!(result, Value::Number(3.0));
        assert_eq!(OBSERVATIONS.with(Cell::get), 3);
        assert_eq!(Rc::strong_count(&big), references, "completion overwrite/frame exit releases the counted value exactly");
        if mode.is_some() && rabbithole_jit::supported() {
            assert!(r.jit.completion_fallbacks >= 6, "both sides of each non-number transition use the helper");
        }
        for (suffix, expected) in [
            ("label:{try{7;}finally{9;break label;}}", Value::Number(9.0)),
            ("label:{try{7;break label;}finally{9;}}", Value::Number(7.0)),
            ("label:{try{7;break label;}finally{break label;}}", Value::Undefined),
            ("try{throw 'kept';}catch(e){e;}", Value::str("kept")),
        ] {
            let mut r = realm(mode);
            let source = format!("let n=0;for(let i=0;i<32;i++)n+=i;{suffix}");
            let actual = evaluate(&mut r, &source, mode);
            match (actual.v(), expected.v()) {
                (V::String(actual), V::String(expected)) => assert_eq!(actual.to_rust_string(), expected.to_rust_string()),
                _ => assert_eq!(actual, expected),
            }
        }
    }
}

fn retire_extra(r: &mut Realm, replace: bool) {
    let frame = r.frames.last_mut().unwrap();
    let old = frame.extra.take().expect("the running script owns extra");
    if replace {
        frame.extra = Some(Box::new(FrameExtra {
            code: old.code.clone(), arguments_obj: old.arguments_obj.clone(),
            generator: old.generator, completion: Value::Number(-777.0),
            resume_slot: old.resume_slot, saved: old.saved.clone(),
        }));
    }
    RETIRED.with(|retired| retired.borrow_mut().push(old));
}

fn remove_extra(r: &mut Realm, _: Value, _: &[Value]) -> JsResult<Value> {
    if r.jit_enabled() && rabbithole_jit::supported() { assert!(r.jit.live_entries > 0); }
    retire_extra(r, false);
    Ok(Value::Number(33.0))
}

fn verify_completion(r: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    assert_eq!(r.frames.last().unwrap().extra.as_ref().unwrap().completion, args[0]);
    VERIFIED.with(|verified| verified.set(true));
    Ok(Value::Undefined)
}

fn reenter_and_replace(r: &mut Realm, _: Value, _: &[Value]) -> JsResult<Value> {
    if r.jit_enabled() && rabbithole_jit::supported() { assert!(r.jit.live_entries > 0); }
    let frames_before = r.frames.capacity();
    let depth = (frames_before + 8).max(16);
    assert!(depth <= 128, "fixture must stay bounded");
    // Keep 256 locals live across a recursive JS call: this actually grows
    // the frame stack and exercises nested generated frame switches.
    let locals = (0..256).map(|n| format!("a{n}=n")).collect::<Vec<_>>().join(",");
    let sum = (0..256).map(|n| format!("a{n}")).collect::<Vec<_>>().join("+");
    let source = format!("function completionDeep(n){{let {locals};if(n===0)return 11;const v=completionDeep(n-1);return v+{sum};}}completionDeep({depth})");
    let entries = r.jit_entries();
    let result = r.eval(&source, "completion-reentry.js")?;
    assert_eq!(result, Value::Number((11 + 256 * depth * (depth + 1) / 2) as f64));
    assert!(r.frames.capacity() > frames_before, "real reentry must grow Vec<Frame>");
    if r.jit_enabled() && rabbithole_jit::supported() { assert!(r.jit_entries() > entries); }
    // Guarantee a slot-buffer move rather than treating capacity growth as
    // proof of relocation. Allocate while the old buffer is still owned.
    let old_slots = r.vstack.as_ptr();
    let mut moved = Vec::with_capacity(r.vstack.capacity() + 1);
    moved.append(&mut r.vstack);
    r.vstack = moved;
    assert_ne!(r.vstack.as_ptr(), old_slots);
    r.gc();
    retire_extra(r, true);
    REENTERED.with(|done| done.set(true));
    Ok(Value::Number(5.0))
}

fn replace_and_panic(r: &mut Realm, _: Value, _: &[Value]) -> JsResult<Value> {
    if r.jit_enabled() && rabbithole_jit::supported() { assert!(r.jit.live_entries > 0); }
    retire_extra(r, true);
    panic!("numeric completion panic canary")
}

#[test]
fn numeric_completion_refreshes_after_box_removal_reentry_replacement_and_panic() {
    for mode in [None, Some(0), Some(3)] {
        let mut r = realm(mode);
        RETIRED.with(|retired| retired.borrow_mut().clear());
        for (name, function) in [
            ("removeExtra", remove_extra as crate::NativeFn),
            ("verifyCompletion", verify_completion as crate::NativeFn),
            ("reenterAndReplace", reenter_and_replace as crate::NativeFn),
            ("replaceAndPanic", replace_and_panic as crate::NativeFn),
        ] {
            let function = r.new_native_function(name, 1, function);
            r.define_global(name, Value::Object(function));
        }
        VERIFIED.with(|done| done.set(false));
        assert_eq!(evaluate(&mut r, "let a=0;for(let i=0;i<17;i++)a+=i;removeExtra();var ok=verifyCompletion(33);let b=0;for(let j=0;j<32;j++)b+=j;", mode), Value::Number(496.0));
        assert!(VERIFIED.with(Cell::get));
        RETIRED.with(|retired| assert_eq!(retired.borrow()[0].completion, Value::Number(136.0), "missing extra must take the helper, never the old Box"));
        if mode.is_some() && rabbithole_jit::supported() { assert!(r.jit.completion_fallbacks > 0); }
        RETIRED.with(|retired| retired.borrow_mut().clear());
        VERIFIED.with(|done| done.set(false));
        REENTERED.with(|done| done.set(false));
        assert_eq!(evaluate(&mut r, "let c=0;for(let i=0;i<17;i++)c+=i;reenterAndReplace();var ok2=verifyCompletion(5);let d=0;for(let j=0;j<32;j++)d+=j;", mode), Value::Number(496.0));
        assert!(REENTERED.with(Cell::get) && VERIFIED.with(Cell::get));
        RETIRED.with(|retired| assert_eq!(retired.borrow()[0].completion, Value::Number(136.0), "generated stores must write the replacement, not its retained predecessor"));
        RETIRED.with(|retired| retired.borrow_mut().clear());
        // Host panics are caught by call_native and become an exact JS
        // error, rather than unwinding through the generated activation.
        if mode == Some(3) { r.tick = 2; }
        let error = r.eval("let p=0;for(let i=0;i<17;i++)p+=i;replaceAndPanic();", "completion-panic.js")
            .expect_err("the host panic becomes a JS error");
        let name = r.get_str(error.clone(), "name").unwrap();
        let message = r.get_str(error, "message").unwrap();
        let V::String(name) = name.v() else { panic!("error name must be a string") };
        let V::String(message) = message.v() else { panic!("error message must be a string") };
        assert_eq!(name.to_rust_string(), "Error");
        assert_eq!(message.to_rust_string(), "internal error: numeric completion panic canary");
        assert!(r.frames.is_empty() && r.vstack.is_empty() && r.new_target_stack.is_empty());
        assert_eq!(r.jit.live_entries, 0, "the converted host panic leaves no generated activation");
        RETIRED.with(|retired| assert_eq!(retired.borrow()[0].completion, Value::Number(136.0)));
        RETIRED.with(|retired| retired.borrow_mut().clear());
        assert_eq!(evaluate(&mut r, "let recovered=0;for(let i=0;i<32;i++)recovered+=i;", mode), Value::Number(496.0));
    }
}
