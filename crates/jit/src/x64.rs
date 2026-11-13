//! A minimal x86-64 assembler for template compilers.
//!
//! Only the encodings the baseline compilers use, all with explicit
//! operand sizes. Jumps are always rel32 (no relaxation: simpler and the
//! templates are small). With a listing enabled, every instruction also
//! records its offset and an Intel-syntax mnemonic, so debug dumps need no
//! disassembler.

pub const RAX: u8 = 0;
pub const RCX: u8 = 1;
pub const RDX: u8 = 2;
pub const RBX: u8 = 3;
pub const RSP: u8 = 4;
pub const RBP: u8 = 5;
pub const RSI: u8 = 6;
pub const RDI: u8 = 7;
pub const R8: u8 = 8;
pub const R9: u8 = 9;
pub const R10: u8 = 10;
pub const R11: u8 = 11;
pub const R12: u8 = 12;
pub const R13: u8 = 13;
pub const R14: u8 = 14;
pub const R15: u8 = 15;

pub const XMM0: u8 = 0;
pub const XMM1: u8 = 1;
pub const XMM2: u8 = 2;
pub const XMM3: u8 = 3;

/// Condition codes (the low nibble of `Jcc` / `SETcc` / `CMOVcc`).
pub const CC_O: u8 = 0;
pub const CC_NO: u8 = 1;
pub const CC_B: u8 = 2;
pub const CC_AE: u8 = 3;
pub const CC_E: u8 = 4;
pub const CC_NE: u8 = 5;
pub const CC_BE: u8 = 6;
pub const CC_A: u8 = 7;
pub const CC_S: u8 = 8;
pub const CC_NS: u8 = 9;
pub const CC_P: u8 = 10;
pub const CC_NP: u8 = 11;
pub const CC_L: u8 = 12;
pub const CC_GE: u8 = 13;
pub const CC_LE: u8 = 14;
pub const CC_G: u8 = 15;

/// Scalar double operations (`F2 0F xx`).
pub const SSE_ADD: u8 = 0x58;
pub const SSE_MUL: u8 = 0x59;
pub const SSE_SUB: u8 = 0x5c;
pub const SSE_DIV: u8 = 0x5e;

/// Group-1 ALU operations (the `/digit` of `81`/`83` and the base opcode).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Alu {
    Add = 0,
    Or = 1,
    And = 4,
    Sub = 5,
    Xor = 6,
    Cmp = 7,
}

/// Shift operations by `cl` (the `/digit` of `D3`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shift {
    Shl = 4,
    Shr = 5,
    Sar = 7,
}

/// A memory operand `[base + index*scale + disp]`.
#[derive(Clone, Copy, Debug)]
pub struct Mem {
    pub base: u8,
    pub index: Option<(u8, u8)>,
    pub disp: i32,
}

impl Mem {
    pub fn base(base: u8, disp: i32) -> Mem {
        Mem { base, index: None, disp }
    }
    /// `[base + index*scale + disp]`, scale 1, 2, 4 or 8.
    pub fn index(base: u8, index: u8, scale: u8, disp: i32) -> Mem {
        debug_assert!(index != RSP);
        Mem { base, index: Some((index, scale)), disp }
    }
}

/// A position in the code, bound once.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Label(u32);

pub struct Asm {
    pub buf: Vec<u8>,
    labels: Vec<Option<u32>>,
    /// rel32 fields to patch: (field position, label)
    fixups: Vec<(u32, Label)>,
    listing: Option<Vec<(u32, String)>>,
}

const NAMES64: [&str; 16] = ["rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi", "r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15"];
const NAMES32: [&str; 16] = ["eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi", "r8d", "r9d", "r10d", "r11d", "r12d", "r13d", "r14d", "r15d"];
const CC_NAMES: [&str; 16] = ["o", "no", "b", "ae", "e", "ne", "be", "a", "s", "ns", "p", "np", "l", "ge", "le", "g"];

