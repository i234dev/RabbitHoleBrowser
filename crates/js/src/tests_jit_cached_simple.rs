//! Controls for the executed cached-simple CallR helper, independent of timing.
use crate::{Realm, Value, JsResult};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

fn evaluate(realm: &mut Realm, source: &str) -> String {
    let value = realm.eval(source, "cached-simple.js").unwrap_or_else(|error| panic!("{}", realm.error_to_string(&error)));
    realm.to_string(value).unwrap().to_rust_string()
}
fn normal_gc(realm: &mut Realm) {
    realm.gc_enabled = true;
    realm.gcx.no_gc = false;
    realm.gcx.stress = 0;
    realm.gcx.incremental = true;
    realm.tick = 1024;
}
fn empty_stacks(realm: &Realm) {
    assert!(realm.frames.is_empty() && realm.vstack.is_empty() && realm.new_target_stack.is_empty());
    assert_eq!(realm.native_depth, 0);
    assert_eq!(realm.jit.live_entries, 0);
}

#[test]
fn cached_simple_witness_is_per_invocation_and_collection_invalidates_it() {
    for requested in [false, true] {
        let mut realm = Realm::new();
        realm.set_jit(requested, 0);
        normal_gc(&mut realm);
        // Padding makes the dispatcher eligible for entry from interpreted
        // callers under the unchanged ENTRY_MIN_OPS policy. No other JS call
        // occurs inside it, so every hit below belongs to the target callsite.
        let padding = "padding += 1;".repeat(32);
        evaluate(&mut realm, &format!("function invoke(f,x) {{ var padding=0; {padding} return f(x); }} function probe(x){{return x+1}}; function make(n){{return function(x){{return n+x}}}}; var left=make(1),right=make(10);"));
        let active = realm.jit_enabled();
        let invoke = |realm: &mut Realm, source: &str, expected: &str, hit: bool| {
            let before = realm.jit.cached_simple_calls;
            assert_eq!(evaluate(realm, source), expected);
            assert_eq!(realm.jit.cached_simple_calls - before, u64::from(active && hit), "{source}: exact invocation witness");
            empty_stacks(realm);
        };
        invoke(&mut realm, "invoke(probe,2)", "3", false);
        invoke(&mut realm, "invoke(probe,3)", "4", true);
        let epoch = realm.heap.collections;
        realm.gc();
        assert!(realm.heap.collections > epoch);
        invoke(&mut realm, "invoke(probe,4)", "5", false);
        invoke(&mut realm, "invoke(probe,5)", "6", true);
        // Same FunctionCode, distinct closure/scope; each identity must warm.
        invoke(&mut realm, "invoke(left,3)", "4", false);
        invoke(&mut realm, "invoke(left,4)", "5", true);
        invoke(&mut realm, "invoke(right,3)", "13", false);
        invoke(&mut realm, "invoke(right,4)", "14", true);
        invoke(&mut realm, "invoke(left,5)", "6", false);
        invoke(&mut realm, "invoke(left,6)", "7", true);
        eprintln!("native-canary cached-simple target={}-{} requested={requested} enabled={active} hits={} collections={}", std::env::consts::OS, std::env::consts::ARCH, realm.jit.cached_simple_calls, realm.heap.collections);
    }
}

