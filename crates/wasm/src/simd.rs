//! Fixed-width SIMD (v128): validation/translation and a scalar
//! implementation.

use crate::binary::{malformed, Result};
use crate::compile::FC;
use crate::ir::*;
use crate::runtime::{MemInst, Trap};
use crate::types::ValType;
use crate::validate::invalid;

/// Text names of the SIMD instructions and their `0xfd` sub-opcodes.
pub const SIMD_NAMES: &[(&str, u32)] = &[
    ("v128.load", 0),
    ("v128.load8x8_s", 1),
    ("v128.load8x8_u", 2),
    ("v128.load16x4_s", 3),
    ("v128.load16x4_u", 4),
    ("v128.load32x2_s", 5),
    ("v128.load32x2_u", 6),
    ("v128.load8_splat", 7),
    ("v128.load16_splat", 8),
    ("v128.load32_splat", 9),
    ("v128.load64_splat", 10),
    ("v128.store", 11),
    ("v128.const", 12),
    ("i8x16.shuffle", 13),
    ("i8x16.swizzle", 14),
    ("i8x16.splat", 15),
    ("i16x8.splat", 16),
    ("i32x4.splat", 17),
    ("i64x2.splat", 18),
    ("f32x4.splat", 19),
    ("f64x2.splat", 20),
    ("i8x16.extract_lane_s", 21),
    ("i8x16.extract_lane_u", 22),
    ("i8x16.replace_lane", 23),
    ("i16x8.extract_lane_s", 24),
    ("i16x8.extract_lane_u", 25),
    ("i16x8.replace_lane", 26),
    ("i32x4.extract_lane", 27),
    ("i32x4.replace_lane", 28),
    ("i64x2.extract_lane", 29),
    ("i64x2.replace_lane", 30),
    ("f32x4.extract_lane", 31),
    ("f32x4.replace_lane", 32),
    ("f64x2.extract_lane", 33),
    ("f64x2.replace_lane", 34),
    ("v128.not", 77),
    ("v128.and", 78),
    ("v128.andnot", 79),
    ("v128.or", 80),
    ("v128.xor", 81),
    ("v128.bitselect", 82),
    ("v128.any_true", 83),
    ("v128.load8_lane", 84),
    ("v128.load16_lane", 85),
    ("v128.load32_lane", 86),
    ("v128.load64_lane", 87),
    ("v128.store8_lane", 88),
    ("v128.store16_lane", 89),
    ("v128.store32_lane", 90),
    ("v128.store64_lane", 91),
    ("v128.load32_zero", 92),
    ("v128.load64_zero", 93),
    ("f32x4.demote_f64x2_zero", 94),
    ("f64x2.promote_low_f32x4", 95),
    ("i8x16.abs", 96),
    ("i8x16.neg", 97),
    ("i8x16.popcnt", 98),
    ("i8x16.all_true", 99),
    ("i8x16.bitmask", 100),
    ("i8x16.narrow_i16x8_s", 101),
    ("i8x16.narrow_i16x8_u", 102),
    ("f32x4.ceil", 103),
    ("f32x4.floor", 104),
    ("f32x4.trunc", 105),
    ("f32x4.nearest", 106),
    ("i8x16.shl", 107),
    ("i8x16.shr_s", 108),
    ("i8x16.shr_u", 109),
    ("i8x16.add", 110),
    ("i8x16.add_sat_s", 111),
    ("i8x16.add_sat_u", 112),
    ("i8x16.sub", 113),
    ("i8x16.sub_sat_s", 114),
    ("i8x16.sub_sat_u", 115),
    ("f64x2.ceil", 116),
    ("f64x2.floor", 117),
    ("i8x16.min_s", 118),
    ("i8x16.min_u", 119),
    ("i8x16.max_s", 120),
    ("i8x16.max_u", 121),
    ("f64x2.trunc", 122),
    ("i8x16.avgr_u", 123),
    ("i16x8.extadd_pairwise_i8x16_s", 124),
    ("i16x8.extadd_pairwise_i8x16_u", 125),
    ("i32x4.extadd_pairwise_i16x8_s", 126),
    ("i32x4.extadd_pairwise_i16x8_u", 127),
    ("i16x8.abs", 128),
    ("i16x8.neg", 129),
    ("i16x8.q15mulr_sat_s", 130),
    ("i16x8.all_true", 131),
    ("i16x8.bitmask", 132),
    ("i16x8.narrow_i32x4_s", 133),
    ("i16x8.narrow_i32x4_u", 134),
    ("i16x8.extend_low_i8x16_s", 135),
    ("i16x8.extend_high_i8x16_s", 136),
    ("i16x8.extend_low_i8x16_u", 137),
    ("i16x8.extend_high_i8x16_u", 138),
    ("i16x8.shl", 139),
    ("i16x8.shr_s", 140),
    ("i16x8.shr_u", 141),
    ("i16x8.add", 142),
    ("i16x8.add_sat_s", 143),
    ("i16x8.add_sat_u", 144),
    ("i16x8.sub", 145),
    ("i16x8.sub_sat_s", 146),
    ("i16x8.sub_sat_u", 147),
    ("f64x2.nearest", 148),
    ("i16x8.mul", 149),
    ("i16x8.min_s", 150),
    ("i16x8.min_u", 151),
    ("i16x8.max_s", 152),
    ("i16x8.max_u", 153),
    ("i16x8.avgr_u", 155),
    ("i16x8.extmul_low_i8x16_s", 156),
    ("i16x8.extmul_high_i8x16_s", 157),
    ("i16x8.extmul_low_i8x16_u", 158),
    ("i16x8.extmul_high_i8x16_u", 159),
    ("i32x4.abs", 160),
    ("i32x4.neg", 161),
    ("i32x4.all_true", 163),
    ("i32x4.bitmask", 164),
    ("i32x4.extend_low_i16x8_s", 167),
    ("i32x4.extend_high_i16x8_s", 168),
    ("i32x4.extend_low_i16x8_u", 169),
    ("i32x4.extend_high_i16x8_u", 170),
    ("i32x4.shl", 171),
    ("i32x4.shr_s", 172),
    ("i32x4.shr_u", 173),
    ("i32x4.add", 174),
    ("i32x4.sub", 177),
    ("i32x4.mul", 181),
    ("i32x4.min_s", 182),
    ("i32x4.min_u", 183),
    ("i32x4.max_s", 184),
    ("i32x4.max_u", 185),
    ("i32x4.dot_i16x8_s", 186),
    ("i32x4.extmul_low_i16x8_s", 188),
    ("i32x4.extmul_high_i16x8_s", 189),
    ("i32x4.extmul_low_i16x8_u", 190),
    ("i32x4.extmul_high_i16x8_u", 191),
    ("i64x2.abs", 192),
    ("i64x2.neg", 193),
    ("i64x2.all_true", 195),
    ("i64x2.bitmask", 196),
    ("i64x2.extend_low_i32x4_s", 199),
    ("i64x2.extend_high_i32x4_s", 200),
    ("i64x2.extend_low_i32x4_u", 201),
    ("i64x2.extend_high_i32x4_u", 202),
    ("i64x2.shl", 203),
    ("i64x2.shr_s", 204),
    ("i64x2.shr_u", 205),
    ("i64x2.add", 206),
    ("i64x2.sub", 209),
    ("i64x2.mul", 213),
    ("i64x2.eq", 214),
    ("i64x2.ne", 215),
    ("i64x2.lt_s", 216),
    ("i64x2.gt_s", 217),
    ("i64x2.le_s", 218),
    ("i64x2.ge_s", 219),
    ("i64x2.extmul_low_i32x4_s", 220),
    ("i64x2.extmul_high_i32x4_s", 221),
    ("i64x2.extmul_low_i32x4_u", 222),
    ("i64x2.extmul_high_i32x4_u", 223),
    ("f32x4.abs", 224),
    ("f32x4.neg", 225),
    ("f32x4.sqrt", 227),
    ("f32x4.add", 228),
    ("f32x4.sub", 229),
    ("f32x4.mul", 230),
    ("f32x4.div", 231),
    ("f32x4.min", 232),
    ("f32x4.max", 233),
    ("f32x4.pmin", 234),
    ("f32x4.pmax", 235),
    ("f64x2.abs", 236),
    ("f64x2.neg", 237),
    ("f64x2.sqrt", 239),
    ("f64x2.add", 240),
    ("f64x2.sub", 241),
    ("f64x2.mul", 242),
    ("f64x2.div", 243),
    ("f64x2.min", 244),
    ("f64x2.max", 245),
    ("f64x2.pmin", 246),
    ("f64x2.pmax", 247),
    ("i32x4.trunc_sat_f32x4_s", 248),
    ("i32x4.trunc_sat_f32x4_u", 249),
    ("f32x4.convert_i32x4_s", 250),
    ("f32x4.convert_i32x4_u", 251),
    ("i32x4.trunc_sat_f64x2_s_zero", 252),
    ("i32x4.trunc_sat_f64x2_u_zero", 253),
    ("f64x2.convert_low_i32x4_s", 254),
    ("f64x2.convert_low_i32x4_u", 255),
    ("i8x16.relaxed_swizzle", 256),
    ("i32x4.relaxed_trunc_f32x4_s", 257),
    ("i32x4.relaxed_trunc_f32x4_u", 258),
    ("i32x4.relaxed_trunc_f64x2_s_zero", 259),
    ("i32x4.relaxed_trunc_f64x2_u_zero", 260),
    ("f32x4.relaxed_madd", 261),
    ("f32x4.relaxed_nmadd", 262),
    ("f64x2.relaxed_madd", 263),
    ("f64x2.relaxed_nmadd", 264),
    ("i8x16.relaxed_laneselect", 265),
    ("i16x8.relaxed_laneselect", 266),
    ("i32x4.relaxed_laneselect", 267),
    ("i64x2.relaxed_laneselect", 268),
    ("f32x4.relaxed_min", 269),
    ("f32x4.relaxed_max", 270),
    ("f64x2.relaxed_min", 271),
    ("f64x2.relaxed_max", 272),
    ("i16x8.relaxed_q15mulr_s", 273),
    ("i16x8.relaxed_dot_i8x16_i7x16_s", 274),
    ("i32x4.relaxed_dot_i8x16_i7x16_add_s", 275),
    ("i8x16.eq", 35),
    ("i8x16.ne", 36),
    ("i8x16.lt_s", 37),
    ("i8x16.lt_u", 38),
    ("i8x16.gt_s", 39),
    ("i8x16.gt_u", 40),
    ("i8x16.le_s", 41),
    ("i8x16.le_u", 42),
    ("i8x16.ge_s", 43),
    ("i8x16.ge_u", 44),
    ("i16x8.eq", 45),
    ("i16x8.ne", 46),
    ("i16x8.lt_s", 47),
    ("i16x8.lt_u", 48),
    ("i16x8.gt_s", 49),
    ("i16x8.gt_u", 50),
    ("i16x8.le_s", 51),
    ("i16x8.le_u", 52),
    ("i16x8.ge_s", 53),
    ("i16x8.ge_u", 54),
    ("i32x4.eq", 55),
    ("i32x4.ne", 56),
    ("i32x4.lt_s", 57),
    ("i32x4.lt_u", 58),
    ("i32x4.gt_s", 59),
    ("i32x4.gt_u", 60),
    ("i32x4.le_s", 61),
    ("i32x4.le_u", 62),
    ("i32x4.ge_s", 63),
    ("i32x4.ge_u", 64),
    ("f32x4.eq", 65),
    ("f32x4.ne", 66),
    ("f32x4.lt", 67),
    ("f32x4.gt", 68),
    ("f32x4.le", 69),
    ("f32x4.ge", 70),
    ("f64x2.eq", 71),
    ("f64x2.ne", 72),
    ("f64x2.lt", 73),
    ("f64x2.gt", 74),
    ("f64x2.le", 75),
    ("f64x2.ge", 76),
];


