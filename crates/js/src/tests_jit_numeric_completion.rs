//! Exercise the generated completion store, including the helper boundary.
use crate::bigint::BigInt;
use crate::bytecode::{FunctionCode, Op};
use crate::realm::Realm;
use crate::vm::FrameExtra;
use crate::{JsResult, Value, V};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

thread_local! {
    static OBSERVATIONS: Cell<usize> = const { Cell::new(0) };
    static NATIVE_ITERATIONS: Cell<usize> = const { Cell::new(0) };
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

fn evaluate(realm: &mut Realm, source: &str, mode: Option<u32>, check_pair: bool) -> Value {
    let code = realm.compile(source, "numeric-completion.js", false).unwrap();
    assert!(code.ops.iter().any(|op| matches!(op, Op::StoreCompletionR(_))));
    if check_pair {
        let stores: Vec<usize> = code.ops.iter().enumerate()
            .filter_map(|(pc, op)| matches!(op, Op::StoreCompletionR(_)).then_some(pc)).collect();
        // Prove the fixture still contains a pair inside a loop, with only
        // number/register templates between stores. Lowering must not remove
        // one store or insert a helper that would invalidate their cache.
        assert!(stores.windows(2).any(|pair| {
            code.ops[pair[0] + 1..pair[1]].iter().all(|op| matches!(op,
                Op::Mov(..) | Op::MovRaw(..) | Op::MovChecked(..) | Op::CheckInit(..) | Op::AddRR(..)))
                && code.ops.iter().enumerate().any(|(latch, op)|
                    op.jump_target().is_some_and(|target| (target as usize) <= pair[0] && pair[1] < latch))
        }), "the numeric pair must survive lowering without a helper between it");
    }
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

fn mark_native_iteration(r: &mut Realm, _: Value, _: &[Value]) -> JsResult<Value> {
    if r.jit.live_entries > 0 {
        NATIVE_ITERATIONS.with(|n| n.set(n.get() + 1));
    }
    Ok(Value::Undefined)
}

#[test]
fn numeric_completion_loop_last_statement_writes_numbers_and_preserves_bits() {
    for mode in [None, Some(0), Some(3)] {
        for (source, expected) in [
            ("for(let i=0,sum=0;i<80;i++){var observed=markCompletionNative();7;sum+=i;}", 3160.0),
            ("for(let i=0,v=-0;i<80;i++){var observed=markCompletionNative();v;v;}", -0.0),
            ("for(let i=0,v=nanPayload;i<80;i++){var observed=markCompletionNative();v;v;}", f64::from_bits(0xFFF8_0000_0000_0042)),
        ] {
            let mut r = realm(mode);
            let mark = r.new_native_function("markCompletionNative", 0, mark_native_iteration);
            r.define_global("markCompletionNative", Value::Object(mark));
            NATIVE_ITERATIONS.with(|n| n.set(0));
            if expected.is_nan() {
                // Explicit negative quiet-NaN payload: arithmetic such as
                // 0/0 has no portable sign/payload oracle. This tests copying
                // these exact Number bits through the completion fast store.
                r.define_global("nanPayload", Value::Number(expected));
            }
            let value = evaluate(&mut r, source, mode, true);
            if expected.is_nan() { assert!(value.as_number().unwrap().is_nan()); }
            // No final expression overwrites the loop completion. This also
            // compares -0/NaN bits, without a JS coercion masking a wrong store.
            assert_eq!(value.bits(), Value::Number(expected).bits(), "{source}");
            let native_iterations = NATIVE_ITERATIONS.with(Cell::get) as u64;
            if mode.is_some() && rabbithole_jit::supported() {
                assert!((64..=80).contains(&native_iterations), "the measured body must run natively: {native_iterations}");
                // Each host mark invalidates before the pair. The first
                // numeric store must use the helper and activate the cache;
                // the second has only register/number operations before it.
                // The sole extra helper allowed is the For's initial
                // undefined completion, when entry already runs natively.
                // Calling the helper for BOTH stores costs 2*N and fails.
                assert!((native_iterations..=native_iterations + 1).contains(&r.jit.completion_fallbacks),
                    "native iterations={native_iterations}, helper fallbacks={}", r.jit.completion_fallbacks);
            } else {
                assert_eq!(native_iterations, 0);
                assert_eq!(r.jit.completion_fallbacks, 0);
            }
            eprintln!("numeric-completion loop mode={mode:?} supported={} entries={} native_iterations={native_iterations} helper_fallbacks={} result_bits={:016x}", rabbithole_jit::supported(), r.jit_entries(), r.jit.completion_fallbacks, value.bits());
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
        "#, mode, false);
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
            let actual = evaluate(&mut r, &source, mode, false);
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
        assert_eq!(evaluate(&mut r, "let a=0;for(let i=0;i<17;i++)a+=i;removeExtra();var ok=verifyCompletion(33);let b=0;for(let j=0;j<32;j++)b+=j;", mode, false), Value::Number(496.0));
        assert!(VERIFIED.with(Cell::get));
        RETIRED.with(|retired| assert_eq!(retired.borrow()[0].completion, Value::Number(136.0), "missing extra must take the helper, never the old Box"));
        if mode.is_some() && rabbithole_jit::supported() { assert!(r.jit.completion_fallbacks > 0); }
        RETIRED.with(|retired| retired.borrow_mut().clear());
        VERIFIED.with(|done| done.set(false));
        REENTERED.with(|done| done.set(false));
        assert_eq!(evaluate(&mut r, "let c=0;for(let i=0;i<17;i++)c+=i;reenterAndReplace();var ok2=verifyCompletion(5);let d=0;for(let j=0;j<32;j++)d+=j;", mode, false), Value::Number(496.0));
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
        assert_eq!(evaluate(&mut r, "let recovered=0;for(let i=0;i<32;i++)recovered+=i;", mode, false), Value::Number(496.0));
    }
}


// No calls, allocations from JS, coercions, or explicit GC. run_script also
// has no entry_tick (unlike Realm::call), so only the loop's jumps poll.
const TICK_PROBE: &str = "for(let i=0,total=0;i<80;i++)total+=i;";
const TICK_POLLS: u32 = 80;

fn tick_probe_code(realm: &mut Realm) -> Rc<FunctionCode> {
    let code = realm.compile(TICK_PROBE, "completion-tick.js", false).unwrap();
    assert!(code.functions.is_empty() && code.handlers.is_empty());
    assert!(!code.ops.iter().any(|op| matches!(op, Op::CallR(..) | Op::NewR(..))));
    let latch = code.ops.iter().enumerate().filter_map(|(pc, op)| match *op {
        Op::IncJumpIfLtRK(register, constant, target) => Some((pc, register, constant, target)),
        _ => None,
    }).collect::<Vec<_>>();
    let entry = code.ops.iter().enumerate().filter_map(|(pc, op)| match *op {
        Op::JumpIfLtRK(register, constant, target) => Some((pc, register, constant, target)),
        _ => None,
    }).collect::<Vec<_>>();
    assert_eq!(latch.len(), 1);
    assert_eq!(entry.len(), 1);
    assert_eq!(entry[0].0, latch[0].0 + 1, "the false latch skips the entry test");
    assert_eq!((entry[0].1, entry[0].2, entry[0].3), (latch[0].1, latch[0].2, latch[0].3));
    assert_eq!(code.constants[entry[0].2 as usize].as_number(), Some(TICK_POLLS as f64));
    assert_eq!(code.ops.iter().enumerate().filter(|(pc, op)|
        op.jump_target().is_some_and(|target| (target as usize) <= *pc)).count(), 2);
    // VM jump! and Gen::jump both poll the initially taken backward entry
    // test once, then 79 taken fused latches. The final false latch skips the
    // entry test in BOTH paths: 80 polls, not an assumed VM/JIT equality.
    code
}

fn run_tick_probe(realm: &mut Realm, code: Rc<FunctionCode>, mode: Option<u32>) {
    let before = realm.jit_entries();
    assert_eq!(realm.run_script(code.clone()).unwrap(), Value::Number(3160.0));
    if mode.is_some() && rabbithole_jit::supported() {
        assert!(realm.jit_entries() > before);
        assert_ne!(code.jit.s.entry.get(), 0, "the actual loop ran natively");
    } else { assert_eq!(realm.jit_entries(), before); }
    assert!(realm.frames.is_empty() && realm.vstack.is_empty());
    assert_eq!(realm.jit.live_entries, 0);
}

#[test]
fn numeric_completion_tick_updates_the_real_countdown_without_resetting_it() {
    for mode in [None, Some(0)] {
        let mut realm = realm(mode);
        let code = tick_probe_code(&mut realm);
        realm.gcx.stress = 0;
        realm.tick = 10_000;
        let collections = realm.heap.collections;
        let slices = realm.gcx.stress_n;
        run_tick_probe(&mut realm, code, mode);
        assert_eq!(realm.tick, 10_000 - TICK_POLLS, "mode {mode:?}: decrement Realm.tick itself");
        assert_eq!(realm.heap.collections, collections, "the high countdown never reached a safe point");
        assert_eq!(realm.gcx.stress_n, slices);
        eprintln!("numeric-completion tick mode={mode:?} entries={} remaining={}", realm.jit_entries(), realm.tick);
    }
}

#[test]
fn numeric_completion_tick_drives_automatic_full_and_incremental_gc() {
    for mode in [None, Some(0)] {
        for stress in [crate::realm::GC_STRESS_FULL, crate::realm::GC_STRESS_INCREMENTAL] {
            let mut realm = realm(mode);
            let code = tick_probe_code(&mut realm);
            realm.gc_enabled = true;
            realm.gcx.no_gc = false;
            realm.gcx.stress = stress;
            realm.gcx.incremental = stress == crate::realm::GC_STRESS_INCREMENTAL;
            realm.gcx.stress_n = 0;
            realm.tick = 1;
            let collections = realm.heap.collections;
            // Realm bootstrap can leave an incremental cycle pending under
            // ambient stress. The first full gc finishes that existing cycle
            // before starting its own; account for it without manual GC.
            let prior_cycle = u32::from(realm.gc_in_progress());
            run_tick_probe(&mut realm, code, mode);
            assert_eq!(realm.tick, 1, "every poll resets the stress countdown");
            if stress == crate::realm::GC_STRESS_FULL {
                // One new synchronous full collection per poll, plus the
                // single prior cycle if gc() first had to finish it.
                assert_eq!(realm.heap.collections - collections, TICK_POLLS + prior_cycle);
                assert_eq!(realm.gcx.stress_n, 0);
            } else {
                // maybe_gc performs one budgeted slice per poll. Do not
                // claim a completed cycle from this bounded number of slices.
                assert_eq!(realm.gcx.stress_n, TICK_POLLS);
            }
            eprintln!("numeric-completion automatic-gc mode={mode:?} stress={stress} entries={} prior_cycle={prior_cycle} collections={} slices={}", realm.jit_entries(), realm.heap.collections - collections, realm.gcx.stress_n);
        }
    }
}
