//! Function body validation and translation to the register IR (see
//! [`crate::ir`]) in a single pass.
//!
//! The validator keeps the usual operand and control stacks; every operand
//! also records *where* its value lives: its canonical stack slot, a local
//! (`local.get` is lazy: no copy until the local is overwritten or the value
//! must sit at a fixed place, e.g. as a call argument or branch value) or a
//! constant slot. Consumers read sources from wherever they are.

use crate::binary::{malformed, Reader, Result};
use crate::ir::*;
use crate::module::*;
use crate::types::*;
use crate::validate::{check_heap, check_val, invalid, TypeCtx};
use std::collections::{HashMap, HashSet};

pub struct ModuleCtx<'a> {
    pub m: &'a Module,
    pub tc: &'a TypeCtx<'a>,
    pub refs: &'a HashSet<u32>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Loc {
    Stack,
    Local(u32),
    Const(u32),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Operand {
    /// `None`: the bottom type (unreachable code).
    pub ty: Option<ValType>,
    /// Canonical slot of this stack position.
    pub slot: u32,
    pub loc: Loc,
}

impl Operand {
    #[inline]
    pub fn src(&self) -> u32 {
        match self.loc {
            Loc::Stack => self.slot,
            Loc::Local(s) | Loc::Const(s) => s,
        }
    }
    fn width(&self) -> u32 {
        self.ty.map(|t| t.slots()).unwrap_or(1)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Func,
    Block,
    Loop,
    If,
    Else,
    TryTable,
    Try,
    Catch,
    CatchAll,
}

#[derive(Clone, Copy, Debug)]
enum Patch {
    Instr(usize),
    Table(usize),
    Handler(usize),
}

pub(crate) struct Ctl {
    kind: Kind,
    params: Vec<ValType>,
    results: Vec<ValType>,
    height: usize,
    slot: u32,
    /// Validation: the rest of the block is unreachable (polymorphic stack).
    unreachable: bool,
    /// Translation: the block was entered in unreachable code (no IR).
    dead: bool,
    start: u32,
    patches: Vec<Patch>,
    else_patch: Option<usize>,
    init_height: usize,
    /// try_table: (kind, tag, absolute ctl index of the target label)
    catches: Vec<(CatchKind, u32, usize)>,
    /// legacy try: end of the protected range, pending catch handlers
    try_end: Option<u32>,
    exn_slot: u32,
}

enum Target {
    Known(u32),
    Pending(usize),
    Return,
}

pub(crate) struct FC<'a> {
    pub cx: &'a ModuleCtx<'a>,
    pub r: Reader<'a>,
    locals: Vec<ValType>,
    local_slot: Vec<u32>,
    inited: Vec<bool>,
    init_log: Vec<u32>,
    pub stack: Vec<Operand>,
    ctls: Vec<Ctl>,
    pub instrs: Vec<Instr>,
    consts: Vec<u64>,
    const_map: HashMap<u64, u32>,
    v128_map: HashMap<u128, u32>,
    const_base: u32,
    const_cap: u32,
    exn_next: u32,
    exn_end: u32,
    stack_base: u32,
    max_slot: u32,
    br_tables: Vec<u32>,
    call_types: Vec<(u32, u32)>,
    call_type_map: HashMap<u32, u32>,
    pub memargs: Vec<MemArg>,
    pub v128s: Vec<u128>,
    handlers: Vec<Handler>,
    last_pure: Option<usize>,
    results: Vec<ValType>,
}

const MAX_CONSTS: u32 = 2048;

pub fn compile_function(cx: &ModuleCtx, def_index: u32) -> Result<Code> {
    let m = cx.m;
    let func_index = m.num_imported_funcs + def_index;
    let ft = m.func_type(func_index).clone();
    let body = &m.bodies[def_index as usize];
    let bytes: &[u8] = &m.bytes;
    let mut locals: Vec<ValType> = ft.params.to_vec();
    let mut inited = vec![true; locals.len()];
    for &(n, t) in &body.locals {
        check_val(m.types.len() as u32, t)?;
        for _ in 0..n {
            locals.push(t);
            inited.push(t.defaultable());
        }
    }
    let mut local_slot = Vec::with_capacity(locals.len());
    let mut s = 0u32;
    for t in &locals {
        local_slot.push(s);
        s += t.slots();
    }
    let local_slots = s;
    // Upper bounds of constant and hidden exception slots from a byte scan
    // (every constant instruction starts with one of these bytes).
    let code = &bytes[body.code.clone()];
    let mut cbound = 0u32;
    let mut ebound = 0u32;
    for &b in code {
        match b {
            0x41..=0x44 | 0xd0 => cbound += 1,
            0xfd => cbound += 2,
            0x07 | 0x19 => ebound += 1,
            _ => {}
        }
    }
    let const_cap = cbound.min(MAX_CONSTS);
    let const_base = local_slots;
    let exn_base = const_base + const_cap;
    let stack_base = exn_base + ebound;
    let mut fc = FC {
        cx,
        r: Reader::sub(bytes, body.code.start, body.code.end),
        locals,
        local_slot,
        inited,
        init_log: Vec::new(),
        stack: Vec::new(),
        ctls: Vec::new(),
        instrs: Vec::with_capacity(code.len() / 2),
        consts: Vec::new(),
        const_map: HashMap::new(),
        v128_map: HashMap::new(),
        const_base,
        const_cap,
        exn_next: exn_base,
        exn_end: stack_base,
        stack_base,
        max_slot: stack_base,
        br_tables: Vec::new(),
        call_types: Vec::new(),
        call_type_map: HashMap::new(),
        memargs: Vec::new(),
        v128s: Vec::new(),
        handlers: Vec::new(),
        last_pure: None,
        results: ft.results.to_vec(),
    };
    fc.ctls.push(Ctl {
        kind: Kind::Func,
        params: vec![],
        results: ft.results.to_vec(),
        height: 0,
        slot: stack_base,
        unreachable: false,
        dead: false,
        start: 0,
        patches: vec![],
        else_patch: None,
        init_height: 0,
        catches: vec![],
        try_end: None,
        exn_slot: 0,
    });
    fc.run()?;
    let result_slots = ft.result_slots();
    Ok(Code {
        instrs: fc.instrs.into(),
        consts: fc.consts.into(),
        param_slots: ft.param_slots(),
        local_slots,
        const_base,
        frame_slots: fc.max_slot.max(result_slots).max(local_slots),
        result_slots,
        br_tables: fc.br_tables.into(),
        call_types: fc.call_types.into(),
        memargs: fc.memargs.into(),
        v128s: fc.v128s.into(),
        handlers: fc.handlers.into(),
        func_index,
    })
}

/// Is `op` a (single byte) opcode of some instruction?
pub fn is_known_opcode(op: u8) -> bool {
    matches!(op, 0x00..=0x15 | 0x18..=0x1c | 0x1f..=0x26 | 0x28..=0xc4 | 0xd0..=0xd6 | 0xfb..=0xfe)
}

fn mismatch<T>(what: &str) -> Result<T> {
    invalid(format!("type mismatch: {what}"))
}

impl<'a> FC<'a> {
    // ------------------------------------------------------------ helpers

    #[inline]
    pub fn live(&self) -> bool {
        self.ctls.last().map(|c| !c.unreachable && !c.dead).unwrap_or(false)
    }

    #[inline]
    pub fn emit(&mut self, i: Instr) -> usize {
        let pc = self.instrs.len();
        if self.live() {
            self.instrs.push(i);
            let s = sig(i.op);
            self.last_pure = if matches!(s, Sig::Dss | Sig::Dsi | Sig::Dii) { Some(pc) } else { None };
        }
        pc
    }

    fn bind_label(&mut self) -> u32 {
        self.last_pure = None;
        self.instrs.len() as u32
    }

    #[inline]
    pub fn top_slot(&self) -> u32 {
        match self.stack.last() {
            Some(o) => o.slot + o.width(),
            None => self.stack_base,
        }
    }

    pub fn push(&mut self, ty: ValType) -> u32 {
        let slot = self.top_slot();
        let end = slot + ty.slots();
        if end > self.max_slot {
            self.max_slot = end;
        }
        self.stack.push(Operand { ty: Some(ty), slot, loc: Loc::Stack });
        slot
    }

    fn push_opt(&mut self, ty: Option<ValType>, loc: Loc) -> u32 {
        let slot = self.top_slot();
        let end = slot + ty.map(|t| t.slots()).unwrap_or(1);
        if end > self.max_slot {
            self.max_slot = end;
        }
        self.stack.push(Operand { ty, slot, loc });
        slot
    }

