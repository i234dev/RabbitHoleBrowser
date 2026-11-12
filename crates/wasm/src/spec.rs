//! Runner for the WebAssembly specification test suite (`.wast` scripts).

use crate::runtime::*;
use crate::text::wast::{parse_script, Action, Cmd, ModuleDecl};
use crate::text::{num, Sx};
use crate::types::*;
use crate::CompiledModule;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Default, Debug, Clone)]
pub struct Stats {
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
    pub failures: Vec<String>,
}

impl Stats {
    pub fn total(&self) -> u32 {
        self.passed + self.failed
    }
    pub fn add(&mut self, o: &Stats) {
        self.passed += o.passed;
        self.failed += o.failed;
        self.skipped += o.skipped;
    }
}

struct SpecHost {
    deadline: std::time::Instant,
}

impl SpecHost {
    fn new() -> SpecHost {
        SpecHost { deadline: std::time::Instant::now() + std::time::Duration::from_secs(20) }
    }
}

impl Host for SpecHost {
    fn call_host(&mut self, _store: &Store, _func: &Rc<FuncInst>, _args: &[Val]) -> Result<Vec<Val>, Trap> {
        Ok(vec![])
    }
    fn check_interrupt(&mut self) -> bool {
        std::time::Instant::now() > self.deadline
    }
}

type Exports = HashMap<String, Extern>;

pub struct Runner {
    store: Store,
    instances: HashMap<String, Rc<Instance>>,
    defs: HashMap<String, Rc<CompiledModule>>,
    last: Option<Rc<Instance>>,
    registry: HashMap<String, Exports>,
}

fn spectest(store: &Store) -> Exports {
    let mut e = Exports::new();
    let f = |ps: Vec<ValType>| Extern::Func(store.host_func(FuncType::new(ps, vec![]), Rc::new(())));
    e.insert("print".into(), f(vec![]));
    e.insert("print_i32".into(), f(vec![ValType::I32]));
    e.insert("print_i64".into(), f(vec![ValType::I64]));
    e.insert("print_f32".into(), f(vec![ValType::F32]));
    e.insert("print_f64".into(), f(vec![ValType::F64]));
    e.insert("print_i32_f32".into(), f(vec![ValType::I32, ValType::F32]));
    e.insert("print_f64_f64".into(), f(vec![ValType::F64, ValType::F64]));
    let g = |t, v| Extern::Global(store.new_global(GlobalType { ty: t, mutable: false }, v));
    e.insert("global_i32".into(), g(ValType::I32, Val::I32(666)));
    e.insert("global_i64".into(), g(ValType::I64, Val::I64(666)));
    e.insert("global_f32".into(), g(ValType::F32, Val::F32(666.6f32.to_bits())));
    e.insert("global_f64".into(), g(ValType::F64, Val::F64(666.6f64.to_bits())));
    let t = store.new_table(TableType { elem: RefType::FUNCREF, limits: Limits { min: 10, max: Some(20) }, table64: false }, 0).unwrap();
    e.insert("table".into(), Extern::Table(t));
    let t64 = store.new_table(TableType { elem: RefType::FUNCREF, limits: Limits { min: 10, max: Some(20) }, table64: true }, 0).unwrap();
    e.insert("table64".into(), Extern::Table(t64));
    let m = store.new_memory(MemoryType { limits: Limits { min: 1, max: Some(2) }, shared: false, memory64: false, page_log2: 16 }).unwrap();
    e.insert("memory".into(), Extern::Memory(m));
    let sm = store.new_memory(MemoryType { limits: Limits { min: 1, max: Some(2) }, shared: true, memory64: false, page_log2: 16 }).unwrap();
    e.insert("shared_memory".into(), Extern::Memory(sm));
    e
}

fn parse_val(sx: &Sx) -> Option<Val> {
    let l = sx.list()?;
    let a = l.get(1).and_then(|s| s.atom());
    Some(match sx.head()? {
        "i32.const" => Val::I32(num::parse_int(a?, 32)? as u32 as i32),
        "i64.const" => Val::I64(num::parse_int(a?, 64)? as i64),
        "f32.const" => Val::F32(num::parse_f32(a?)?),
        "f64.const" => Val::F64(num::parse_f64(a?)?),
        "v128.const" => {
            let mut i = 1;
            Val::V128(crate::text::wat::v128_const(l, &mut i).ok()?)
        }
        "ref.null" => Val::Ref(0),
        "ref.extern" => Val::Ref(num::parse_u64(a?)? + 1),
        "ref.host" => Val::Ref((num::parse_u64(a?)? + 1) << 2 | 2),
        _ => return None,
    })
}

