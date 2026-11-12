//! The interpreter's internal representation.
//!
//! Each function body is translated once (at module compilation, together
//! with validation) into a flat array of fixed-size three-address
//! [`Instr`]uctions over *frame slots*: 64-bit cells addressed relative to
//! the frame base. A frame is laid out as
//!
//! ```text
//! [ params | declared locals | constants | operand stack ... ]
//! ```
//!
//! Every operand stack position has a static slot (wasm stack heights are
//! known at every instruction), so `i32.add` becomes `I32Add a <- b, c`
//! with all three slots resolved at translation time. `local.get` and
//! constants do not move values: the consumer reads the local's or the
//! constant's slot directly. A `v128` takes two consecutive slots.
//!
//! Calls are zero-copy: the arguments are materialized at the top of the
//! caller's operand stack and the callee's frame starts right there; the
//! callee leaves its results at its own slots `0..n`, which are exactly the
//! caller's result positions. Branch targets are instruction indices,
//! resolved at translation time; there is no decoding at run time.
//!
//! The format is deliberately simple to feed a template JIT later (see
//! docs/jit-design.md): each instruction maps to a short machine code
//! template over `[fp + slot*8]` operands.

/// Frame slot index (relative to the frame base).
pub type Slot = u32;

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct Instr {
    pub op: Op,
    /// Small immediate: SIMD sub-opcode, lane index, load kind...
    pub x: u16,
    pub a: u32,
    pub b: u32,
    pub c: u32,
}

/// Operand roles of each opcode (see [`sig`]): `D` destination slot, `S`
/// source slot (may be a constant slot), `B` base slot of consecutive
/// materialized operands, `I` immediate, `T` branch target, `_` unused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sig {
    /// a = dst, b, c = sources (single-slot pure result)
    Dss,
    /// a = dst, b = source, c = immediate (single-slot pure result)
    Dsi,
    /// a = dst, b, c = immediates (single-slot pure result)
    Dii,
    /// a, b = sources, c = immediate
    Ssi,
    /// a = source, b, c = immediates
    Sii,
    /// a, b = sources, c = target
    Sst,
    /// a = source, c = target
    Sit,
    /// target in c
    T,
    /// a = base, b, c = immediates
    Bii,
    /// No slot operands that need fixing; handled specially.
    Other,
    /// a = dst (may be 2 slots), b, c = sources (any width): not rewritable
    Vss,
    /// a = dst, b = source, c = immediate: not rewritable
    Vsi,
    /// a = source, b = source, c = immediate (stores of v128...)
    Vssi,
}