    pub fn pop(&mut self) -> Result<Operand> {
        let c = self.ctls.last().unwrap();
        if self.stack.len() <= c.height {
            if c.unreachable {
                let slot = self.top_slot();
                return Ok(Operand { ty: None, slot, loc: Loc::Stack });
            }
            return mismatch("instruction requires an operand");
        }
        Ok(self.stack.pop().unwrap())
    }

    pub fn pop_expect(&mut self, t: ValType) -> Result<Operand> {
        let o = self.pop()?;
        if let Some(ot) = o.ty {
            if !self.cx.tc.val_sub(ot, t) {
                return mismatch(&format!("expected {t}, found {ot}"));
            }
        }
        Ok(o)
    }

    fn pop_ref(&mut self) -> Result<Operand> {
        let o = self.pop()?;
        match o.ty {
            None | Some(ValType::Ref(_)) => Ok(o),
            Some(t) => mismatch(&format!("expected reference, found {t}")),
        }
    }

    /// Copy operand `i` to its canonical slot if it lives elsewhere.
    pub fn materialize(&mut self, i: usize) {
        let o = self.stack[i];
        if o.loc != Loc::Stack {
            if let Some(t) = o.ty {
                let op = if t.slots() == 2 { Op::Copy2 } else { Op::Copy };
                self.emit(Instr::new(op, o.slot, o.src(), 0));
            }
            self.stack[i].loc = Loc::Stack;
        }
    }

    /// Materialize the top `n` operands (those above the current frame's
    /// height only).
    pub fn materialize_top(&mut self, n: usize) {
        let len = self.stack.len();
        let h = self.ctls.last().unwrap().height;
        for i in len.saturating_sub(n).max(h)..len {
            self.materialize(i);
        }
    }

    /// Before a local's slot is overwritten: materialize lazy reads of it.
    fn preserve_local(&mut self, slot: u32) {
        for i in 0..self.stack.len() {
            if self.stack[i].loc == Loc::Local(slot) {
                self.materialize(i);
            }
        }
    }

    fn const_slot(&mut self, bits: u64) -> Option<u32> {
        if let Some(&s) = self.const_map.get(&bits) {
            return Some(s);
        }
        if (self.consts.len() as u32) < self.const_cap {
            let s = self.const_base + self.consts.len() as u32;
            self.consts.push(bits);
            self.const_map.insert(bits, s);
            return Some(s);
        }
        None
    }

    pub fn push_const(&mut self, ty: ValType, bits: u64) {
        match self.const_slot(bits) {
            Some(s) => {
                self.push_opt(Some(ty), Loc::Const(s));
            }
            None => {
                let d = self.push(ty);
                self.emit(Instr::new(Op::Const, d, bits as u32, (bits >> 32) as u32));
            }
        }
    }

    pub fn push_v128_const(&mut self, v: u128) {
        if let Some(&s) = self.v128_map.get(&v) {
            self.push_opt(Some(ValType::V128), Loc::Const(s));
            return;
        }
        if (self.consts.len() as u32) + 2 <= self.const_cap {
            let s = self.const_base + self.consts.len() as u32;
            self.consts.push(v as u64);
            self.consts.push((v >> 64) as u64);
            self.v128_map.insert(v, s);
            self.push_opt(Some(ValType::V128), Loc::Const(s));
            return;
        }
        let d = self.push(ValType::V128);
        let idx = self.v128s.len() as u32;
        self.v128s.push(v);
        self.emit(Instr::with_x(Op::Simd, crate::simd::V128_CONST, d, idx, 0));
    }

    fn label_types(&self, depth: u32) -> Result<(Vec<ValType>, usize)> {
        let n = self.ctls.len();
        if depth as usize >= n {
            return invalid(format!("unknown label {depth}"));
        }
        let idx = n - 1 - depth as usize;
        let c = &self.ctls[idx];
        Ok((if c.kind == Kind::Loop { c.params.clone() } else { c.results.clone() }, idx))
    }

    fn target_of(&self, idx: usize) -> Target {
        let c = &self.ctls[idx];
        if c.kind == Kind::Loop {
            Target::Known(c.start)
        } else if idx == 0 {
            Target::Return
        } else {
            Target::Pending(idx)
        }
    }

    fn add_patch(&mut self, ctl: usize, p: Patch) {
        if self.live() {
            self.ctls[ctl].patches.push(p);
        }
    }

    /// Check that the top operands match `types` (without popping).
    fn check_top(&self, types: &[ValType]) -> Result<()> {
        let c = self.ctls.last().unwrap();
        let avail = self.stack.len() - c.height;
        for (k, t) in types.iter().rev().enumerate() {
            if k >= avail {
                if !c.unreachable {
                    return mismatch("not enough operands for branch");
                }
                break;
            }
            let o = &self.stack[self.stack.len() - 1 - k];
            if let Some(ot) = o.ty {
                if !self.cx.tc.val_sub(ot, *t) {
                    return mismatch(&format!("expected {t}, found {ot}"));
                }
            }
        }
        Ok(())
    }

    /// Moves needed to place the top `types.len()` operands (skipping the
    /// `skip` topmost) at the slots starting with `dst`.
    fn moves(&self, n: usize, skip: usize, dst: u32) -> Vec<(u32, u32, u32)> {
        let mut v = Vec::new();
        if !self.live() {
            return v;
        }
        let len = self.stack.len();
        if len < n + skip {
            return v;
        }
        let mut d = dst;
        for i in len - n - skip..len - skip {
            let o = self.stack[i];
            let w = o.width();
            if o.src() != d {
                v.push((d, o.src(), w));
            }
            d += w;
        }
        v
    }

    fn emit_moves(&mut self, mv: &[(u32, u32, u32)]) {
        for &(d, s, w) in mv {
            self.emit(Instr::new(if w == 2 { Op::Copy2 } else { Op::Copy }, d, s, 0));
        }
    }

    fn emit_jump(&mut self, t: &Target) {
        match *t {
            Target::Known(pc) => {
                self.emit(Instr::new(Op::Br, 0, 0, pc));
            }
            Target::Pending(idx) => {
                let pc = self.emit(Instr::new(Op::Br, 0, 0, u32::MAX));
                self.add_patch(idx, Patch::Instr(pc));
            }
            Target::Return => self.emit_return(),
        }
    }

    /// Return with the function results on top of the stack.
    fn emit_return(&mut self) {
        if !self.live() {
            return;
        }
        let n = self.results.len();
        let len = self.stack.len();
        if n == 1 {
            let o = self.stack[len - 1];
            if o.src() != 0 {
                let op = if o.width() == 2 { Op::Copy2 } else { Op::Copy };
                self.emit(Instr::new(op, 0, o.src(), 0));
            }
        } else if n > 1 {
            // Branch paths may return: do not change the operand locations
            // seen by the fall-through path.
            let saved = self.stack.clone();
            for i in len - n..len {
                self.materialize(i);
            }
            let mat = self.stack.clone();
            self.stack = saved;
            let mut d = 0;
            for o in &mat[len - n..len] {
                if o.slot != d {
                    let op = if o.width() == 2 { Op::Copy2 } else { Op::Copy };
                    self.emit(Instr::new(op, d, o.slot, 0));
                }
                d += o.width();
            }
            self.emit(Instr::new(Op::Return, 0, 0, 0));
            return;
        }
        self.emit(Instr::new(Op::Return, 0, 0, 0));
    }

    fn set_unreachable(&mut self) {
        let c = self.ctls.last_mut().unwrap();
        c.unreachable = true;
        let h = c.height;
        self.stack.truncate(h);
        self.last_pure = None;
    }

    fn push_ctl(&mut self, kind: Kind, params: Vec<ValType>, results: Vec<ValType>) -> Result<()> {
        // Check and materialize the block parameters.
        self.check_top(&params)?;
        let n = params.len();
        let h = self.ctls.last().unwrap().height;
        let avail = self.stack.len() - h;
        if avail < n {
            // Unreachable code: pad with bottom operands.
            let pad = n - avail;
            let at = self.stack.len();
            for _ in 0..pad {
                self.push_opt(None, Loc::Stack);
            }
            let _ = at;
        }
        let len = self.stack.len();
        for i in len - n..len {
            self.materialize(i);
            self.stack[i].ty = Some(params[i - (len - n)]);
        }
        // Recompute slots of the params (types may have been refined).
        let height = len - n;
        let slot = if height == 0 { self.stack_base } else { self.stack[height - 1].slot + self.stack[height - 1].width() };
        let dead = !self.live();
        let start = self.bind_label();
        self.ctls.push(Ctl {
            kind,
            params,
            results,
            height,
            slot,
            unreachable: false,
            dead,
            start,
            patches: vec![],
            else_patch: None,
            init_height: self.init_log.len(),
            catches: vec![],
            try_end: None,
            exn_slot: 0,
        });
        Ok(())
    }