/// Sub-opcode of `v128.const` materialized from `Code::v128s`.
pub const V128_CONST: u16 = 12;

/// Lane index position in `Instr::x` (sub-opcode in the low bits).
pub const LANE_SHIFT: u16 = 9;

#[derive(Clone, Copy, PartialEq, Debug)]
enum K {
    Bin,
    Un,
    ToI32,
    Shift,
    Splat(ValType),
    Extract(ValType, u8),
    Replace(ValType, u8),
    Tern,
    Load,
    Store,
    LoadLane(u8),
    StoreLane(u8),
}

fn kind(sub: u32) -> Option<K> {
    use ValType::*;
    use K::*;
    Some(match sub {
        0..=10 | 92 | 93 => Load,
        11 => Store,
        14 => Bin,
        15..=17 => Splat(I32),
        18 => Splat(I64),
        19 => Splat(F32),
        20 => Splat(F64),
        21 | 22 => Extract(I32, 16),
        23 => Replace(I32, 16),
        24 | 25 => Extract(I32, 8),
        26 => Replace(I32, 8),
        27 => Extract(I32, 4),
        28 => Replace(I32, 4),
        29 => Extract(I64, 2),
        30 => Replace(I64, 2),
        31 => Extract(F32, 4),
        32 => Replace(F32, 4),
        33 => Extract(F64, 2),
        34 => Replace(F64, 2),
        35..=76 => Bin,
        77 => Un,
        78..=81 => Bin,
        82 => Tern,
        83 => ToI32,
        84 => LoadLane(16),
        85 => LoadLane(8),
        86 => LoadLane(4),
        87 => LoadLane(2),
        88 => StoreLane(16),
        89 => StoreLane(8),
        90 => StoreLane(4),
        91 => StoreLane(2),
        94..=98 => Un,
        99 | 100 => ToI32,
        101 | 102 => Bin,
        103..=106 => Un,
        107..=109 => Shift,
        110..=115 => Bin,
        116 | 117 => Un,
        118..=121 => Bin,
        122 => Un,
        123 => Bin,
        124..=129 => Un,
        130 => Bin,
        131 | 132 => ToI32,
        133 | 134 => Bin,
        135..=138 => Un,
        139..=141 => Shift,
        142..=147 => Bin,
        148 => Un,
        149..=153 | 155..=159 => Bin,
        160 | 161 => Un,
        163 | 164 => ToI32,
        167..=170 => Un,
        171..=173 => Shift,
        174 | 177 | 181..=186 | 188..=191 => Bin,
        192 | 193 => Un,
        195 | 196 => ToI32,
        199..=202 => Un,
        203..=205 => Shift,
        206 | 209 | 213..=223 => Bin,
        224 | 225 | 227 => Un,
        228..=235 => Bin,
        236 | 237 | 239 => Un,
        240..=247 => Bin,
        248..=255 => Un,
        256 => Bin,
        257..=260 => Un,
        261..=268 => Tern,
        269..=274 => Bin,
        275 => Tern,
        _ => return None,
    })
}

