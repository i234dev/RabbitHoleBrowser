//! Stack-to-register translation of compiled functions.
//!
//! The compiler emits code for a stack machine. Every instruction's
//! operand stack depth is static, so the value at depth `d` can live in a
//! fixed frame slot (`nregs + d`, right after the function's registers);
//! the operand stack is then just more registers and needs no stack
//! pointer at run time.
//!
//! This pass rewrites the stack code into register form with a virtual
//! stack: pushes of registers and constants are deferred (they become
//! operands of the instruction that consumes them instead of copies), the
//! results of register instructions are written straight into their
//! destination (`x = a + b` is one `AddRR`), and compare-and-branch pairs
//! are fused. Instructions without a register form run as stack
//! instructions whose operands sit at the statically known depth
//! (`FunctionCode::depths`).
//!
//! Invariants of the virtual stack (entry `i` is at depth `i`):
//! * `Slot` entries hold their value in their own slot `nregs + i`;
//! * `Reg(r)` / `Const(k)` entries are deferred reads, materialized into
//!   their own slot when needed (before stack instructions that consume
//!   them, at control-flow joins, before register `r` is overwritten);
//! * `Copy(s)` entries are deferred copies of a lower `Slot` entry's slot
//!   (`s < nregs + i`), which stays valid while the entry exists because a
//!   slot is only rewritten after everything above it has been popped.
//!
//! At every jump and jump target the virtual stack is canonical (all
//! `Slot`), so all paths into an instruction agree on where values are.

use crate::ast::Pos;
use crate::value::V;
use crate::bytecode::{Handler, Op, BIN_OPS, CMP_OPS, UN_OPS};
use crate::value::Value;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Ent {
    /// Value in the entry's own slot; `def` is the output instruction that
    /// wrote it (`NO_DEF` when unknown), which may be retargeted.
    Slot { def: u32 },
    /// Deferred read of a register (GetReg semantics).
    Reg(u16),
    /// Deferred constant.
    Const(u32),
    /// Deferred raw copy of a lower slot.
    Copy(u16),
}

const NO_DEF: u32 = u32::MAX;

/// Result of the translation.
pub struct Translated {
    pub ops: Vec<Op>,
    pub positions: Vec<Pos>,
    pub handlers: Vec<Handler>,
    /// Operand stack depth before each instruction.
    pub depths: Vec<u16>,
    /// Maximum operand stack depth.
    pub max_depth: u32,
}

struct Tr<'a> {
    out: Vec<Op>,
    pos: Vec<Pos>,
    depths: Vec<u16>,
    vs: Vec<Ent>,
    nregs: u32,
    max_depth: u32,
    cur_pos: Pos,
    constants: &'a mut Vec<Value>,
    special: [Option<u32>; 5],
    /// (output index, raw jump target)
    fixups: Vec<(usize, u32)>,
}