fn r64(r: u8) -> &'static str {
    NAMES64[r as usize & 15]
}
fn r32(r: u8) -> &'static str {
    NAMES32[r as usize & 15]
}
fn mem_str(m: Mem) -> String {
    let mut s = format!("[{}", r64(m.base));
    if let Some((i, sc)) = m.index {
        s += &format!("+{}*{}", r64(i), sc);
    }
    if m.disp != 0 {
        s += &format!("{:+#x}", m.disp);
    }
    s + "]"
}

impl Asm {
    /// A new assembler; `listing` records a mnemonic per instruction.
    pub fn new(listing: bool) -> Asm {
        Asm { buf: Vec::with_capacity(4096), labels: Vec::new(), fixups: Vec::new(), listing: if listing { Some(Vec::new()) } else { None } }
    }

    #[inline]
    pub fn pos(&self) -> usize {
        self.buf.len()
    }

    #[inline]
    fn note(&mut self, at: usize, f: impl FnOnce() -> String) {
        if let Some(l) = &mut self.listing {
            l.push((at as u32, f()));
        }
    }

    /// Add a free-form comment line to the listing (e.g. the source
    /// instruction a template implements).
    pub fn comment(&mut self, f: impl FnOnce() -> String) {
        let at = self.pos();
        if let Some(l) = &mut self.listing {
            l.push((at as u32, format!("; {}", f())));
        }
    }

    pub fn listing_enabled(&self) -> bool {
        self.listing.is_some()
    }

    /// The listing: one line per instruction, `offset: bytes  mnemonic`.
    pub fn listing(&self) -> Option<String> {
        let l = self.listing.as_ref()?;
        let mut out = String::new();
        for (i, (at, text)) in l.iter().enumerate() {
            if text.starts_with(';') {
                out += &format!("{text}\n");
                continue;
            }
            let end = l[i + 1..].iter().find(|(_, t)| !t.starts_with(';')).map(|(a, _)| *a as usize).unwrap_or(self.buf.len());
            let bytes: String = self.buf[*at as usize..end.max(*at as usize)].iter().map(|b| format!("{b:02x}")).collect();
            out += &format!("{at:6x}: {bytes:<24} {text}\n");
        }
        Some(out)
    }

    // ------------------------------------------------------------ raw bytes

    #[inline]
    pub fn b(&mut self, x: u8) {
        self.buf.push(x);
    }
    #[inline]
    pub fn d32(&mut self, x: i32) {
        self.buf.extend_from_slice(&x.to_le_bytes());
    }
    pub fn d64(&mut self, x: u64) {
        self.buf.extend_from_slice(&x.to_le_bytes());
    }
    pub fn align(&mut self, n: usize) {
        while self.buf.len() % n != 0 {
            self.b(0xcc);
        }
    }

    // ------------------------------------------------------------ labels

    pub fn label(&mut self) -> Label {
        self.labels.push(None);
        Label(self.labels.len() as u32 - 1)
    }
    pub fn bind(&mut self, l: Label) {
        debug_assert!(self.labels[l.0 as usize].is_none(), "label bound twice");
        let at = self.pos();
        self.labels[l.0 as usize] = Some(at as u32);
        self.note(at, || format!("L{}:", l.0));
    }
    pub fn is_bound(&self, l: Label) -> bool {
        self.labels[l.0 as usize].is_some()
    }
    pub fn label_offset(&self, l: Label) -> Option<usize> {
        self.labels[l.0 as usize].map(|x| x as usize)
    }
    fn rel32(&mut self, l: Label) {
        let at = self.pos() as u32;
        self.fixups.push((at, l));
        self.d32(0);
    }
    /// Patch every jump; panics on an unbound label (a compiler bug).
    pub fn finish_labels(&mut self) {
        for &(at, l) in &self.fixups {
            let target = self.labels[l.0 as usize].expect("unbound label") as i64;
            let rel = target - (at as i64 + 4);
            self.buf[at as usize..at as usize + 4].copy_from_slice(&(rel as i32).to_le_bytes());
        }
        self.fixups.clear();
    }