fn natural(sub: u32) -> u32 {
    match sub {
        0 | 11 => 16,
        1..=6 | 10 | 93 => 8,
        7 | 84 | 88 => 1,
        8 | 85 | 89 => 2,
        9 | 86 | 90 | 92 => 4,
        _ => 8,
    }
}

pub(crate) fn simd_op(fc: &mut FC) -> Result<()> {
    let sub = fc.r.u32()?;
    let v = ValType::V128;
    let x = sub as u16;
    if sub == 12 {
        let c = fc.r.v128()?;
        fc.push_v128_const(c);
        return Ok(());
    }
    if sub == 13 {
        let mut lanes = [0u8; 16];
        for l in lanes.iter_mut() {
            *l = fc.r.byte()?;
            if *l >= 32 {
                return invalid("invalid lane index");
            }
        }
        fc.materialize_top(2);
        fc.pop_expect(v)?;
        let a = fc.pop_expect(v)?;
        let idx = fc.v128s.len() as u32;
        fc.v128s.push(u128::from_le_bytes(lanes));
        let d = fc.push(v);
        let _ = a;
        fc.emit(Instr::with_x(Op::Shuffle, x, d, idx, 0));
        return Ok(());
    }
    let Some(k) = kind(sub) else { return malformed(format!("illegal opcode 0xfd {sub}")) };
    match k {
        K::Bin => {
            let b = fc.pop_expect(v)?;
            let a = fc.pop_expect(v)?;
            let d = fc.push(v);
            fc.emit(Instr::with_x(Op::Simd, x, d, a.src(), b.src()));
        }
        K::Un => {
            let a = fc.pop_expect(v)?;
            let d = fc.push(v);
            fc.emit(Instr::with_x(Op::Simd, x, d, a.src(), 0));
        }
        K::ToI32 => {
            let a = fc.pop_expect(v)?;
            let d = fc.push(ValType::I32);
            fc.emit(Instr::with_x(Op::Simd, x, d, a.src(), 0));
        }
        K::Shift => {
            let b = fc.pop_expect(ValType::I32)?;
            let a = fc.pop_expect(v)?;
            let d = fc.push(v);
            fc.emit(Instr::with_x(Op::Simd, x, d, a.src(), b.src()));
        }
        K::Splat(t) => {
            let a = fc.pop_expect(t)?;
            let d = fc.push(v);
            fc.emit(Instr::with_x(Op::Simd, x, d, a.src(), 0));
        }
        K::Extract(t, n) => {
            let lane = fc.r.byte()?;
            if lane >= n {
                return invalid("invalid lane index");
            }
            let a = fc.pop_expect(v)?;
            let d = fc.push(t);
            fc.emit(Instr::with_x(Op::Simd, x | (lane as u16) << LANE_SHIFT, d, a.src(), 0));
        }
        K::Replace(t, n) => {
            let lane = fc.r.byte()?;
            if lane >= n {
                return invalid("invalid lane index");
            }
            let b = fc.pop_expect(t)?;
            let a = fc.pop_expect(v)?;
            let d = fc.push(v);
            fc.emit(Instr::with_x(Op::Simd, x | (lane as u16) << LANE_SHIFT, d, a.src(), b.src()));
        }
        K::Tern => {
            fc.materialize_top(3);
            fc.pop_expect(v)?;
            fc.pop_expect(v)?;
            fc.pop_expect(v)?;
            let d = fc.push(v);
            fc.emit(Instr::with_x(Op::Simd3, x, d, 0, 0));
        }
        K::Load => {
            let (mem, offset) = fc.memarg(natural(sub))?;
            let at = fc.addr_type(mem);
            let a = fc.pop_expect(at)?;
            let d = fc.push(v);
            let mi = fc.memarg_index(mem, offset);
            fc.emit(Instr::with_x(Op::SimdLoad, x, d, a.src(), mi));
        }
        K::Store => {
            let (mem, offset) = fc.memarg(natural(sub))?;
            let at = fc.addr_type(mem);
            let b = fc.pop_expect(v)?;
            let a = fc.pop_expect(at)?;
            let mi = fc.memarg_index(mem, offset);
            fc.emit(Instr::with_x(Op::SimdStore, x, a.src(), b.src(), mi));
        }
        K::LoadLane(n) => {
            let (mem, offset) = fc.memarg(natural(sub))?;
            let lane = fc.r.byte()?;
            if lane >= n {
                return invalid("invalid lane index");
            }
            let at = fc.addr_type(mem);
            fc.materialize_top(2);
            fc.pop_expect(v)?;
            let a = fc.pop_expect(at)?;
            let d = fc.push(v);
            let mi = fc.memarg_index(mem, offset);
            fc.emit(Instr::with_x(Op::SimdLoad, x | (lane as u16) << LANE_SHIFT, d, a.slot, mi));
        }
        K::StoreLane(n) => {
            let (mem, offset) = fc.memarg(natural(sub))?;
            let lane = fc.r.byte()?;
            if lane >= n {
                return invalid("invalid lane index");
            }
            let at = fc.addr_type(mem);
            let b = fc.pop_expect(v)?;
            let a = fc.pop_expect(at)?;
            let mi = fc.memarg_index(mem, offset);
            fc.emit(Instr::with_x(Op::SimdStore, x | (lane as u16) << LANE_SHIFT, a.src(), b.src(), mi));
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ execution

#[inline]
unsafe fn rv(regs: *mut u64, s: u32) -> u128 {
    *regs.add(s as usize) as u128 | (*regs.add(s as usize + 1) as u128) << 64
}

#[inline]
unsafe fn wv(regs: *mut u64, s: u32, v: u128) {
    *regs.add(s as usize) = v as u64;
    *regs.add(s as usize + 1) = (v >> 64) as u64;
}

macro_rules! lanes {
    ($name:ident, $from:ident, $t:ty, $n:expr, $sz:expr) => {
        #[inline]
        fn $name(v: u128) -> [$t; $n] {
            let b = v.to_le_bytes();
            let mut out = [<$t>::default(); $n];
            for (i, o) in out.iter_mut().enumerate() {
                let mut a = [0u8; $sz];
                a.copy_from_slice(&b[i * $sz..(i + 1) * $sz]);
                *o = <$t>::from_le_bytes(a);
            }
            out
        }
        #[inline]
        fn $from(l: [$t; $n]) -> u128 {
            let mut b = [0u8; 16];
            for (i, x) in l.iter().enumerate() {
                b[i * $sz..(i + 1) * $sz].copy_from_slice(&x.to_le_bytes());
            }
            u128::from_le_bytes(b)
        }
    };
}

lanes!(i8s, from_i8s, i8, 16, 1);
lanes!(u8s, from_u8s, u8, 16, 1);
lanes!(i16s, from_i16s, i16, 8, 2);
lanes!(u16s, from_u16s, u16, 8, 2);
lanes!(i32s, from_i32s, i32, 4, 4);
lanes!(u32s, from_u32s, u32, 4, 4);
lanes!(i64s, from_i64s, i64, 2, 8);
lanes!(u64s, from_u64s, u64, 2, 8);
lanes!(f32s, from_f32s, f32, 4, 4);
lanes!(f64s, from_f64s, f64, 2, 8);

fn map<T: Copy, const N: usize>(a: [T; N], f: impl Fn(T) -> T) -> [T; N] {
    let mut o = a;
    for x in o.iter_mut() {
        *x = f(*x);
    }
    o
}

fn zip<T: Copy, const N: usize>(a: [T; N], b: [T; N], f: impl Fn(T, T) -> T) -> [T; N] {
    let mut o = a;
    for i in 0..N {
        o[i] = f(a[i], b[i]);
    }
    o
}

fn cmp<T: Copy, const N: usize>(a: [T; N], b: [T; N], f: impl Fn(T, T) -> bool) -> [bool; N] {
    let mut o = [false; N];
    for i in 0..N {
        o[i] = f(a[i], b[i]);
    }
    o
}

fn mask<const N: usize>(m: [bool; N]) -> u128 {
    let w = 128 / N;
    let lane: u128 = if w == 128 { u128::MAX } else { (1u128 << w) - 1 };
    let mut v = 0u128;
    for (i, &b) in m.iter().enumerate() {
        if b {
            v |= lane << (i * w);
        }
    }
    v
}

fn fmin32(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        return f32::from_bits(0x7fc0_0000);
    }
    if a == b {
        return f32::from_bits(a.to_bits() | b.to_bits());
    }
    a.min(b)
}
fn fmax32(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        return f32::from_bits(0x7fc0_0000);
    }
    if a == b {
        return f32::from_bits(a.to_bits() & b.to_bits());
    }
    a.max(b)
}
fn fmin64(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return f64::from_bits(0x7ff8_0000_0000_0000);
    }
    if a == b {
        return f64::from_bits(a.to_bits() | b.to_bits());
    }
    a.min(b)
}
fn fmax64(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return f64::from_bits(0x7ff8_0000_0000_0000);
    }
    if a == b {
        return f64::from_bits(a.to_bits() & b.to_bits());
    }
    a.max(b)
}