/// Translate `ops` (raw compiler output) into register form. `ic_keys`
/// receives the key constant of every inline-cache slot. Fails when the
/// code is malformed (inconsistent stack depths).
pub fn translate(ops: &[Op], positions: &[Pos], handlers: &[Handler], constants: &mut Vec<Value>, nregs: u32, ic_keys: &mut [u32]) -> Result<Translated, String> {
    let depth = crate::bytecode::compute_depths(ops, handlers)?;
    let n = ops.len();
    let max_depth = depth.iter().flatten().copied().max().unwrap_or(0) + 4;
    for op in ops {
        match *op {
            Op::GetProp(k, ic) | Op::SetProp(k, ic) | Op::GetPropKeep(k, ic) | Op::GetMethod(k, ic) | Op::DefineProp(k, ic) | Op::GetGlobal(k, ic) | Op::SetGlobal(k, ic) => {
                if let Some(slot) = ic_keys.get_mut(ic as usize) {
                    *slot = k;
                }
            }
            _ => {}
        }
    }
    if nregs as u64 + max_depth as u64 + 8 >= u16::MAX as u64 {
        // Too many slots for register operands: keep the stack code.
        return Ok(untranslated(ops, positions, handlers, &depth, max_depth));
    }
    let mut is_label = vec![false; n + 1];
    for op in ops {
        if let Some(t) = op.jump_target() {
            is_label[(t as usize).min(n)] = true;
        }
    }
    for h in handlers {
        for x in [h.start, h.end, h.target] {
            is_label[(x as usize).min(n)] = true;
        }
    }
    let mut tr = Tr {
        out: Vec::with_capacity(n),
        pos: Vec::with_capacity(n),
        depths: Vec::with_capacity(n),
        vs: Vec::new(),
        nregs,
        max_depth,
        cur_pos: Pos::default(),
        constants,
        special: [None; 5],
        fixups: Vec::new(),
    };
    let mut map = vec![0u32; n + 1];
    let mut dead = false;
    let mut i = 0;
    while i < n {
        if is_label[i] {
            tr.flush_all();
            dead = false;
            match depth[i] {
                Some(d) => {
                    tr.vs.clear();
                    tr.vs.resize(d as usize, Ent::Slot { def: NO_DEF });
                }
                None => dead = true,
            }
        }
        map[i] = tr.out.len() as u32;
        if dead || depth[i].is_none() {
            i += 1;
            continue;
        }
        debug_assert_eq!(tr.vs.len() as u32, depth[i].unwrap(), "virtual stack out of sync at {i} {:?}", ops[i]);
        if tr.vs.len() as u32 != depth[i].unwrap() {
            return Err(format!("register translation out of sync at {i}"));
        }
        tr.cur_pos = positions.get(i).copied().unwrap_or_default();
        let op = ops[i];
        let next = if i + 1 < n && !is_label[i + 1] { Some(ops[i + 1]) } else { None };
        let next2 = if i + 2 < n && !is_label[i + 1] && !is_label[i + 2] { Some(ops[i + 2]) } else { None };
        let mut consumed = 1;
        match op {
            // `Dup; Set*; Pop`: an assignment whose value is discarded.
            Op::Dup if matches!(next, Some(Op::SetReg(_) | Op::SetLocal(..) | Op::InitLocal(..) | Op::SetGlobal(..))) && next2 == Some(Op::Pop) => {
                for j in i + 1..i + 3 {
                    map[j] = tr.out.len() as u32;
                }
                tr.op(next.unwrap(), None, &mut 0)?;
                consumed = 3;
            }
            // `Dup; GetProp; Swap`: method lookup for a call (obj -> method obj).
            Op::Dup if matches!(next, Some(Op::GetProp(..))) && next2 == Some(Op::Swap) => {
                for j in i + 1..i + 3 {
                    map[j] = tr.out.len() as u32;
                }
                if let Some(Op::GetProp(k, ic)) = next {
                    tr.op(Op::GetMethod(k, ic), None, &mut 0)?;
                }
                consumed = 3;
            }
            _ => {
                let mut extra = 0;
                tr.op(op, next, &mut extra)?;
                for j in i + 1..i + 1 + extra {
                    map[j] = tr.out.len() as u32;
                }
                consumed += extra;
            }
        }
        if op.is_terminator() {
            dead = true;
        }
        i += consumed;
    }
    map[n] = tr.out.len() as u32;
    let Tr { mut out, pos, depths, fixups, max_depth, .. } = tr;
    for (at, t) in fixups {
        let nt = map[(t as usize).min(n)];
        out[at] = out[at].with_target(nt);
    }
    let handlers = handlers
        .iter()
        .map(|h| Handler { start: map[(h.start as usize).min(n)], end: map[(h.end as usize).min(n)], target: map[(h.target as usize).min(n)], ..*h })
        .collect();
    fuse_loop_latches(&mut out);
    Ok(Translated { ops: out, positions: pos, handlers, depths, max_depth })
}

/// `for (...; i < n; i++)` ends in `IncR(i, i)` followed by the loop test
/// `JumpIfLt(i, n, top)` jumping backwards; the increment becomes one
/// instruction doing both (the test stays where it is: the loop is entered
/// by jumping to it).
fn fuse_loop_latches(ops: &mut [Op]) {
    for i in 0..ops.len().saturating_sub(1) {
        if let Op::IncR(d, s) = ops[i] {
            if d != s {
                continue;
            }
            ops[i] = match ops[i + 1] {
                Op::JumpIfLtRR(a, b, t) if a == d && (t as usize) <= i => Op::IncJumpIfLtRR(a, b, t),
                Op::JumpIfLtRK(a, k, t) if a == d && (t as usize) <= i => Op::IncJumpIfLtRK(a, k, t),
                other => {
                    let _ = other;
                    continue;
                }
            };
        }
    }
}

/// The stack code unchanged (every instruction runs as a stack instruction).
fn untranslated(ops: &[Op], positions: &[Pos], handlers: &[Handler], depth: &[Option<u32>], max_depth: u32) -> Translated {
    Translated {
        ops: ops.to_vec(),
        positions: positions.to_vec(),
        handlers: handlers.to_vec(),
        depths: (0..ops.len()).map(|i| depth[i].unwrap_or(0).min(u16::MAX as u32) as u16).collect(),
        max_depth,
    }
}

fn is_commutative(op: Op) -> bool {
    matches!(op, Op::Mul | Op::BitAnd | Op::BitOr | Op::BitXor | Op::StrictEq | Op::StrictNotEq | Op::Eq | Op::NotEq)
}