    /// Pop the results of the current block (validating them) and check the
    /// stack is empty to its height.
    fn pop_results(&mut self) -> Result<()> {
        let results = self.ctls.last().unwrap().results.clone();
        // Materialize fallthrough results in place.
        if self.live() {
            let c = self.ctls.last().unwrap();
            let len = self.stack.len();
            if len - c.height == results.len() {
                for i in c.height..len {
                    self.materialize(i);
                }
            }
        }
        for t in results.iter().rev() {
            self.pop_expect(*t)?;
        }
        let c = self.ctls.last().unwrap();
        if self.stack.len() != c.height {
            return mismatch("values remaining on stack at end of block");
        }
        Ok(())
    }

    fn patch_to(&mut self, patches: Vec<Patch>, pc: u32) {
        for p in patches {
            match p {
                Patch::Instr(i) => self.instrs[i].c = pc,
                Patch::Table(i) => self.br_tables[i] = pc,
                Patch::Handler(i) => self.handlers[i].target = pc,
            }
        }
    }

    fn reset_inits(&mut self, h: usize) {
        while self.init_log.len() > h {
            let l = self.init_log.pop().unwrap();
            self.inited[l as usize] = false;
        }
    }

    fn block_type(&mut self) -> Result<(Vec<ValType>, Vec<ValType>)> {
        let b = match self.r.peek() {
            Some(b) => b,
            None => return malformed("unexpected end"),
        };
        if b == 0x40 {
            self.r.byte()?;
            return Ok((vec![], vec![]));
        }
        if matches!(b, 0x7b..=0x7f | 0x63 | 0x64 | 0x69..=0x74) {
            let t = self.r.val_type()?;
            check_val(self.cx.m.types.len() as u32, t)?;
            return Ok((vec![], vec![t]));
        }
        let idx = self.r.s33()?;
        if idx < 0 {
            return malformed("malformed block type");
        }
        match self.cx.tc.func_type(idx as u32) {
            Some(f) => Ok((f.params.to_vec(), f.results.to_vec())),
            None => invalid(format!("unknown type {idx}")),
        }
    }

    fn local(&mut self) -> Result<(u32, ValType, u32)> {
        let i = self.r.u32()?;
        match self.locals.get(i as usize) {
            Some(&t) => Ok((i, t, self.local_slot[i as usize])),
            None => invalid(format!("unknown local {i}")),
        }
    }

    pub fn memarg(&mut self, natural: u32) -> Result<(u32, u64)> {
        let mut align = self.r.u32()?;
        let mut mem = 0;
        if align & 0x40 != 0 {
            align &= !0x40;
            mem = self.r.u32()?;
        }
        if align >= 64 {
            return malformed("malformed memop flags");
        }
        let offset = self.r.u64()?;
        let Some(mt) = self.cx.m.memories.get(mem as usize) else {
            return invalid(format!("unknown memory {mem}"));
        };
        if !mt.memory64 && offset > u32::MAX as u64 {
            return malformed("offset out of range");
        }
        if (1u64 << align) > natural as u64 {
            return invalid("alignment must not be larger than natural");
        }
        Ok((mem, offset))
    }

    pub fn addr_type(&self, mem: u32) -> ValType {
        if self.cx.m.memories[mem as usize].memory64 {
            ValType::I64
        } else {
            ValType::I32
        }
    }

    fn table_addr(&self, t: u32) -> ValType {
        if self.cx.m.tables[t as usize].table64 {
            ValType::I64
        } else {
            ValType::I32
        }
    }

    fn mem_fast(&self, mem: u32, offset: u64) -> bool {
        mem == 0 && !self.cx.m.memories[0].memory64 && offset <= u32::MAX as u64
    }

    pub fn memarg_index(&mut self, mem: u32, offset: u64) -> u32 {
        self.memargs.push(MemArg { memory: mem, offset });
        (self.memargs.len() - 1) as u32
    }

    fn load(&mut self, op: u8, ty: ValType, natural: u32, fast: Op) -> Result<()> {
        let (mem, offset) = self.memarg(natural)?;
        let at = self.addr_type(mem);
        let a = self.pop_expect(at)?;
        let d = self.push(ty);
        if self.mem_fast(mem, offset) {
            self.emit(Instr::new(fast, d, a.src(), offset as u32));
        } else {
            let mi = self.memarg_index(mem, offset);
            self.emit(Instr::with_x(Op::MemLoad, op as u16, d, a.src(), mi));
        }
        Ok(())
    }

    fn store(&mut self, op: u8, ty: ValType, natural: u32, fast: Op) -> Result<()> {
        let (mem, offset) = self.memarg(natural)?;
        let at = self.addr_type(mem);
        let v = self.pop_expect(ty)?;
        let a = self.pop_expect(at)?;
        if self.mem_fast(mem, offset) {
            self.emit(Instr::new(fast, a.src(), v.src(), offset as u32));
        } else {
            let mi = self.memarg_index(mem, offset);
            self.emit(Instr::with_x(Op::MemStore, op as u16, a.src(), v.src(), mi));
        }
        Ok(())
    }

    pub fn unop(&mut self, tin: ValType, tout: ValType, op: Op) -> Result<()> {
        let a = self.pop_expect(tin)?;
        let d = self.push(tout);
        self.emit(Instr::new(op, d, a.src(), 0));
        Ok(())
    }

    pub fn binop(&mut self, t: ValType, tout: ValType, op: Op) -> Result<()> {
        let b = self.pop_expect(t)?;
        let a = self.pop_expect(t)?;
        let d = self.push(tout);
        self.emit(Instr::new(op, d, a.src(), b.src()));
        Ok(())
    }

    fn call_type_index(&mut self, ty: u32) -> u32 {
        if let Some(&i) = self.call_type_map.get(&ty) {
            return i;
        }
        let ft = self.cx.tc.func_type(ty).unwrap();
        let ps = ft.param_slots();
        let canon = self.cx.m.canon[ty as usize];
        self.call_types.push((canon, ps));
        let i = (self.call_types.len() - 1) as u32;
        self.call_type_map.insert(ty, i);
        i
    }

    /// Pop call arguments (materialized), returning the base slot.
    fn call_args(&mut self, params: &[ValType], extra: usize) -> Result<u32> {
        // Validate types (args below `extra` operands already popped).
        let _ = extra;
        self.check_top(params)?;
        let n = params.len();
        self.materialize_top(n);
        for t in params.iter().rev() {
            self.pop_expect(*t)?;
        }
        Ok(self.top_slot())
    }

    fn check_return_types(&self, results: &[ValType]) -> Result<()> {
        if results.len() != self.results.len() || !results.iter().zip(self.results.iter()).all(|(a, b)| self.cx.tc.val_sub(*a, *b)) {
            return mismatch("tail call result type");
        }
        Ok(())
    }

    // ------------------------------------------------------------ main loop