    // ------------------------------------------------------------ encoding

    fn rex(&mut self, w: bool, reg: u8, index: u8, base: u8, force: bool) {
        let rex = 0x40 | (w as u8) << 3 | ((reg >> 3) & 1) << 2 | ((index >> 3) & 1) << 1 | ((base >> 3) & 1);
        if rex != 0x40 || force {
            self.b(rex);
        }
    }

    /// `[prefix] [REX] opcode modrm [sib] [disp]` with a memory operand.
    fn op_mem(&mut self, prefix: Option<u8>, w: bool, opcode: &[u8], reg: u8, m: Mem) {
        if let Some(p) = prefix {
            self.b(p);
        }
        self.rex(w, reg, m.index.map(|i| i.0).unwrap_or(0), m.base, false);
        self.buf.extend_from_slice(opcode);
        let md = if m.disp == 0 && m.base & 7 != 5 {
            0b00
        } else if (-128..128).contains(&m.disp) {
            0b01
        } else {
            0b10
        };
        let need_sib = m.index.is_some() || m.base & 7 == 4;
        let rm = if need_sib { 4 } else { m.base & 7 };
        self.b(md << 6 | (reg & 7) << 3 | rm);
        if need_sib {
            let (idx, ss) = match m.index {
                Some((i, s)) => (
                    i & 7,
                    match s {
                        1 => 0,
                        2 => 1,
                        4 => 2,
                        _ => 3,
                    },
                ),
                None => (4, 0),
            };
            self.b(ss << 6 | idx << 3 | (m.base & 7));
        }
        match md {
            0b01 => self.b(m.disp as i8 as u8),
            0b10 => self.d32(m.disp),
            _ => {}
        }
    }

    /// `[prefix] [REX] opcode modrm` with two registers (`reg`, `rm`).
    fn op_rr(&mut self, prefix: Option<u8>, w: bool, opcode: &[u8], reg: u8, rm: u8) {
        if let Some(p) = prefix {
            self.b(p);
        }
        self.rex(w, reg, 0, rm, false);
        self.buf.extend_from_slice(opcode);
        self.b(0b11 << 6 | (reg & 7) << 3 | (rm & 7));
    }

    // ------------------------------------------------------------ moves