fn sat_i8(x: i32) -> i8 {
    x.clamp(i8::MIN as i32, i8::MAX as i32) as i8
}
fn sat_u8(x: i32) -> u8 {
    x.clamp(0, u8::MAX as i32) as u8
}
fn sat_i16(x: i32) -> i16 {
    x.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}
fn sat_u16(x: i32) -> u16 {
    x.clamp(0, u16::MAX as i32) as u16
}

fn swizzle(a: u128, s: u128) -> u128 {
    let a = u8s(a);
    let s = u8s(s);
    let mut o = [0u8; 16];
    for i in 0..16 {
        o[i] = if (s[i] as usize) < 16 { a[s[i] as usize] } else { 0 };
    }
    from_u8s(o)
}

fn q15(a: i16, b: i16) -> i16 {
    sat_i16(((a as i32 * b as i32) + 0x4000) >> 15)
}

/// Binary/unary/lane SIMD operations.
///
/// # Safety
/// `regs` must point at a frame covering the instruction's slots.
pub unsafe fn exec(code: &Code, i: Instr, regs: *mut u64) {
    let sub = i.x & ((1 << LANE_SHIFT) - 1);
    let lane = (i.x >> LANE_SHIFT) as usize;
    let a = || rv(regs, i.b);
    let b = || rv(regs, i.c);
    let sc = |s: u32| *regs.add(s as usize);
    let r: u128 = match sub {
        V128_CONST => code.v128s[i.b as usize],
        14 => swizzle(a(), b()),
        15 => from_u8s([sc(i.b) as u8; 16]),
        16 => from_u16s([sc(i.b) as u16; 8]),
        17 => from_u32s([sc(i.b) as u32; 4]),
        18 => from_u64s([sc(i.b); 2]),
        19 => from_u32s([sc(i.b) as u32; 4]),
        20 => from_u64s([sc(i.b); 2]),
        21 => {
            *regs.add(i.a as usize) = i8s(a())[lane] as i32 as u32 as u64;
            return;
        }
        22 => {
            *regs.add(i.a as usize) = u8s(a())[lane] as u64;
            return;
        }
        24 => {
            *regs.add(i.a as usize) = i16s(a())[lane] as i32 as u32 as u64;
            return;
        }
        25 => {
            *regs.add(i.a as usize) = u16s(a())[lane] as u64;
            return;
        }
        27 | 31 => {
            *regs.add(i.a as usize) = u32s(a())[lane] as u64;
            return;
        }
        29 | 33 => {
            *regs.add(i.a as usize) = u64s(a())[lane];
            return;
        }
        23 => {
            let mut l = u8s(a());
            l[lane] = sc(i.c) as u8;
            from_u8s(l)
        }
        26 => {
            let mut l = u16s(a());
            l[lane] = sc(i.c) as u16;
            from_u16s(l)
        }
        28 | 32 => {
            let mut l = u32s(a());
            l[lane] = sc(i.c) as u32;
            from_u32s(l)
        }
        30 | 34 => {
            let mut l = u64s(a());
            l[lane] = sc(i.c);
            from_u64s(l)
        }
        // comparisons
        35 => mask(cmp(i8s(a()), i8s(b()), |x, y| x == y)),
        36 => mask(cmp(i8s(a()), i8s(b()), |x, y| x != y)),
        37 => mask(cmp(i8s(a()), i8s(b()), |x, y| x < y)),
        38 => mask(cmp(u8s(a()), u8s(b()), |x, y| x < y)),
        39 => mask(cmp(i8s(a()), i8s(b()), |x, y| x > y)),
        40 => mask(cmp(u8s(a()), u8s(b()), |x, y| x > y)),
        41 => mask(cmp(i8s(a()), i8s(b()), |x, y| x <= y)),
        42 => mask(cmp(u8s(a()), u8s(b()), |x, y| x <= y)),
        43 => mask(cmp(i8s(a()), i8s(b()), |x, y| x >= y)),
        44 => mask(cmp(u8s(a()), u8s(b()), |x, y| x >= y)),
        45 => mask(cmp(i16s(a()), i16s(b()), |x, y| x == y)),
        46 => mask(cmp(i16s(a()), i16s(b()), |x, y| x != y)),
        47 => mask(cmp(i16s(a()), i16s(b()), |x, y| x < y)),
        48 => mask(cmp(u16s(a()), u16s(b()), |x, y| x < y)),
        49 => mask(cmp(i16s(a()), i16s(b()), |x, y| x > y)),
        50 => mask(cmp(u16s(a()), u16s(b()), |x, y| x > y)),
        51 => mask(cmp(i16s(a()), i16s(b()), |x, y| x <= y)),
        52 => mask(cmp(u16s(a()), u16s(b()), |x, y| x <= y)),
        53 => mask(cmp(i16s(a()), i16s(b()), |x, y| x >= y)),
        54 => mask(cmp(u16s(a()), u16s(b()), |x, y| x >= y)),
        55 => mask(cmp(i32s(a()), i32s(b()), |x, y| x == y)),
        56 => mask(cmp(i32s(a()), i32s(b()), |x, y| x != y)),
        57 => mask(cmp(i32s(a()), i32s(b()), |x, y| x < y)),
        58 => mask(cmp(u32s(a()), u32s(b()), |x, y| x < y)),
        59 => mask(cmp(i32s(a()), i32s(b()), |x, y| x > y)),
        60 => mask(cmp(u32s(a()), u32s(b()), |x, y| x > y)),
        61 => mask(cmp(i32s(a()), i32s(b()), |x, y| x <= y)),
        62 => mask(cmp(u32s(a()), u32s(b()), |x, y| x <= y)),
        63 => mask(cmp(i32s(a()), i32s(b()), |x, y| x >= y)),
        64 => mask(cmp(u32s(a()), u32s(b()), |x, y| x >= y)),
        65 => mask(cmp(f32s(a()), f32s(b()), |x, y| x == y)),
        66 => mask(cmp(f32s(a()), f32s(b()), |x, y| x != y)),
        67 => mask(cmp(f32s(a()), f32s(b()), |x, y| x < y)),
        68 => mask(cmp(f32s(a()), f32s(b()), |x, y| x > y)),
        69 => mask(cmp(f32s(a()), f32s(b()), |x, y| x <= y)),
        70 => mask(cmp(f32s(a()), f32s(b()), |x, y| x >= y)),
        71 => mask(cmp(f64s(a()), f64s(b()), |x, y| x == y)),
        72 => mask(cmp(f64s(a()), f64s(b()), |x, y| x != y)),
        73 => mask(cmp(f64s(a()), f64s(b()), |x, y| x < y)),
        74 => mask(cmp(f64s(a()), f64s(b()), |x, y| x > y)),
        75 => mask(cmp(f64s(a()), f64s(b()), |x, y| x <= y)),
        76 => mask(cmp(f64s(a()), f64s(b()), |x, y| x >= y)),
        77 => !a(),
        78 => a() & b(),
        79 => a() & !b(),
        80 => a() | b(),
        81 => a() ^ b(),
        83 => {
            *regs.add(i.a as usize) = (a() != 0) as u64;
            return;
        }
        94 => {
            let l = f64s(a());
            from_f32s([l[0] as f32, l[1] as f32, 0.0, 0.0])
        }
        95 => {
            let l = f32s(a());
            from_f64s([l[0] as f64, l[1] as f64])
        }
        96 => from_i8s(map(i8s(a()), |x| x.wrapping_abs())),
        97 => from_i8s(map(i8s(a()), |x| x.wrapping_neg())),
        98 => from_u8s(map(u8s(a()), |x| x.count_ones() as u8)),
        99 => {
            *regs.add(i.a as usize) = u8s(a()).iter().all(|&x| x != 0) as u64;
            return;
        }
        100 => {
            let l = i8s(a());
            let mut m = 0u64;
            for (k, x) in l.iter().enumerate() {
                if *x < 0 {
                    m |= 1 << k;
                }
            }
            *regs.add(i.a as usize) = m;
            return;
        }
        101 | 102 => {
            let (x, y) = (i16s(a()), i16s(b()));
            let mut o = [0u8; 16];
            for k in 0..8 {
                o[k] = if sub == 101 { sat_i8(x[k] as i32) as u8 } else { sat_u8(x[k] as i32) };
                o[k + 8] = if sub == 101 { sat_i8(y[k] as i32) as u8 } else { sat_u8(y[k] as i32) };
            }
            from_u8s(o)
        }
        103 => from_f32s(map(f32s(a()), |x| crate::interp::q32(x.ceil()))),
        104 => from_f32s(map(f32s(a()), |x| crate::interp::q32(x.floor()))),
        105 => from_f32s(map(f32s(a()), |x| crate::interp::q32(x.trunc()))),
        106 => from_f32s(map(f32s(a()), |x| crate::interp::q32(x.round_ties_even()))),
        107 => {
            let s = sc(i.c) as u32 & 7;
            from_u8s(map(u8s(a()), |x| x << s))
        }
        108 => {
            let s = sc(i.c) as u32 & 7;
            from_i8s(map(i8s(a()), |x| x >> s))
        }
        109 => {
            let s = sc(i.c) as u32 & 7;
            from_u8s(map(u8s(a()), |x| x >> s))
        }
        110 => from_u8s(zip(u8s(a()), u8s(b()), |x, y| x.wrapping_add(y))),
        111 => from_i8s(zip(i8s(a()), i8s(b()), |x, y| x.saturating_add(y))),
        112 => from_u8s(zip(u8s(a()), u8s(b()), |x, y| x.saturating_add(y))),
        113 => from_u8s(zip(u8s(a()), u8s(b()), |x, y| x.wrapping_sub(y))),
        114 => from_i8s(zip(i8s(a()), i8s(b()), |x, y| x.saturating_sub(y))),
        115 => from_u8s(zip(u8s(a()), u8s(b()), |x, y| x.saturating_sub(y))),
        116 => from_f64s(map(f64s(a()), |x| crate::interp::q64(x.ceil()))),
        117 => from_f64s(map(f64s(a()), |x| crate::interp::q64(x.floor()))),
        118 => from_i8s(zip(i8s(a()), i8s(b()), |x, y| x.min(y))),
        119 => from_u8s(zip(u8s(a()), u8s(b()), |x, y| x.min(y))),
        120 => from_i8s(zip(i8s(a()), i8s(b()), |x, y| x.max(y))),
        121 => from_u8s(zip(u8s(a()), u8s(b()), |x, y| x.max(y))),
        122 => from_f64s(map(f64s(a()), |x| crate::interp::q64(x.trunc()))),
        123 => from_u8s(zip(u8s(a()), u8s(b()), |x, y| ((x as u16 + y as u16 + 1) >> 1) as u8)),
        124 => {
            let l = i8s(a());
            let mut o = [0i16; 8];
            for k in 0..8 {
                o[k] = l[2 * k] as i16 + l[2 * k + 1] as i16;
            }
            from_i16s(o)
        }
        125 => {
            let l = u8s(a());
            let mut o = [0u16; 8];
            for k in 0..8 {
                o[k] = l[2 * k] as u16 + l[2 * k + 1] as u16;
            }
            from_u16s(o)
        }
        126 => {
            let l = i16s(a());
            let mut o = [0i32; 4];
            for k in 0..4 {
                o[k] = l[2 * k] as i32 + l[2 * k + 1] as i32;
            }
            from_i32s(o)
        }
        127 => {
            let l = u16s(a());
            let mut o = [0u32; 4];
            for k in 0..4 {
                o[k] = l[2 * k] as u32 + l[2 * k + 1] as u32;
            }
            from_u32s(o)
        }
        128 => from_i16s(map(i16s(a()), |x| x.wrapping_abs())),
        129 => from_i16s(map(i16s(a()), |x| x.wrapping_neg())),
        130 | 273 => from_i16s(zip(i16s(a()), i16s(b()), q15)),
        131 => {
            *regs.add(i.a as usize) = u16s(a()).iter().all(|&x| x != 0) as u64;
            return;
        }
        132 => {
            let l = i16s(a());
            let mut m = 0u64;
            for (k, x) in l.iter().enumerate() {
                if *x < 0 {
                    m |= 1 << k;
                }
            }
            *regs.add(i.a as usize) = m;
            return;
        }
        133 | 134 => {
            let (x, y) = (i32s(a()), i32s(b()));
            let mut o = [0u16; 8];
            for k in 0..4 {
                o[k] = if sub == 133 { sat_i16(x[k]) as u16 } else { sat_u16(x[k]) };
                o[k + 4] = if sub == 133 { sat_i16(y[k]) as u16 } else { sat_u16(y[k]) };
            }
            from_u16s(o)
        }
        135 | 136 => {
            let l = i8s(a());
            let off = if sub == 135 { 0 } else { 8 };
            let mut o = [0i16; 8];
            for k in 0..8 {
                o[k] = l[off + k] as i16;
            }
            from_i16s(o)
        }
        137 | 138 => {
            let l = u8s(a());
            let off = if sub == 137 { 0 } else { 8 };
            let mut o = [0u16; 8];
            for k in 0..8 {
                o[k] = l[off + k] as u16;
            }
            from_u16s(o)
        }
        139 => {
            let s = sc(i.c) as u32 & 15;
            from_u16s(map(u16s(a()), |x| x << s))
        }
        140 => {
            let s = sc(i.c) as u32 & 15;
            from_i16s(map(i16s(a()), |x| x >> s))
        }
        141 => {
            let s = sc(i.c) as u32 & 15;
            from_u16s(map(u16s(a()), |x| x >> s))
        }
        142 => from_u16s(zip(u16s(a()), u16s(b()), |x, y| x.wrapping_add(y))),
        143 => from_i16s(zip(i16s(a()), i16s(b()), |x, y| x.saturating_add(y))),
        144 => from_u16s(zip(u16s(a()), u16s(b()), |x, y| x.saturating_add(y))),
        145 => from_u16s(zip(u16s(a()), u16s(b()), |x, y| x.wrapping_sub(y))),
        146 => from_i16s(zip(i16s(a()), i16s(b()), |x, y| x.saturating_sub(y))),
        147 => from_u16s(zip(u16s(a()), u16s(b()), |x, y| x.saturating_sub(y))),
        148 => from_f64s(map(f64s(a()), |x| crate::interp::q64(x.round_ties_even()))),
        149 => from_u16s(zip(u16s(a()), u16s(b()), |x, y| x.wrapping_mul(y))),
        150 => from_i16s(zip(i16s(a()), i16s(b()), |x, y| x.min(y))),
        151 => from_u16s(zip(u16s(a()), u16s(b()), |x, y| x.min(y))),
        152 => from_i16s(zip(i16s(a()), i16s(b()), |x, y| x.max(y))),
        153 => from_u16s(zip(u16s(a()), u16s(b()), |x, y| x.max(y))),
        155 => from_u16s(zip(u16s(a()), u16s(b()), |x, y| ((x as u32 + y as u32 + 1) >> 1) as u16)),
        156..=159 => {
            let off = if sub == 156 || sub == 158 { 0 } else { 8 };
            let signed = sub == 156 || sub == 157;
            let mut o = [0u16; 8];
            if signed {
                let (x, y) = (i8s(a()), i8s(b()));
                for k in 0..8 {
                    o[k] = (x[off + k] as i16).wrapping_mul(y[off + k] as i16) as u16;
                }
            } else {
                let (x, y) = (u8s(a()), u8s(b()));
                for k in 0..8 {
                    o[k] = (x[off + k] as u16).wrapping_mul(y[off + k] as u16);
                }
            }
            from_u16s(o)
        }
        160 => from_i32s(map(i32s(a()), |x| x.wrapping_abs())),
        161 => from_i32s(map(i32s(a()), |x| x.wrapping_neg())),
        163 => {
            *regs.add(i.a as usize) = u32s(a()).iter().all(|&x| x != 0) as u64;
            return;
        }
        164 => {
            let l = i32s(a());
            let mut m = 0u64;
            for (k, x) in l.iter().enumerate() {
                if *x < 0 {
                    m |= 1 << k;
                }
            }
            *regs.add(i.a as usize) = m;
            return;
        }
        167 | 168 => {
            let l = i16s(a());
            let off = if sub == 167 { 0 } else { 4 };
            let mut o = [0i32; 4];
            for k in 0..4 {
                o[k] = l[off + k] as i32;
            }
            from_i32s(o)
        }
        169 | 170 => {
            let l = u16s(a());
            let off = if sub == 169 { 0 } else { 4 };
            let mut o = [0u32; 4];
            for k in 0..4 {
                o[k] = l[off + k] as u32;
            }
            from_u32s(o)
        }
        171 => {
            let s = sc(i.c) as u32 & 31;
            from_u32s(map(u32s(a()), |x| x << s))
        }
        172 => {
            let s = sc(i.c) as u32 & 31;
            from_i32s(map(i32s(a()), |x| x >> s))
        }
        173 => {
            let s = sc(i.c) as u32 & 31;
            from_u32s(map(u32s(a()), |x| x >> s))
        }
        174 => from_u32s(zip(u32s(a()), u32s(b()), |x, y| x.wrapping_add(y))),
        177 => from_u32s(zip(u32s(a()), u32s(b()), |x, y| x.wrapping_sub(y))),
        181 => from_u32s(zip(u32s(a()), u32s(b()), |x, y| x.wrapping_mul(y))),
        182 => from_i32s(zip(i32s(a()), i32s(b()), |x, y| x.min(y))),
        183 => from_u32s(zip(u32s(a()), u32s(b()), |x, y| x.min(y))),
        184 => from_i32s(zip(i32s(a()), i32s(b()), |x, y| x.max(y))),
        185 => from_u32s(zip(u32s(a()), u32s(b()), |x, y| x.max(y))),
        186 => {
            let (x, y) = (i16s(a()), i16s(b()));
            let mut o = [0i32; 4];
            for k in 0..4 {
                o[k] = (x[2 * k] as i32 * y[2 * k] as i32).wrapping_add(x[2 * k + 1] as i32 * y[2 * k + 1] as i32);
            }
            from_i32s(o)
        }
        188..=191 => {
            let off = if sub == 188 || sub == 190 { 0 } else { 4 };
            let signed = sub == 188 || sub == 189;
            let mut o = [0u32; 4];
            if signed {
                let (x, y) = (i16s(a()), i16s(b()));
                for k in 0..4 {
                    o[k] = (x[off + k] as i32).wrapping_mul(y[off + k] as i32) as u32;
                }
            } else {
                let (x, y) = (u16s(a()), u16s(b()));
                for k in 0..4 {
                    o[k] = (x[off + k] as u32).wrapping_mul(y[off + k] as u32);
                }
            }
            from_u32s(o)
        }
        192 => from_i64s(map(i64s(a()), |x| x.wrapping_abs())),
        193 => from_i64s(map(i64s(a()), |x| x.wrapping_neg())),
        195 => {
            *regs.add(i.a as usize) = u64s(a()).iter().all(|&x| x != 0) as u64;
            return;
        }
        196 => {
            let l = i64s(a());
            *regs.add(i.a as usize) = (l[0] < 0) as u64 | ((l[1] < 0) as u64) << 1;
            return;
        }
        199 | 200 => {
            let l = i32s(a());
            let off = if sub == 199 { 0 } else { 2 };
            from_i64s([l[off] as i64, l[off + 1] as i64])
        }
        201 | 202 => {
            let l = u32s(a());
            let off = if sub == 201 { 0 } else { 2 };
            from_u64s([l[off] as u64, l[off + 1] as u64])
        }
        203 => {
            let s = sc(i.c) as u32 & 63;
            from_u64s(map(u64s(a()), |x| x << s))
        }
        204 => {
            let s = sc(i.c) as u32 & 63;
            from_i64s(map(i64s(a()), |x| x >> s))
        }
        205 => {
            let s = sc(i.c) as u32 & 63;
            from_u64s(map(u64s(a()), |x| x >> s))
        }
        206 => from_u64s(zip(u64s(a()), u64s(b()), |x, y| x.wrapping_add(y))),
        209 => from_u64s(zip(u64s(a()), u64s(b()), |x, y| x.wrapping_sub(y))),
        213 => from_u64s(zip(u64s(a()), u64s(b()), |x, y| x.wrapping_mul(y))),
        214 => mask(cmp(i64s(a()), i64s(b()), |x, y| x == y)),
        215 => mask(cmp(i64s(a()), i64s(b()), |x, y| x != y)),
        216 => mask(cmp(i64s(a()), i64s(b()), |x, y| x < y)),
        217 => mask(cmp(i64s(a()), i64s(b()), |x, y| x > y)),
        218 => mask(cmp(i64s(a()), i64s(b()), |x, y| x <= y)),
        219 => mask(cmp(i64s(a()), i64s(b()), |x, y| x >= y)),
        220..=223 => {
            let off = if sub == 220 || sub == 222 { 0 } else { 2 };
            if sub == 220 || sub == 221 {
                let (x, y) = (i32s(a()), i32s(b()));
                from_i64s([(x[off] as i64).wrapping_mul(y[off] as i64), (x[off + 1] as i64).wrapping_mul(y[off + 1] as i64)])
            } else {
                let (x, y) = (u32s(a()), u32s(b()));
                from_u64s([(x[off] as u64).wrapping_mul(y[off] as u64), (x[off + 1] as u64).wrapping_mul(y[off + 1] as u64)])
            }
        }
        224 => from_u32s(map(u32s(a()), |x| x & 0x7fff_ffff)),
        225 => from_u32s(map(u32s(a()), |x| x ^ 0x8000_0000)),
        227 => from_f32s(map(f32s(a()), |x| x.sqrt())),
        228 => from_f32s(zip(f32s(a()), f32s(b()), |x, y| x + y)),
        229 => from_f32s(zip(f32s(a()), f32s(b()), |x, y| x - y)),
        230 => from_f32s(zip(f32s(a()), f32s(b()), |x, y| x * y)),
        231 => from_f32s(zip(f32s(a()), f32s(b()), |x, y| x / y)),
        232 | 269 => from_f32s(zip(f32s(a()), f32s(b()), fmin32)),
        233 | 270 => from_f32s(zip(f32s(a()), f32s(b()), fmax32)),
        234 => from_f32s(zip(f32s(a()), f32s(b()), |x, y| if y < x { y } else { x })),
        235 => from_f32s(zip(f32s(a()), f32s(b()), |x, y| if x < y { y } else { x })),
        236 => from_u64s(map(u64s(a()), |x| x & 0x7fff_ffff_ffff_ffff)),
        237 => from_u64s(map(u64s(a()), |x| x ^ 0x8000_0000_0000_0000)),
        239 => from_f64s(map(f64s(a()), |x| x.sqrt())),
        240 => from_f64s(zip(f64s(a()), f64s(b()), |x, y| x + y)),
        241 => from_f64s(zip(f64s(a()), f64s(b()), |x, y| x - y)),
        242 => from_f64s(zip(f64s(a()), f64s(b()), |x, y| x * y)),
        243 => from_f64s(zip(f64s(a()), f64s(b()), |x, y| x / y)),
        244 | 271 => from_f64s(zip(f64s(a()), f64s(b()), fmin64)),
        245 | 272 => from_f64s(zip(f64s(a()), f64s(b()), fmax64)),
        246 => from_f64s(zip(f64s(a()), f64s(b()), |x, y| if y < x { y } else { x })),
        247 => from_f64s(zip(f64s(a()), f64s(b()), |x, y| if x < y { y } else { x })),
        248 | 257 => {
            let l = f32s(a());
            from_i32s([l[0] as i32, l[1] as i32, l[2] as i32, l[3] as i32])
        }
        249 | 258 => {
            let l = f32s(a());
            from_u32s([l[0] as u32, l[1] as u32, l[2] as u32, l[3] as u32])
        }
        250 => {
            let l = i32s(a());
            from_f32s([l[0] as f32, l[1] as f32, l[2] as f32, l[3] as f32])
        }
        251 => {
            let l = u32s(a());
            from_f32s([l[0] as f32, l[1] as f32, l[2] as f32, l[3] as f32])
        }
        252 | 259 => {
            let l = f64s(a());
            from_i32s([l[0] as i32, l[1] as i32, 0, 0])
        }
        253 | 260 => {
            let l = f64s(a());
            from_u32s([l[0] as u32, l[1] as u32, 0, 0])
        }
        254 => {
            let l = i32s(a());
            from_f64s([l[0] as f64, l[1] as f64])
        }
        255 => {
            let l = u32s(a());
            from_f64s([l[0] as f64, l[1] as f64])
        }
        256 => swizzle(a(), b()),
        274 => {
            let (x, y) = (i8s(a()), i8s(b()));
            let mut o = [0i16; 8];
            for k in 0..8 {
                o[k] = (x[2 * k] as i16 * y[2 * k] as i16).wrapping_add(x[2 * k + 1] as i16 * y[2 * k + 1] as i16);
            }
            from_i16s(o)
        }
        _ => 0,
    };
    wv(regs, i.a, r);
}