macro_rules! ops {
    ($($(#[$m:meta])* $name:ident = $sig:ident,)*) => {
        #[repr(u16)]
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum Op { $($(#[$m])* $name,)* }
        pub fn sig(op: Op) -> Sig {
            match op { $(Op::$name => Sig::$sig,)* }
        }
    };
}

ops! {
    // ------------------------------------------------------------ control
    Unreachable = Other,
    Br = T,
    BrIfNez = Sit,
    BrIfEqz = Sit,
    /// a = index, b = first entry in `br_tables`, c = entry count (the
    /// last entry is the default)
    BrTable = Sii,
    Return = Other,
    /// a = base, b = function index
    Call = Bii,
    /// a = base, b = table, c = call type
    CallIndirect = Bii,
    /// a = base, c = call type
    CallRef = Bii,
    ReturnCall = Bii,
    /// `Call` that first copies its arguments: x = slot count, a = base,
    /// b = function index, c = first source slot in `arg_lists`
    CallArgs = Other,
    /// Copy one slot (a) to slot 0 and return
    Return1 = Other,
    ReturnCallIndirect = Bii,
    ReturnCallRef = Bii,
    /// a = base of the payload, b = tag index
    Throw = Bii,
    /// a = exnref
    ThrowRef = Sii,
    BrOnNull = Sit,
    BrOnNonNull = Sit,
    BrI32Eq = Sst,
    BrI32Ne = Sst,
    BrI32LtS = Sst,
    BrI32LtU = Sst,
    BrI32GtS = Sst,
    BrI32GtU = Sst,
    BrI32LeS = Sst,
    BrI32LeU = Sst,
    BrI32GeS = Sst,
    BrI32GeU = Sst,
    BrI64Eq = Sst,
    BrI64Ne = Sst,
    BrI64LtS = Sst,
    BrI64LtU = Sst,
    BrI64GtS = Sst,
    BrI64GtU = Sst,
    BrI64LeS = Sst,
    BrI64LeU = Sst,
    BrI64GeS = Sst,
    BrI64GeU = Sst,
    BrI64Eqz = Sit,
    BrI64Nez = Sit,

    // ------------------------------------------------------------ moves
    Copy = Dsi,
    Copy2 = Vsi,
    /// a = dst, b = low 32 bits, c = high 32 bits
    Const = Dii,
    /// a = dst (holds the first value), b = second value, c = condition
    Select = Other,
    Select2 = Other,
    GlobalGet = Dii,
    GlobalSet = Sii,
    GlobalGet2 = Other,
    GlobalSet2 = Other,

    // ------------------------------------------------------------ memory 0
    I32Load = Dsi,
    I64Load = Dsi,
    I32Load8S = Dsi,
    I32Load8U = Dsi,
    I32Load16S = Dsi,
    I32Load16U = Dsi,
    I64Load8S = Dsi,
    I64Load8U = Dsi,
    I64Load16S = Dsi,
    I64Load16U = Dsi,
    I64Load32S = Dsi,
    I64Load32U = Dsi,
    I32Store = Ssi,
    I64Store = Ssi,
    I32Store8 = Ssi,
    I32Store16 = Ssi,
    I64Store32 = Ssi,
    /// Any memory / 64-bit address / large offset: x = kind, a = dst,
    /// b = address, c = memarg index
    MemLoad = Vsi,
    /// x = kind, a = address, b = value, c = memarg index
    MemStore = Vssi,
    /// a = dst, b = memory
    MemorySize = Other,
    /// a = dst, b = delta, c = memory
    MemoryGrow = Other,
    /// a = base, b = memory
    MemoryFill = Other,
    /// a = base, b = dst memory, c = src memory
    MemoryCopy = Other,
    /// a = base, b = segment, c = memory
    MemoryInit = Other,
    DataDrop = Other,

    // ------------------------------------------------------------ tables
    /// a = dst, b = index, c = table
    TableGet = Other,
    /// a = index, b = value, c = table
    TableSet = Other,
    TableSize = Other,
    /// a = base (init, n), c = table
    TableGrow = Other,
    TableFill = Other,
    /// a = base, b = dst table, c = src table
    TableCopy = Other,
    /// a = base, b = segment, c = table
    TableInit = Other,
    ElemDrop = Other,

    // ------------------------------------------------------------ refs
    RefIsNull = Dsi,
    /// a = dst, b = function index
    RefFunc = Dii,
    RefAsNonNull = Sii,
    RefEq = Dss,

    // ------------------------------------------------------------ i32
    I32Eqz = Dsi,
    I32Eq = Dss,
    I32Ne = Dss,
    I32LtS = Dss,
    I32LtU = Dss,
    I32GtS = Dss,
    I32GtU = Dss,
    I32LeS = Dss,
    I32LeU = Dss,
    I32GeS = Dss,
    I32GeU = Dss,
    I32Clz = Dsi,
    I32Ctz = Dsi,
    I32Popcnt = Dsi,
    I32Add = Dss,
    I32Sub = Dss,
    I32Mul = Dss,
    I32DivS = Dss,
    I32DivU = Dss,
    I32RemS = Dss,
    I32RemU = Dss,
    I32And = Dss,
    I32Or = Dss,
    I32Xor = Dss,
    I32Shl = Dss,
    I32ShrS = Dss,
    I32ShrU = Dss,
    I32Rotl = Dss,
    I32Rotr = Dss,

    // ------------------------------------------------------------ i64
    I64Eqz = Dsi,
    I64Eq = Dss,
    I64Ne = Dss,
    I64LtS = Dss,
    I64LtU = Dss,
    I64GtS = Dss,
    I64GtU = Dss,
    I64LeS = Dss,
    I64LeU = Dss,
    I64GeS = Dss,
    I64GeU = Dss,
    I64Clz = Dsi,
    I64Ctz = Dsi,
    I64Popcnt = Dsi,
    I64Add = Dss,
    I64Sub = Dss,
    I64Mul = Dss,
    I64DivS = Dss,
    I64DivU = Dss,
    I64RemS = Dss,
    I64RemU = Dss,
    I64And = Dss,
    I64Or = Dss,
    I64Xor = Dss,
    I64Shl = Dss,
    I64ShrS = Dss,
    I64ShrU = Dss,
    I64Rotl = Dss,
    I64Rotr = Dss,

    // ------------------------------------------------------------ f32
    F32Eq = Dss,
    F32Ne = Dss,
    F32Lt = Dss,
    F32Gt = Dss,
    F32Le = Dss,
    F32Ge = Dss,
    F32Abs = Dsi,
    F32Neg = Dsi,
    F32Ceil = Dsi,
    F32Floor = Dsi,
    F32Trunc = Dsi,
    F32Nearest = Dsi,
    F32Sqrt = Dsi,
    F32Add = Dss,
    F32Sub = Dss,
    F32Mul = Dss,
    F32Div = Dss,
    F32Min = Dss,
    F32Max = Dss,
    F32Copysign = Dss,

    // ------------------------------------------------------------ f64
    F64Eq = Dss,
    F64Ne = Dss,
    F64Lt = Dss,
    F64Gt = Dss,
    F64Le = Dss,
    F64Ge = Dss,
    F64Abs = Dsi,
    F64Neg = Dsi,
    F64Ceil = Dsi,
    F64Floor = Dsi,
    F64Trunc = Dsi,
    F64Nearest = Dsi,
    F64Sqrt = Dsi,
    F64Add = Dss,
    F64Sub = Dss,
    F64Mul = Dss,
    F64Div = Dss,
    F64Min = Dss,
    F64Max = Dss,
    F64Copysign = Dss,

    // ------------------------------------------------------------ conversions
    I32WrapI64 = Dsi,
    I32TruncF32S = Dsi,
    I32TruncF32U = Dsi,
    I32TruncF64S = Dsi,
    I32TruncF64U = Dsi,
    I64ExtendI32S = Dsi,
    I64ExtendI32U = Dsi,
    I64TruncF32S = Dsi,
    I64TruncF32U = Dsi,
    I64TruncF64S = Dsi,
    I64TruncF64U = Dsi,
    F32ConvertI32S = Dsi,
    F32ConvertI32U = Dsi,
    F32ConvertI64S = Dsi,
    F32ConvertI64U = Dsi,
    F32DemoteF64 = Dsi,
    F64ConvertI32S = Dsi,
    F64ConvertI32U = Dsi,
    F64ConvertI64S = Dsi,
    F64ConvertI64U = Dsi,
    F64PromoteF32 = Dsi,
    I32Extend8S = Dsi,
    I32Extend16S = Dsi,
    I64Extend8S = Dsi,
    I64Extend16S = Dsi,
    I64Extend32S = Dsi,
    I32TruncSatF32S = Dsi,
    I32TruncSatF32U = Dsi,
    I32TruncSatF64S = Dsi,
    I32TruncSatF64U = Dsi,
    I64TruncSatF32S = Dsi,
    I64TruncSatF32U = Dsi,
    I64TruncSatF64S = Dsi,
    I64TruncSatF64U = Dsi,

    // ------------------------------------------------------------ SIMD
    /// x = sub-opcode; a = dst, b, c = sources
    Simd = Vss,
    /// x = sub-opcode; a = base of three consecutive operands (and dst)
    Simd3 = Other,
    /// x = sub-opcode | lane << 8; a = dst, b = address (or base for lane
    /// loads), c = memarg index
    SimdLoad = Other,
    /// x = sub-opcode | lane << 8; a = address, b = value, c = memarg index
    SimdStore = Other,
    /// a = base of two consecutive v128, b = shuffle immediate index
    Shuffle = Other,

    // ------------------------------------------------------------ misc
    /// Legacy `rethrow`: a = slot holding the caught exception
    Rethrow = Sii,
    /// GC instructions: x = sub-opcode (see `gc.rs`)
    Gc = Other,
    /// a = reference, b = cast index, c = target
    BrOnCast = Other,
    BrOnCastFail = Other,
    /// Threads proposal: x = sub-opcode, a = base, c = memarg index
    Atomic = Other,
}

impl Instr {
    #[inline]
    pub fn new(op: Op, a: u32, b: u32, c: u32) -> Instr {
        Instr { op, x: 0, a, b, c }
    }
    #[inline]
    pub fn with_x(op: Op, x: u16, a: u32, b: u32, c: u32) -> Instr {
        Instr { op, x, a, b, c }
    }
}

/// Exception handler of a `try_table` (or legacy `try`) region.
#[derive(Clone, Debug)]
pub struct Handler {
    /// Instruction range covered (half open).
    pub start: u32,
    pub end: u32,
    pub kind: CatchKind,
    /// Tag index (for `Catch`/`CatchRef`).
    pub tag: u32,
    /// Where the payload (and exnref) go.
    pub dst: Slot,
    /// Branch target (`u32::MAX - 1`: return from the function).
    pub target: u32,
    /// Nesting depth of the handler's block (for `delegate`).
    pub depth: u32,
    /// Legacy `catch`: slot receiving the exception (for `rethrow`).
    pub exn_slot: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatchKind {
    Catch,
    CatchRef,
    CatchAll,
    CatchAllRef,
    /// Legacy `delegate`: rethrow to the handlers enclosing `target`'s
    /// label (stored as a handler depth search from this one).
    Delegate,
}

/// A memory argument that does not fit the fast path.
#[derive(Clone, Copy, Debug)]
pub struct MemArg {
    pub memory: u32,
    pub offset: u64,
}

/// A translated function.
#[derive(Debug, Default)]
pub struct Code {
    pub instrs: Box<[Instr]>,
    /// Values of the constant slots, copied into `[const_base..]` on entry.
    pub consts: Box<[u64]>,
    pub param_slots: u32,
    /// Params plus declared locals.
    pub local_slots: u32,
    pub const_base: u32,
    /// Total frame size in slots.
    pub frame_slots: u32,
    pub result_slots: u32,
    pub br_tables: Box<[u32]>,
    /// (canonical type id, param slots) of call_indirect / call_ref sites.
    pub call_types: Box<[(u32, u32)]>,
    pub memargs: Box<[MemArg]>,
    pub v128s: Box<[u128]>,
    /// Source slots of `CallArgs` arguments.
    pub arg_lists: Box<[u32]>,
    pub handlers: Box<[Handler]>,
    /// (heap type with canonical ids, nullable) of GC casts.
    pub casts: Box<[(crate::types::HeapType, bool)]>,
    /// Module function index (for names in traces).
    pub func_index: u32,
    /// Native code (compiled on first use; see `jit.rs`).
    pub jit: std::cell::OnceCell<Option<crate::jit::Compiled>>,
}