/// The comparison with swapped operands (`a < b` is `b > a`).
fn swapped_cmp(op: Op) -> Option<Op> {
    Some(match op {
        Op::Lt => Op::Gt,
        Op::LtEq => Op::GtEq,
        Op::Gt => Op::Lt,
        Op::GtEq => Op::LtEq,
        o if is_commutative(o) => o,
        _ => return None,
    })
}

/// The same instruction writing `d` instead of its destination slot.
fn with_dst(op: Op, d: u16) -> Option<Op> {
    Some(match op {
        Op::Mov(_, s) => Op::Mov(d, s),
        Op::MovRaw(_, s) => Op::MovRaw(d, s),
        Op::LoadK(_, k) => Op::LoadK(d, k),
        Op::AddRR(_, a, b) => Op::AddRR(d, a, b),
        Op::SubRR(_, a, b) => Op::SubRR(d, a, b),
        Op::MulRR(_, a, b) => Op::MulRR(d, a, b),
        Op::DivRR(_, a, b) => Op::DivRR(d, a, b),
        Op::ModRR(_, a, b) => Op::ModRR(d, a, b),
        Op::AddRK(_, a, b) => Op::AddRK(d, a, b),
        Op::SubRK(_, a, b) => Op::SubRK(d, a, b),
        Op::MulRK(_, a, b) => Op::MulRK(d, a, b),
        Op::DivRK(_, a, b) => Op::DivRK(d, a, b),
        Op::ModRK(_, a, b) => Op::ModRK(d, a, b),
        Op::BinRR(k, _, a, b) => Op::BinRR(k, d, a, b),
        Op::BinRK(k, _, a, b) => Op::BinRK(k, d, a, b),
        Op::CmpRR(k, _, a, b) => Op::CmpRR(k, d, a, b),
        Op::CmpRK(k, _, a, b) => Op::CmpRK(k, d, a, b),
        Op::NotR(_, s) => Op::NotR(d, s),
        Op::IncR(_, s) => Op::IncR(d, s),
        Op::DecR(_, s) => Op::DecR(d, s),
        Op::ToNumericR(_, s) => Op::ToNumericR(d, s),
        Op::UnR(k, _, s) => Op::UnR(k, d, s),
        Op::LoadThis(_) => Op::LoadThis(d),
        Op::GetPropR(_, o, ic) => Op::GetPropR(d, o, ic),
        Op::GetElemR(_, o, k) => Op::GetElemR(d, o, k),
        Op::GetGlobalR(_, ic) => Op::GetGlobalR(d, ic),
        Op::GetLocalR(_, a, b) => Op::GetLocalR(d, a, b),
        _ => return None,
    })
}