    fn run(&mut self) -> Result<()> {
        loop {
            if self.r.eof() {
                return malformed("unexpected end of section or function");
            }
            let op = self.r.byte()?;
            match op {
                0x00 => {
                    self.emit(Instr::new(Op::Unreachable, 0, 0, 0));
                    self.set_unreachable();
                }
                0x01 => {}
                0x02 | 0x03 => {
                    let (p, r) = self.block_type()?;
                    self.push_ctl(if op == 0x02 { Kind::Block } else { Kind::Loop }, p, r)?;
                }
                0x04 => {
                    let (p, r) = self.block_type()?;
                    let c = self.pop_expect(ValType::I32)?;
                    self.check_top(&p)?;
                    self.materialize_top(p.len());
                    let pc = self.cond_branch(c, false, u32::MAX);
                    self.push_ctl(Kind::If, p, r)?;
                    self.ctls.last_mut().unwrap().else_patch = pc;
                }
                0x05 => {
                    let k = self.ctls.last().unwrap().kind;
                    if k != Kind::If {
                        return malformed("else without if");
                    }
                    self.pop_results()?;
                    let idx = self.ctls.len() - 1;
                    if self.live() {
                        let pc = self.emit(Instr::new(Op::Br, 0, 0, u32::MAX));
                        self.ctls[idx].patches.push(Patch::Instr(pc));
                    }
                    let here = self.bind_label();
                    let c = &mut self.ctls[idx];
                    let ep = c.else_patch.take();
                    c.kind = Kind::Else;
                    c.unreachable = false;
                    let params = c.params.clone();
                    let ih = c.init_height;
                    if let Some(ep) = ep {
                        self.instrs[ep].c = here;
                    }
                    self.reset_inits(ih);
                    for t in params {
                        self.push(t);
                    }
                }
                0x0b => {
                    if self.end()? {
                        if !self.r.eof() {
                            return malformed("operators remaining after end of function");
                        }
                        return Ok(());
                    }
                }
                0x0c => {
                    let l = self.r.u32()?;
                    let (types, idx) = self.label_types(l)?;
                    self.check_top(&types)?;
                    let t = self.target_of(idx);
                    if !matches!(t, Target::Return) {
                        let mv = self.moves(types.len(), 0, self.ctls[idx].slot);
                        self.emit_moves(&mv);
                    }
                    self.emit_jump(&t);
                    self.set_unreachable();
                }
                0x0d => {
                    let l = self.r.u32()?;
                    let c = self.pop_expect(ValType::I32)?;
                    let (types, idx) = self.label_types(l)?;
                    self.check_top(&types)?;
                    self.br_if(c, &types, idx)?;
                    let mut ops = Vec::with_capacity(types.len());
                    for t in types.iter().rev() {
                        ops.push(self.pop_expect(*t)?);
                    }
                    for (o, t) in ops.into_iter().rev().zip(types) {
                        self.push_opt(Some(t), o.loc);
                    }
                }
                0x0e => self.br_table()?,
                0x0f => {
                    let rs = self.results.clone();
                    self.check_top(&rs)?;
                    self.emit_return();
                    self.set_unreachable();
                }
                0x10 => {
                    let f = self.r.u32()?;
                    let Some(&ty) = self.cx.m.funcs.get(f as usize) else {
                        return invalid(format!("unknown function {f}"));
                    };
                    let ft = self.cx.tc.func_type(ty).unwrap().clone();
                    let base = self.call_args(&ft.params, 0)?;
                    self.emit(Instr::new(Op::Call, base, f, 0));
                    for &t in ft.results.iter() {
                        self.push(t);
                    }
                }
                0x11 | 0x13 => {
                    let ty = self.r.u32()?;
                    let table = self.r.u32()?;
                    let Some(tt) = self.cx.m.tables.get(table as usize) else {
                        return invalid(format!("unknown table {table}"));
                    };
                    if !self.cx.tc.ref_sub(tt.elem, RefType::FUNCREF) {
                        return mismatch("call_indirect table must be funcref");
                    }
                    let Some(ft) = self.cx.tc.func_type(ty).cloned() else {
                        return invalid(format!("unknown type {ty}"));
                    };
                    let at = self.table_addr(table);
                    // index on top of the args
                    self.check_top(&[at])?;
                    let mut all: Vec<ValType> = ft.params.to_vec();
                    all.push(at);
                    let base = self.call_args(&all, 0)?;
                    let ct = self.call_type_index(ty);
                    if op == 0x11 {
                        self.emit(Instr::new(Op::CallIndirect, base, table, ct));
                        for &t in ft.results.iter() {
                            self.push(t);
                        }
                    } else {
                        self.check_return_types(&ft.results)?;
                        self.emit(Instr::new(Op::ReturnCallIndirect, base, table, ct));
                        self.set_unreachable();
                    }
                }
                0x12 => {
                    let f = self.r.u32()?;
                    let Some(&ty) = self.cx.m.funcs.get(f as usize) else {
                        return invalid(format!("unknown function {f}"));
                    };
                    let ft = self.cx.tc.func_type(ty).unwrap().clone();
                    self.check_return_types(&ft.results)?;
                    let base = self.call_args(&ft.params, 0)?;
                    self.emit(Instr::new(Op::ReturnCall, base, f, 0));
                    self.set_unreachable();
                }
                0x14 | 0x15 => {
                    let ty = self.r.u32()?;
                    let Some(ft) = self.cx.tc.func_type(ty).cloned() else {
                        return invalid(format!("unknown type {ty}"));
                    };
                    let rt = ValType::Ref(RefType { nullable: true, heap: HeapType::Concrete(ty) });
                    let mut all: Vec<ValType> = ft.params.to_vec();
                    all.push(rt);
                    let base = self.call_args(&all, 0)?;
                    let ct = self.call_type_index(ty);
                    if op == 0x14 {
                        self.emit(Instr::new(Op::CallRef, base, 0, ct));
                        for &t in ft.results.iter() {
                            self.push(t);
                        }
                    } else {
                        self.check_return_types(&ft.results)?;
                        self.emit(Instr::new(Op::ReturnCallRef, base, 0, ct));
                        self.set_unreachable();
                    }
                }
                0x06 => {
                    let (p, r) = self.block_type()?;
                    self.push_ctl(Kind::Try, p, r)?;
                }
                0x07 | 0x19 => self.legacy_catch(op == 0x19)?,
                0x18 => self.legacy_delegate()?,
                0x08 => {
                    let tag = self.r.u32()?;
                    let Some(&tt) = self.cx.m.tags.get(tag as usize) else {
                        return invalid(format!("unknown tag {tag}"));
                    };
                    let ft = self.cx.tc.func_type(tt).unwrap().clone();
                    let base = self.call_args(&ft.params, 0)?;
                    self.emit(Instr::new(Op::Throw, base, tag, 0));
                    self.set_unreachable();
                }
                0x09 => {
                    let l = self.r.u32()?;
                    let n = self.ctls.len();
                    if l as usize >= n {
                        return invalid(format!("unknown label {l}"));
                    }
                    let c = &self.ctls[n - 1 - l as usize];
                    if !matches!(c.kind, Kind::Catch | Kind::CatchAll) {
                        return invalid("invalid rethrow label");
                    }
                    let s = c.exn_slot;
                    self.emit(Instr::new(Op::Rethrow, s, 0, 0));
                    self.set_unreachable();
                }
                0x0a => {
                    let o = self.pop_expect(ValType::Ref(RefType::EXNREF))?;
                    self.emit(Instr::new(Op::ThrowRef, o.src(), 0, 0));
                    self.set_unreachable();
                }
                0x1f => self.try_table()?,
                0x1a => {
                    self.pop()?;
                }
                0x1b => {
                    let c = self.pop_expect(ValType::I32)?;
                    let b = self.pop()?;
                    let a = self.pop()?;
                    let t = match (a.ty, b.ty) {
                        (Some(x), Some(y)) => {
                            if x != y || x.is_ref() {
                                return mismatch("select operands");
                            }
                            Some(x)
                        }
                        (Some(x), None) | (None, Some(x)) => {
                            if x.is_ref() {
                                return mismatch("select operands");
                            }
                            Some(x)
                        }
                        (None, None) => None,
                    };
                    self.select(a, b, c, t);
                }
                0x1c => {
                    let n = self.r.u32()?;
                    if n != 1 {
                        return invalid("invalid result arity");
                    }
                    let t = self.r.val_type()?;
                    check_val(self.cx.m.types.len() as u32, t)?;
                    let c = self.pop_expect(ValType::I32)?;
                    let b = self.pop_expect(t)?;
                    let a = self.pop_expect(t)?;
                    self.select(a, b, c, Some(t));
                }
                0x20 => {
                    let (i, t, s) = self.local()?;
                    if !self.inited[i as usize] {
                        return invalid("uninitialized local");
                    }
                    self.push_opt(Some(t), Loc::Local(s));
                }
                0x21 | 0x22 => {
                    let (i, t, s) = self.local()?;
                    let v = self.pop_expect(t)?;
                    if !self.inited[i as usize] {
                        self.inited[i as usize] = true;
                        self.init_log.push(i);
                    }
                    self.preserve_local(s);
                    let rewritten = self.try_retarget(&v, s);
                    if !rewritten && v.src() != s {
                        let op = if t.slots() == 2 { Op::Copy2 } else { Op::Copy };
                        self.emit(Instr::new(op, s, v.src(), 0));
                    }
                    if rewritten {
                        self.last_pure = None;
                    }
                    if op == 0x22 {
                        self.push_opt(Some(t), Loc::Local(s));
                    }
                }
                0x23 => {
                    let g = self.r.u32()?;
                    let Some(gt) = self.cx.m.globals.get(g as usize).copied() else {
                        return invalid(format!("unknown global {g}"));
                    };
                    let d = self.push(gt.ty);
                    if gt.ty.slots() == 2 {
                        self.emit(Instr::new(Op::GlobalGet2, d, g, 0));
                    } else {
                        self.emit(Instr::new(Op::GlobalGet, d, g, 0));
                    }
                }
                0x24 => {
                    let g = self.r.u32()?;
                    let Some(gt) = self.cx.m.globals.get(g as usize).copied() else {
                        return invalid(format!("unknown global {g}"));
                    };
                    if !gt.mutable {
                        return invalid("global is immutable");
                    }
                    let v = self.pop_expect(gt.ty)?;
                    if gt.ty.slots() == 2 {
                        self.emit(Instr::new(Op::GlobalSet2, v.src(), g, 0));
                    } else {
                        self.emit(Instr::new(Op::GlobalSet, v.src(), g, 0));
                    }
                }
                0x25 => {
                    let t = self.r.u32()?;
                    let Some(tt) = self.cx.m.tables.get(t as usize).copied() else {
                        return invalid(format!("unknown table {t}"));
                    };
                    let i = self.pop_expect(self.table_addr(t))?;
                    let d = self.push(ValType::Ref(tt.elem));
                    self.emit(Instr::new(Op::TableGet, d, i.src(), t));
                }
                0x26 => {
                    let t = self.r.u32()?;
                    let Some(tt) = self.cx.m.tables.get(t as usize).copied() else {
                        return invalid(format!("unknown table {t}"));
                    };
                    let v = self.pop_expect(ValType::Ref(tt.elem))?;
                    let i = self.pop_expect(self.table_addr(t))?;
                    self.emit(Instr::new(Op::TableSet, i.src(), v.src(), t));
                }
                0x28 => self.load(op, ValType::I32, 4, Op::I32Load)?,
                0x29 => self.load(op, ValType::I64, 8, Op::I64Load)?,
                0x2a => self.load(op, ValType::F32, 4, Op::I32Load)?,
                0x2b => self.load(op, ValType::F64, 8, Op::I64Load)?,
                0x2c => self.load(op, ValType::I32, 1, Op::I32Load8S)?,
                0x2d => self.load(op, ValType::I32, 1, Op::I32Load8U)?,
                0x2e => self.load(op, ValType::I32, 2, Op::I32Load16S)?,
                0x2f => self.load(op, ValType::I32, 2, Op::I32Load16U)?,
                0x30 => self.load(op, ValType::I64, 1, Op::I64Load8S)?,
                0x31 => self.load(op, ValType::I64, 1, Op::I64Load8U)?,
                0x32 => self.load(op, ValType::I64, 2, Op::I64Load16S)?,
                0x33 => self.load(op, ValType::I64, 2, Op::I64Load16U)?,
                0x34 => self.load(op, ValType::I64, 4, Op::I64Load32S)?,
                0x35 => self.load(op, ValType::I64, 4, Op::I64Load32U)?,
                0x36 => self.store(op, ValType::I32, 4, Op::I32Store)?,
                0x37 => self.store(op, ValType::I64, 8, Op::I64Store)?,
                0x38 => self.store(op, ValType::F32, 4, Op::I32Store)?,
                0x39 => self.store(op, ValType::F64, 8, Op::I64Store)?,
                0x3a => self.store(op, ValType::I32, 1, Op::I32Store8)?,
                0x3b => self.store(op, ValType::I32, 2, Op::I32Store16)?,
                0x3c => self.store(op, ValType::I64, 1, Op::I32Store8)?,
                0x3d => self.store(op, ValType::I64, 2, Op::I32Store16)?,
                0x3e => self.store(op, ValType::I64, 4, Op::I64Store32)?,
                0x3f => {
                    let mem = self.r.u32()?;
                    if mem as usize >= self.cx.m.memories.len() {
                        return invalid(format!("unknown memory {mem}"));
                    }
                    let d = self.push(self.addr_type(mem));
                    self.emit(Instr::new(Op::MemorySize, d, mem, 0));
                }
                0x40 => {
                    let mem = self.r.u32()?;
                    if mem as usize >= self.cx.m.memories.len() {
                        return invalid(format!("unknown memory {mem}"));
                    }
                    let at = self.addr_type(mem);
                    let n = self.pop_expect(at)?;
                    let d = self.push(at);
                    self.emit(Instr::new(Op::MemoryGrow, d, n.src(), mem));
                }
                0x41 => {
                    let v = self.r.s32()?;
                    self.push_const(ValType::I32, v as u32 as u64);
                }
                0x42 => {
                    let v = self.r.s64()?;
                    self.push_const(ValType::I64, v as u64);
                }
                0x43 => {
                    let v = self.r.f32()?;
                    self.push_const(ValType::F32, v as u64);
                }
                0x44 => {
                    let v = self.r.f64()?;
                    self.push_const(ValType::F64, v);
                }
                0x45..=0xc4 => self.numeric(op)?,
                0xd0 => {
                    let h = self.r.heap_type()?;
                    check_heap(self.cx.m.types.len() as u32, h)?;
                    self.push_const(ValType::Ref(RefType { nullable: true, heap: h }), 0);
                }
                0xd1 => {
                    let o = self.pop_ref()?;
                    let d = self.push(ValType::I32);
                    self.emit(Instr::new(Op::I64Eqz, d, o.src(), 0));
                }
                0xd2 => {
                    let f = self.r.u32()?;
                    let Some(&ty) = self.cx.m.funcs.get(f as usize) else {
                        return invalid(format!("unknown function {f}"));
                    };
                    if !self.cx.refs.contains(&f) {
                        return invalid("undeclared function reference");
                    }
                    let d = self.push(ValType::Ref(RefType { nullable: false, heap: HeapType::Concrete(ty) }));
                    self.emit(Instr::new(Op::RefFunc, d, f, 0));
                }
                0xd3 => {
                    let eq = ValType::Ref(RefType { nullable: true, heap: HeapType::Eq });
                    self.binop(eq, ValType::I32, Op::I64Eq)?;
                }
                0xd4 => {
                    let o = self.pop_ref()?;
                    self.emit(Instr::new(Op::RefAsNonNull, o.src(), 0, 0));
                    let t = o.ty.map(|t| match t {
                        ValType::Ref(r) => ValType::Ref(RefType { nullable: false, heap: r.heap }),
                        t => t,
                    });
                    self.push_opt(t, o.loc);
                }
                0xd5 => {
                    let l = self.r.u32()?;
                    let o = self.pop_ref()?;
                    let (types, idx) = self.label_types(l)?;
                    self.check_top(&types)?;
                    self.br_on(o.src(), Op::BrOnNull, Op::BrOnNonNull, &types, 0, idx);
                    let t = o.ty.map(|t| match t {
                        ValType::Ref(r) => ValType::Ref(RefType { nullable: false, heap: r.heap }),
                        t => t,
                    });
                    self.push_opt(t, o.loc);
                }
                0xd6 => {
                    let l = self.r.u32()?;
                    let (types, idx) = self.label_types(l)?;
                    let Some((&last, rest)) = types.split_last() else {
                        return mismatch("br_on_non_null label has no reference result");
                    };
                    let ValType::Ref(_) = last else {
                        return mismatch("br_on_non_null label has no reference result");
                    };
                    let o = self.pop_ref()?;
                    let nn = o.ty.map(|t| match t {
                        ValType::Ref(r) => ValType::Ref(RefType { nullable: false, heap: r.heap }),
                        t => t,
                    });
                    if let Some(nn) = nn {
                        if !self.cx.tc.val_sub(nn, last) {
                            return mismatch("br_on_non_null");
                        }
                    }
                    self.check_top(rest)?;
                    // Branch values include the reference (now on top again).
                    self.push_opt(nn, o.loc);
                    let src = o.src();
                    self.br_on(src, Op::BrOnNonNull, Op::BrOnNull, &types, 0, idx);
                    self.pop()?;
                }
                0xfc => self.prefix_fc()?,
                0xfd => crate::simd::simd_op(self)?,
                0xfb => return invalid("GC instructions are not supported"),
                0xfe => return invalid("atomic instructions are not supported"),
                _ => return malformed(format!("illegal opcode {op:#x}")),
            }
        }
    }

