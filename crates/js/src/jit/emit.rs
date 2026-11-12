//! x86-64 templates for the register bytecode.
//!
//! Fixed registers: `rbx` = [`Ctx`], `r12` = frame slots (reloaded from
//! `ctx.slots` after every helper), `r13` = `TAG_MIN` (first non-number
//! bit pattern, also `undefined`), `r14` = `TAG_BIGINT` (first reference
//! counted pattern), `r15` = constants, `rbp` = frame pointer. Values are
//! never kept in registers across instructions: every instruction reads
//! its operands from the frame and writes its result back, so any
//! instruction boundary is a valid entry or exit point.
//!
//! Every instruction is classified by an exhaustive `match` (no wildcard):
//! a native fast path that falls back to a helper when the types do not
//! fit, a dedicated helper, the generic helper (`h_exec`: the
//! interpreter's own out-of-line code), or "not compilable".
//!
//! Native code never copies or overwrites a reference-counted value
//! (strings, BigInts): such cases go to a helper. JavaScript constants are
//! loaded from the constant table, never embedded as immediates.

use super::helpers::*;
use crate::bytecode::{FunctionCode, Op};
use crate::value::{Value, TAG_MIN};
use rabbithole_jit::x64::*;
use rabbithole_jit::ExecMem;

const CTX: u8 = RBX;
const SLOTS: u8 = R12;
const TAGMIN: u8 = R13;
const RCMIN: u8 = R14;
const CONSTS: u8 = R15;

const TAG_BIGINT: u64 = 0xFFFC_0000_0000_0000;
const CANON_NAN: u64 = 0x7FF8_0000_0000_0000;
const ONE: u64 = 0x3FF0_0000_0000_0000;
const SIGN: u64 = 0x8000_0000_0000_0000;
/// Offsets from `TAG_MIN` of the special values.
const NULL: i32 = 1;
const EMPTY: i32 = 2;
const FALSE: i32 = 4;
const TRUE: i32 = 5;

fn slot(i: u16) -> Mem {
    Mem::base(SLOTS, i as i32 * 8)
}

fn konst(i: u32) -> Mem {
    Mem::base(CONSTS, i as i32 * 8)
}