impl Tr<'_> {
    #[inline]
    fn slot(&self, depth: usize) -> u16 {
        (self.nregs as usize + depth) as u16
    }

    fn emit(&mut self, op: Op) -> u32 {
        let at = self.out.len() as u32;
        self.out.push(op);
        self.pos.push(self.cur_pos);
        self.depths.push(self.vs.len() as u16);
        if self.vs.len() as u32 + 2 > self.max_depth {
            self.max_depth = self.vs.len() as u32 + 2;
        }
        at
    }

    fn emit_jump(&mut self, op: Op) {
        let t = op.jump_target().unwrap();
        let at = self.emit(op);
        self.fixups.push((at as usize, t));
    }

    fn last(&self) -> u32 {
        (self.out.len() as u32).wrapping_sub(1)
    }

    fn special_const(&mut self, which: usize) -> u32 {
        if let Some(k) = self.special[which] {
            return k;
        }
        let v = match which {
            0 => Value::Undefined,
            1 => Value::Null,
            2 => Value::Bool(true),
            3 => Value::Bool(false),
            _ => Value::Empty,
        };
        let k = self.constants.len() as u32;
        self.constants.push(v);
        self.special[which] = Some(k);
        k
    }

    /// Put entry `i` into its own slot.
    fn materialize(&mut self, i: usize) {
        let s = self.slot(i);
        let op = match self.vs[i] {
            Ent::Slot { .. } => return,
            Ent::Reg(r) => Op::Mov(s, r),
            Ent::Const(k) => Op::LoadK(s, k),
            Ent::Copy(src) => Op::MovRaw(s, src),
        };
        let at = self.emit(op);
        self.vs[i] = Ent::Slot { def: at };
    }

    fn flush_all(&mut self) {
        for i in 0..self.vs.len() {
            self.materialize(i);
        }
    }

    fn flush_top(&mut self, n: usize) {
        let len = self.vs.len();
        for i in len - n..len {
            self.materialize(i);
        }
    }

    /// Before register `r` is written: deferred reads of it must happen now.
    fn invalidate_reg(&mut self, r: u16) {
        for i in 0..self.vs.len() {
            if self.vs[i] == Ent::Reg(r) {
                self.materialize(i);
            }
        }
    }

    /// Before slot `s` is written by something other than the virtual
    /// stack discipline (never happens for stack slots; registers only).
    fn pop(&mut self) -> (Ent, usize) {
        let e = self.vs.pop().expect("virtual stack underflow");
        (e, self.vs.len())
    }

    /// Read operand for an entry at `depth` (constants are materialized).
    fn opnd(&mut self, e: Ent, depth: usize) -> u16 {
        match e {
            Ent::Reg(r) => r,
            Ent::Copy(s) => s,
            Ent::Slot { .. } => self.slot(depth),
            Ent::Const(k) => {
                let s = self.slot(depth);
                self.emit(Op::LoadK(s, k));
                s
            }
        }
    }

    /// Operand of an entry that stays on the stack (constants are
    /// materialized in place).
    fn opnd_keep(&mut self, i: usize) -> u16 {
        if let Ent::Const(_) = self.vs[i] {
            self.materialize(i);
        }
        match self.vs[i] {
            Ent::Reg(r) => r,
            Ent::Copy(s) => s,
            _ => self.slot(i),
        }
    }

    fn push_def(&mut self, at: u32) {
        self.vs.push(Ent::Slot { def: at });
    }

    /// Emit an instruction writing the slot of the next pushed entry, then push it.
    fn emit_push(&mut self, op: Op) {
        let at = self.emit(op);
        self.push_def(at);
    }

    /// Store entry `e` (at `depth`) into register `r`.
    fn store_reg(&mut self, r: u16, e: Ent, depth: usize) {
        let before = self.out.len();
        self.invalidate_reg(r);
        let clean = self.out.len() == before;
        match e {
            Ent::Slot { def } => {
                if clean && def != NO_DEF && def == self.last() {
                    if let Some(op) = with_dst(self.out[def as usize], r) {
                        // The value computed into the stack slot goes
                        // straight to the register.
                        if dst_of(self.out[def as usize]) == Some(self.slot(depth)) {
                            self.out[def as usize] = op;
                            return;
                        }
                    }
                }
                let s = self.slot(depth);
                self.emit(Op::MovRaw(r, s));
            }
            Ent::Reg(q) => {
                self.emit(Op::Mov(r, q));
            }
            Ent::Const(k) => {
                self.emit(Op::LoadK(r, k));
            }
            Ent::Copy(s) => {
                self.emit(Op::MovRaw(r, s));
            }
        }
    }

    /// The value of an assignment expression (`obj.x = v`): `v` popped from
    /// `vdepth` becomes the entry at `rdepth` (< vdepth). A following `Pop`
    /// discards it without any copy.
    fn push_assign_result(&mut self, v: Ent, vdepth: usize, next: Option<Op>, extra: &mut usize) {
        debug_assert_eq!(self.vs.len(), vdepth.min(self.vs.len()));
        if next == Some(Op::Pop) {
            *extra = 1;
            return;
        }
        if next == Some(Op::StoreCompletion) {
            // `obj.x = v;` as a script statement: v is the completion value.
            let s = match v {
                Ent::Reg(r) => r,
                Ent::Copy(s) => s,
                Ent::Slot { .. } => self.slot(vdepth),
                Ent::Const(k) => {
                    let d = self.slot(self.vs.len());
                    self.emit(Op::LoadK(d, k));
                    d
                }
            };
            self.emit(Op::StoreCompletionR(s));
            *extra = 1;
            return;
        }
        match v {
            Ent::Reg(_) | Ent::Const(_) | Ent::Copy(_) => self.vs.push(v),
            Ent::Slot { .. } => {
                let d = self.slot(self.vs.len());
                let s = self.slot(vdepth);
                self.emit_push(Op::MovRaw(d, s));
            }
        }
    }

    /// A stack instruction run by the generic interpreter path: its inputs
    /// are materialized on top of the stack.
    fn generic(&mut self, op: Op) {
        let (p, q) = op.stack_effect();
        self.flush_top(p as usize);
        self.emit(op);
        let len = self.vs.len();
        self.vs.truncate(len - p as usize);
        for _ in 0..q {
            self.vs.push(Ent::Slot { def: NO_DEF });
        }
    }

    fn op(&mut self, op: Op, next: Option<Op>, extra: &mut usize) -> Result<(), String> {
        match op {
            Op::Const(k) => self.vs.push(Ent::Const(k)),
            Op::Undefined => {
                let k = self.special_const(0);
                self.vs.push(Ent::Const(k));
            }
            Op::Null => {
                let k = self.special_const(1);
                self.vs.push(Ent::Const(k));
            }
            Op::True => {
                let k = self.special_const(2);
                self.vs.push(Ent::Const(k));
            }
            Op::False => {
                let k = self.special_const(3);
                self.vs.push(Ent::Const(k));
            }
            Op::Empty => {
                let k = self.special_const(4);
                self.vs.push(Ent::Const(k));
            }
            Op::Pop => {
                self.vs.pop();
            }
            Op::Dup => {
                let i = self.vs.len() - 1;
                let e = match self.vs[i] {
                    Ent::Slot { .. } => Ent::Copy(self.slot(i)),
                    other => other,
                };
                self.vs.push(e);
            }
            Op::GetReg(r) => self.vs.push(Ent::Reg(r)),
            Op::GetRegChecked(r) => {
                let d = self.slot(self.vs.len());
                self.emit_push(Op::MovChecked(d, r));
            }
            Op::SetReg(r) => {
                let (e, d) = self.pop();
                self.store_reg(r, e, d);
            }
            Op::SetRegChecked(r) => {
                let (e, d) = self.pop();
                self.invalidate_reg(r);
                self.emit(Op::CheckInit(r));
                self.store_reg(r, e, d);
            }
            Op::GetLocal(dp, s) => {
                let d = self.slot(self.vs.len());
                self.emit_push(Op::GetLocalR(d, dp, s));
            }
            Op::SetLocal(dp, s) | Op::InitLocal(dp, s) => {
                // Storing the TDZ marker must keep the raw value: the stack
                // instruction does (register operands read it as undefined).
                if let Some(Ent::Const(k)) = self.vs.last() {
                    if matches!(self.constants[*k as usize], Value::Empty) {
                        self.generic(op);
                        return Ok(());
                    }
                }
                let (e, d) = self.pop();
                let v = self.opnd(e, d);
                self.emit(Op::SetLocalR(v, dp, s));
            }
            Op::GetGlobal(_, ic) => {
                let d = self.slot(self.vs.len());
                self.emit_push(Op::GetGlobalR(d, ic));
            }
            Op::SetGlobal(_, ic) => {
                let (e, d) = self.pop();
                let v = self.opnd(e, d);
                self.emit(Op::SetGlobalR(v, ic));
            }
            Op::This => {
                let d = self.slot(self.vs.len());
                self.emit_push(Op::LoadThis(d));
            }
            Op::GetProp(_, ic) => {
                let (e, d) = self.pop();
                let o = self.opnd(e, d);
                let dst = self.slot(d);
                self.emit_push(Op::GetPropR(dst, o, ic));
            }
            Op::GetPropKeep(_, ic) => {
                let i = self.vs.len() - 1;
                let o = self.opnd_keep(i);
                let dst = self.slot(i + 1);
                self.emit_push(Op::GetPropR(dst, o, ic));
            }
            Op::GetMethod(_, ic) => {
                // obj -> method obj
                let i = self.vs.len() - 1;
                if let Ent::Const(_) = self.vs[i] {
                    self.materialize(i);
                }
                match self.vs[i] {
                    Ent::Slot { .. } => {
                        let (s, s1) = (self.slot(i), self.slot(i + 1));
                        let mv = self.emit(Op::MovRaw(s1, s));
                        let at = self.emit(Op::GetPropR(s, s1, ic));
                        self.vs[i] = Ent::Slot { def: at };
                        self.vs.push(Ent::Slot { def: mv });
                    }
                    e @ (Ent::Reg(_) | Ent::Copy(_)) => {
                        let o = match e {
                            Ent::Reg(r) => r,
                            Ent::Copy(s) => s,
                            _ => unreachable!(),
                        };
                        let s = self.slot(i);
                        let at = self.emit(Op::GetPropR(s, o, ic));
                        self.vs[i] = Ent::Slot { def: at };
                        self.vs.push(e);
                    }
                    Ent::Const(_) => unreachable!(),
                }
            }
            Op::SetProp(_, ic) => {
                let (v, dv) = self.pop();
                let (o, d) = self.pop();
                let vo = self.opnd(v, dv);
                let oo = self.opnd(o, d);
                self.emit(Op::SetPropR(oo, vo, ic));
                let v = if let Ent::Const(_) = v { Ent::Slot { def: NO_DEF } } else { v };
                self.push_assign_result(v, dv, next, extra);
            }
            Op::DefineProp(_, ic) => {
                let (v, dv) = self.pop();
                let vo = self.opnd(v, dv);
                let i = self.vs.len() - 1;
                let oo = self.opnd_keep(i);
                self.emit(Op::DefinePropR(oo, vo, ic));
            }
            Op::GetElem => {
                let (k, dk) = self.pop();
                let (o, d) = self.pop();
                let oo = self.opnd(o, d);
                let ko = self.opnd(k, dk);
                let dst = self.slot(d);
                self.emit_push(Op::GetElemR(dst, oo, ko));
            }
            Op::GetElemKeep => {
                let i = self.vs.len() - 2;
                let oo = self.opnd_keep(i);
                let ko = self.opnd_keep(i + 1);
                let dst = self.slot(i + 2);
                self.emit_push(Op::GetElemR(dst, oo, ko));
            }
            Op::SetElem => {
                let (v, dv) = self.pop();
                let (k, dk) = self.pop();
                let (o, d) = self.pop();
                let oo = self.opnd(o, d);
                let ko = self.opnd(k, dk);
                let vo = self.opnd(v, dv);
                self.emit(Op::SetElemR(oo, ko, vo));
                let v = if let Ent::Const(_) = v { Ent::Slot { def: NO_DEF } } else { v };
                self.push_assign_result(v, dv, next, extra);
            }
            Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Mod | Op::Exp | Op::BitAnd | Op::BitOr | Op::BitXor | Op::Shl | Op::Shr | Op::UShr | Op::Lt | Op::LtEq | Op::Gt | Op::GtEq | Op::StrictEq | Op::StrictNotEq | Op::Eq | Op::NotEq => {
                self.binary(op);
            }
            Op::Not | Op::Inc | Op::Dec | Op::ToNumeric | Op::Neg | Op::Plus | Op::BitNot | Op::Typeof | Op::ToString | Op::ToPropertyKey => {
                let (e, d) = self.pop();
                let s = self.opnd(e, d);
                let dst = self.slot(d);
                let r = match op {
                    Op::Not => Op::NotR(dst, s),
                    Op::Inc => Op::IncR(dst, s),
                    Op::Dec => Op::DecR(dst, s),
                    Op::ToNumeric => Op::ToNumericR(dst, s),
                    other => Op::UnR(UN_OPS.iter().position(|o| *o == other).unwrap() as u8, dst, s),
                };
                self.emit_push(r);
            }
            Op::Void => {
                self.vs.pop();
                let k = self.special_const(0);
                self.vs.push(Ent::Const(k));
            }
            Op::Call(argc, desc) => {
                self.flush_top(argc as usize + 2);
                let base = self.vs.len() - argc as usize - 2;
                let b = self.slot(base);
                self.emit(Op::CallR(b, argc, crate::bytecode::NO_CALL_IC, desc));
                self.vs.truncate(base);
                self.vs.push(Ent::Slot { def: NO_DEF });
            }
            Op::New(argc) => {
                self.flush_top(argc as usize + 1);
                let base = self.vs.len() - argc as usize - 1;
                let b = self.slot(base);
                self.emit(Op::NewR(b, argc));
                self.vs.truncate(base);
                self.vs.push(Ent::Slot { def: NO_DEF });
            }
            Op::Return => {
                let (e, d) = self.pop();
                let s = self.opnd(e, d);
                self.emit(Op::ReturnR(s));
            }
            Op::StoreCompletion => {
                let (e, d) = self.pop();
                let s = self.opnd(e, d);
                self.emit(Op::StoreCompletionR(s));
            }
            Op::Jump(_) => {
                self.flush_all();
                self.emit_jump(op);
            }
            Op::JumpIfFalse(t) | Op::JumpIfTrue(t) => {
                let if_true = matches!(op, Op::JumpIfTrue(_));
                let (e, d) = self.pop();
                // Constant condition (`while (true)`).
                if let Ent::Const(k) = e {
                    let truthy = match self.constants[k as usize].v() {
                        V::Bool(b) => Some(b),
                        V::Undefined | V::Null => Some(false),
                        V::Number(n) => Some(!(n == 0.0 || n.is_nan())),
                        _ => None,
                    };
                    if let Some(b) = truthy {
                        if b == if_true {
                            self.flush_all();
                            self.emit_jump(Op::Jump(t));
                        }
                        return Ok(());
                    }
                }
                if let Ent::Slot { def } = e {
                    if def != NO_DEF && def == self.last() {
                        if let Some(fused) = fuse_branch(self.out[def as usize], self.slot(d), if_true, t) {
                            // Drop the boolean-producing instruction; anything
                            // the jump target needs materialized goes first.
                            self.out.pop();
                            self.pos.pop();
                            self.depths.pop();
                            self.flush_all();
                            self.emit_jump(fused);
                            return Ok(());
                        }
                    }
                }
                let s = self.opnd(e, d);
                self.flush_all();
                self.emit_jump(if if_true { Op::JumpIfTrueR(s, t) } else { Op::JumpIfFalseR(s, t) });
            }
            Op::JumpIfNullish(t) | Op::JumpIfUndefined(t) | Op::JumpIfNotUndefined(t) => {
                let (e, d) = self.pop();
                let s = self.opnd(e, d);
                self.flush_all();
                self.emit_jump(match op {
                    Op::JumpIfNullish(_) => Op::JumpIfNullishR(s, t),
                    Op::JumpIfUndefined(_) => Op::JumpIfUndefinedR(s, t),
                    _ => Op::JumpIfNotUndefinedR(s, t),
                });
            }
            Op::JumpIfFalseKeep(t) | Op::JumpIfTrueKeep(t) => {
                self.flush_all();
                let s = self.slot(self.vs.len() - 1);
                self.emit_jump(if matches!(op, Op::JumpIfTrueKeep(_)) { Op::JumpIfTrueR(s, t) } else { Op::JumpIfFalseR(s, t) });
            }
            Op::JumpIfNotNullishKeep(t) => {
                self.flush_all();
                let s = self.slot(self.vs.len() - 1);
                self.emit_jump(Op::JumpIfNotNullishR(s, t));
                self.vs.pop();
            }
            Op::JumpIfStrictEq(t) => {
                let (b, db) = self.pop();
                let (a, da) = self.pop();
                let jump = match (a, b) {
                    (_, Ent::Const(k)) if k <= u16::MAX as u32 => {
                        let ao = self.opnd(a, da);
                        Op::JumpIfStrictEqRK(ao, k as u16, t)
                    }
                    (Ent::Const(k), _) if k <= u16::MAX as u32 => {
                        let bo = self.opnd(b, db);
                        Op::JumpIfStrictEqRK(bo, k as u16, t)
                    }
                    _ => {
                        let ao = self.opnd(a, da);
                        let bo = self.opnd(b, db);
                        Op::JumpIfStrictEqRR(ao, bo, t)
                    }
                };
                self.flush_all();
                self.emit_jump(jump);
            }
            _ => {
                if op.jump_target().is_some() {
                    // Remaining (rare) conditional jumps: canonical stack.
                    self.flush_all();
                    let (p, q) = op.stack_effect();
                    self.emit_jump(op);
                    let len = self.vs.len();
                    self.vs.truncate(len - p as usize);
                    for _ in 0..q {
                        self.vs.push(Ent::Slot { def: NO_DEF });
                    }
                } else {
                    self.generic(op);
                }
            }
        }
        Ok(())
    }

    fn binary(&mut self, op: Op) {
        let (b, db) = self.pop();
        let (a, da) = self.pop();
        let dst = self.slot(da);
        let is_cmp = CMP_OPS.contains(&op);
        let (mut op, mut a, mut b, mut da, mut db) = (op, a, b, da, db);
        // A constant left operand of a commutative operator or a
        // comparison goes to the right.
        if matches!(a, Ent::Const(_)) && !matches!(b, Ent::Const(_)) {
            if let Some(sw) = if is_cmp { swapped_cmp(op) } else if is_commutative(op) { Some(op) } else { None } {
                op = sw;
                std::mem::swap(&mut a, &mut b);
                std::mem::swap(&mut da, &mut db);
            }
        }
        if let Ent::Const(k) = b {
            if k <= u16::MAX as u32 {
                let k = k as u16;
                let ao = self.opnd(a, da);
                let r = match op {
                    Op::Add => Op::AddRK(dst, ao, k),
                    Op::Sub => Op::SubRK(dst, ao, k),
                    Op::Mul => Op::MulRK(dst, ao, k),
                    Op::Div => Op::DivRK(dst, ao, k),
                    Op::Mod => Op::ModRK(dst, ao, k),
                    o if is_cmp => Op::CmpRK(cmp_index(o), dst, ao, k),
                    o => Op::BinRK(bin_index(o), dst, ao, k),
                };
                self.emit_push(r);
                return;
            }
        }
        let ao = self.opnd(a, da);
        let bo = self.opnd(b, db);
        let r = match op {
            Op::Add => Op::AddRR(dst, ao, bo),
            Op::Sub => Op::SubRR(dst, ao, bo),
            Op::Mul => Op::MulRR(dst, ao, bo),
            Op::Div => Op::DivRR(dst, ao, bo),
            Op::Mod => Op::ModRR(dst, ao, bo),
            o if is_cmp => Op::CmpRR(cmp_index(o), dst, ao, bo),
            o => Op::BinRR(bin_index(o), dst, ao, bo),
        };
        self.emit_push(r);
    }
}