    /// Replace the last instruction's destination by `slot` when it
    /// produced `v` (local.set after an arithmetic instruction).
    fn try_retarget(&mut self, v: &Operand, slot: u32) -> bool {
        if !self.live() || v.loc != Loc::Stack {
            return false;
        }
        let Some(pc) = self.last_pure else { return false };
        if pc + 1 != self.instrs.len() {
            return false;
        }
        let i = self.instrs[pc];
        if i.a != v.slot || v.width() != 1 {
            return false;
        }
        self.instrs[pc].a = slot;
        true
    }

    fn select(&mut self, a: Operand, b: Operand, c: Operand, t: Option<ValType>) {
        // `a` is materialized at its canonical slot, then conditionally
        // overwritten by `b`.
        let d = self.push_opt(t, Loc::Stack);
        if self.live() {
            let w = t.map(|t| t.slots()).unwrap_or(1);
            if a.src() != d {
                self.emit(Instr::new(if w == 2 { Op::Copy2 } else { Op::Copy }, d, a.src(), 0));
            }
            self.emit(Instr::new(if w == 2 { Op::Select2 } else { Op::Select }, d, b.src(), c.src()));
            self.last_pure = None;
        }
    }

    /// Conditional jump on `c` (an i32 operand already popped): jumps to
    /// `target` when the condition is `when`. Fuses a preceding comparison.
    /// Returns the instruction index to patch.
    fn cond_branch(&mut self, c: Operand, when: bool, target: u32) -> Option<usize> {
        if !self.live() {
            return None;
        }
        if c.loc == Loc::Stack {
            if let Some(pc) = self.last_pure {
                if pc + 1 == self.instrs.len() && self.instrs[pc].a == c.slot {
                    let i = self.instrs[pc];
                    if let Some(f) = fused(i.op, when) {
                        self.instrs[pc] = match sig(f) {
                            Sig::Sit => Instr::new(f, i.b, 0, target),
                            _ => Instr::new(f, i.b, i.c, target),
                        };
                        self.last_pure = None;
                        return Some(pc);
                    }
                }
            }
        }
        let op = if when { Op::BrIfNez } else { Op::BrIfEqz };
        let pc = self.emit(Instr::new(op, c.src(), 0, target));
        self.last_pure = None;
        Some(pc)
    }