    pub fn load64(&mut self, r: u8, m: Mem) {
        let at = self.pos();
        self.op_mem(None, true, &[0x8b], r, m);
        self.note(at, || format!("mov {}, qword {}", r64(r), mem_str(m)));
    }
    pub fn load32(&mut self, r: u8, m: Mem) {
        let at = self.pos();
        self.op_mem(None, false, &[0x8b], r, m);
        self.note(at, || format!("mov {}, dword {}", r32(r), mem_str(m)));
    }
    pub fn store64(&mut self, m: Mem, r: u8) {
        let at = self.pos();
        self.op_mem(None, true, &[0x89], r, m);
        self.note(at, || format!("mov qword {}, {}", mem_str(m), r64(r)));
    }
    pub fn store32(&mut self, m: Mem, r: u8) {
        let at = self.pos();
        self.op_mem(None, false, &[0x89], r, m);
        self.note(at, || format!("mov dword {}, {}", mem_str(m), r32(r)));
    }
    /// `mov dword [m], imm32`
    pub fn store32_imm(&mut self, m: Mem, v: u32) {
        let at = self.pos();
        self.op_mem(None, false, &[0xc7], 0, m);
        self.d32(v as i32);
        self.note(at, || format!("mov dword {}, {v:#x}", mem_str(m)));
    }
    pub fn mov_rr64(&mut self, dst: u8, src: u8) {
        let at = self.pos();
        self.op_rr(None, true, &[0x89], src, dst);
        self.note(at, || format!("mov {}, {}", r64(dst), r64(src)));
    }
    pub fn mov_imm64(&mut self, r: u8, v: u64) {
        let at = self.pos();
        if v <= u32::MAX as u64 {
            self.rex(false, 0, 0, r, false);
            self.b(0xb8 + (r & 7));
            self.d32(v as u32 as i32);
        } else {
            self.rex(true, 0, 0, r, false);
            self.b(0xb8 + (r & 7));
            self.d64(v);
        }
        self.note(at, || format!("mov {}, {v:#x}", r64(r)));
    }
    pub fn mov_imm32(&mut self, r: u8, v: u32) {
        let at = self.pos();
        self.rex(false, 0, 0, r, false);
        self.b(0xb8 + (r & 7));
        self.d32(v as i32);
        self.note(at, || format!("mov {}, {v:#x}", r32(r)));
    }
    pub fn lea(&mut self, r: u8, m: Mem) {
        let at = self.pos();
        self.op_mem(None, true, &[0x8d], r, m);
        self.note(at, || format!("lea {}, {}", r64(r), mem_str(m)));
    }
    /// `lea r, [rip + label]`
    pub fn lea_label(&mut self, r: u8, l: Label) {
        let at = self.pos();
        self.rex(true, r, 0, 0, false);
        self.b(0x8d);
        self.b((r & 7) << 3 | 0b101);
        self.rel32(l);
        self.note(at, || format!("lea {}, [rip+L{}]", r64(r), l.0));
    }
    /// `movsxd r64, dword [m]`
    pub fn movsxd(&mut self, r: u8, m: Mem) {
        let at = self.pos();
        self.op_mem(None, true, &[0x63], r, m);
        self.note(at, || format!("movsxd {}, dword {}", r64(r), mem_str(m)));
    }
    /// `movzx r32, byte [m]`
    pub fn movzx8(&mut self, r: u8, m: Mem) {
        let at = self.pos();
        self.op_mem(None, false, &[0x0f, 0xb6], r, m);
        self.note(at, || format!("movzx {}, byte {}", r32(r), mem_str(m)));
    }
    pub fn cmov64(&mut self, cc: u8, dst: u8, src: u8) {
        let at = self.pos();
        self.op_rr(None, true, &[0x0f, 0x40 + cc], dst, src);
        self.note(at, || format!("cmov{} {}, {}", CC_NAMES[cc as usize], r64(dst), r64(src)));
    }
    /// `setcc al`-style on the low byte of `r` (rax..rbx only), then zero-extend.
    pub fn setcc_zx(&mut self, cc: u8, r: u8) {
        debug_assert!(r < 4);
        let at = self.pos();
        self.buf.extend_from_slice(&[0x0f, 0x90 + cc, 0xc0 | r]);
        self.buf.extend_from_slice(&[0x0f, 0xb6, 0xc0 | r << 3 | r]);
        self.note(at, || format!("set{} {}l ; movzx", CC_NAMES[cc as usize], ["a", "c", "d", "b"][r as usize]));
    }

    // ------------------------------------------------------------ ALU