fn nan_match(pat: &str, bits: u64, is32: bool) -> Option<bool> {
    let (exp, quiet, mant_mask) = if is32 { (0x7f80_0000u64, 0x0040_0000u64, 0x007f_ffffu64) } else { (0x7ff0_0000_0000_0000, 0x0008_0000_0000_0000, 0x000f_ffff_ffff_ffff) };
    let is_nan = bits & exp == exp && bits & mant_mask != 0;
    match pat {
        "nan:canonical" => Some(is_nan && bits & mant_mask == quiet),
        "nan:arithmetic" => Some(is_nan && bits & quiet != 0),
        _ => None,
    }
}

fn match_result(store: &Store, sx: &Sx, v: &Val) -> bool {
    if sx.head() == Some("either") {
        return sx.list().unwrap()[1..].iter().any(|s| match_result(store, s, v));
    }
    let Some(l) = sx.list() else { return false };
    let a = l.get(1).and_then(|s| s.atom());
    match (sx.head(), v) {
        (Some("f32.const"), Val::F32(b)) => {
            if let Some(m) = a.and_then(|a| nan_match(a, *b as u64, true)) {
                return m;
            }
            a.and_then(num::parse_f32) == Some(*b)
        }
        (Some("f64.const"), Val::F64(b)) => {
            if let Some(m) = a.and_then(|a| nan_match(a, *b, false)) {
                return m;
            }
            a.and_then(num::parse_f64) == Some(*b)
        }
        (Some("v128.const"), Val::V128(x)) => {
            let shape = a.unwrap_or("");
            let (n, bits) = match shape {
                "f32x4" => (4, 32),
                "f64x2" => (2, 64),
                _ => {
                    let mut i = 1;
                    return crate::text::wat::v128_const(l, &mut i).ok() == Some(*x);
                }
            };
            for k in 0..n {
                let lane = (*x >> (k * bits)) as u64 & if bits == 64 { u64::MAX } else { 0xffff_ffff };
                let Some(p) = l.get(2 + k).and_then(|s| s.atom()) else { return false };
                let ok = match nan_match(p, lane, bits == 32) {
                    Some(m) => m,
                    None => {
                        if bits == 32 {
                            num::parse_f32(p).map(|v| v as u64) == Some(lane)
                        } else {
                            num::parse_f64(p) == Some(lane)
                        }
                    }
                };
                if !ok {
                    return false;
                }
            }
            true
        }
        (Some("ref.null"), Val::Ref(r)) => *r == 0,
        (Some("ref.extern"), Val::Ref(r)) => match a {
            Some(a) => num::parse_u64(a).map(|n| n + 1) == Some(*r),
            None => *r != 0,
        },
        (Some("ref.host"), Val::Ref(r)) => match a {
            Some(a) => num::parse_u64(a).map(|n| (n + 1) << 2 | 2) == Some(*r),
            None => *r != 0,
        },
        (Some("ref.i31"), Val::Ref(r)) => *r & 1 == 1,
        (Some("ref.struct" | "ref.array"), Val::Ref(r)) => match crate::gc::obj_index(*r) {
            Some(i) => store.gc.borrow()[i].is_array == (sx.head() == Some("ref.array")),
            None => false,
        },
        (Some("ref.eq"), Val::Ref(r)) => *r & 1 == 1 || crate::gc::obj_index(*r).is_some(),
        (Some("ref.func"), Val::Ref(r)) => *r != 0 && (*r as usize) <= store.funcs.borrow().len(),
        (Some("ref.any" | "ref.exn"), Val::Ref(r)) => *r != 0,
        _ => match parse_val(sx) {
            Some(e) => e == *v,
            None => false,
        },
    }
}

fn describe(d: &ModuleDecl) -> String {
    let s = match &d.src {
        crate::text::wast::ModuleSrc::Quote(q) => q.clone(),
        crate::text::wast::ModuleSrc::Text(t) => format!("{t:?}"),
        crate::text::wast::ModuleSrc::Binary(b) => format!("binary {:?}", &b[..b.len().min(64)]),
    };
    s.chars().take(300).collect()
}