    fn br_if(&mut self, c: Operand, types: &[ValType], idx: usize) -> Result<()> {
        let t = self.target_of(idx);
        let mv = if matches!(t, Target::Return) { vec![] } else { self.moves(types.len(), 0, self.ctls[idx].slot) };
        match t {
            Target::Known(pc) if mv.is_empty() => {
                self.cond_branch(c, true, pc);
            }
            Target::Pending(ci) if mv.is_empty() => {
                if let Some(pc) = self.cond_branch(c, true, u32::MAX) {
                    self.add_patch(ci, Patch::Instr(pc));
                }
            }
            _ => {
                // Skip over the moves when the condition is false.
                let skip = self.cond_branch(c, false, u32::MAX);
                self.emit_moves(&mv);
                self.emit_jump(&t);
                let here = self.bind_label();
                if let Some(s) = skip {
                    self.instrs[s].c = here;
                }
            }
        }
        Ok(())
    }

    /// br_on_null / br_on_non_null: `taken` jumps when the reference test
    /// holds; `inverse` is the opposite test. The branch values are the top
    /// `types.len()` operands below `skip` operands.
    fn br_on(&mut self, src: u32, taken: Op, inverse: Op, types: &[ValType], skip: usize, idx: usize) {
        if !self.live() {
            return;
        }
        let t = self.target_of(idx);
        let mv = if matches!(t, Target::Return) { vec![] } else { self.moves(types.len(), skip, self.ctls[idx].slot) };
        match t {
            Target::Known(pc) if mv.is_empty() => {
                self.emit(Instr::new(taken, src, 0, pc));
            }
            Target::Pending(ci) if mv.is_empty() => {
                let pc = self.emit(Instr::new(taken, src, 0, u32::MAX));
                self.add_patch(ci, Patch::Instr(pc));
            }
            _ => {
                let s = self.emit(Instr::new(inverse, src, 0, u32::MAX));
                self.emit_moves(&mv);
                self.emit_jump(&t);
                let here = self.bind_label();
                self.instrs[s].c = here;
            }
        }
        self.last_pure = None;
    }

    fn br_table(&mut self) -> Result<()> {
        let n = self.r.u32()?;
        if n > 1_000_000 {
            return malformed("br_table too large");
        }
        let mut labels = Vec::with_capacity(n as usize + 1);
        for _ in 0..=n {
            labels.push(self.r.u32()?);
        }
        let c = self.pop_expect(ValType::I32)?;
        let (dtypes, _) = self.label_types(*labels.last().unwrap())?;
        let arity = dtypes.len();
        let mut infos = Vec::with_capacity(labels.len());
        for &l in &labels {
            let (types, idx) = self.label_types(l)?;
            if types.len() != arity {
                return mismatch("br_table label arity");
            }
            self.check_top(&types)?;
            infos.push(idx);
        }
        if self.live() {
            let start = self.br_tables.len();
            self.br_tables.resize(start + labels.len(), u32::MAX);
            self.emit(Instr::new(Op::BrTable, c.src(), start as u32, labels.len() as u32));
            // Stubs for targets that need moves (or are returns), one per
            // distinct label.
            let mut stub_of: HashMap<usize, u32> = HashMap::new();
            for (k, &idx) in infos.iter().enumerate() {
                let t = self.target_of(idx);
                let mv = if matches!(t, Target::Return) { vec![] } else { self.moves(arity, 0, self.ctls[idx].slot) };
                match t {
                    Target::Known(pc) if mv.is_empty() => self.br_tables[start + k] = pc,
                    Target::Pending(ci) if mv.is_empty() => {
                        if self.live() {
                            self.ctls[ci].patches.push(Patch::Table(start + k));
                        }
                    }
                    _ => {
                        let pc = match stub_of.get(&idx) {
                            Some(&pc) => pc,
                            None => {
                                let pc = self.bind_label();
                                self.emit_moves(&mv);
                                self.emit_jump(&t);
                                stub_of.insert(idx, pc);
                                pc
                            }
                        };
                        self.br_tables[start + k] = pc;
                    }
                }
            }
        }
        self.set_unreachable();
        Ok(())
    }

    /// `end`: returns true at the end of the function.
    fn end(&mut self) -> Result<bool> {
        let kind = self.ctls.last().unwrap().kind;
        if kind == Kind::If {
            // Implicit else: params must match results.
            let c = self.ctls.last().unwrap();
            if c.params.len() != c.results.len() || !c.params.iter().zip(c.results.iter()).all(|(a, b)| self.cx.tc.val_sub(*a, *b)) {
                return mismatch("if without else must have matching param and result types");
            }
        }
        if kind == Kind::Func {
            let rs = self.results.clone();
            self.check_top(&rs)?;
            {
                let c = self.ctls.last().unwrap();
                if self.stack.len() - c.height > rs.len() || (!c.unreachable && self.stack.len() - c.height != rs.len()) {
                    return mismatch("values remaining on stack at end of function");
                }
            }
            if self.live() {
                self.emit_return();
            }
            let c = self.ctls.pop().unwrap();
            // Branches to the function label were emitted as returns.
            let _ = c;
            return Ok(true);
        }
        self.pop_results()?;
        let c = self.ctls.pop().unwrap();
        let here = self.bind_label();
        if let Some(ep) = c.else_patch {
            self.instrs[ep].c = here;
        }
        match c.kind {
            Kind::TryTable => {
                // Install handlers covering the body.
                for &(kind, tag, abs) in &c.catches {
                    if c.dead {
                        break;
                    }
                    let label = &self.ctls[abs];
                    let dst = if abs == 0 { 0 } else { label.slot };
                    let hidx = self.handlers.len();
                    let target = if label.kind == Kind::Loop { label.start } else { u32::MAX };
                    self.handlers.push(Handler { start: c.start, end: here, kind, tag, dst, target, depth: self.ctls.len() as u32, exn_slot: u32::MAX });

                    if label.kind != Kind::Loop {
                        if abs == 0 {
                            // Catch to the function label: return.
                            self.handlers[hidx].target = u32::MAX - 1;
                        } else {
                            self.ctls[abs].patches.push(Patch::Handler(hidx));
                        }
                    }
                }
            }
            Kind::Try => {
                // No catch clauses: nothing to install.
            }
            _ => {}
        }
        self.patch_to(c.patches, here);
        self.reset_inits(c.init_height);
        for t in c.results {
            self.push(t);
        }
        Ok(false)
    }

    fn try_table(&mut self) -> Result<()> {
        let (p, r) = self.block_type()?;
        let n = self.r.u32()?;
        let mut catches = Vec::new();
        for _ in 0..n {
            let k = self.r.byte()?;
            let (kind, tag) = match k {
                0 => (CatchKind::Catch, self.r.u32()?),
                1 => (CatchKind::CatchRef, self.r.u32()?),
                2 => (CatchKind::CatchAll, 0),
                3 => (CatchKind::CatchAllRef, 0),
                _ => return malformed("malformed catch clause"),
            };
            let l = self.r.u32()?;
            let (types, abs) = self.label_types(l)?;
            let mut payload: Vec<ValType> = Vec::new();
            if matches!(kind, CatchKind::Catch | CatchKind::CatchRef) {
                let Some(&tt) = self.cx.m.tags.get(tag as usize) else {
                    return invalid(format!("unknown tag {tag}"));
                };
                payload.extend(self.cx.tc.func_type(tt).unwrap().params.iter());
            }
            if matches!(kind, CatchKind::CatchRef | CatchKind::CatchAllRef) {
                payload.push(ValType::Ref(RefType { nullable: false, heap: HeapType::Exn }));
            }
            if payload.len() != types.len() || !payload.iter().zip(types.iter()).all(|(a, b)| self.cx.tc.val_sub(*a, *b)) {
                return mismatch("catch clause does not match label");
            }
            catches.push((kind, tag, abs));
        }
        self.push_ctl(Kind::TryTable, p, r)?;
        self.ctls.last_mut().unwrap().catches = catches;
        Ok(())
    }