fn cmp_index(op: Op) -> u8 {
    CMP_OPS.iter().position(|o| *o == op).unwrap() as u8
}

fn bin_index(op: Op) -> u8 {
    BIN_OPS.iter().position(|o| *o == op).unwrap() as u8
}

/// Destination slot of a register instruction.
fn dst_of(op: Op) -> Option<u16> {
    Some(match op {
        Op::Mov(d, _)
        | Op::MovRaw(d, _)
        | Op::LoadK(d, _)
        | Op::AddRR(d, ..)
        | Op::SubRR(d, ..)
        | Op::MulRR(d, ..)
        | Op::DivRR(d, ..)
        | Op::ModRR(d, ..)
        | Op::AddRK(d, ..)
        | Op::SubRK(d, ..)
        | Op::MulRK(d, ..)
        | Op::DivRK(d, ..)
        | Op::ModRK(d, ..)
        | Op::NotR(d, _)
        | Op::IncR(d, _)
        | Op::DecR(d, _)
        | Op::ToNumericR(d, _)
        | Op::LoadThis(d)
        | Op::GetPropR(d, ..)
        | Op::GetElemR(d, ..)
        | Op::GetGlobalR(d, _)
        | Op::GetLocalR(d, ..) => d,
        Op::BinRR(_, d, ..) | Op::BinRK(_, d, ..) | Op::CmpRR(_, d, ..) | Op::CmpRK(_, d, ..) | Op::UnR(_, d, _) => d,
        _ => return None,
    })
}