/// What an instruction compiles to.
enum Class {
    /// A template of its own.
    Native,
    /// A call to `h_exec` (or a dedicated helper) and nothing else.
    Helper(usize, &'static str),
    /// Not compilable (the function stays interpreted).
    Reject(&'static str),
}

/// Stage 2 phase the compiler is at (docs/jit-design.md, section 7):
/// 2 straight-line numeric code and returns, 3 control flow and safe-point
/// ticks, 4 every non-call instruction (generic helper, property and
/// global caches), 5 calls and exceptions. Instructions of later phases
/// make a function not compilable.
pub(crate) const PHASE: u8 = 5;

fn classify(op: &Op) -> Class {
    let c = classify_all(op);
    let phase = match op {
        Op::Jump(..) | Op::IncJumpIfLtRR(..) | Op::IncJumpIfLtRK(..) => 3,
        Op::JumpUnlessLtRR(..) | Op::JumpUnlessLtEqRR(..) | Op::JumpUnlessGtRR(..) | Op::JumpUnlessGtEqRR(..) => 3,
        Op::JumpUnlessLtRK(..) | Op::JumpUnlessLtEqRK(..) | Op::JumpUnlessGtRK(..) | Op::JumpUnlessGtEqRK(..) => 3,
        Op::JumpIfLtRR(..) | Op::JumpIfLtEqRR(..) | Op::JumpIfGtRR(..) | Op::JumpIfGtEqRR(..) => 3,
        Op::JumpIfLtRK(..) | Op::JumpIfLtEqRK(..) | Op::JumpIfGtRK(..) | Op::JumpIfGtEqRK(..) => 3,
        Op::JumpIfTrueR(..) | Op::JumpIfFalseR(..) => 3,
        Op::JumpIfStrictEqRR(..) | Op::JumpIfStrictNotEqRR(..) | Op::JumpIfStrictEqRK(..) | Op::JumpIfStrictNotEqRK(..) => 4,
        Op::JumpIfNullishR(..) | Op::JumpIfNotNullishR(..) | Op::JumpIfUndefinedR(..) | Op::JumpIfNotUndefinedR(..) => 4,
        Op::NotR(..) => 4,
        Op::GetGlobalR(..) | Op::SetGlobalR(..) => 4,
        Op::CallR(..) | Op::NewR(..) => 5,
        Op::ReturnR(..) => 2,
        _ => match c {
            Class::Native => 2,
            Class::Helper(..) => {
                if matches!(op, Op::Call(..) | Op::CallSpread | Op::New(..) | Op::NewSpread | Op::SuperCall(..) | Op::SuperCallSpread | Op::Return | Op::Throw | Op::ThrowConst(..)) {
                    5
                } else {
                    4
                }
            }
            Class::Reject(_) => 0,
        },
    };
    if phase > PHASE {
        return Class::Reject("instruction of a later phase");
    }
    c
}

fn classify_all(op: &Op) -> Class {
    use Class::*;
    let exec = Helper(h_exec as *const () as usize, "h_exec");
    let access = Helper(h_access as *const () as usize, "h_access");
    match op {
        // Native templates.
        Op::Mov(..) | Op::MovRaw(..) | Op::LoadK(..) | Op::MovChecked(..) | Op::CheckInit(..) => Native,
        Op::AddRR(..) | Op::SubRR(..) | Op::MulRR(..) | Op::DivRR(..) | Op::ModRR(..) => Native,
        Op::AddRK(..) | Op::SubRK(..) | Op::MulRK(..) | Op::DivRK(..) | Op::ModRK(..) => Native,
        Op::BinRR(..) | Op::BinRK(..) | Op::CmpRR(..) | Op::CmpRK(..) => Native,
        Op::IncR(..) | Op::DecR(..) | Op::ToNumericR(..) | Op::UnR(..) | Op::NotR(..) => Native,
        Op::Jump(..) => Native,
        Op::JumpUnlessLtRR(..) | Op::JumpUnlessLtEqRR(..) | Op::JumpUnlessGtRR(..) | Op::JumpUnlessGtEqRR(..) => Native,
        Op::JumpUnlessLtRK(..) | Op::JumpUnlessLtEqRK(..) | Op::JumpUnlessGtRK(..) | Op::JumpUnlessGtEqRK(..) => Native,
        Op::JumpIfLtRR(..) | Op::JumpIfLtEqRR(..) | Op::JumpIfGtRR(..) | Op::JumpIfGtEqRR(..) => Native,
        Op::JumpIfLtRK(..) | Op::JumpIfLtEqRK(..) | Op::JumpIfGtRK(..) | Op::JumpIfGtEqRK(..) => Native,
        Op::JumpIfStrictEqRR(..) | Op::JumpIfStrictNotEqRR(..) | Op::JumpIfStrictEqRK(..) | Op::JumpIfStrictNotEqRK(..) => Native,
        Op::JumpIfTrueR(..) | Op::JumpIfFalseR(..) => Native,
        Op::JumpIfNullishR(..) | Op::JumpIfNotNullishR(..) | Op::JumpIfUndefinedR(..) | Op::JumpIfNotUndefinedR(..) => Native,
        Op::IncJumpIfLtRR(..) | Op::IncJumpIfLtRK(..) => Native,
        Op::Nop | Op::Debugger => Native,
        // Dedicated helpers.
        Op::ReturnR(..) => Helper(h_return as *const () as usize, "h_return"),
        Op::CallR(..) | Op::NewR(..) => Native,
        Op::GetGlobalR(..) | Op::SetGlobalR(..) => Native,
        Op::GetPropR(..) => Helper(h_get_prop as *const () as usize, "h_get_prop"),
        Op::SetPropR(..) => Helper(h_set_prop as *const () as usize, "h_set_prop"),
        Op::GetElemR(..) => Helper(h_get_elem as *const () as usize, "h_get_elem"),
        Op::SetElemR(..) => Helper(h_set_elem as *const () as usize, "h_set_elem"),
        Op::DefinePropR(..) | Op::GetLocalR(..) | Op::SetLocalR(..) | Op::LoadThis(..) | Op::StoreCompletionR(..) => access,
        // Suspension points and direct eval stay in the interpreter.
        Op::Yield | Op::Await | Op::AsyncYield | Op::IteratorNextWith => Reject("suspends"),
        Op::CallEval(..) | Op::EvalSite(..) | Op::NewEvalVars | Op::DeclareEvalVar(..) | Op::DeclareEvalGlobalVar(..) => Reject("direct eval"),
        Op::ResumeIsReturn | Op::YieldStarStep(..) | Op::YieldStarCheck | Op::YStarIs(..) | Op::YieldRaw | Op::ResumeMode => Reject("suspends"),
        Op::IteratorStepReg(..) | Op::IteratorRestReg(..) | Op::IteratorCloseIfNotDoneReg(..) | Op::IteratorCloseOnThrow(..) => Reject("writes registers out of line"),
        Op::GetPrivate | Op::GetPrivateKeep | Op::SetPrivate | Op::DefinePrivate | Op::CopyPrivate | Op::TakePrivate => exec,
        // Everything else: the interpreter's out-of-line code.
        Op::Const(..) | Op::Undefined | Op::Null | Op::True | Op::False | Op::Empty | Op::Pop | Op::Dup | Op::Dup2 | Op::Swap | Op::Rot3 | Op::Rot4 => exec,
        Op::GetReg(..) | Op::GetRegChecked(..) | Op::SetReg(..) | Op::SetRegChecked(..) => exec,
        Op::GetLocal(..) | Op::GetLocalChecked(..) | Op::SetLocal(..) | Op::InitLocal(..) | Op::SetLocalChecked(..) | Op::GetImport(..) => exec,
        Op::GetGlobal(..) | Op::SetGlobal(..) | Op::TypeofGlobal(..) | Op::DeclareGlobalVar(..) | Op::DeclareGlobalFunction(..) => exec,
        Op::DeclareGlobalLex(..) | Op::InitGlobalLex(..) | Op::PushScope(..) | Op::PopScope | Op::CopyScope => exec,
        Op::GetProp(..) | Op::SetProp(..) | Op::GetElem | Op::SetElem | Op::GetElemKeep | Op::GetPropKeep(..) | Op::GetMethod(..) => exec,
        Op::DeleteProp(..) | Op::DeleteElem | Op::DefineProp(..) | Op::DefineElem | Op::DefineMethod(..) | Op::DefineGetter(..) => exec,
        Op::DefineSetter(..) | Op::CopyDataProps | Op::CopyDataPropsExcluding | Op::SetProtoLiteral => exec,
        Op::GetSuperProp(..) | Op::SetSuperProp(..) | Op::GetSuperElem | Op::SetSuperElem => exec,
        Op::Call(..) | Op::CallSpread | Op::New(..) | Op::NewSpread | Op::SuperCall(..) | Op::SuperCallSpread => exec,
        Op::Return | Op::Throw | Op::ThrowConst(..) => exec,
        Op::JumpIfFalse(..) | Op::JumpIfTrue(..) | Op::JumpIfNullish(..) | Op::JumpIfNotNullishKeep(..) | Op::JumpIfFalseKeep(..) => exec,
        Op::JumpIfTrueKeep(..) | Op::JumpIfUndefined(..) | Op::JumpIfNotUndefined(..) | Op::JumpIfStrictEq(..) => exec,
        Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Mod | Op::Exp | Op::BitAnd | Op::BitOr | Op::BitXor | Op::Shl | Op::Shr | Op::UShr => exec,
        Op::Eq | Op::NotEq | Op::StrictEq | Op::StrictNotEq | Op::Lt | Op::LtEq | Op::Gt | Op::GtEq | Op::In | Op::Instanceof => exec,
        Op::WithHas(..) | Op::ToObject | Op::Neg | Op::Plus | Op::Not | Op::BitNot | Op::Typeof | Op::Void | Op::Inc | Op::Dec => exec,
        Op::ToNumeric | Op::ToString | Op::ToPropertyKey | Op::PrivateIn(..) | Op::NewObject | Op::NewArray(..) | Op::ArrayPush => exec,
        Op::ArrayHole | Op::ArraySpread | Op::NewRegExp(..) | Op::TemplateObject(..) | Op::MakeClosure(..) | Op::MakeClass(..) => exec,
        Op::SetClassFields | Op::NewPrivateName(..) | Op::InitFields | Op::This | Op::ThisChecked | Op::NewTarget | Op::Arguments => exec,
        Op::ArgsLength | Op::ArgsGet | Op::Callee | Op::ImportMeta | Op::DynamicImport | Op::GetIterator | Op::GetAsyncIterator => exec,
        Op::IteratorNextResult | Op::IteratorNext | Op::IteratorClose | Op::ForInStart | Op::ForInNext | Op::IteratorStep => exec,
        Op::IteratorRest | Op::IteratorCloseIfNotDone | Op::StoreCompletion | Op::LoadCompletion => exec,
    }
}

/// Validate operands the templates use directly (slots, constants, jump
/// targets); helpers index through the interpreter's checked paths.
fn validate(code: &FunctionCode) -> Result<(), String> {
    let n = code.ops.len();
    let fs = code.frame_size();
    let nk = code.constants.len();
    if nk >= 1 << 27 {
        return Err("constant table too large".into());
    }
    let bad = |what: &str, pc: usize| Err(format!("invalid {what} at {pc}"));
    for (pc, op) in code.ops.iter().enumerate() {
        let (slots, k, t): (Vec<u16>, Option<u32>, Option<u32>) = match *op {
            Op::Mov(d, s) | Op::MovRaw(d, s) | Op::MovChecked(d, s) | Op::IncR(d, s) | Op::DecR(d, s) | Op::ToNumericR(d, s) | Op::NotR(d, s) | Op::UnR(_, d, s) => (vec![d, s], None, None),
            Op::CheckInit(r) | Op::ReturnR(r) => (vec![r], None, None),
            Op::LoadK(d, c) => (vec![d], Some(c), None),
            Op::AddRR(d, a, b) | Op::SubRR(d, a, b) | Op::MulRR(d, a, b) | Op::DivRR(d, a, b) | Op::ModRR(d, a, b) | Op::BinRR(_, d, a, b) | Op::CmpRR(_, d, a, b) => (vec![d, a, b], None, None),
            Op::AddRK(d, a, c) | Op::SubRK(d, a, c) | Op::MulRK(d, a, c) | Op::DivRK(d, a, c) | Op::ModRK(d, a, c) | Op::BinRK(_, d, a, c) | Op::CmpRK(_, d, a, c) => (vec![d, a], Some(c as u32), None),
            Op::Jump(t) => (vec![], None, Some(t)),
            Op::JumpUnlessLtRR(a, b, t)
            | Op::JumpUnlessLtEqRR(a, b, t)
            | Op::JumpUnlessGtRR(a, b, t)
            | Op::JumpUnlessGtEqRR(a, b, t)
            | Op::JumpIfLtRR(a, b, t)
            | Op::JumpIfLtEqRR(a, b, t)
            | Op::JumpIfGtRR(a, b, t)
            | Op::JumpIfGtEqRR(a, b, t)
            | Op::JumpIfStrictEqRR(a, b, t)
            | Op::JumpIfStrictNotEqRR(a, b, t)
            | Op::IncJumpIfLtRR(a, b, t) => (vec![a, b], None, Some(t)),
            Op::JumpUnlessLtRK(a, c, t)
            | Op::JumpUnlessLtEqRK(a, c, t)
            | Op::JumpUnlessGtRK(a, c, t)
            | Op::JumpUnlessGtEqRK(a, c, t)
            | Op::JumpIfLtRK(a, c, t)
            | Op::JumpIfLtEqRK(a, c, t)
            | Op::JumpIfGtRK(a, c, t)
            | Op::JumpIfGtEqRK(a, c, t)
            | Op::JumpIfStrictEqRK(a, c, t)
            | Op::JumpIfStrictNotEqRK(a, c, t)
            | Op::IncJumpIfLtRK(a, c, t) => (vec![a], Some(c as u32), Some(t)),
            Op::JumpIfTrueR(s, t) | Op::JumpIfFalseR(s, t) | Op::JumpIfNullishR(s, t) | Op::JumpIfNotNullishR(s, t) | Op::JumpIfUndefinedR(s, t) | Op::JumpIfNotUndefinedR(s, t) => (vec![s], None, Some(t)),
            _ => (vec![], None, None),
        };
        if slots.iter().any(|&s| s as usize >= fs) {
            return bad("slot", pc);
        }
        if k.map(|k| k as usize >= nk).unwrap_or(false) {
            return bad("constant", pc);
        }
        if t.map(|t| t as usize >= n).unwrap_or(false) {
            return bad("jump target", pc);
        }
        if matches!(op, Op::IncJumpIfLtRR(..) | Op::IncJumpIfLtRK(..)) && pc + 2 >= n {
            return bad("loop latch", pc);
        }
    }
    // Execution never falls off the end (every path ends in a return,
    // throw or jump), but the last instruction must not fall through
    // into the out-of-line code either.
    match code.ops.last() {
        Some(Op::ReturnR(_) | Op::Return | Op::Throw | Op::ThrowConst(..) | Op::Jump(_)) => Ok(()),
        _ => Err("last instruction falls through".into()),
    }
}

/// An out-of-line path, emitted after the function body.
enum Stub {
    /// Call `helper(ctx, arg)`, then continue at `resume` (or follow the
    /// status).
    Helper { from: Label, helper: usize, name: &'static str, arg: u32, resume: Label },
}

struct Gen<'a> {
    a: Asm,
    code: &'a FunctionCode,
    labels: Vec<Label>,
    stubs: Vec<Stub>,
    /// Shared: non-zero helper status (exit or internal jump).
    status: Label,
}

impl Gen<'_> {
    fn lbl(&self, pc: usize) -> Label {
        self.labels[pc]
    }

    /// Call `helper(ctx, arg)` inline; on a non-zero status leave through
    /// the status stub; reload the slot base.
    fn call_helper(&mut self, helper: usize, name: &str, arg: u32) {
        self.a.mov_rr64(RDI, CTX);
        self.a.mov_imm32(RSI, arg);
        self.a.call_abs(helper, name);
        self.a.load64(SLOTS, Mem::base(CTX, CTX_SLOTS));
        self.a.test_rr32(RAX, RAX);
        self.a.jcc(CC_NE, self.status);
    }

    /// A jump to an out-of-line call of `helper(ctx, pc)` that continues
    /// at `resume`.
    fn slow(&mut self, helper: usize, name: &'static str, pc: usize, resume: Label) -> Label {
        let from = self.a.label();
        self.stubs.push(Stub::Helper { from, helper, name, arg: pc as u32, resume });
        from
    }

    /// The generic slow path of the instruction at `pc`, continuing with
    /// the next one.
    fn exec_slow(&mut self, pc: usize) -> Label {
        let next = self.lbl(pc + 1);
        self.slow(h_exec as *const () as usize, "h_exec", pc, next)
    }

    /// Decrement the safe-point tick; at zero, `h_tick(ctx, fpc)` and
    /// continue at `resume`.
    fn tick(&mut self, fpc: usize, resume: Label) {
        self.a.load64(RAX, Mem::base(CTX, CTX_TICK));
        self.a.alu_mi32(Alu::Sub, Mem::base(RAX, 0), 1);
        let s = self.slow(h_tick as *const () as usize, "h_tick", fpc, resume);
        self.a.jcc(CC_E, s);
    }

    /// Jump to instruction `t` from instruction `pc` (backward jumps tick).
    fn jump(&mut self, pc: usize, t: usize) {
        if t <= pc {
            let target = self.lbl(t);
            self.tick(t, target);
        }
        let l = self.lbl(t);
        self.a.jmp(l);
    }

    /// Conditional jump to `t` on `cc` (backward jumps tick).
    fn jump_if(&mut self, cc: u8, pc: usize, t: usize) {
        if t > pc {
            let l = self.lbl(t);
            self.a.jcc(cc, l);
        } else {
            let skip = self.a.label();
            self.a.jcc(cc ^ 1, skip);
            self.jump(pc, t);
            self.a.bind(skip);
        }
    }

    /// `rax` = slot `s`, jumping to `slow` unless it is a number.
    fn load_num(&mut self, r: u8, s: u16, slow: Label) {
        self.a.load64(r, slot(s));
        self.a.cmp_rr64(r, TAGMIN);
        self.a.jcc(CC_AE, slow);
    }

    /// `xmm` = constant `k` when it is a number (`false`: it is not).
    fn load_knum(&mut self, x: u8, k: u32) -> bool {
        if !self.code.constants[k as usize].is_number() {
            return false;
        }
        self.a.movsd_load(x, konst(k));
        true
    }

    /// Canonicalize the number bits in `rax` (a NaN pattern in the tag
    /// space becomes the canonical NaN).
    fn canon(&mut self) {
        let ok = self.a.label();
        self.a.cmp_rr64(RAX, TAGMIN);
        self.a.jcc(CC_B, ok);
        self.a.mov_imm64(RAX, CANON_NAN);
        self.a.bind(ok);
    }

    /// Store `rax` (not reference counted) into slot `d`, unless the slot
    /// holds a reference-counted value (then `slow`, which redoes the
    /// whole instruction).
    fn store(&mut self, d: u16, slow: Label) {
        self.a.load64(RDX, slot(d));
        self.a.cmp_rr64(RDX, RCMIN);
        self.a.jcc(CC_AE, slow);
        self.a.store64(slot(d), RAX);
    }

    /// Store the number in `xmm0` into slot `d`.
    fn store_xmm0(&mut self, d: u16, slow: Label) {
        self.a.movq_r_x(RAX, XMM0);
        self.canon();
        self.store(d, slow);
    }

    /// ToInt32 of the number in `xmm` into `r32`, or `slow` (out of range
    /// of the 64-bit truncation, or NaN).
    fn to_int32(&mut self, r: u8, x: u8, slow: Label) {
        self.a.cvttsd2si64(r, x);
        self.a.mov_imm64(RDX, SIGN);
        self.a.cmp_rr64(r, RDX);
        self.a.jcc(CC_E, slow);
    }

    /// Truthiness of `rax` (a slot value): jumps to `yes` or `no`.
    fn truthy(&mut self, yes: Label, no: Label) {
        let nonnum = self.a.label();
        self.a.cmp_rr64(RAX, TAGMIN);
        self.a.jcc(CC_AE, nonnum);
        self.a.movq_x_r(XMM0, RAX);
        self.a.xorpd(XMM1, XMM1);
        self.a.ucomisd(XMM0, XMM1);
        // ZF: equal to zero or unordered (NaN): falsy.
        self.a.jcc(CC_E, no);
        self.a.jmp(yes);
        self.a.bind(nonnum);
        self.a.lea(RCX, Mem::base(TAGMIN, TRUE));
        self.a.cmp_rr64(RAX, RCX);
        self.a.jcc(CC_E, yes);
        // undefined / null / empty / false
        self.a.mov_rr64(RCX, RAX);
        self.a.shr_ri64(RCX, 48);
        self.a.alu_ri32(Alu::Cmp, RCX, 0xFFF9);
        self.a.jcc(CC_E, no);
        // strings, objects, symbols, BigInts: the runtime decides.
        self.a.mov_rr64(RDI, CTX);
        self.a.mov_rr64(RSI, RAX);
        self.a.call_abs(h_truthy as *const () as usize, "h_truthy");
        self.a.test_rr32(RAX, RAX);
        self.a.jcc(CC_NE, yes);
        self.a.jmp(no);
    }

    /// Numeric comparison of `xmm0` (a) and `xmm1` (b): flags such that
    /// `cc` means the relation holds. `rel` 0 `<`, 1 `<=`, 2 `>`, 3 `>=`.
    fn compare(&mut self, rel: u8) -> u8 {
        match rel {
            0 => {
                self.a.ucomisd(XMM1, XMM0);
                CC_A
            }
            1 => {
                self.a.ucomisd(XMM1, XMM0);
                CC_AE
            }
            2 => {
                self.a.ucomisd(XMM0, XMM1);
                CC_A
            }
            _ => {
                self.a.ucomisd(XMM0, XMM1);
                CC_AE
            }
        }
    }

    /// Load operands `a` (slot) and `b` (slot or constant) as numbers into
    /// xmm0 / xmm1, or jump to `slow`. `false`: `b` is a constant that is
    /// not a number (the instruction always takes the slow path).
    fn operands(&mut self, a: u16, b: Result<u16, u32>, slow: Label) -> bool {
        match b {
            Ok(b) => {
                self.load_num(RAX, a, slow);
                self.load_num(RCX, b, slow);
                self.a.movq_x_r(XMM0, RAX);
                self.a.movq_x_r(XMM1, RCX);
            }
            Err(k) => {
                if !self.load_knum(XMM1, k) {
                    return false;
                }
                self.load_num(RAX, a, slow);
                self.a.movq_x_r(XMM0, RAX);
            }
        }
        true
    }

    fn arith(&mut self, pc: usize, sse: Option<u8>, d: u16, a: u16, b: Result<u16, u32>) {
        let slow = if sse == Some(SSE_ADD) {
            let next = self.lbl(pc + 1);
            self.slow(h_add as *const () as usize, "h_add", pc, next)
        } else {
            self.exec_slow(pc)
        };
        if !self.operands(a, b, slow) {
            self.a.jmp(slow);
            return;
        }
        match sse {
            Some(op) => self.a.sse_rr(op, XMM0, XMM1),
            None => self.a.call_abs(h_mod as *const () as usize, "h_mod"),
        }
        self.store_xmm0(d, slow);
    }

    fn bitop(&mut self, pc: usize, kd: u8, d: u16, a: u16, b: Result<u16, u32>) {
        let slow = self.exec_slow(pc);
        if !self.operands(a, b, slow) {
            self.a.jmp(slow);
            return;
        }
        match kd {
            0 => self.a.sse_rr(SSE_ADD, XMM0, XMM1),
            1 => self.a.sse_rr(SSE_SUB, XMM0, XMM1),
            2 => self.a.sse_rr(SSE_MUL, XMM0, XMM1),
            3 => self.a.sse_rr(SSE_DIV, XMM0, XMM1),
            4 => self.a.call_abs(h_mod as *const () as usize, "h_mod"),
            11 => self.a.call_abs(h_pow as *const () as usize, "h_pow"),
            5..=10 => {
                self.to_int32(RAX, XMM0, slow);
                self.to_int32(RCX, XMM1, slow);
                match kd {
                    5 => self.a.alu_rr32(Alu::And, RAX, RCX),
                    6 => self.a.alu_rr32(Alu::Or, RAX, RCX),
                    7 => self.a.alu_rr32(Alu::Xor, RAX, RCX),
                    8 => self.a.shift_cl32(Shift::Shl, RAX),
                    9 => self.a.shift_cl32(Shift::Sar, RAX),
                    _ => self.a.shift_cl32(Shift::Shr, RAX),
                }
                if kd == 10 {
                    // unsigned: zero-extended 32 bits converted as 64-bit
                    self.a.alu_rr32(Alu::Or, RAX, RAX);
                    self.a.cvtsi2sd64(XMM0, RAX);
                } else {
                    self.a.cvtsi2sd32(XMM0, RAX);
                }
            }
            _ => {
                self.a.jmp(slow);
                return;
            }
        }
        self.store_xmm0(d, slow);
    }

    fn cmp(&mut self, pc: usize, kd: u8, d: u16, a: u16, b: Result<u16, u32>) {
        let slow = self.exec_slow(pc);
        let done = self.a.label();
        let (yes, no) = (self.a.label(), self.a.label());
        if kd == 4 || kd == 5 {
            // Strict (in)equality: numbers inline, anything else through
            // the runtime's strict_equals (pure).
            let generic = self.a.label();
            self.a.load64(RAX, slot(a));
            match b {
                Ok(b) => self.a.load64(RCX, slot(b)),
                Err(k) => self.a.load64(RCX, konst(k)),
            }
            self.a.cmp_rr64(RAX, TAGMIN);
            self.a.jcc(CC_AE, generic);
            self.a.cmp_rr64(RCX, TAGMIN);
            self.a.jcc(CC_AE, generic);
            self.a.movq_x_r(XMM0, RAX);
            self.a.movq_x_r(XMM1, RCX);
            self.a.ucomisd(XMM0, XMM1);
            let (eq, ne) = if kd == 4 { (yes, no) } else { (no, yes) };
            self.a.jcc(CC_P, ne);
            self.a.jcc(CC_E, eq);
            self.a.jmp(ne);
            self.a.bind(generic);
            self.a.mov_rr64(RDI, CTX);
            self.a.mov_rr64(RSI, RAX);
            self.a.mov_rr64(RDX, RCX);
            self.a.call_abs(h_strict_eq as *const () as usize, "h_strict_eq");
            self.a.test_rr32(RAX, RAX);
            self.a.jcc(CC_NE, eq);
            self.a.jmp(ne);
        } else {
            if !self.operands(a, b, slow) {
                self.a.jmp(slow);
                return;
            }
            match kd {
                0..=3 => {
                    let cc = self.compare(kd);
                    self.a.jcc(cc, yes);
                    self.a.jmp(no);
                }
                6 => {
                    self.a.ucomisd(XMM0, XMM1);
                    self.a.jcc(CC_P, no);
                    self.a.jcc(CC_E, yes);
                    self.a.jmp(no);
                }
                _ => {
                    self.a.ucomisd(XMM0, XMM1);
                    self.a.jcc(CC_P, yes);
                    self.a.jcc(CC_NE, yes);
                    self.a.jmp(no);
                }
            }
        }
        self.a.bind(yes);
        self.a.lea(RAX, Mem::base(TAGMIN, TRUE));
        self.a.jmp(done);
        self.a.bind(no);
        self.a.lea(RAX, Mem::base(TAGMIN, FALSE));
        self.a.bind(done);
        self.store(d, slow);
    }

    /// Fused compare-and-jump: `jump_when` true jumps when the relation
    /// holds (JumpIf*), false when it does not (JumpUnless*).
    fn cmp_jump(&mut self, pc: usize, rel: u8, jump_when: bool, a: u16, b: Result<u16, u32>, t: u32) {
        let slow = self.exec_slow(pc);
        if !self.operands(a, b, slow) {
            self.a.jmp(slow);
            return;
        }
        let cc = self.compare(rel);
        // NaN: the relation is false (unordered clears `a`/`ae`).
        self.jump_if(if jump_when { cc } else { cc ^ 1 }, pc, t as usize);
    }

    /// `JumpIfStrict(Not)Eq`.
    fn eq_jump(&mut self, pc: usize, eq: bool, a: u16, b: Result<u16, u32>, t: u32) {
        let (yes, no) = (self.a.label(), self.a.label());
        let generic = self.a.label();
        self.a.load64(RAX, slot(a));
        match b {
            Ok(b) => self.a.load64(RCX, slot(b)),
            Err(k) => self.a.load64(RCX, konst(k)),
        }
        self.a.cmp_rr64(RAX, TAGMIN);
        self.a.jcc(CC_AE, generic);
        self.a.cmp_rr64(RCX, TAGMIN);
        self.a.jcc(CC_AE, generic);
        self.a.movq_x_r(XMM0, RAX);
        self.a.movq_x_r(XMM1, RCX);
        self.a.ucomisd(XMM0, XMM1);
        self.a.jcc(CC_P, no);
        self.a.jcc(CC_E, yes);
        self.a.jmp(no);
        self.a.bind(generic);
        // Identical bits of non-strings are equal (strings compare by
        // content, and NaN is handled above).
        self.a.mov_rr64(RDI, CTX);
        self.a.mov_rr64(RSI, RAX);
        self.a.mov_rr64(RDX, RCX);
        self.a.call_abs(h_strict_eq as *const () as usize, "h_strict_eq");
        self.a.test_rr32(RAX, RAX);
        self.a.jcc(CC_NE, yes);
        self.a.jmp(no);
        let (taken, fall) = if eq { (yes, no) } else { (no, yes) };
        self.a.bind(taken);
        self.jump(pc, t as usize);
        self.a.bind(fall);
    }

    /// Move slot `s` to slot `d` (`empty` converts `Empty` to undefined;
    /// `checked` throws on `Empty` through the helper).
    fn mov(&mut self, pc: usize, d: u16, s: u16, empty: bool, checked: bool) {
        let slow = self.exec_slow(pc);
        self.a.load64(RAX, slot(s));
        self.a.cmp_rr64(RAX, RCMIN);
        self.a.jcc(CC_AE, slow);
        if empty || checked {
            let ok = self.a.label();
            self.a.lea(RCX, Mem::base(TAGMIN, EMPTY));
            self.a.cmp_rr64(RAX, RCX);
            if checked {
                self.a.jcc(CC_E, slow);
            } else {
                self.a.jcc(CC_NE, ok);
                self.a.mov_rr64(RAX, TAGMIN);
                self.a.bind(ok);
            }
        }
        self.store(d, slow);
    }

    /// `GetGlobalR` (`Ok(dst)`) / `SetGlobalR` (`Err(src)`) through the
    /// site's cache: script `let`/`const` bindings (`IC_GLOBAL_LEX`) and
    /// global object data properties (`IC_GLOBAL_OBJ`) inline; misses,
    /// reference-counted values, TDZ and constants through the helper.
    fn global(&mut self, pc: usize, ic: u32, dst: Result<u16, u16>) {
        let (h, name) = match dst {
            Ok(_) => (h_get_global as *const () as usize, "h_get_global"),
            Err(_) => (h_set_global as *const () as usize, "h_set_global"),
        };
        let next = self.lbl(pc + 1);
        let slow = self.slow(h, name, pc, next);
        let Some(cell) = self.code.ics.get(ic as usize) else {
            self.a.jmp(slow);
            return;
        };
        let realm_gl = std::mem::offset_of!(crate::realm::Realm, global_lex);
        let gl_epoch = (realm_gl + std::mem::offset_of!(crate::ic::GlobalLex, epoch)) as i32;
        let gl_raw = (realm_gl + std::mem::offset_of!(crate::ic::GlobalLex, raw)) as i32;
        let lex_stride = std::mem::size_of::<(crate::value::Value, bool)>();
        let lex_val = std::mem::offset_of!((crate::value::Value, bool), 0) as i32;
        let lex_const = std::mem::offset_of!((crate::value::Value, bool), 1) as i32;
        let p_value = std::mem::offset_of!(crate::heap::Property, value) as i32;
        let p_flags = std::mem::offset_of!(crate::heap::Property, flags) as i32;
        let ic_kind = std::mem::offset_of!(crate::bytecode::PropIc, kind) as i32;
        let ic_shape = std::mem::offset_of!(crate::bytecode::PropIc, shape) as i32;
        let ic_slot = std::mem::offset_of!(crate::bytecode::PropIc, slot) as i32;
        let ic_holder = std::mem::offset_of!(crate::bytecode::PropIc, holder) as i32;
        let ic_mid = std::mem::offset_of!(crate::bytecode::PropIc, mid) as i32;
        debug_assert_eq!(std::mem::offset_of!(crate::bytecode::PropIc, mid_shape) as i32, ic_mid + 4);
        debug_assert_eq!(lex_stride, 16);
        let (obj, have) = (self.a.label(), self.a.label());
        // rcx = the cache record, rax = realm
        self.a.mov_imm64(RCX, cell as *const std::cell::Cell<crate::bytecode::PropIc> as u64);
        self.a.load64(RAX, Mem::base(CTX, CTX_REALM));
        self.a.load32(RDX, Mem::base(RCX, ic_shape));
        self.a.alu_rm32(Alu::Cmp, RDX, Mem::base(RAX, gl_epoch));
        self.a.jcc(CC_NE, slow);
        self.a.movzx8(RDX, Mem::base(RCX, ic_kind));
        self.a.alu_ri32(Alu::Cmp, RDX, crate::ic::IC_GLOBAL_LEX as i32);
        self.a.jcc(CC_NE, obj);
        // Lexical binding: rdx = &vals[slot]
        self.a.load32(RDX, Mem::base(RCX, ic_slot));
        self.a.alu_rr64(Alu::Add, RDX, RDX);
        self.a.load64(RAX, Mem::base(RAX, gl_raw));
        self.a.lea(RDX, Mem::index(RAX, RDX, 8, 0));
        if dst.is_err() {
            // `const`: the helper throws
            self.a.test_mi8(Mem::base(RDX, lex_const), 1);
            self.a.jcc(CC_NE, slow);
        }
        self.a.lea(RDX, Mem::base(RDX, lex_val));
        self.a.jmp(have);
        // Global object data property: rdx = &property.value
        self.a.bind(obj);
        self.a.alu_ri32(Alu::Cmp, RDX, crate::ic::IC_GLOBAL_OBJ as i32);
        self.a.jcc(CC_NE, slow);
        self.a.mov_imm64(RAX, crate::shape::proto_epoch_addr() as u64);
        self.a.load32(RAX, Mem::base(RAX, 0));
        self.a.alu_rm32(Alu::Cmp, RAX, Mem::base(RCX, ic_holder));
        self.a.jcc(CC_NE, slow);
        self.a.load64(RDX, Mem::base(RCX, ic_mid));
        let mask = if dst.is_ok() { crate::heap::ACCESSOR } else { crate::heap::ACCESSOR | crate::heap::WRITABLE };
        let want = if dst.is_ok() { 0 } else { crate::heap::WRITABLE };
        self.a.movzx8(RAX, Mem::base(RDX, p_flags));
        self.a.alu_ri32(Alu::And, RAX, mask as i32);
        self.a.alu_ri32(Alu::Cmp, RAX, want as i32);
        self.a.jcc(CC_NE, slow);
        self.a.lea(RDX, Mem::base(RDX, p_value));
        // rdx = the binding's value cell
        self.a.bind(have);
        match dst {
            Ok(d) => {
                self.a.load64(RAX, Mem::base(RDX, 0));
                // Uninitialized lexical binding (TDZ) or a counted value.
                self.a.lea(RCX, Mem::base(TAGMIN, EMPTY));
                self.a.cmp_rr64(RAX, RCX);
                self.a.jcc(CC_E, slow);
                self.a.cmp_rr64(RAX, RCMIN);
                self.a.jcc(CC_AE, slow);
                self.store(d, slow);
            }
            Err(s) => {
                self.a.load64(RCX, Mem::base(RDX, 0));
                // TDZ, or the old value is counted
                self.a.lea(RAX, Mem::base(TAGMIN, EMPTY));
                self.a.cmp_rr64(RCX, RAX);
                self.a.jcc(CC_E, slow);
                self.a.cmp_rr64(RCX, RCMIN);
                self.a.jcc(CC_AE, slow);
                self.a.load64(RAX, slot(s));
                self.a.cmp_rr64(RAX, RCMIN);
                self.a.jcc(CC_AE, slow);
                self.a.lea(RCX, Mem::base(TAGMIN, EMPTY));
                self.a.cmp_rr64(RAX, RCX);
                self.a.jcc(CC_E, slow);
                self.a.store64(Mem::base(RDX, 0), RAX);
            }
        }
    }

    fn emit(&mut self, pc: usize, op: Op) {
        match op {
            Op::Mov(d, s) => self.mov(pc, d, s, true, false),
            Op::MovRaw(d, s) => self.mov(pc, d, s, false, false),
            Op::MovChecked(d, s) => self.mov(pc, d, s, false, true),
            Op::LoadK(d, c) => {
                let slow = self.exec_slow(pc);
                if self.code.constants[c as usize].bits() >= TAG_BIGINT {
                    self.a.jmp(slow);
                } else {
                    self.a.load64(RAX, konst(c));
                    self.store(d, slow);
                }
            }
            Op::CheckInit(r) => {
                let slow = self.exec_slow(pc);
                self.a.load64(RAX, slot(r));
                self.a.lea(RCX, Mem::base(TAGMIN, EMPTY));
                self.a.cmp_rr64(RAX, RCX);
                self.a.jcc(CC_E, slow);
            }
            Op::AddRR(d, a, b) => self.arith(pc, Some(SSE_ADD), d, a, Ok(b)),
            Op::SubRR(d, a, b) => self.arith(pc, Some(SSE_SUB), d, a, Ok(b)),
            Op::MulRR(d, a, b) => self.arith(pc, Some(SSE_MUL), d, a, Ok(b)),
            Op::DivRR(d, a, b) => self.arith(pc, Some(SSE_DIV), d, a, Ok(b)),
            Op::ModRR(d, a, b) => self.arith(pc, None, d, a, Ok(b)),
            Op::AddRK(d, a, c) => self.arith(pc, Some(SSE_ADD), d, a, Err(c as u32)),
            Op::SubRK(d, a, c) => self.arith(pc, Some(SSE_SUB), d, a, Err(c as u32)),
            Op::MulRK(d, a, c) => self.arith(pc, Some(SSE_MUL), d, a, Err(c as u32)),
            Op::DivRK(d, a, c) => self.arith(pc, Some(SSE_DIV), d, a, Err(c as u32)),
            Op::ModRK(d, a, c) => self.arith(pc, None, d, a, Err(c as u32)),
            Op::BinRR(kd, d, a, b) => self.bitop(pc, kd, d, a, Ok(b)),
            Op::BinRK(kd, d, a, c) => self.bitop(pc, kd, d, a, Err(c as u32)),
            Op::CmpRR(kd, d, a, b) => self.cmp(pc, kd, d, a, Ok(b)),
            Op::CmpRK(kd, d, a, c) => self.cmp(pc, kd, d, a, Err(c as u32)),
            Op::IncR(d, s) | Op::DecR(d, s) => {
                let slow = self.exec_slow(pc);
                self.load_num(RAX, s, slow);
                self.a.movq_x_r(XMM0, RAX);
                self.a.mov_imm64(RCX, ONE);
                self.a.movq_x_r(XMM1, RCX);
                self.a.sse_rr(if matches!(op, Op::IncR(..)) { SSE_ADD } else { SSE_SUB }, XMM0, XMM1);
                self.store_xmm0(d, slow);
            }
            Op::ToNumericR(d, s) => {
                let slow = self.exec_slow(pc);
                self.load_num(RAX, s, slow);
                self.store(d, slow);
            }
            Op::UnR(k, d, s) => {
                let slow = self.exec_slow(pc);
                match k {
                    0 => {
                        self.load_num(RAX, s, slow);
                        self.a.mov_imm64(RCX, SIGN);
                        self.a.alu_rr64(Alu::Xor, RAX, RCX);
                        self.canon();
                        self.store(d, slow);
                    }
                    1 | 5 => {
                        self.load_num(RAX, s, slow);
                        self.store(d, slow);
                    }
                    2 => {
                        self.load_num(RAX, s, slow);
                        self.a.movq_x_r(XMM0, RAX);
                        self.to_int32(RAX, XMM0, slow);
                        self.a.not32(RAX);
                        self.a.cvtsi2sd32(XMM0, RAX);
                        self.store_xmm0(d, slow);
                    }
                    _ => self.a.jmp(slow),
                }
            }
            Op::NotR(d, s) => {
                let slow = self.exec_slow(pc);
                let (yes, no, done) = (self.a.label(), self.a.label(), self.a.label());
                self.a.load64(RAX, slot(s));
                self.truthy(yes, no);
                self.a.bind(yes);
                self.a.lea(RAX, Mem::base(TAGMIN, FALSE));
                self.a.jmp(done);
                self.a.bind(no);
                self.a.lea(RAX, Mem::base(TAGMIN, TRUE));
                self.a.bind(done);
                self.store(d, slow);
            }
            Op::Jump(t) => self.jump(pc, t as usize),
            Op::JumpUnlessLtRR(a, b, t) => self.cmp_jump(pc, 0, false, a, Ok(b), t),
            Op::JumpUnlessLtEqRR(a, b, t) => self.cmp_jump(pc, 1, false, a, Ok(b), t),
            Op::JumpUnlessGtRR(a, b, t) => self.cmp_jump(pc, 2, false, a, Ok(b), t),
            Op::JumpUnlessGtEqRR(a, b, t) => self.cmp_jump(pc, 3, false, a, Ok(b), t),
            Op::JumpUnlessLtRK(a, c, t) => self.cmp_jump(pc, 0, false, a, Err(c as u32), t),
            Op::JumpUnlessLtEqRK(a, c, t) => self.cmp_jump(pc, 1, false, a, Err(c as u32), t),
            Op::JumpUnlessGtRK(a, c, t) => self.cmp_jump(pc, 2, false, a, Err(c as u32), t),
            Op::JumpUnlessGtEqRK(a, c, t) => self.cmp_jump(pc, 3, false, a, Err(c as u32), t),
            Op::JumpIfLtRR(a, b, t) => self.cmp_jump(pc, 0, true, a, Ok(b), t),
            Op::JumpIfLtEqRR(a, b, t) => self.cmp_jump(pc, 1, true, a, Ok(b), t),
            Op::JumpIfGtRR(a, b, t) => self.cmp_jump(pc, 2, true, a, Ok(b), t),
            Op::JumpIfGtEqRR(a, b, t) => self.cmp_jump(pc, 3, true, a, Ok(b), t),
            Op::JumpIfLtRK(a, c, t) => self.cmp_jump(pc, 0, true, a, Err(c as u32), t),
            Op::JumpIfLtEqRK(a, c, t) => self.cmp_jump(pc, 1, true, a, Err(c as u32), t),
            Op::JumpIfGtRK(a, c, t) => self.cmp_jump(pc, 2, true, a, Err(c as u32), t),
            Op::JumpIfGtEqRK(a, c, t) => self.cmp_jump(pc, 3, true, a, Err(c as u32), t),
            Op::JumpIfStrictEqRR(a, b, t) => self.eq_jump(pc, true, a, Ok(b), t),
            Op::JumpIfStrictNotEqRR(a, b, t) => self.eq_jump(pc, false, a, Ok(b), t),
            Op::JumpIfStrictEqRK(a, c, t) => self.eq_jump(pc, true, a, Err(c as u32), t),
            Op::JumpIfStrictNotEqRK(a, c, t) => self.eq_jump(pc, false, a, Err(c as u32), t),
            Op::JumpIfTrueR(s, t) | Op::JumpIfFalseR(s, t) => {
                let (yes, no) = (self.a.label(), self.a.label());
                self.a.load64(RAX, slot(s));
                self.truthy(yes, no);
                let (taken, fall) = if matches!(op, Op::JumpIfTrueR(..)) { (yes, no) } else { (no, yes) };
                self.a.bind(taken);
                self.jump(pc, t as usize);
                self.a.bind(fall);
            }
            Op::JumpIfNullishR(s, t) | Op::JumpIfNotNullishR(s, t) | Op::JumpIfUndefinedR(s, t) | Op::JumpIfNotUndefinedR(s, t) => {
                // undefined (+0), null (+1), empty (+2) relative to TAG_MIN
                let (yes, no) = (self.a.label(), self.a.label());
                self.a.load64(RAX, slot(s));
                self.a.alu_rr64(Alu::Sub, RAX, TAGMIN);
                let nullish = matches!(op, Op::JumpIfNullishR(..) | Op::JumpIfNotNullishR(..));
                self.a.jcc(CC_E, yes);
                if nullish {
                    self.a.alu_ri64(Alu::Cmp, RAX, NULL);
                    self.a.jcc(CC_E, yes);
                }
                self.a.alu_ri64(Alu::Cmp, RAX, EMPTY);
                self.a.jcc(CC_E, yes);
                self.a.jmp(no);
                let (taken, fall) = if matches!(op, Op::JumpIfNullishR(..) | Op::JumpIfUndefinedR(..)) { (yes, no) } else { (no, yes) };
                self.a.bind(taken);
                self.jump(pc, t as usize);
                self.a.bind(fall);
            }
            Op::IncJumpIfLtRR(r, _, t) | Op::IncJumpIfLtRK(r, _, t) => {
                let next = self.lbl(pc + 1);
                let slow = self.slow(h_inc as *const () as usize, "h_inc", pc, next);
                match op {
                    Op::IncJumpIfLtRR(_, b, _) => {
                        self.load_num(RCX, b, slow);
                        self.a.movq_x_r(XMM1, RCX);
                    }
                    Op::IncJumpIfLtRK(_, c, _) => {
                        if !self.load_knum(XMM1, c as u32) {
                            self.a.jmp(slow);
                            return;
                        }
                    }
                    _ => unreachable!(),
                }
                self.load_num(RAX, r, slow);
                self.a.movq_x_r(XMM0, RAX);
                self.a.mov_imm64(RCX, ONE);
                self.a.movq_x_r(XMM2, RCX);
                self.a.sse_rr(SSE_ADD, XMM0, XMM2);
                self.a.movq_r_x(RAX, XMM0);
                self.canon();
                // The old value was a number: overwrite freely.
                self.a.store64(slot(r), RAX);
                // n < lim: loop; else skip the entry test that follows.
                self.a.ucomisd(XMM1, XMM0);
                let exit = self.lbl(pc + 2);
                self.a.jcc(CC_BE, exit);
                self.jump(pc, t as usize);
            }
            Op::GetGlobalR(d, ic) => self.global(pc, ic, Ok(d)),
            Op::SetGlobalR(s, ic) => self.global(pc, ic, Err(s)),
            Op::Nop | Op::Debugger => {}
            Op::CallR(..) | Op::NewR(..) => {
                // The safe-point tick of a call, then the call helper.
                let here = self.a.label();
                self.tick(pc + 1, here);
                self.a.bind(here);
                let (h, name) = if matches!(op, Op::CallR(..)) { (h_call as *const () as usize, "h_call") } else { (h_new as *const () as usize, "h_new") };
                self.call_helper(h, name, pc as u32);
            }
            other => match classify(&other) {
                Class::Helper(h, name) => self.call_helper(h, name, pc as u32),
                Class::Native => unreachable!("{other:?} has a template"),
                Class::Reject(_) => unreachable!(),
            },
        }
    }
}

/// Offset of the inner entry (past the shared prologue) in every
/// function's code.
pub(super) const INNER_ENTRY: usize = 40;

/// Compile `code` (validated here) into native code.
pub(super) fn compile(code: &FunctionCode, dump: bool) -> Result<(ExecMem, Option<String>), String> {
    for op in &code.ops {
        if let Class::Reject(why) = classify(op) {
            return Err(format!("{why} ({op:?})"));
        }
    }
    validate(code)?;
    let n = code.ops.len();
    let mut a = Asm::new(dump);
    let labels: Vec<Label> = (0..n).map(|_| a.label()).collect();
    let (status, dispatch, epilogue, table, interp) = (a.label(), a.label(), a.label(), a.label(), a.label());
    // Prologue: entry(ctx = rdi, pc = esi).
    a.comment(|| "prologue".into());
    a.push(RBP);
    a.mov_rr64(RBP, RSP);
    for r in [RBX, R12, R13, R14, R15] {
        a.push(r);
    }
    a.alu_ri64(Alu::Sub, RSP, 8);
    a.mov_rr64(CTX, RDI);
    a.mov_imm64(TAGMIN, TAG_MIN);
    a.mov_imm64(RCMIN, TAG_BIGINT);
    // Inner entry (INNER_ENTRY bytes in): the native frame is already set
    // up by some function's prologue (they are all identical) and `ctx`
    // describes this function's frame; `esi` = pc.
    assert_eq!(a.pos(), INNER_ENTRY, "prologue size");
    a.load64(SLOTS, Mem::base(CTX, CTX_SLOTS));
    a.load64(CONSTS, Mem::base(CTX, CTX_CONSTS));
    a.alu_rr32(Alu::Or, RSI, RSI); // zero-extend the pc
    a.bind(dispatch);
    a.alu_ri32(Alu::Cmp, RSI, n as i32);
    a.jcc(CC_AE, interp);
    a.lea_label(RAX, table);
    a.movsxd(RCX, Mem::index(RAX, RSI, 4, 0));
    a.alu_rr64(Alu::Add, RAX, RCX);
    a.jmp_r(RAX);

    let mut g = Gen { a, code, labels, stubs: Vec::new(), status };
    for (pc, op) in code.ops.iter().enumerate() {
        g.a.bind(g.labels[pc]);
        if g.a.listing_enabled() {
            g.a.comment(|| format!("{pc}: {op:?}"));
        }
        g.emit(pc, *op);
    }
    // Defensive: execution never reaches here (validated).
    g.a.int3();
    // Out-of-line paths.
    let stubs = std::mem::take(&mut g.stubs);
    for s in stubs {
        match s {
            Stub::Helper { from, helper, name, arg, resume } => {
                g.a.bind(from);
                g.call_helper(helper, name, arg);
                g.a.jmp(resume);
            }
        }
    }
    let mut a = g.a;
    // Non-zero status: an internal jump continues at ctx.pc; after a call
    // or return the new last frame continues in its own native code when
    // it has some (h_switch rewrites ctx); anything else returns to the run
    // loop.
    a.comment(|| "status".into());
    a.bind(status);
    let (jump, reload) = (a.label(), a.label());
    a.alu_ri32(Alu::Cmp, RAX, ST_JUMP as i32);
    a.jcc(CC_E, jump);
    a.alu_ri32(Alu::Cmp, RAX, ST_RELOAD as i32);
    a.jcc(CC_NE, epilogue);
    a.mov_rr64(RDI, CTX);
    a.call_abs(h_switch as *const () as usize, "h_switch");
    a.test_rr64(RAX, RAX);
    a.jcc(CC_E, reload);
    a.load32(RSI, Mem::base(CTX, CTX_PC));
    a.jmp_r(RAX);
    a.bind(reload);
    a.mov_imm32(RAX, ST_RELOAD);
    a.jmp(epilogue);
    a.bind(jump);
    a.load32(RSI, Mem::base(CTX, CTX_PC));
    a.jmp(dispatch);
    a.bind(interp);
    a.mov_imm32(RAX, ST_INTERP);
    a.bind(epilogue);
    a.comment(|| "epilogue".into());
    a.alu_ri64(Alu::Add, RSP, 8);
    for r in [R15, R14, R13, R12, RBX] {
        a.pop(r);
    }
    a.pop(RBP);
    a.ret();
    // Entry table: offset of each instruction from the table.
    a.align(4);
    a.bind(table);
    let base = a.pos() as i64;
    for l in &g.labels {
        let off = a.label_offset(*l).expect("instruction label") as i64 - base;
        a.d32(off as i32);
    }
    a.finish_labels();
    let listing = a.listing();
    let t = std::time::Instant::now();
    let mem = ExecMem::new(&a.buf, |_, _| {}).ok_or("executable memory unavailable")?;
    super::MAP_NS.with(|m| m.set(m.get() + t.elapsed().as_nanos() as u64));
    Ok((mem, listing))
}

#[allow(dead_code)]
fn _value_layout_checks() {
    // The templates depend on the NaN-boxing layout.
    const _: () = assert!(std::mem::size_of::<Value>() == 8);
    const _: () = assert!(TAG_MIN == 0xFFF9_0000_0000_0000);
}