#[test]
fn cached_simple_differential_adversaries_preserve_fallbacks_and_counted_values() {
    const CASES: &[(&str, &str, &str)] = &[
        ("closure-environments", include_str!("../tests/fixtures/jit-call-resume/closure-environments.js"), "L:1|R:11|L:2|R:12"),
        ("constructors-and-new-target", include_str!("../tests/fixtures/jit-call-resume/constructors-and-new-target.js"), "base|object|TypeError|good|C:4"),
        ("receiver-and-indirect-fallbacks", include_str!("../tests/fixtures/jit-call-resume/receiver-and-indirect-fallbacks.js"), "number:3|[object Number]|number:4|5|5"),
        ("native-callback-finally", include_str!("../tests/fixtures/jit-call-resume/native-callback-finally.js"), "0,3,two,9|0,1,2,3"),
        ("wide-slots-and-stack-growth", include_str!("../tests/fixtures/jit-call-resume/wide-slots-and-stack-growth.js"), "caller-left|v299|caller-right"),
        ("polymorphic-callsite-rest-and-arguments", include_str!("../tests/fixtures/jit-call-resume/polymorphic-callsite-rest-and-arguments.js"), "2|3|3|1:4|1:5|TypeError|8"),
        ("getter-coercion-resume", include_str!("../tests/fixtures/jit-call-resume/getter-coercion-resume.js"), "inner:3|8|inner:4|get,value,bad"),
        ("interpreted-bridge-and-suspension", include_str!("../tests/fixtures/jit-call-resume/interpreted-bridge-and-suspension.js"), "v1|v2|true"),
    ];
    for stress in [0, crate::realm::GC_STRESS_FULL, crate::realm::GC_STRESS_INCREMENTAL] {
        for &(name, source, expected) in CASES {
            let mut baseline = None;
            for mode in [None, Some(0), Some(3)] {
                let mut realm = Realm::new();
                realm.set_jit(mode.is_some(), mode.unwrap_or(0));
                normal_gc(&mut realm);
                realm.gcx.stress = stress;
                realm.gcx.incremental = stress != crate::realm::GC_STRESS_FULL;
                // Changing stress alone does not reset the running countdown.
                // Activate it for this fixture's first call/backedge and prove
                // that collection work really happened during its execution.
                realm.tick = realm.tick_interval();
                realm.gcx.stress_n = 0;
                let collections = realm.heap.collections;
                let console = Rc::new(RefCell::new(String::new()));
                let output = console.clone();
                realm.set_console_sink(Box::new(move |_, text| { output.borrow_mut().push_str(text); output.borrow_mut().push('\n'); }));
                realm.set_deadline(Some(std::time::Instant::now() + std::time::Duration::from_secs(30)));
                let result = evaluate(&mut realm, source);
                realm.run_jobs();
                realm.set_deadline(None);
                if stress == crate::realm::GC_STRESS_FULL {
                    assert!(realm.heap.collections > collections, "{name}: no full collection during fixture mode={mode:?}");
                } else if stress == crate::realm::GC_STRESS_INCREMENTAL {
                    assert!(realm.gcx.stress_n > 0, "{name}: no incremental slice during fixture mode={mode:?}");
                }
                assert_eq!(result, expected, "{name} stress={stress} mode={mode:?}");
                let output = console.borrow().clone();
                if name == "interpreted-bridge-and-suspension" { assert_eq!(output, "async:v3\n"); }
                let observed = (result, output);
                if let Some(expected) = &baseline { assert_eq!(expected, &observed, "{name} stress={stress} mode={mode:?}"); }
                else { baseline = Some(observed); }
                empty_stacks(&realm);
            }
        }
    }
}

#[derive(Default)]
struct StackGrowth { before_ptr: Cell<usize>, before_capacity: Cell<usize>, before_hits: Cell<u64>, grew_on_hit: Cell<bool> }
fn stack_before(realm: &mut Realm, _: Value, _: &[Value]) -> JsResult<Value> {
    let state = realm.native_data::<StackGrowth>(realm.current_native().unwrap()).unwrap();
    state.before_ptr.set(realm.vstack.as_ptr() as usize);
    state.before_capacity.set(realm.vstack.capacity());
    state.before_hits.set(realm.jit.cached_simple_calls);
    Ok(Value::Undefined)
}
fn stack_after(realm: &mut Realm, _: Value, _: &[Value]) -> JsResult<Value> {
    let state = realm.native_data::<StackGrowth>(realm.current_native().unwrap()).unwrap();
    if realm.jit.cached_simple_calls > state.before_hits.get()
        && (realm.vstack.as_ptr() as usize != state.before_ptr.get() || realm.vstack.capacity() > state.before_capacity.get()) {
        state.grew_on_hit.set(true);
    }
    Ok(Value::Undefined)
}

#[test]
fn cached_simple_refreshes_slots_after_a_warm_push_grows_the_stack() {
    let mut realm = Realm::new();
    realm.set_jit(true, 0);
    normal_gc(&mut realm);
    let state = Rc::new(StackGrowth::default());
    let global = realm.global();
    for (name, callback) in [("__stackBefore", stack_before as crate::NativeFn), ("__stackAfter", stack_after as crate::NativeFn)] {
        let function = realm.new_native_function_full(name, 0, callback, Some(state.clone()), Vec::new(), false);
        realm.define_property(global, name, Value::Object(function), crate::HIDDEN);
    }
    let padding = "padding += 1;".repeat(32);
    let locals = (0..300).map(|i| format!("var slot{i}='counted-{i}';")).collect::<String>();
    evaluate(&mut realm, &format!("function wide(x){{{locals} __stackAfter();return slot299+':'+x}} function invokeWide(f,x){{var padding=0;{padding} __stackBefore();return f(x)}}"));
    assert_eq!(evaluate(&mut realm, "invokeWide(wide,'warm')"), "counted-299:warm");
    // The live dispatcher fits a small allocation; its already-warm target
    // needs >300 slots and must enlarge it while the helper is executing.
    realm.vstack.shrink_to_fit();
    state.grew_on_hit.set(false);
    assert_eq!(evaluate(&mut realm, "invokeWide(wide,'actual')"), "counted-299:actual");
    if realm.jit_enabled() { assert!(state.grew_on_hit.get(), "the warm fast push must really grow/move vstack"); }
    empty_stacks(&realm);
}