/// Ternary operations on three consecutive v128 operands at `i.a`.
///
/// # Safety
/// As [`exec`].
pub unsafe fn exec3(i: Instr, regs: *mut u64) {
    let x = rv(regs, i.a);
    let y = rv(regs, i.a + 2);
    let z = rv(regs, i.a + 4);
    let r = match i.x {
        82 | 265..=268 => (x & z) | (y & !z),
        261 => from_f32s({
            let (a, b, c) = (f32s(x), f32s(y), f32s(z));
            [a[0] * b[0] + c[0], a[1] * b[1] + c[1], a[2] * b[2] + c[2], a[3] * b[3] + c[3]]
        }),
        262 => from_f32s({
            let (a, b, c) = (f32s(x), f32s(y), f32s(z));
            [-(a[0] * b[0]) + c[0], -(a[1] * b[1]) + c[1], -(a[2] * b[2]) + c[2], -(a[3] * b[3]) + c[3]]
        }),
        263 => from_f64s({
            let (a, b, c) = (f64s(x), f64s(y), f64s(z));
            [a[0] * b[0] + c[0], a[1] * b[1] + c[1]]
        }),
        264 => from_f64s({
            let (a, b, c) = (f64s(x), f64s(y), f64s(z));
            [-(a[0] * b[0]) + c[0], -(a[1] * b[1]) + c[1]]
        }),
        275 => {
            let (a, b, c) = (i8s(x), i8s(y), i32s(z));
            let mut o = [0i32; 4];
            for k in 0..4 {
                let mut s = 0i32;
                for j in 0..4 {
                    s += a[4 * k + j] as i32 * b[4 * k + j] as i32;
                }
                o[k] = s.wrapping_add(c[k]);
            }
            from_i32s(o)
        }
        _ => 0,
    };
    wv(regs, i.a, r);
}

