//! Exact subset and fallback controls for the isolated constant remainder
//! template. These are semantic/native-path tests, not performance results.
use crate::tests_jit::run_mode;

fn check(source:&str, expected:&str) {
    let vm=run_mode(source,None);
    assert_eq!(vm.lines().next().unwrap_or(""),format!("ok {expected}"));
    for threshold in [0,2] {
        assert_eq!(run_mode(source,Some(threshold)),vm,"threshold {threshold}: {source}");
    }
}

#[test]
fn modrk_positive_constant_boundaries_and_signed_zero_fallback() {
    check(r#"
function rem7(x){ return x % 7; }
var xs=[0,1,6,7,2147483647,-0,-7,-8,-2147483648,-2147483649,
        1.5,2147483648,9007199254740992,Infinity,-Infinity,NaN];
var out=[];
for(var i=0;i<xs.length;i++) {
  var r=rem7(xs[i]);
  out.push(r===0 ? (1/r===-Infinity ? '-0' : '+0') : String(r));
}
out.push(rem7(5e-324)===5e-324);
out.join('|')
"#,"+0|1|6|+0|1|-0|-0|-1|-2|-3|1.5|2|4|NaN|NaN|NaN|true");
}

#[test]
fn modrk_other_constant_divisors_keep_the_existing_number_semantics() {
    check(r#"
function zero(x){return x%0;}
function minuszero(x){return x%-0;}
function negative(x){return x%-7;}
function minusone(x){return x%-1;}
function fractional(x){return x%2.5;}
function large(x){return x%2147483648;}
function infinite(x){return x%Infinity;}
function nan(x){return x%NaN;}
String([zero(3),minuszero(3),negative(8),1/minusone(-2147483648),
        fractional(8),large(17),infinite(-0),1/infinite(-0),nan(3)])
"#,"NaN,NaN,1,-Infinity,0.5,17,0,-Infinity,NaN");
    // Each small positive constant is a distinct RK template. Golden values
    // come from integer arithmetic here, independent of VM js_mod.
    for divisor in [1i32,2,3,7,1_000_003,i32::MAX] {
        let values=[0i32,1,17,i32::MAX];
        let expected=values.iter().map(|x|(x%divisor).to_string()).collect::<Vec<_>>().join(",");
        check(&format!("function f(x){{return x%{divisor};}}String([f(0),f(1),f(17),f(2147483647)])"),&expected);
    }
}

#[test]
fn modrk_type_fallback_keeps_conversion_order_errors_and_bigint_rules() {
    check(r#"
var log=[];
function f(x){return x%7;}
var v={valueOf(){log.push('v');return 22;}};
var bad={valueOf(){log.push('bad');throw 'sentinel';}};
var out=[f(v),f('22'),f(true),f(null),String(f(undefined))];
try{f(bad);}catch(e){out.push(e);}
try{f(10n);}catch(e){out.push(e.name);}
try{f(Symbol('x'));}catch(e){out.push(e.name);}
out.join('|')+':'+log.join(',')
"#,"1|1|1|0|NaN|sentinel|TypeError|TypeError:v,bad");
}

#[cfg(target_arch="x86_64")]
fn eval(realm:&mut crate::Realm, source:&str)->String {
    let value=realm.eval(source,"modrk-native.js").unwrap();
    realm.to_string(value).unwrap().to_rust_string()
}

#[cfg(target_arch="x86_64")]
fn explicit_slots(x:f64,divisor:f64,held:crate::Value,destination:u16,preserve_source:bool)->std::rc::Rc<crate::bytecode::FunctionCode> {
    use crate::bytecode::Op;
    let program=crate::parse_program("0",false).unwrap();
    let mut code=crate::compiler::compile_program(&program,"modrk-slots.js","0").unwrap();
    let c=std::rc::Rc::get_mut(&mut code).expect("fresh code is uniquely owned");
    c.constants=vec![crate::Value::Number(x),crate::Value::Number(divisor),held];
    c.nregs=4;c.max_depth=0;
    c.reg_names=vec![crate::JsStr::from("");4];
    // Force eligibility from the interpreter entry without altering its
    // threshold policy; the bytecode has no loops/calls/safe points.
    c.ops=vec![Op::Nop;32];
    c.ops.extend([Op::LoadK(0,0),Op::LoadK(1,2),Op::MovRaw(2,1),Op::ModRK(destination,0,1)]);
    if preserve_source {
        c.ops.extend([Op::AddRR(3,0,destination),Op::ReturnR(3)]);
    } else {c.ops.push(Op::ReturnR(destination));}
    c.depths=vec![0;c.ops.len()];
    c.positions=vec![Default::default();c.ops.len()];
    code
}

#[test]
#[cfg(target_arch="x86_64")]
fn modrk_native_fast_and_numeric_helper_paths_are_both_observed() {
    if !rabbithole_jit::supported(){return;}
    let prepare=r#"
function rem7(x){return x%7;}
function go(){
 var xs=[0,1,6,7,2147483647,-0,-7,1.5,Infinity,NaN,2147483648],out=[];
 for(var i=0;i<xs.length;i++){
  var v=rem7(xs[i]);out.push(v===0?(1/v===-Infinity?'-0':'+0'):String(v));
 }
 return out.join('|');
}
go();go();
"#;
    for requested in [false,true] {
        for threshold in [0,2] {
            let mut realm=crate::Realm::new();
            realm.set_jit(requested,threshold);
            let expected="+0|1|6|+0|1|-0|-0|1.5|NaN|NaN|2";
            assert_eq!(eval(&mut realm,prepare),expected);
            let before=realm.jit_entries();
            crate::jit::reset_modrk_witness();
            assert_eq!(eval(&mut realm,"go()"),expected);
            let (fast,helper)=crate::jit::modrk_witness();
            if requested {
                assert!(realm.jit_entries()>before,"the measured call entered native code");
                assert_eq!((fast,helper),(5,6),"five admitted RK values, six actual pure-helper calls");
            } else {
                assert_eq!(realm.jit_entries(),before);
                assert_eq!((fast,helper),(0,0));
            }
            eprintln!("native-canary modrk target={}-{} requested_override={requested} threshold={threshold} entries={} fast={fast} helper={helper}",std::env::consts::OS,std::env::consts::ARCH,realm.jit_entries()-before);
        }
    }
}

#[test]
#[cfg(target_arch="x86_64")]
fn modrk_explicit_slot_aliases_and_shared_counted_destinations_survive() {
    use crate::bytecode::Op;
    if !rabbithole_jit::supported(){return;}
    for requested in [false,true] {
        for destination in [0u16,1] {
            for held_source in ["'counted remainder destination'","123456789012345678901234567890n"] {
                let mut realm=crate::Realm::new();
                realm.set_jit(requested,0);
                let held=realm.eval(held_source,"held.js").unwrap();
                let expected_held=realm.to_string(held.clone()).unwrap().to_rust_string();
                // For d!=a, returning source+result catches a corrupted
                // source slot: 22+1 must not become 1+1.
                let code=explicit_slots(22.,7.,held.clone(),destination,destination!=0);
                assert!(code.ops.iter().any(|op|matches!(op,Op::ModRK(d,0,1) if *d==destination)));
                code.set_realm(realm.current_realm);
                crate::jit::reset_modrk_witness();
                let before=realm.jit_entries();
                let result=realm.run_script(code.clone()).unwrap();
                assert_eq!(result.as_number(),Some(if destination==0 {1.} else {23.}));
                if requested {
                    assert!(realm.jit_entries()>before);
                    assert_eq!(crate::jit::modrk_witness(),(1,0));
                } else {assert_eq!(crate::jit::modrk_witness(),(0,0));}
                assert_eq!(realm.to_string(held.clone()).unwrap().to_rust_string(),expected_held);
                assert_eq!(realm.to_string(code.constants[2].clone()).unwrap().to_rust_string(),expected_held);
                drop(code);
                realm.gc();
                assert_eq!(realm.to_string(held).unwrap().to_rust_string(),expected_held);
            }
        }
    }
}

#[test]
#[cfg(target_arch="x86_64")]
fn modrk_excluded_divisors_reach_the_numeric_helper_in_native_code() {
    if !rabbithole_jit::supported(){return;}
    for requested in [false,true] {
        for (divisor,expected) in [(0.,f64::NAN),(-0.,f64::NAN),(-7.,1.),(-1.,0.),
            (2.5,2.),(2147483648.,22.),(f64::INFINITY,22.),(f64::NEG_INFINITY,22.),(f64::NAN,f64::NAN)] {
            let mut realm=crate::Realm::new();
            realm.set_jit(requested,0);
            let code=explicit_slots(22.,divisor,crate::Value::Undefined,0,false);
            code.set_realm(realm.current_realm);
            let before=realm.jit_entries();
            crate::jit::reset_modrk_witness();
            let result=realm.run_script(code).unwrap().as_number().expect("Number result");
            if expected.is_nan(){assert!(result.is_nan());}
            else {assert_eq!(result.to_bits(),expected.to_bits());}
            assert_eq!(crate::jit::modrk_witness(),(0,u64::from(requested)),"divisor {divisor}");
            if requested {assert!(realm.jit_entries()>before);}
        }
    }
}

#[test]
#[cfg(target_arch="x86_64")]
fn modrk_object_conversion_can_collect_and_return_to_the_same_frame() {
    fn collect(realm:&mut crate::Realm,_this:crate::Value,_args:&[crate::Value])->crate::JsResult<crate::Value> {
        realm.gc();
        Ok(crate::Value::Undefined)
    }
    for requested in [false,true] {
        for threshold in [0,2] {
            let mut realm=crate::Realm::new();
            realm.set_jit(requested,threshold);
            let global=realm.global;
            realm.define_method(global,"collectForModRK",0,collect);
            realm.mark_gc_safe(global,"collectForModRK");
            let source=r#"
function rem7(x){return x%7;}
function go(){
 let keep={tag:'kept'},out=[];
 let value={valueOf(){out.push(keep.tag);collectForModRK();return 22;}};
 let result=rem7(value);
 try{rem7(Symbol('bad'));}catch(e){out.push(e.name);}
 out.push(keep.tag);
 return result+':'+out.join(',');
}
go();go();
"#;
            assert_eq!(eval(&mut realm,source),"1:kept,TypeError,kept");
            let before=realm.heap.collections;
            assert_eq!(eval(&mut realm,"go()"),"1:kept,TypeError,kept");
            assert!(realm.heap.collections>before,"the coercion actually completed a collection");
        }
    }
}