    pub fn alu_rr64(&mut self, op: Alu, dst: u8, src: u8) {
        let at = self.pos();
        self.op_rr(None, true, &[(op as u8) << 3 | 1], src, dst);
        self.note(at, || format!("{} {}, {}", alu_name(op), r64(dst), r64(src)));
    }
    pub fn alu_rr32(&mut self, op: Alu, dst: u8, src: u8) {
        let at = self.pos();
        self.op_rr(None, false, &[(op as u8) << 3 | 1], src, dst);
        self.note(at, || format!("{} {}, {}", alu_name(op), r32(dst), r32(src)));
    }
    /// `op r64, qword [m]`
    pub fn alu_rm64(&mut self, op: Alu, dst: u8, m: Mem) {
        let at = self.pos();
        self.op_mem(None, true, &[(op as u8) << 3 | 3], dst, m);
        self.note(at, || format!("{} {}, qword {}", alu_name(op), r64(dst), mem_str(m)));
    }
    /// `op r32, dword [m]`
    pub fn alu_rm32(&mut self, op: Alu, dst: u8, m: Mem) {
        let at = self.pos();
        self.op_mem(None, false, &[(op as u8) << 3 | 3], dst, m);
        self.note(at, || format!("{} {}, dword {}", alu_name(op), r32(dst), mem_str(m)));
    }
    /// `test byte [m], imm8`
    pub fn test_mi8(&mut self, m: Mem, imm: u8) {
        let at = self.pos();
        self.op_mem(None, false, &[0xf6], 0, m);
        self.b(imm);
        self.note(at, || format!("test byte {}, {imm:#x}", mem_str(m)));
    }
    pub fn alu_ri64(&mut self, op: Alu, dst: u8, imm: i32) {
        let at = self.pos();
        if (-128..128).contains(&imm) {
            self.op_rr(None, true, &[0x83], op as u8, dst);
            self.b(imm as i8 as u8);
        } else {
            self.op_rr(None, true, &[0x81], op as u8, dst);
            self.d32(imm);
        }
        self.note(at, || format!("{} {}, {imm:#x}", alu_name(op), r64(dst)));
    }
    pub fn alu_ri32(&mut self, op: Alu, dst: u8, imm: i32) {
        let at = self.pos();
        if (-128..128).contains(&imm) {
            self.op_rr(None, false, &[0x83], op as u8, dst);
            self.b(imm as i8 as u8);
        } else {
            self.op_rr(None, false, &[0x81], op as u8, dst);
            self.d32(imm);
        }
        self.note(at, || format!("{} {}, {imm:#x}", alu_name(op), r32(dst)));
    }
    /// `op dword [m], imm`
    pub fn alu_mi32(&mut self, op: Alu, m: Mem, imm: i32) {
        let at = self.pos();
        if (-128..128).contains(&imm) {
            self.op_mem(None, false, &[0x83], op as u8, m);
            self.b(imm as i8 as u8);
        } else {
            self.op_mem(None, false, &[0x81], op as u8, m);
            self.d32(imm);
        }
        self.note(at, || format!("{} dword {}, {imm:#x}", alu_name(op), mem_str(m)));
    }
    pub fn add_rr64(&mut self, dst: u8, src: u8) {
        self.alu_rr64(Alu::Add, dst, src);
    }
    pub fn add_ri64(&mut self, dst: u8, imm: i32) {
        self.alu_ri64(Alu::Add, dst, imm);
    }
    pub fn cmp_rr64(&mut self, a: u8, b: u8) {
        self.alu_rr64(Alu::Cmp, a, b);
    }
    pub fn xor_rr32(&mut self, dst: u8, src: u8) {
        self.alu_rr32(Alu::Xor, dst, src);
    }
    pub fn test_rr32(&mut self, a: u8, b: u8) {
        let at = self.pos();
        self.op_rr(None, false, &[0x85], b, a);
        self.note(at, || format!("test {}, {}", r32(a), r32(b)));
    }
    pub fn test_rr64(&mut self, a: u8, b: u8) {
        let at = self.pos();
        self.op_rr(None, true, &[0x85], b, a);
        self.note(at, || format!("test {}, {}", r64(a), r64(b)));
    }
    /// `shift r32, cl`
    pub fn shift_cl32(&mut self, op: Shift, r: u8) {
        let at = self.pos();
        self.op_rr(None, false, &[0xd3], op as u8, r);
        self.note(at, || format!("{} {}, cl", ["", "", "", "", "shl", "shr", "", "sar"][op as usize], r32(r)));
    }
    /// `shr r64, imm8`
    pub fn shr_ri64(&mut self, r: u8, n: u8) {
        let at = self.pos();
        self.op_rr(None, true, &[0xc1], 5, r);
        self.b(n);
        self.note(at, || format!("shr {}, {n}", r64(r)));
    }
    /// `shl r64, imm8`
    pub fn shl_ri64(&mut self, r: u8, n: u8) {
        let at = self.pos();
        self.op_rr(None, true, &[0xc1], 4, r);
        self.b(n);
        self.note(at, || format!("shl {}, {n}", r64(r)));
    }
    /// `op qword [m], imm`
    pub fn alu_mi64(&mut self, op: Alu, m: Mem, imm: i32) {
        let at = self.pos();
        if (-128..128).contains(&imm) {
            self.op_mem(None, true, &[0x83], op as u8, m);
            self.b(imm as i8 as u8);
        } else {
            self.op_mem(None, true, &[0x81], op as u8, m);
            self.d32(imm);
        }
        self.note(at, || format!("{} qword {}, {imm:#x}", alu_name(op), mem_str(m)));
    }
    /// `imul dst64, src64, imm32`
    pub fn imul_rri64(&mut self, dst: u8, src: u8, imm: i32) {
        let at = self.pos();
        self.op_rr(None, true, &[0x69], dst, src);
        self.d32(imm);
        self.note(at, || format!("imul {}, {}, {imm:#x}", r64(dst), r64(src)));
    }
    /// `not r32`
    pub fn not32(&mut self, r: u8) {
        let at = self.pos();
        self.op_rr(None, false, &[0xf7], 2, r);
        self.note(at, || format!("not {}", r32(r)));
    }
    /// `imul r32, r32`
    pub fn imul_rr32(&mut self, dst: u8, src: u8) {
        let at = self.pos();
        self.op_rr(None, false, &[0x0f, 0xaf], dst, src);
        self.note(at, || format!("imul {}, {}", r32(dst), r32(src)));
    }