    fn legacy_catch(&mut self, all: bool) -> Result<()> {
        let tag = if all { 0 } else { self.r.u32()? };
        let k = self.ctls.last().unwrap().kind;
        if k != Kind::Try && k != Kind::Catch {
            return malformed(if all { "catch_all without try" } else { "catch without try" });
        }
        let payload: Vec<ValType> = if all {
            vec![]
        } else {
            let Some(&tt) = self.cx.m.tags.get(tag as usize) else {
                return invalid(format!("unknown tag {tag}"));
            };
            self.cx.tc.func_type(tt).unwrap().params.to_vec()
        };
        self.pop_results()?;
        let idx = self.ctls.len() - 1;
        if self.live() {
            let pc = self.emit(Instr::new(Op::Br, 0, 0, u32::MAX));
            self.ctls[idx].patches.push(Patch::Instr(pc));
        }
        let here = self.bind_label();
        let exn_slot = self.exn_next;
        if self.exn_next < self.exn_end {
            self.exn_next += 1;
        }
        let depth = idx as u32;
        let c = &mut self.ctls[idx];
        if c.try_end.is_none() {
            c.try_end = Some(here);
        }
        let (start, end) = (c.start, c.try_end.unwrap());
        c.kind = if all { Kind::CatchAll } else { Kind::Catch };
        c.unreachable = false;
        c.exn_slot = exn_slot;
        let dst = c.slot;
        let ih = c.init_height;
        let entry = !c.dead;
        if entry {
            self.handlers.push(Handler {
                start,
                end,
                kind: if all { CatchKind::CatchAll } else { CatchKind::Catch },
                tag,
                dst,
                target: here,
                depth,
                exn_slot,
            });
        }
        self.reset_inits(ih);
        for t in payload {
            self.push(t);
        }
        Ok(())
    }

    fn legacy_delegate(&mut self) -> Result<()> {
        let l = self.r.u32()?;
        if self.ctls.last().unwrap().kind != Kind::Try {
            return malformed("delegate without try");
        }
        self.pop_results()?;
        let c = self.ctls.pop().unwrap();
        let n = self.ctls.len();
        if l as usize >= n {
            return invalid(format!("unknown label {l}"));
        }
        let target_abs = n - 1 - l as usize;
        let here = self.bind_label();
        if !c.dead {
            self.handlers.push(Handler {
                start: c.start,
                end: here,
                kind: CatchKind::Delegate,
                tag: target_abs as u32,
                dst: 0,
                target: 0,
                depth: n as u32,
                exn_slot: u32::MAX,
            });
        }
        self.patch_to(c.patches, here);
        self.reset_inits(c.init_height);
        for t in c.results {
            self.push(t);
        }
        Ok(())
    }

    fn numeric(&mut self, op: u8) -> Result<()> {
        use ValType::*;
        let (i, l, f, d) = (I32, I64, F32, F64);
        match op {
            0x45 => self.unop(i, i, Op::I32Eqz),
            0x46..=0x4f => {
                const OPS: [Op; 10] = [Op::I32Eq, Op::I32Ne, Op::I32LtS, Op::I32LtU, Op::I32GtS, Op::I32GtU, Op::I32LeS, Op::I32LeU, Op::I32GeS, Op::I32GeU];
                self.binop(i, i, OPS[(op - 0x46) as usize])
            }
            0x50 => self.unop(l, i, Op::I64Eqz),
            0x51..=0x5a => {
                const OPS: [Op; 10] = [Op::I64Eq, Op::I64Ne, Op::I64LtS, Op::I64LtU, Op::I64GtS, Op::I64GtU, Op::I64LeS, Op::I64LeU, Op::I64GeS, Op::I64GeU];
                self.binop(l, i, OPS[(op - 0x51) as usize])
            }
            0x5b..=0x60 => {
                const OPS: [Op; 6] = [Op::F32Eq, Op::F32Ne, Op::F32Lt, Op::F32Gt, Op::F32Le, Op::F32Ge];
                self.binop(f, i, OPS[(op - 0x5b) as usize])
            }
            0x61..=0x66 => {
                const OPS: [Op; 6] = [Op::F64Eq, Op::F64Ne, Op::F64Lt, Op::F64Gt, Op::F64Le, Op::F64Ge];
                self.binop(d, i, OPS[(op - 0x61) as usize])
            }
            0x67 => self.unop(i, i, Op::I32Clz),
            0x68 => self.unop(i, i, Op::I32Ctz),
            0x69 => self.unop(i, i, Op::I32Popcnt),
            0x6a..=0x78 => {
                const OPS: [Op; 15] = [
                    Op::I32Add,
                    Op::I32Sub,
                    Op::I32Mul,
                    Op::I32DivS,
                    Op::I32DivU,
                    Op::I32RemS,
                    Op::I32RemU,
                    Op::I32And,
                    Op::I32Or,
                    Op::I32Xor,
                    Op::I32Shl,
                    Op::I32ShrS,
                    Op::I32ShrU,
                    Op::I32Rotl,
                    Op::I32Rotr,
                ];
                self.binop(i, i, OPS[(op - 0x6a) as usize])
            }
            0x79 => self.unop(l, l, Op::I64Clz),
            0x7a => self.unop(l, l, Op::I64Ctz),
            0x7b => self.unop(l, l, Op::I64Popcnt),
            0x7c..=0x8a => {
                const OPS: [Op; 15] = [
                    Op::I64Add,
                    Op::I64Sub,
                    Op::I64Mul,
                    Op::I64DivS,
                    Op::I64DivU,
                    Op::I64RemS,
                    Op::I64RemU,
                    Op::I64And,
                    Op::I64Or,
                    Op::I64Xor,
                    Op::I64Shl,
                    Op::I64ShrS,
                    Op::I64ShrU,
                    Op::I64Rotl,
                    Op::I64Rotr,
                ];
                self.binop(l, l, OPS[(op - 0x7c) as usize])
            }
            0x8b..=0x91 => {
                const OPS: [Op; 7] = [Op::F32Abs, Op::F32Neg, Op::F32Ceil, Op::F32Floor, Op::F32Trunc, Op::F32Nearest, Op::F32Sqrt];
                self.unop(f, f, OPS[(op - 0x8b) as usize])
            }
            0x92..=0x98 => {
                const OPS: [Op; 7] = [Op::F32Add, Op::F32Sub, Op::F32Mul, Op::F32Div, Op::F32Min, Op::F32Max, Op::F32Copysign];
                self.binop(f, f, OPS[(op - 0x92) as usize])
            }
            0x99..=0x9f => {
                const OPS: [Op; 7] = [Op::F64Abs, Op::F64Neg, Op::F64Ceil, Op::F64Floor, Op::F64Trunc, Op::F64Nearest, Op::F64Sqrt];
                self.unop(d, d, OPS[(op - 0x99) as usize])
            }
            0xa0..=0xa6 => {
                const OPS: [Op; 7] = [Op::F64Add, Op::F64Sub, Op::F64Mul, Op::F64Div, Op::F64Min, Op::F64Max, Op::F64Copysign];
                self.binop(d, d, OPS[(op - 0xa0) as usize])
            }
            0xa7 => self.unop(l, i, Op::I32WrapI64),
            0xa8 => self.unop(f, i, Op::I32TruncF32S),
            0xa9 => self.unop(f, i, Op::I32TruncF32U),
            0xaa => self.unop(d, i, Op::I32TruncF64S),
            0xab => self.unop(d, i, Op::I32TruncF64U),
            0xac => self.unop(i, l, Op::I64ExtendI32S),
            0xad => self.unop(i, l, Op::I64ExtendI32U),
            0xae => self.unop(f, l, Op::I64TruncF32S),
            0xaf => self.unop(f, l, Op::I64TruncF32U),
            0xb0 => self.unop(d, l, Op::I64TruncF64S),
            0xb1 => self.unop(d, l, Op::I64TruncF64U),
            0xb2 => self.unop(i, f, Op::F32ConvertI32S),
            0xb3 => self.unop(i, f, Op::F32ConvertI32U),
            0xb4 => self.unop(l, f, Op::F32ConvertI64S),
            0xb5 => self.unop(l, f, Op::F32ConvertI64U),
            0xb6 => self.unop(d, f, Op::F32DemoteF64),
            0xb7 => self.unop(i, d, Op::F64ConvertI32S),
            0xb8 => self.unop(i, d, Op::F64ConvertI32U),
            0xb9 => self.unop(l, d, Op::F64ConvertI64S),
            0xba => self.unop(l, d, Op::F64ConvertI64U),
            0xbb => self.unop(f, d, Op::F64PromoteF32),
            0xbc..=0xbf => {
                // Reinterpretations keep the bits: retype in place.
                let (tin, tout) = match op {
                    0xbc => (f, i),
                    0xbd => (d, l),
                    0xbe => (i, f),
                    _ => (l, d),
                };
                let o = self.pop_expect(tin)?;
                self.push_opt(Some(tout), o.loc);
                Ok(())
            }
            0xc0 => self.unop(i, i, Op::I32Extend8S),
            0xc1 => self.unop(i, i, Op::I32Extend16S),
            0xc2 => self.unop(l, l, Op::I64Extend8S),
            0xc3 => self.unop(l, l, Op::I64Extend16S),
            0xc4 => self.unop(l, l, Op::I64Extend32S),
            _ => unreachable!(),
        }
    }