/// `i8x16.shuffle` of the two v128 at `i.a` (result at `i.a`).
///
/// # Safety
/// As [`exec`].
pub unsafe fn shuffle(code: &Code, i: Instr, regs: *mut u64) {
    let x = u8s(rv(regs, i.a));
    let y = u8s(rv(regs, i.a + 2));
    let l = code.v128s[i.b as usize].to_le_bytes();
    let mut o = [0u8; 16];
    for k in 0..16 {
        let s = l[k] as usize;
        o[k] = if s < 16 { x[s] } else { y[s - 16] };
    }
    wv(regs, i.a, from_u8s(o));
}

/// SIMD loads and stores.
///
/// # Safety
/// As [`exec`].
pub unsafe fn mem_op(mem: &MemInst, offset: u64, i: Instr, regs: *mut u64) -> std::result::Result<(), Trap> {
    let sub = (i.x & ((1 << LANE_SHIFT) - 1)) as u32;
    let lane = (i.x >> LANE_SHIFT) as usize;
    let (p, len) = mem.raw();
    let addr_slot = match i.op {
        Op::SimdLoad => i.b,
        _ => i.a,
    };
    let raw = *regs.add(addr_slot as usize);
    let addr = if mem.ty.memory64 { raw } else { raw as u32 as u64 };
    let size: u64 = match sub {
        0 | 11 => 16,
        1..=6 | 10 | 93 | 87 | 91 => 8,
        7 | 84 | 88 => 1,
        8 | 85 | 89 => 2,
        _ => 4,
    };
    let ea = addr.checked_add(offset).ok_or(Trap::MemoryOutOfBounds)?;
    if ea.checked_add(size).is_none_or(|e| e > len as u64) {
        return Err(Trap::MemoryOutOfBounds);
    }
    let ea = ea as usize;
    let rd = |n: usize| -> u128 {
        let mut b = [0u8; 16];
        std::ptr::copy_nonoverlapping(p.add(ea), b.as_mut_ptr(), n);
        u128::from_le_bytes(b)
    };
    match sub {
        0 => wv(regs, i.a, rd(16)),
        1 | 2 => {
            let b = rd(8).to_le_bytes();
            let mut o = [0u16; 8];
            for k in 0..8 {
                o[k] = if sub == 1 { b[k] as i8 as i16 as u16 } else { b[k] as u16 };
            }
            wv(regs, i.a, from_u16s(o));
        }
        3 | 4 => {
            let v = u16s(rd(8));
            let mut o = [0u32; 4];
            for k in 0..4 {
                o[k] = if sub == 3 { v[k] as i16 as i32 as u32 } else { v[k] as u32 };
            }
            wv(regs, i.a, from_u32s(o));
        }
        5 | 6 => {
            let v = u32s(rd(8));
            let mut o = [0u64; 2];
            for k in 0..2 {
                o[k] = if sub == 5 { v[k] as i32 as i64 as u64 } else { v[k] as u64 };
            }
            wv(regs, i.a, from_u64s(o));
        }
        7 => wv(regs, i.a, from_u8s([rd(1) as u8; 16])),
        8 => wv(regs, i.a, from_u16s([rd(2) as u16; 8])),
        9 => wv(regs, i.a, from_u32s([rd(4) as u32; 4])),
        10 => wv(regs, i.a, from_u64s([rd(8) as u64; 2])),
        92 => wv(regs, i.a, rd(4)),
        93 => wv(regs, i.a, rd(8)),
        11 => {
            let v = rv(regs, i.b).to_le_bytes();
            std::ptr::copy_nonoverlapping(v.as_ptr(), p.add(ea), 16);
        }
        84..=87 => {
            let mut v = rv(regs, i.b + 1).to_le_bytes();
            let n = size as usize;
            std::ptr::copy_nonoverlapping(p.add(ea), v.as_mut_ptr().add(lane * n), n);
            wv(regs, i.a, u128::from_le_bytes(v));
        }
        88..=91 => {
            let v = rv(regs, i.b).to_le_bytes();
            let n = size as usize;
            std::ptr::copy_nonoverlapping(v.as_ptr().add(lane * n), p.add(ea), n);
        }
        _ => {}
    }
    Ok(())
}