    // ------------------------------------------------------------ SSE2

    /// `movq xmm, r64`
    pub fn movq_x_r(&mut self, x: u8, r: u8) {
        let at = self.pos();
        self.op_rr(Some(0x66), true, &[0x0f, 0x6e], x, r);
        self.note(at, || format!("movq xmm{x}, {}", r64(r)));
    }
    /// `movq r64, xmm`
    pub fn movq_r_x(&mut self, r: u8, x: u8) {
        let at = self.pos();
        self.op_rr(Some(0x66), true, &[0x0f, 0x7e], x, r);
        self.note(at, || format!("movq {}, xmm{x}", r64(r)));
    }
    pub fn movsd_load(&mut self, x: u8, m: Mem) {
        let at = self.pos();
        self.op_mem(Some(0xf2), false, &[0x0f, 0x10], x, m);
        self.note(at, || format!("movsd xmm{x}, {}", mem_str(m)));
    }
    /// `addsd/subsd/mulsd/divsd dst, src`
    pub fn sse_rr(&mut self, op: u8, dst: u8, src: u8) {
        let at = self.pos();
        self.op_rr(Some(0xf2), false, &[0x0f, op], dst, src);
        let name = match op {
            SSE_ADD => "addsd",
            SSE_SUB => "subsd",
            SSE_MUL => "mulsd",
            SSE_DIV => "divsd",
            _ => "sse?",
        };
        self.note(at, || format!("{name} xmm{dst}, xmm{src}"));
    }
    /// `addsd/... dst, qword [m]`
    pub fn sse_rm(&mut self, op: u8, dst: u8, m: Mem) {
        let at = self.pos();
        self.op_mem(Some(0xf2), false, &[0x0f, op], dst, m);
        self.note(at, || format!("sse{op:#x} xmm{dst}, {}", mem_str(m)));
    }
    /// `ucomisd a, b`
    pub fn ucomisd(&mut self, a: u8, b: u8) {
        let at = self.pos();
        self.op_rr(Some(0x66), false, &[0x0f, 0x2e], a, b);
        self.note(at, || format!("ucomisd xmm{a}, xmm{b}"));
    }
    /// `cvttsd2si r64, xmm` (0x8000_0000_0000_0000 when out of range)
    pub fn cvttsd2si64(&mut self, r: u8, x: u8) {
        let at = self.pos();
        self.op_rr(Some(0xf2), true, &[0x0f, 0x2c], r, x);
        self.note(at, || format!("cvttsd2si {}, xmm{x}", r64(r)));
    }
    /// `cvtsi2sd xmm, r32` (signed)
    pub fn cvtsi2sd32(&mut self, x: u8, r: u8) {
        let at = self.pos();
        self.op_rr(Some(0xf2), false, &[0x0f, 0x2a], x, r);
        self.note(at, || format!("cvtsi2sd xmm{x}, {}", r32(r)));
    }
    /// `cvtsi2sd xmm, r64` (signed)
    pub fn cvtsi2sd64(&mut self, x: u8, r: u8) {
        let at = self.pos();
        self.op_rr(Some(0xf2), true, &[0x0f, 0x2a], x, r);
        self.note(at, || format!("cvtsi2sd xmm{x}, {}", r64(r)));
    }
    /// `xorpd x, x` style
    pub fn xorpd(&mut self, a: u8, b: u8) {
        let at = self.pos();
        self.op_rr(Some(0x66), false, &[0x0f, 0x57], a, b);
        self.note(at, || format!("xorpd xmm{a}, xmm{b}"));
    }