/// A conditional jump on the boolean computed by `def` into slot `s`.
fn fuse_branch(def: Op, s: u16, if_true: bool, t: u32) -> Option<Op> {
    if dst_of(def) != Some(s) {
        return None;
    }
    let lt = cmp_index(Op::Lt);
    let le = cmp_index(Op::LtEq);
    let gt = cmp_index(Op::Gt);
    let ge = cmp_index(Op::GtEq);
    let se = cmp_index(Op::StrictEq);
    let sne = cmp_index(Op::StrictNotEq);
    Some(match (def, if_true) {
        (Op::NotR(_, x), _) => {
            // `!x` jumps the other way on x itself.
            if if_true {
                Op::JumpIfFalseR(x, t)
            } else {
                Op::JumpIfTrueR(x, t)
            }
        }
        (Op::CmpRR(k, _, a, b), false) if k == lt => Op::JumpUnlessLtRR(a, b, t),
        (Op::CmpRR(k, _, a, b), false) if k == le => Op::JumpUnlessLtEqRR(a, b, t),
        (Op::CmpRR(k, _, a, b), false) if k == gt => Op::JumpUnlessGtRR(a, b, t),
        (Op::CmpRR(k, _, a, b), false) if k == ge => Op::JumpUnlessGtEqRR(a, b, t),
        (Op::CmpRK(k, _, a, b), false) if k == lt => Op::JumpUnlessLtRK(a, b, t),
        (Op::CmpRK(k, _, a, b), false) if k == le => Op::JumpUnlessLtEqRK(a, b, t),
        (Op::CmpRK(k, _, a, b), false) if k == gt => Op::JumpUnlessGtRK(a, b, t),
        (Op::CmpRK(k, _, a, b), false) if k == ge => Op::JumpUnlessGtEqRK(a, b, t),
        (Op::CmpRR(k, _, a, b), true) if k == lt => Op::JumpIfLtRR(a, b, t),
        (Op::CmpRR(k, _, a, b), true) if k == le => Op::JumpIfLtEqRR(a, b, t),
        (Op::CmpRR(k, _, a, b), true) if k == gt => Op::JumpIfGtRR(a, b, t),
        (Op::CmpRR(k, _, a, b), true) if k == ge => Op::JumpIfGtEqRR(a, b, t),
        (Op::CmpRK(k, _, a, b), true) if k == lt => Op::JumpIfLtRK(a, b, t),
        (Op::CmpRK(k, _, a, b), true) if k == le => Op::JumpIfLtEqRK(a, b, t),
        (Op::CmpRK(k, _, a, b), true) if k == gt => Op::JumpIfGtRK(a, b, t),
        (Op::CmpRK(k, _, a, b), true) if k == ge => Op::JumpIfGtEqRK(a, b, t),
        (Op::CmpRR(k, _, a, b), _) if k == se => {
            if if_true {
                Op::JumpIfStrictEqRR(a, b, t)
            } else {
                Op::JumpIfStrictNotEqRR(a, b, t)
            }
        }
        (Op::CmpRR(k, _, a, b), _) if k == sne => {
            if if_true {
                Op::JumpIfStrictNotEqRR(a, b, t)
            } else {
                Op::JumpIfStrictEqRR(a, b, t)
            }
        }
        (Op::CmpRK(k, _, a, b), _) if k == se => {
            if if_true {
                Op::JumpIfStrictEqRK(a, b, t)
            } else {
                Op::JumpIfStrictNotEqRK(a, b, t)
            }
        }
        (Op::CmpRK(k, _, a, b), _) if k == sne => {
            if if_true {
                Op::JumpIfStrictNotEqRK(a, b, t)
            } else {
                Op::JumpIfStrictEqRK(a, b, t)
            }
        }
        _ => return None,
    })
}