#[test]
fn cached_simple_depth_errors_sync_pc_and_restore_for_catch_finally_and_later_calls() {
    let mut realm = Realm::new();
    realm.set_jit(true, 0);
    normal_gc(&mut realm);
    evaluate(&mut realm, "function recurse(n){return n===0?'done':recurse(n-1)}; recurse(4)");
    realm.max_frames = 64;
    let errors = realm.jit.cached_simple_errors;
    assert_eq!(evaluate(&mut realm, "var result=[];try{recurse(1000)}catch(error){result.push(error.name)}finally{result.push('finally')}result.push(recurse(0));result.join('|')"), "RangeError|finally|done");
    if realm.jit_enabled() { assert!(realm.jit.cached_simple_errors > errors, "the failed push must use the new path"); }
    empty_stacks(&realm);
    assert_eq!(evaluate(&mut realm, "recurse(1)"), "done");
}

#[test]
fn cached_simple_call_loop_keeps_watchdog_ticks_before_the_fast_path() {
    let mut realm = Realm::new();
    realm.set_jit(true, 0);
    normal_gc(&mut realm);
    assert_eq!(evaluate(&mut realm, "function leaf(x){return x+1} function spin(n){var i=0;while(i<n){i=leaf(i)}return i}spin(4)"), "4");
    let entries = realm.jit_entries();
    let hits = realm.jit.cached_simple_calls;
    realm.set_deadline(Some(std::time::Instant::now() + std::time::Duration::from_millis(200)));
    let result = realm.eval("spin(Infinity)", "cached-simple-watchdog.js");
    let error = realm.root(result.expect_err("cached calls must not bypass safe-point ticks"));
    let after_entries = realm.jit_entries();
    let after_hits = realm.jit.cached_simple_calls;
    let aborts = realm.watchdog_abort_count();
    realm.set_deadline(None);
    assert!(realm.error_to_string(&error.value()).contains("timed out"));
    assert!(aborts > 0);
    if realm.jit_enabled() {
        assert!(after_entries > entries, "the interrupted invocation must enter native code");
        assert!(after_hits > hits, "the interrupted invocation must execute the new call path");
    }
    empty_stacks(&realm);
    assert_eq!(evaluate(&mut realm, "leaf(0)"), "1");
    eprintln!("native-canary cached-simple-watchdog target={}-{} enabled={} entry_delta={} hit_delta={} watchdog_aborts={aborts}", std::env::consts::OS, std::env::consts::ARCH, realm.jit_enabled(), after_entries-entries, after_hits-hits);
}


#[test]
fn cached_simple_direct_method_distinguishes_strict_and_sloppy_primitive_receivers() {
    for requested in [false, true] {
        let mut realm = Realm::new();
        realm.set_jit(requested, 0);
        normal_gc(&mut realm);
        let padding = "padding += 1;".repeat(32);
        evaluate(&mut realm, &format!("var receiverEffects=0;var strictHandler=Number.prototype.__jitCachedReceiver=function(x){{'use strict';receiverEffects++;return typeof this+':'+this+':'+x}};function directReceiver(receiver,value){{var padding=0;{padding}return receiver.__jitCachedReceiver(value)}};var boxedReceiver=new Number(4);"));
        let active = realm.jit_enabled();
        let mut expected_effects = 0;
        let mut invoke = |realm: &mut Realm, source: &str, expected: &str, hit: bool| {
            let before = realm.jit.cached_simple_calls;
            assert_eq!(evaluate(realm, source), expected);
            assert_eq!(realm.jit.cached_simple_calls-before, u64::from(active && hit), "{source}");
            expected_effects += 1;
            assert_eq!(evaluate(realm, "receiverEffects"), expected_effects.to_string(), "callee must execute exactly once");
            empty_stacks(realm);
        };
        invoke(&mut realm, "directReceiver(3,'cold')", "number:3:cold", false);
        invoke(&mut realm, "directReceiver(3,'warm')", "number:3:warm", true);
        evaluate(&mut realm, "Number.prototype.__jitCachedReceiver=function(x){receiverEffects++;return typeof this+':'+this+':'+x}");
        // Populate the SAME sloppy callee's IC with an object, then send a
        // primitive through that warm record. It must box on the fallback,
        // rather than using the fast path's existing-object receiver rule.
        invoke(&mut realm, "directReceiver(boxedReceiver,'cold')", "object:4:cold", false);
        invoke(&mut realm, "directReceiver(boxedReceiver,'warm')", "object:4:warm", true);
        invoke(&mut realm, "directReceiver(4,'primitive')", "object:4:primitive", false);
        invoke(&mut realm, "directReceiver(5,'again')", "object:5:again", false);
        evaluate(&mut realm, "Number.prototype.__jitCachedReceiver=strictHandler");
        invoke(&mut realm, "directReceiver(6,'cold')", "number:6:cold", false);
        invoke(&mut realm, "directReceiver(6,'warm')", "number:6:warm", true);
        evaluate(&mut realm, "String.prototype.__jitCachedReceiver=strictHandler;BigInt.prototype.__jitCachedReceiver=strictHandler");
        invoke(&mut realm, "directReceiver('counted-this','string')", "string:counted-this:string", true);
        invoke(&mut realm, "directReceiver(7n,'bigint')", "bigint:7:bigint", true);
    }
}