enum Outcome {
    Values(Vec<Val>),
    Trap(Trap),
    Error(String),
}

impl Default for Runner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner {
    pub fn new() -> Runner {
        let mut store = Store::new();
        store.memory_limit = 1 << 30;
        store.table_limit = 4_000_000;
        store.max_depth = 20_000;
        store.max_stack_slots = 2 << 20;
        let mut registry = HashMap::new();
        registry.insert("spectest".to_string(), spectest(&store));
        Runner { store, instances: HashMap::new(), defs: HashMap::new(), last: None, registry }
    }

    fn compile(&self, d: &ModuleDecl) -> Result<Rc<CompiledModule>, String> {
        let bin = d.to_binary().map_err(|e| e.to_string())?;
        CompiledModule::new(&bin).map(Rc::new).map_err(|e| e.to_string())
    }

    fn resolve_imports(&self, cm: &CompiledModule) -> Result<Vec<Extern>, String> {
        let mut out = Vec::new();
        for imp in &cm.module.imports {
            let e = self.registry.get(&imp.module).and_then(|m| m.get(&imp.name)).ok_or_else(|| format!("unknown import {}.{}", imp.module, imp.name))?;
            out.push(e.clone());
        }
        Ok(out)
    }

    fn instantiate(&mut self, cm: &Rc<CompiledModule>) -> Result<Rc<Instance>, Result<Trap, String>> {
        let imports = self.resolve_imports(cm).map_err(Err)?;
        match self.store.instantiate(&mut SpecHost::new(), cm, &imports) {
            Ok(i) => Ok(i),
            Err(InstantiateError::Link(s)) => Err(Err(s)),
            Err(InstantiateError::Trap(t)) => Err(Ok(t)),
        }
    }

    fn instance(&self, id: &Option<String>) -> Option<Rc<Instance>> {
        match id {
            Some(n) => self.instances.get(n).cloned(),
            None => self.last.clone(),
        }
    }

    fn act(&mut self, a: &Action) -> Outcome {
        match a {
            Action::Invoke { module, name, args } => {
                let Some(inst) = self.instance(module) else { return Outcome::Error("no module".into()) };
                let Some(Extern::Func(f)) = inst.export(name).cloned() else { return Outcome::Error(format!("no function export {name}")) };
                let mut vals = Vec::new();
                for a in args {
                    match parse_val(a) {
                        Some(v) => vals.push(v),
                        None => return Outcome::Error(format!("bad argument {a:?}")),
                    }
                }
                match self.store.call(&mut SpecHost::new(), &f, &vals) {
                    Ok(v) => Outcome::Values(v),
                    Err(t) => Outcome::Trap(t),
                }
            }
            Action::Get { module, name } => {
                let Some(inst) = self.instance(module) else { return Outcome::Error("no module".into()) };
                match inst.export(name) {
                    Some(Extern::Global(g)) => Outcome::Values(vec![g.get()]),
                    _ => Outcome::Error(format!("no global export {name}")),
                }
            }
        }
    }