    // ------------------------------------------------------------ control

    pub fn jmp(&mut self, l: Label) {
        let at = self.pos();
        self.b(0xe9);
        self.rel32(l);
        self.note(at, || format!("jmp L{}", l.0));
    }
    pub fn jcc(&mut self, cc: u8, l: Label) {
        let at = self.pos();
        self.b(0x0f);
        self.b(0x80 + cc);
        self.rel32(l);
        self.note(at, || format!("j{} L{}", CC_NAMES[cc as usize], l.0));
    }
    /// `jmp r64`
    pub fn jmp_r(&mut self, r: u8) {
        let at = self.pos();
        self.op_rr(None, false, &[0xff], 4, r);
        self.note(at, || format!("jmp {}", r64(r)));
    }
    /// `call r64`
    pub fn call_r(&mut self, r: u8) {
        let at = self.pos();
        self.op_rr(None, false, &[0xff], 2, r);
        self.note(at, || format!("call {}", r64(r)));
    }
    /// `call` an absolute address (through `rax`).
    pub fn call_abs(&mut self, f: usize, name: &str) {
        self.mov_imm64(RAX, f as u64);
        let at = self.pos();
        self.buf.extend_from_slice(&[0xff, 0xd0]);
        self.note(at, || format!("call rax ; {name}"));
    }
    pub fn push(&mut self, r: u8) {
        let at = self.pos();
        self.rex(false, 0, 0, r, false);
        self.b(0x50 + (r & 7));
        self.note(at, || format!("push {}", r64(r)));
    }
    pub fn pop(&mut self, r: u8) {
        let at = self.pos();
        self.rex(false, 0, 0, r, false);
        self.b(0x58 + (r & 7));
        self.note(at, || format!("pop {}", r64(r)));
    }
    pub fn ret(&mut self) {
        let at = self.pos();
        self.b(0xc3);
        self.note(at, || "ret".into());
    }
    pub fn int3(&mut self) {
        let at = self.pos();
        self.b(0xcc);
        self.note(at, || "int3".into());
    }
}