    fn prefix_fc(&mut self) -> Result<()> {
        use ValType::*;
        let sub = self.r.u32()?;
        match sub {
            0..=7 => {
                const OPS: [(ValType, ValType, Op); 8] = [
                    (F32, I32, Op::I32TruncSatF32S),
                    (F32, I32, Op::I32TruncSatF32U),
                    (F64, I32, Op::I32TruncSatF64S),
                    (F64, I32, Op::I32TruncSatF64U),
                    (F32, I64, Op::I64TruncSatF32S),
                    (F32, I64, Op::I64TruncSatF32U),
                    (F64, I64, Op::I64TruncSatF64S),
                    (F64, I64, Op::I64TruncSatF64U),
                ];
                let (a, b, o) = OPS[sub as usize];
                self.unop(a, b, o)
            }
            8 => {
                let seg = self.r.u32()?;
                let mem = self.r.u32()?;
                if mem as usize >= self.cx.m.memories.len() {
                    return invalid(format!("unknown memory {mem}"));
                }
                let Some(dc) = self.cx.m.data_count else {
                    return malformed("data count section required");
                };
                if seg >= dc {
                    return invalid(format!("unknown data segment {seg}"));
                }
                let at = self.addr_type(mem);
                let base = self.call_args(&[at, I32, I32], 0)?;
                self.emit(Instr::new(Op::MemoryInit, base, seg, mem));
                Ok(())
            }
            9 => {
                let seg = self.r.u32()?;
                let Some(dc) = self.cx.m.data_count else {
                    return malformed("data count section required");
                };
                if seg >= dc {
                    return invalid(format!("unknown data segment {seg}"));
                }
                self.emit(Instr::new(Op::DataDrop, 0, seg, 0));
                Ok(())
            }
            10 => {
                let dm = self.r.u32()?;
                let sm = self.r.u32()?;
                let n = self.cx.m.memories.len() as u32;
                if dm >= n || sm >= n {
                    return invalid(format!("unknown memory {}", dm.max(sm)));
                }
                let (dt, st) = (self.addr_type(dm), self.addr_type(sm));
                let lt = if dt == I64 && st == I64 { I64 } else { I32 };
                let base = self.call_args(&[dt, st, lt], 0)?;
                self.emit(Instr::new(Op::MemoryCopy, base, dm, sm));
                Ok(())
            }
            11 => {
                let mem = self.r.u32()?;
                if mem as usize >= self.cx.m.memories.len() {
                    return invalid(format!("unknown memory {mem}"));
                }
                let at = self.addr_type(mem);
                let base = self.call_args(&[at, I32, at], 0)?;
                self.emit(Instr::new(Op::MemoryFill, base, mem, 0));
                Ok(())
            }
            12 => {
                let seg = self.r.u32()?;
                let t = self.r.u32()?;
                let Some(tt) = self.cx.m.tables.get(t as usize).copied() else {
                    return invalid(format!("unknown table {t}"));
                };
                let Some(e) = self.cx.m.elems.get(seg as usize) else {
                    return invalid(format!("unknown elem segment {seg}"));
                };
                if !self.cx.tc.ref_sub(e.ty, tt.elem) {
                    return mismatch("table.init element type");
                }
                let at = self.table_addr(t);
                let base = self.call_args(&[at, I32, I32], 0)?;
                self.emit(Instr::new(Op::TableInit, base, seg, t));
                Ok(())
            }
            13 => {
                let seg = self.r.u32()?;
                if seg as usize >= self.cx.m.elems.len() {
                    return invalid(format!("unknown elem segment {seg}"));
                }
                self.emit(Instr::new(Op::ElemDrop, 0, seg, 0));
                Ok(())
            }
            14 => {
                let dt = self.r.u32()?;
                let st = self.r.u32()?;
                let tables = &self.cx.m.tables;
                let (Some(d), Some(s)) = (tables.get(dt as usize).copied(), tables.get(st as usize).copied()) else {
                    return invalid(format!("unknown table {}", dt.max(st)));
                };
                if !self.cx.tc.ref_sub(s.elem, d.elem) {
                    return mismatch("table.copy element type");
                }
                let (da, sa) = (self.table_addr(dt), self.table_addr(st));
                let lt = if da == I64 && sa == I64 { I64 } else { I32 };
                let base = self.call_args(&[da, sa, lt], 0)?;
                self.emit(Instr::new(Op::TableCopy, base, dt, st));
                Ok(())
            }
            15 => {
                let t = self.r.u32()?;
                let Some(tt) = self.cx.m.tables.get(t as usize).copied() else {
                    return invalid(format!("unknown table {t}"));
                };
                let at = self.table_addr(t);
                let base = self.call_args(&[ValType::Ref(tt.elem), at], 0)?;
                self.emit(Instr::new(Op::TableGrow, base, 0, t));
                self.push(at);
                Ok(())
            }
            16 => {
                let t = self.r.u32()?;
                if t as usize >= self.cx.m.tables.len() {
                    return invalid(format!("unknown table {t}"));
                }
                let d = self.push(self.table_addr(t));
                self.emit(Instr::new(Op::TableSize, d, 0, t));
                Ok(())
            }
            17 => {
                let t = self.r.u32()?;
                let Some(tt) = self.cx.m.tables.get(t as usize).copied() else {
                    return invalid(format!("unknown table {t}"));
                };
                let at = self.table_addr(t);
                let base = self.call_args(&[at, ValType::Ref(tt.elem), at], 0)?;
                self.emit(Instr::new(Op::TableFill, base, 0, t));
                Ok(())
            }
            _ => malformed(format!("illegal opcode 0xfc {sub}")),
        }
    }
}

/// The fused compare-and-branch for a comparison producing the condition,
/// branching when the comparison is `when`.
fn fused(op: Op, when: bool) -> Option<Op> {
    use Op::*;
    let (t, f) = match op {
        I32Eqz => (BrIfEqz, BrIfNez),
        I64Eqz => (BrI64Eqz, BrI64Nez),
        I32Eq => (BrI32Eq, BrI32Ne),
        I32Ne => (BrI32Ne, BrI32Eq),
        I32LtS => (BrI32LtS, BrI32GeS),
        I32LtU => (BrI32LtU, BrI32GeU),
        I32GtS => (BrI32GtS, BrI32LeS),
        I32GtU => (BrI32GtU, BrI32LeU),
        I32LeS => (BrI32LeS, BrI32GtS),
        I32LeU => (BrI32LeU, BrI32GtU),
        I32GeS => (BrI32GeS, BrI32LtS),
        I32GeU => (BrI32GeU, BrI32LtU),
        I64Eq => (BrI64Eq, BrI64Ne),
        I64Ne => (BrI64Ne, BrI64Eq),
        I64LtS => (BrI64LtS, BrI64GeS),
        I64LtU => (BrI64LtU, BrI64GeU),
        I64GtS => (BrI64GtS, BrI64LeS),
        I64GtU => (BrI64GtU, BrI64LeU),
        I64LeS => (BrI64LeS, BrI64GtS),
        I64LeU => (BrI64LeU, BrI64GtU),
        I64GeS => (BrI64GeS, BrI64LtS),
        I64GeU => (BrI64GeU, BrI64LtU),
        _ => return None,
    };
    Some(if when { t } else { f })
}