    /// Run one command; returns None if not counted, Some(Ok) for a pass,
    /// Some(Err(reason)) for a failure.
    fn cmd(&mut self, c: &Cmd) -> Option<Result<(), String>> {
        match c {
            Cmd::Module(d) => {
                let cm = match self.compile(d) {
                    Ok(m) => m,
                    Err(e) => return Some(Err(format!("module failed to compile: {e}"))),
                };
                if d.definition {
                    if let Some(id) = &d.id {
                        self.defs.insert(id.clone(), cm);
                    }
                    return Some(Ok(()));
                }
                match self.instantiate(&cm) {
                    Ok(inst) => {
                        if let Some(id) = &d.id {
                            self.instances.insert(id.clone(), inst.clone());
                        }
                        self.last = Some(inst);
                        Some(Ok(()))
                    }
                    Err(e) => {
                        self.last = None;
                        Some(Err(format!("module failed to instantiate: {:?}", e)))
                    }
                }
            }
            Cmd::Instance { id, def } => {
                let Some(cm) = def.as_ref().and_then(|d| self.defs.get(d)).cloned() else { return Some(Err("unknown module definition".into())) };
                match self.instantiate(&cm) {
                    Ok(inst) => {
                        if let Some(id) = id {
                            self.instances.insert(id.clone(), inst.clone());
                        }
                        self.last = Some(inst);
                        Some(Ok(()))
                    }
                    Err(e) => Some(Err(format!("instance failed: {e:?}"))),
                }
            }
            Cmd::Register { name, module } => {
                if let Some(inst) = self.instance(module) {
                    let ex: Exports = inst.exports.iter().cloned().collect();
                    self.registry.insert(name.clone(), ex);
                }
                None
            }
            Cmd::Action(a) => match self.act(a) {
                Outcome::Values(_) => None,
                Outcome::Trap(t) => Some(Err(format!("unexpected trap {t:?}"))),
                Outcome::Error(e) => Some(Err(e)),
            },
            Cmd::AssertReturn(a, expected) => match self.act(a) {
                Outcome::Values(v) => {
                    if v.len() == expected.len() && v.iter().zip(expected.iter()).all(|(v, e)| match_result(&self.store, e, v)) {
                        Some(Ok(()))
                    } else {
                        Some(Err(format!("{a:?}: got {v:?}, expected {expected:?}")))
                    }
                }
                Outcome::Trap(t) => Some(Err(format!("{a:?}: trap {t:?}"))),
                Outcome::Error(e) => Some(Err(e)),
            },
            Cmd::AssertTrap(a, m) | Cmd::AssertExhaustion(a, m) => match self.act(a) {
                Outcome::Trap(Trap::Exception(_)) => Some(Err(format!("{a:?}: exception instead of trap {m}"))),
                Outcome::Trap(_) => Some(Ok(())),
                Outcome::Values(v) => Some(Err(format!("{a:?}: expected trap {m}, got {v:?}"))),
                Outcome::Error(e) => Some(Err(e)),
            },
            Cmd::AssertException(a) => match self.act(a) {
                Outcome::Trap(Trap::Exception(_)) => Some(Ok(())),
                Outcome::Trap(t) => Some(Err(format!("{a:?}: expected exception, got trap {t:?}"))),
                Outcome::Values(v) => Some(Err(format!("{a:?}: expected exception, got {v:?}"))),
                Outcome::Error(e) => Some(Err(e)),
            },
            Cmd::AssertTrapModule(d, m) => {
                let cm = match self.compile(d) {
                    Ok(m) => m,
                    Err(e) => return Some(Err(format!("module failed to compile: {e}"))),
                };
                match self.instantiate(&cm) {
                    Err(Ok(_)) => Some(Ok(())),
                    Err(Err(e)) => Some(Err(format!("link error instead of trap {m}: {e}"))),
                    Ok(_) => Some(Err(format!("expected trap {m}"))),
                }
            }
            Cmd::AssertInvalid(d, m) | Cmd::AssertMalformed(d, m) => match self.compile(d) {
                Err(_) => Some(Ok(())),
                Ok(_) => Some(Err(format!("module should fail: {m}: {}", describe(d)))),
            },
            Cmd::AssertUnlinkable(d, m) => {
                let cm = match self.compile(d) {
                    Ok(m) => m,
                    Err(e) => return Some(Err(format!("module failed to compile: {e}"))),
                };
                match self.instantiate(&cm) {
                    Err(_) => Some(Ok(())),
                    Ok(_) => Some(Err(format!("expected link failure {m}"))),
                }
            }
            Cmd::Other(_) => None,
        }
    }
}

/// Run a script; every assertion and module counts as a test.
pub fn run_script(text: &str) -> Stats {
    let mut st = Stats::default();
    let cmds = match parse_script(text) {
        Ok(c) => c,
        Err(e) => {
            st.failed += 1;
            st.failures.push(format!("script parse error: {e}"));
            return st;
        }
    };
    let mut r = Runner::new();
    for c in &cmds {
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.cmd(c)));
        match res {
            Ok(None) => {}
            Ok(Some(Ok(()))) => st.passed += 1,
            Ok(Some(Err(e))) => {
                st.failed += 1;
                if st.failures.len() < 50 {
                    st.failures.push(e);
                }
            }
            Err(_) => {
                st.failed += 1;
                st.failures.push(format!("panic in {c:?}").chars().take(300).collect());
            }
        }
    }
    st
}