fn alu_name(op: Alu) -> &'static str {
    match op {
        Alu::Add => "add",
        Alu::Or => "or",
        Alu::And => "and",
        Alu::Sub => "sub",
        Alu::Xor => "xor",
        Alu::Cmp => "cmp",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(f: impl FnOnce(&mut Asm)) -> Vec<u8> {
        let mut a = Asm::new(false);
        f(&mut a);
        a.buf
    }

    #[test]
    fn encodings() {
        // Checked against the Intel manual by hand.
        assert_eq!(enc(|a| a.load64(RAX, Mem::base(R12, 8))), [0x49, 0x8b, 0x44, 0x24, 0x08]);
        assert_eq!(enc(|a| a.load64(RCX, Mem::base(RBX, 0))), [0x48, 0x8b, 0x0b]);
        assert_eq!(enc(|a| a.load64(RCX, Mem::base(R13, 0))), [0x49, 0x8b, 0x4d, 0x00]);
        assert_eq!(enc(|a| a.store64(Mem::base(R12, 0x100), RDX)), [0x49, 0x89, 0x94, 0x24, 0x00, 0x01, 0x00, 0x00]);
        assert_eq!(enc(|a| a.mov_rr64(RBX, RDI)), [0x48, 0x89, 0xfb]);
        assert_eq!(enc(|a| a.mov_imm64(R13, 0xFFF9_0000_0000_0000)), [0x49, 0xbd, 0, 0, 0, 0, 0, 0, 0xf9, 0xff]);
        assert_eq!(enc(|a| a.cmp_rr64(RAX, R13)), [0x4c, 0x39, 0xe8]);
        assert_eq!(enc(|a| a.movq_x_r(XMM0, RAX)), [0x66, 0x48, 0x0f, 0x6e, 0xc0]);
        assert_eq!(enc(|a| a.movq_r_x(RCX, XMM1)), [0x66, 0x48, 0x0f, 0x7e, 0xc9]);
        assert_eq!(enc(|a| a.sse_rr(SSE_ADD, XMM0, XMM1)), [0xf2, 0x0f, 0x58, 0xc1]);
        assert_eq!(enc(|a| a.ucomisd(XMM0, XMM1)), [0x66, 0x0f, 0x2e, 0xc1]);
        assert_eq!(enc(|a| a.load64(RAX, Mem::index(RCX, RDX, 4, 0))), [0x48, 0x8b, 0x04, 0x91]);
        assert_eq!(enc(|a| a.alu_mi32(Alu::Sub, Mem::base(RAX, 0), 1)), [0x83, 0x28, 0x01]);
        assert_eq!(enc(|a| a.push(R12)), [0x41, 0x54]);
        assert_eq!(enc(|a| a.pop(RBX)), [0x5b]);
        assert_eq!(enc(|a| a.cvttsd2si64(RAX, XMM0)), [0xf2, 0x48, 0x0f, 0x2c, 0xc0]);
        // add qword [rcx-16], 1 ; sub qword [rdi-16], 1
        assert_eq!(enc(|a| a.alu_mi64(Alu::Add, Mem::base(RCX, -16), 1)), [0x48, 0x83, 0x41, 0xf0, 0x01]);
        assert_eq!(enc(|a| a.alu_mi64(Alu::Sub, Mem::base(RDI, -16), 1)), [0x48, 0x83, 0x6f, 0xf0, 0x01]);
        // shl rcx, 16 ; shr r8, 16
        assert_eq!(enc(|a| a.shl_ri64(RCX, 16)), [0x48, 0xc1, 0xe1, 0x10]);
        assert_eq!(enc(|a| a.shr_ri64(R8, 16)), [0x49, 0xc1, 0xe8, 0x10]);
        // imul rax, rdx, 0x100 ; imul r9, r10, 24
        assert_eq!(enc(|a| a.imul_rri64(RAX, RDX, 0x100)), [0x48, 0x69, 0xc2, 0x00, 0x01, 0x00, 0x00]);
        assert_eq!(enc(|a| a.imul_rri64(R9, R10, 24)), [0x4d, 0x69, 0xca, 0x18, 0x00, 0x00, 0x00]);
    }
}
