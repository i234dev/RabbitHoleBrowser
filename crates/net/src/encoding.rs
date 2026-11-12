//! The WHATWG Encoding Standard: labels, and decoders for every encoding
//! (UTF-8, UTF-16BE/LE, the 28 legacy single-byte encodings, GBK and
//! gb18030, Big5, EUC-JP, ISO-2022-JP, Shift_JIS, EUC-KR, replacement and
//! x-user-defined), streaming, with replacement or fatal error modes.
//! Index tables come from `encoding_tables.bin` (generated from the
//! standard's index files; layout documented in `TABLES`).

use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;

static TABLES: &[u8] = include_bytes!("encoding_tables.bin");

const SINGLE_ORDER: [&str; 27] = [
    "IBM866", "ISO-8859-2", "ISO-8859-3", "ISO-8859-4", "ISO-8859-5", "ISO-8859-6", "ISO-8859-7", "ISO-8859-8", "ISO-8859-10", "ISO-8859-13", "ISO-8859-14",
    "ISO-8859-15", "ISO-8859-16", "KOI8-R", "KOI8-U", "macintosh", "windows-874", "windows-1250", "windows-1251", "windows-1252", "windows-1253", "windows-1254",
    "windows-1255", "windows-1256", "windows-1257", "windows-1258", "x-mac-cyrillic",
];
const OFF_JIS0208: usize = 28 * 256;
const OFF_JIS0212: usize = OFF_JIS0208 + 11104 * 2;
const OFF_GB18030: usize = OFF_JIS0212 + 7211 * 2;
const OFF_EUCKR: usize = OFF_GB18030 + 23940 * 2;
const OFF_BIG5: usize = OFF_EUCKR + 23750 * 2;
const OFF_KATAKANA: usize = OFF_BIG5 + 19782 * 4;
const OFF_RANGES: usize = OFF_KATAKANA + 63 * 2;

fn u16_at(off: usize, i: usize) -> u32 {
    u16::from_le_bytes([TABLES[off + 2 * i], TABLES[off + 2 * i + 1]]) as u32
}

fn u32_at(off: usize) -> u32 {
    u32::from_le_bytes([TABLES[off], TABLES[off + 1], TABLES[off + 2], TABLES[off + 3]])
}

fn index16(off: usize, len: usize, pointer: usize) -> Option<u32> {
    (pointer < len).then(|| u16_at(off, pointer)).filter(|&c| c != 0)
}

fn jis0208(p: usize) -> Option<u32> {
    index16(OFF_JIS0208, 11104, p)
}
fn jis0212(p: usize) -> Option<u32> {
    index16(OFF_JIS0212, 7211, p)
}
fn gb18030_index(p: usize) -> Option<u32> {
    index16(OFF_GB18030, 23940, p)
}
fn euckr(p: usize) -> Option<u32> {
    index16(OFF_EUCKR, 23750, p)
}
fn big5(p: usize) -> Option<u32> {
    (p < 19782).then(|| u32_at(OFF_BIG5 + 4 * p)).filter(|&c| c != 0)
}

/// Index gb18030 ranges code point.
fn gb18030_ranges(pointer: u32) -> Option<u32> {
    if (pointer > 39419 && pointer < 189000) || pointer > 1237575 {
        return None;
    }
    if pointer == 7457 {
        return Some(0xE7C7);
    }
    let mut best = (0u32, 0u32);
    for i in 0..207 {
        let p = u32_at(OFF_RANGES + 8 * i);
        if p <= pointer {
            best = (p, u32_at(OFF_RANGES + 8 * i + 4));
        } else {
            break;
        }
    }
    Some(best.1 + pointer - best.0)
}

/// An encoding (index into [`NAMES`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Encoding(usize);

impl Encoding {
    pub const UTF_8: Encoding = Encoding(0);
    pub const WINDOWS_1252: Encoding = Encoding(21);
    pub const REPLACEMENT: Encoding = Encoding(36);
    pub const UTF_16BE: Encoding = Encoding(37);
    pub const UTF_16LE: Encoding = Encoding(38);

    /// "Get an encoding" from a label (trimmed, ASCII case-insensitive).
    pub fn for_label(label: &str) -> Option<Encoding> {
        let l = label.trim_matches(|c: char| matches!(c, '\t' | '\n' | '\u{c}' | '\r' | ' ')).to_ascii_lowercase();
        label_index(&l).map(Encoding)
    }

    /// The canonical name (`UTF-8`, `Shift_JIS`, ...).
    pub fn name(self) -> &'static str {
        NAMES[self.0]
    }

    fn single_byte_table(self) -> Option<usize> {
        let n = if self.name() == "ISO-8859-8-I" { "ISO-8859-8" } else { self.name() };
        SINGLE_ORDER.iter().position(|&s| s == n)
    }

    pub fn new_decoder(self) -> Decoder {
        Decoder::new(self)
    }

    /// Decode a whole buffer with replacement characters (no BOM
    /// handling).
    pub fn decode_without_bom(self, bytes: &[u8]) -> String {
        let mut d = Decoder::new(self);
        let mut out = String::with_capacity(bytes.len());
        let _ = d.decode(bytes, true, false, &mut out);
        out
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Iso2022 {
    Ascii,
    Roman,
    Katakana,
    LeadByte,
    TrailByte,
    EscapeStart,
    Escape,
}

/// A streaming decoder.
pub struct Decoder {
    enc: Encoding,
    pending: VecDeque<u8>,
    // UTF-8
    needed: u8,
    seen: u8,
    cp: u32,
    lower: u8,
    upper: u8,
    // multi-byte leads
    lead: u8,
    first: u8,
    second: u8,
    third: u8,
    jis0212: bool,
    // UTF-16
    lead_byte: Option<u8>,
    lead_surrogate: Option<u16>,
    // ISO-2022-JP
    iso_state: Iso2022,
    iso_output_state: Iso2022,
    iso_output: bool,
    // replacement
    replaced: bool,
}

/// One decoding step's result.
enum Step {
    Continue,
    Emit(u32),
    Emit2(u32, u32),
    Error,
    Finished,
}

impl Decoder {
    pub fn new(enc: Encoding) -> Decoder {
        Decoder {
            enc,
            pending: VecDeque::new(),
            needed: 0,
            seen: 0,
            cp: 0,
            lower: 0x80,
            upper: 0xBF,
            lead: 0,
            first: 0,
            second: 0,
            third: 0,
            jis0212: false,
            lead_byte: None,
            lead_surrogate: None,
            iso_state: Iso2022::Ascii,
            iso_output_state: Iso2022::Ascii,
            iso_output: false,
            replaced: false,
        }
    }

    pub fn encoding(&self) -> Encoding {
        self.enc
    }

    /// Decode `input`, appending to `out`. `last`: the end of the stream
    /// (flush pending state). With `fatal`, returns Err at the first error
    /// (the decoder must not be used further).
    pub fn decode(&mut self, input: &[u8], last: bool, fatal: bool, out: &mut String) -> Result<(), ()> {
        let mut i = 0;
        loop {
            let byte = if let Some(b) = self.pending.pop_front() {
                Some(b)
            } else if i < input.len() {
                i += 1;
                Some(input[i - 1])
            } else if last {
                None
            } else {
                return Ok(());
            };
            let step = self.step(byte);
            match step {
                Step::Continue => {}
                Step::Emit(c) => push_cp(out, c),
                Step::Emit2(a, b) => {
                    push_cp(out, a);
                    push_cp(out, b);
                }
                Step::Error => {
                    if fatal {
                        return Err(());
                    }
                    out.push('\u{fffd}');
                }
                Step::Finished => return Ok(()),
            }
            if byte.is_none() && self.pending.is_empty() && self.at_rest() {
                return Ok(());
            }
        }
    }

    /// No buffered state left (end of queue handled).
    fn at_rest(&self) -> bool {
        self.needed == 0 && self.lead == 0 && self.first == 0 && self.second == 0 && self.third == 0 && self.lead_byte.is_none() && self.lead_surrogate.is_none()
            && !matches!(self.iso_state, Iso2022::EscapeStart | Iso2022::Escape | Iso2022::TrailByte)
    }

    fn prepend(&mut self, bytes: &[u8]) {
        for &b in bytes.iter().rev() {
            self.pending.push_front(b);
        }
    }

    fn step(&mut self, byte: Option<u8>) -> Step {
        let name = self.enc.name();
        match name {
            "UTF-8" => self.utf8(byte),
            "UTF-16BE" => self.utf16(byte, true),
            "UTF-16LE" => self.utf16(byte, false),
            "GBK" | "gb18030" => self.gb18030(byte),
            "Big5" => self.big5(byte),
            "EUC-JP" => self.euc_jp(byte),
            "ISO-2022-JP" => self.iso_2022_jp(byte),
            "Shift_JIS" => self.shift_jis(byte),
            "EUC-KR" => self.euc_kr(byte),
            "replacement" => match byte {
                None => Step::Finished,
                Some(_) if !self.replaced => {
                    self.replaced = true;
                    Step::Error
                }
                Some(_) => Step::Finished,
            },
            "x-user-defined" => match byte {
                None => Step::Finished,
                Some(b) if b < 0x80 => Step::Emit(b as u32),
                Some(b) => Step::Emit(0xF780 + b as u32 - 0x80),
            },
            _ => {
                let Some(t) = self.enc.single_byte_table() else { return Step::Finished };
                match byte {
                    None => Step::Finished,
                    Some(b) if b < 0x80 => Step::Emit(b as u32),
                    Some(b) => match u16_at(t * 256, (b - 0x80) as usize) {
                        0 => Step::Error,
                        c => Step::Emit(c),
                    },
                }
            }
        }
    }

    fn utf8(&mut self, byte: Option<u8>) -> Step {
        let Some(b) = byte else {
            if self.needed != 0 {
                self.needed = 0;
                return Step::Error;
            }
            return Step::Finished;
        };
        if self.needed == 0 {
            match b {
                0x00..=0x7F => return Step::Emit(b as u32),
                0xC2..=0xDF => {
                    self.needed = 1;
                    self.cp = (b & 0x1F) as u32;
                }
                0xE0..=0xEF => {
                    if b == 0xE0 {
                        self.lower = 0xA0;
                    }
                    if b == 0xED {
                        self.upper = 0x9F;
                    }
                    self.needed = 2;
                    self.cp = (b & 0xF) as u32;
                }
                0xF0..=0xF4 => {
                    if b == 0xF0 {
                        self.lower = 0x90;
                    }
                    if b == 0xF4 {
                        self.upper = 0x8F;
                    }
                    self.needed = 3;
                    self.cp = (b & 0x7) as u32;
                }
                _ => return Step::Error,
            }
            return Step::Continue;
        }
        if b < self.lower || b > self.upper {
            self.cp = 0;
            self.needed = 0;
            self.seen = 0;
            self.lower = 0x80;
            self.upper = 0xBF;
            self.prepend(&[b]);
            return Step::Error;
        }
        self.lower = 0x80;
        self.upper = 0xBF;
        self.cp = (self.cp << 6) | (b & 0x3F) as u32;
        self.seen += 1;
        if self.seen != self.needed {
            return Step::Continue;
        }
        let cp = self.cp;
        self.cp = 0;
        self.needed = 0;
        self.seen = 0;
        Step::Emit(cp)
    }

    fn utf16(&mut self, byte: Option<u8>, be: bool) -> Step {
        let Some(b) = byte else {
            if self.lead_byte.is_some() || self.lead_surrogate.is_some() {
                self.lead_byte = None;
                self.lead_surrogate = None;
                return Step::Error;
            }
            return Step::Finished;
        };
        let Some(lead) = self.lead_byte.take() else {
            self.lead_byte = Some(b);
            return Step::Continue;
        };
        let cu = if be { ((lead as u16) << 8) | b as u16 } else { ((b as u16) << 8) | lead as u16 };
        if let Some(s) = self.lead_surrogate.take() {
            if (0xDC00..=0xDFFF).contains(&cu) {
                return Step::Emit(0x10000 + ((s as u32 - 0xD800) << 10) + (cu as u32 - 0xDC00));
            }
            let (b1, b2) = ((cu >> 8) as u8, (cu & 0xFF) as u8);
            if be {
                self.prepend(&[b1, b2]);
            } else {
                self.prepend(&[b2, b1]);
            }
            return Step::Error;
        }
        if (0xD800..=0xDBFF).contains(&cu) {
            self.lead_surrogate = Some(cu);
            return Step::Continue;
        }
        if (0xDC00..=0xDFFF).contains(&cu) {
            return Step::Error;
        }
        Step::Emit(cu as u32)
    }

    fn gb18030(&mut self, byte: Option<u8>) -> Step {
        let Some(b) = byte else {
            if self.first == 0 && self.second == 0 && self.third == 0 {
                return Step::Finished;
            }
            self.first = 0;
            self.second = 0;
            self.third = 0;
            return Step::Error;
        };
        if self.third != 0 {
            if !(0x30..=0x39).contains(&b) {
                let (s, t) = (self.second, self.third);
                self.prepend(&[s, t, b]);
                self.first = 0;
                self.second = 0;
                self.third = 0;
                return Step::Error;
            }
            let pointer = (((self.first as u32 - 0x81) * 10 + self.second as u32 - 0x30) * 126 + self.third as u32 - 0x81) * 10 + b as u32 - 0x30;
            self.first = 0;
            self.second = 0;
            self.third = 0;
            return match gb18030_ranges(pointer) {
                Some(c) => Step::Emit(c),
                None => Step::Error,
            };
        }
        if self.second != 0 {
            if (0x81..=0xFE).contains(&b) {
                self.third = b;
                return Step::Continue;
            }
            let s = self.second;
            self.prepend(&[s, b]);
            self.first = 0;
            self.second = 0;
            return Step::Error;
        }
        if self.first != 0 {
            if (0x30..=0x39).contains(&b) {
                self.second = b;
                return Step::Continue;
            }
            let lead = self.first as usize;
            self.first = 0;
            let offset = if b < 0x7F { 0x40 } else { 0x41 };
            let pointer = if (0x40..=0x7E).contains(&b) || (0x80..=0xFE).contains(&b) { Some((lead - 0x81) * 190 + (b as usize - offset)) } else { None };
            if let Some(c) = pointer.and_then(gb18030_index) {
                return Step::Emit(c);
            }
            if b < 0x80 {
                self.prepend(&[b]);
            }
            return Step::Error;
        }
        match b {
            0x00..=0x7F => Step::Emit(b as u32),
            0x80 => Step::Emit(0x20AC),
            0x81..=0xFE => {
                self.first = b;
                Step::Continue
            }
            _ => Step::Error,
        }
    }

    fn big5(&mut self, byte: Option<u8>) -> Step {
        let Some(b) = byte else {
            if self.lead != 0 {
                self.lead = 0;
                return Step::Error;
            }
            return Step::Finished;
        };
        if self.lead != 0 {
            let lead = self.lead as usize;
            self.lead = 0;
            let offset = if b < 0x7F { 0x40 } else { 0x62 };
            let pointer = if (0x40..=0x7E).contains(&b) || (0xA1..=0xFE).contains(&b) { Some((lead - 0x81) * 157 + (b as usize - offset)) } else { None };
            match pointer {
                Some(1133) => return Step::Emit2(0x00CA, 0x0304),
                Some(1135) => return Step::Emit2(0x00CA, 0x030C),
                Some(1164) => return Step::Emit2(0x00EA, 0x0304),
                Some(1166) => return Step::Emit2(0x00EA, 0x030C),
                _ => {}
            }
            if let Some(c) = pointer.and_then(big5) {
                return Step::Emit(c);
            }
            if b < 0x80 {
                self.prepend(&[b]);
            }
            return Step::Error;
        }
        match b {
            0x00..=0x7F => Step::Emit(b as u32),
            0x81..=0xFE => {
                self.lead = b;
                Step::Continue
            }
            _ => Step::Error,
        }
    }

    fn euc_jp(&mut self, byte: Option<u8>) -> Step {
        let Some(b) = byte else {
            if self.lead != 0 {
                self.lead = 0;
                return Step::Error;
            }
            return Step::Finished;
        };
        if self.lead == 0x8E && (0xA1..=0xDF).contains(&b) {
            self.lead = 0;
            return Step::Emit(0xFF61 - 0xA1 + b as u32);
        }
        if self.lead == 0x8F && (0xA1..=0xFE).contains(&b) {
            self.jis0212 = true;
            self.lead = b;
            return Step::Continue;
        }
        if self.lead != 0 {
            let lead = self.lead;
            self.lead = 0;
            let mut cp = None;
            if (0xA1..=0xFE).contains(&lead) && (0xA1..=0xFE).contains(&b) {
                let p = (lead as usize - 0xA1) * 94 + b as usize - 0xA1;
                cp = if self.jis0212 { jis0212(p) } else { jis0208(p) };
            }
            self.jis0212 = false;
            if let Some(c) = cp {
                return Step::Emit(c);
            }
            if b < 0x80 {
                self.prepend(&[b]);
            }
            return Step::Error;
        }
        match b {
            0x00..=0x7F => Step::Emit(b as u32),
            0x8E | 0x8F | 0xA1..=0xFE => {
                self.lead = b;
                Step::Continue
            }
            _ => Step::Error,
        }
    }

    fn shift_jis(&mut self, byte: Option<u8>) -> Step {
        let Some(b) = byte else {
            if self.lead != 0 {
                self.lead = 0;
                return Step::Error;
            }
            return Step::Finished;
        };
        if self.lead != 0 {
            let lead = self.lead as usize;
            self.lead = 0;
            let offset = if b < 0x7F { 0x40 } else { 0x41 };
            let lead_offset = if lead < 0xA0 { 0x81 } else { 0xC1 };
            let pointer = if (0x40..=0x7E).contains(&b) || (0x80..=0xFC).contains(&b) { Some((lead - lead_offset) * 188 + b as usize - offset) } else { None };
            if let Some(p) = pointer {
                if (8836..=10715).contains(&p) {
                    return Step::Emit(0xE000 - 8836 + p as u32);
                }
            }
            if let Some(c) = pointer.and_then(jis0208) {
                return Step::Emit(c);
            }
            if b < 0x80 {
                self.prepend(&[b]);
            }
            return Step::Error;
        }
        match b {
            0x00..=0x80 => Step::Emit(b as u32),
            0xA1..=0xDF => Step::Emit(0xFF61 - 0xA1 + b as u32),
            0x81..=0x9F | 0xE0..=0xFC => {
                self.lead = b;
                Step::Continue
            }
            _ => Step::Error,
        }
    }

    fn euc_kr(&mut self, byte: Option<u8>) -> Step {
        let Some(b) = byte else {
            if self.lead != 0 {
                self.lead = 0;
                return Step::Error;
            }
            return Step::Finished;
        };
        if self.lead != 0 {
            let lead = self.lead as usize;
            self.lead = 0;
            let pointer = if (0x41..=0xFE).contains(&b) { Some((lead - 0x81) * 190 + (b as usize - 0x41)) } else { None };
            if let Some(c) = pointer.and_then(euckr) {
                return Step::Emit(c);
            }
            if b < 0x80 {
                self.prepend(&[b]);
            }
            return Step::Error;
        }
        match b {
            0x00..=0x7F => Step::Emit(b as u32),
            0x81..=0xFE => {
                self.lead = b;
                Step::Continue
            }
            _ => Step::Error,
        }
    }

    fn iso_2022_jp(&mut self, byte: Option<u8>) -> Step {
        use Iso2022::*;
        match self.iso_state {
            Ascii => match byte {
                Some(0x1B) => {
                    self.iso_state = EscapeStart;
                    Step::Continue
                }
                Some(b) if b <= 0x7F && b != 0x0E && b != 0x0F => {
                    self.iso_output = false;
                    Step::Emit(b as u32)
                }
                None => Step::Finished,
                Some(_) => {
                    self.iso_output = false;
                    Step::Error
                }
            },
            Roman => match byte {
                Some(0x1B) => {
                    self.iso_state = EscapeStart;
                    Step::Continue
                }
                Some(0x5C) => {
                    self.iso_output = false;
                    Step::Emit(0xA5)
                }
                Some(0x7E) => {
                    self.iso_output = false;
                    Step::Emit(0x203E)
                }
                Some(b) if b <= 0x7F && b != 0x0E && b != 0x0F => {
                    self.iso_output = false;
                    Step::Emit(b as u32)
                }
                None => Step::Finished,
                Some(_) => {
                    self.iso_output = false;
                    Step::Error
                }
            },
            Katakana => match byte {
                Some(0x1B) => {
                    self.iso_state = EscapeStart;
                    Step::Continue
                }
                Some(b @ 0x21..=0x5F) => {
                    self.iso_output = false;
                    Step::Emit(0xFF61 - 0x21 + b as u32)
                }
                None => Step::Finished,
                Some(_) => {
                    self.iso_output = false;
                    Step::Error
                }
            },
            LeadByte => match byte {
                Some(0x1B) => {
                    self.iso_state = EscapeStart;
                    Step::Continue
                }
                Some(b @ 0x21..=0x7E) => {
                    self.iso_output = false;
                    self.lead = b;
                    self.iso_state = TrailByte;
                    Step::Continue
                }
                None => Step::Finished,
                Some(_) => {
                    self.iso_output = false;
                    Step::Error
                }
            },
            TrailByte => match byte {
                Some(0x1B) => {
                    self.iso_state = EscapeStart;
                    self.lead = 0;
                    Step::Error
                }
                Some(b @ 0x21..=0x7E) => {
                    self.iso_state = LeadByte;
                    let p = (self.lead as usize - 0x21) * 94 + b as usize - 0x21;
                    self.lead = 0;
                    match jis0208(p) {
                        Some(c) => Step::Emit(c),
                        None => Step::Error,
                    }
                }
                None => {
                    self.iso_state = LeadByte;
                    self.lead = 0;
                    Step::Error
                }
                Some(_) => {
                    self.iso_state = LeadByte;
                    self.lead = 0;
                    Step::Error
                }
            },
            EscapeStart => match byte {
                Some(b @ (0x24 | 0x28)) => {
                    self.lead = b;
                    self.iso_state = Escape;
                    Step::Continue
                }
                _ => {
                    if let Some(b) = byte {
                        self.prepend(&[b]);
                    }
                    self.iso_output = false;
                    self.iso_state = self.iso_output_state;
                    Step::Error
                }
            },
            Escape => {
                let lead = self.lead;
                self.lead = 0;
                let state = match (lead, byte) {
                    (0x28, Some(0x42)) => Some(Ascii),
                    (0x28, Some(0x4A)) => Some(Roman),
                    (0x28, Some(0x49)) => Some(Katakana),
                    (0x24, Some(0x40 | 0x42)) => Some(LeadByte),
                    _ => None,
                };
                if let Some(s) = state {
                    self.iso_state = s;
                    self.iso_output_state = s;
                    let was = self.iso_output;
                    self.iso_output = true;
                    return if was { Step::Error } else { Step::Continue };
                }
                match byte {
                    Some(b) => self.prepend(&[lead, b]),
                    None => self.prepend(&[lead]),
                }
                self.iso_output = false;
                self.iso_state = self.iso_output_state;
                Step::Error
            }
        }
    }
}

fn push_cp(out: &mut String, c: u32) {
    out.push(char::from_u32(c).unwrap_or('\u{fffd}'));
}

/// Sniff a byte order mark: (encoding, BOM length).
pub fn bom_sniff(bytes: &[u8]) -> Option<(Encoding, usize)> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        Some((Encoding::UTF_8, 3))
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        Some((Encoding::UTF_16BE, 2))
    } else if bytes.starts_with(&[0xFF, 0xFE]) {
        Some((Encoding::UTF_16LE, 2))
    } else {
        None
    }
}

/// "Decode": BOM sniffing, then `fallback`.
pub fn decode_with_bom(bytes: &[u8], fallback: Encoding) -> String {
    match bom_sniff(bytes) {
        Some((e, n)) => e.decode_without_bom(&bytes[n..]),
        None => fallback.decode_without_bom(bytes),
    }
}

pub const NAMES: [&str; 40] = ["UTF-8", "IBM866", "ISO-8859-2", "ISO-8859-3", "ISO-8859-4", "ISO-8859-5", "ISO-8859-6", "ISO-8859-7", "ISO-8859-8", "ISO-8859-8-I", "ISO-8859-10", "ISO-8859-13", "ISO-8859-14", "ISO-8859-15", "ISO-8859-16", "KOI8-R", "KOI8-U", "macintosh", "windows-874", "windows-1250", "windows-1251", "windows-1252", "windows-1253", "windows-1254", "windows-1255", "windows-1256", "windows-1257", "windows-1258", "x-mac-cyrillic", "GBK", "gb18030", "Big5", "EUC-JP", "ISO-2022-JP", "Shift_JIS", "EUC-KR", "replacement", "UTF-16BE", "UTF-16LE", "x-user-defined"];

fn label_index(label: &str) -> Option<usize> {
    Some(match label {
        "unicode-1-1-utf-8" | "unicode11utf8" | "unicode20utf8" | "utf-8" | "utf8" | "x-unicode20utf8" => 0,
        "866" | "cp866" | "csibm866" | "ibm866" => 1,
        "csisolatin2" | "iso-8859-2" | "iso-ir-101" | "iso8859-2" | "iso88592" | "iso_8859-2" | "iso_8859-2:1987" | "l2" | "latin2" => 2,
        "csisolatin3" | "iso-8859-3" | "iso-ir-109" | "iso8859-3" | "iso88593" | "iso_8859-3" | "iso_8859-3:1988" | "l3" | "latin3" => 3,
        "csisolatin4" | "iso-8859-4" | "iso-ir-110" | "iso8859-4" | "iso88594" | "iso_8859-4" | "iso_8859-4:1988" | "l4" | "latin4" => 4,
        "csisolatincyrillic" | "cyrillic" | "iso-8859-5" | "iso-ir-144" | "iso8859-5" | "iso88595" | "iso_8859-5" | "iso_8859-5:1988" => 5,
        "arabic" | "asmo-708" | "csiso88596e" | "csiso88596i" | "csisolatinarabic" | "ecma-114" | "iso-8859-6" | "iso-8859-6-e" | "iso-8859-6-i" | "iso-ir-127" | "iso8859-6" | "iso88596" | "iso_8859-6" | "iso_8859-6:1987" => 6,
        "csisolatingreek" | "ecma-118" | "elot_928" | "greek" | "greek8" | "iso-8859-7" | "iso-ir-126" | "iso8859-7" | "iso88597" | "iso_8859-7" | "iso_8859-7:1987" | "sun_eu_greek" => 7,
        "csiso88598e" | "csisolatinhebrew" | "hebrew" | "iso-8859-8" | "iso-8859-8-e" | "iso-ir-138" | "iso8859-8" | "iso88598" | "iso_8859-8" | "iso_8859-8:1988" | "visual" => 8,
        "csiso88598i" | "iso-8859-8-i" | "logical" => 9,
        "csisolatin6" | "iso-8859-10" | "iso-ir-157" | "iso8859-10" | "iso885910" | "l6" | "latin6" => 10,
        "iso-8859-13" | "iso8859-13" | "iso885913" => 11,
        "iso-8859-14" | "iso8859-14" | "iso885914" => 12,
        "csisolatin9" | "iso-8859-15" | "iso8859-15" | "iso885915" | "iso_8859-15" | "l9" => 13,
        "iso-8859-16" => 14,
        "cskoi8r" | "koi" | "koi8" | "koi8-r" | "koi8_r" => 15,
        "koi8-ru" | "koi8-u" => 16,
        "csmacintosh" | "mac" | "macintosh" | "x-mac-roman" => 17,
        "dos-874" | "iso-8859-11" | "iso8859-11" | "iso885911" | "tis-620" | "windows-874" => 18,
        "cp1250" | "windows-1250" | "x-cp1250" => 19,
        "cp1251" | "windows-1251" | "x-cp1251" => 20,
        "ansi_x3.4-1968" | "ascii" | "cp1252" | "cp819" | "csisolatin1" | "ibm819" | "iso-8859-1" | "iso-ir-100" | "iso8859-1" | "iso88591" | "iso_8859-1" | "iso_8859-1:1987" | "l1" | "latin1" | "us-ascii" | "windows-1252" | "x-cp1252" => 21,
        "cp1253" | "windows-1253" | "x-cp1253" => 22,
        "cp1254" | "csisolatin5" | "iso-8859-9" | "iso-ir-148" | "iso8859-9" | "iso88599" | "iso_8859-9" | "iso_8859-9:1989" | "l5" | "latin5" | "windows-1254" | "x-cp1254" => 23,
        "cp1255" | "windows-1255" | "x-cp1255" => 24,
        "cp1256" | "windows-1256" | "x-cp1256" => 25,
        "cp1257" | "windows-1257" | "x-cp1257" => 26,
        "cp1258" | "windows-1258" | "x-cp1258" => 27,
        "x-mac-cyrillic" | "x-mac-ukrainian" => 28,
        "chinese" | "csgb2312" | "csiso58gb231280" | "gb2312" | "gb_2312" | "gb_2312-80" | "gbk" | "iso-ir-58" | "x-gbk" => 29,
        "gb18030" => 30,
        "big5" | "big5-hkscs" | "cn-big5" | "csbig5" | "x-x-big5" => 31,
        "cseucpkdfmtjapanese" | "euc-jp" | "x-euc-jp" => 32,
        "csiso2022jp" | "iso-2022-jp" => 33,
        "csshiftjis" | "ms932" | "ms_kanji" | "shift-jis" | "shift_jis" | "sjis" | "windows-31j" | "x-sjis" => 34,
        "cseuckr" | "csksc56011987" | "euc-kr" | "iso-ir-149" | "korean" | "ks_c_5601-1987" | "ks_c_5601-1989" | "ksc5601" | "ksc_5601" | "windows-949" => 35,
        "csiso2022kr" | "hz-gb-2312" | "iso-2022-cn" | "iso-2022-cn-ext" | "iso-2022-kr" | "replacement" => 36,
        "unicodefffe" | "utf-16be" => 37,
        "csunicode" | "iso-10646-ucs-2" | "ucs-2" | "unicode" | "unicodefeff" | "utf-16" | "utf-16le" => 38,
        "x-user-defined" => 39,
        _ => return None,
    })
}

// ------------------------------------------------------------- encoders


/// Reverse index: code point -> first pointer (16-bit indexes).
fn reverse16(off: usize, len: usize, skip: impl Fn(usize) -> bool) -> HashMap<u32, usize> {
    let mut m = HashMap::with_capacity(len);
    for p in 0..len {
        if skip(p) {
            continue;
        }
        let c = u16_at(off, p);
        if c != 0 {
            m.entry(c).or_insert(p);
        }
    }
    m
}

fn rev_jis0208() -> &'static HashMap<u32, usize> {
    static M: OnceLock<HashMap<u32, usize>> = OnceLock::new();
    M.get_or_init(|| reverse16(OFF_JIS0208, 11104, |_| false))
}

fn rev_shift_jis() -> &'static HashMap<u32, usize> {
    static M: OnceLock<HashMap<u32, usize>> = OnceLock::new();
    M.get_or_init(|| reverse16(OFF_JIS0208, 11104, |p| (8272..=8835).contains(&p)))
}

fn rev_gb18030() -> &'static HashMap<u32, usize> {
    static M: OnceLock<HashMap<u32, usize>> = OnceLock::new();
    M.get_or_init(|| reverse16(OFF_GB18030, 23940, |_| false))
}

fn rev_euckr() -> &'static HashMap<u32, usize> {
    static M: OnceLock<HashMap<u32, usize>> = OnceLock::new();
    M.get_or_init(|| reverse16(OFF_EUCKR, 23750, |_| false))
}

fn rev_big5() -> &'static HashMap<u32, usize> {
    static M: OnceLock<HashMap<u32, usize>> = OnceLock::new();
    M.get_or_init(|| {
        let mut m = HashMap::new();
        for p in ((0xA1 - 0x81) * 157)..19782 {
            let c = u32_at(OFF_BIG5 + 4 * p);
            if c == 0 {
                continue;
            }
            // The last pointer for a few code points, the first otherwise.
            if matches!(c, 0x2550 | 0x255E | 0x2561 | 0x256A | 0x5341 | 0x5345) {
                m.insert(c, p);
            } else {
                m.entry(c).or_insert(p);
            }
        }
        m
    })
}

fn gb18030_ranges_pointer(cp: u32) -> u32 {
    if cp == 0xE7C7 {
        return 7457;
    }
    let mut best = (0u32, 0u32);
    for i in 0..207 {
        let c = u32_at(OFF_RANGES + 8 * i + 4);
        if c <= cp {
            best = (u32_at(OFF_RANGES + 8 * i), c);
        } else {
            break;
        }
    }
    best.0 + cp - best.1
}

/// ISO-2022-JP encoder state.
#[derive(Clone, Copy, PartialEq, Eq)]
enum JisState {
    Ascii,
    Roman,
    Jis0208,
}

impl Encoding {
    /// The output encoding: UTF-16 and replacement encode as UTF-8.
    pub fn output_encoding(self) -> Encoding {
        match self.name() {
            "replacement" | "UTF-16BE" | "UTF-16LE" => Encoding::UTF_8,
            _ => self,
        }
    }

    /// Encode `s`; code points the encoding cannot represent are passed
    /// to `on_error` (which appends a replacement, e.g. `&#NNNN;`).
    pub fn encode_with(self, s: &str, out: &mut Vec<u8>, mut on_error: impl FnMut(u32, &mut Vec<u8>)) {
        let enc = self.output_encoding();
        let name = enc.name();
        if name == "UTF-8" {
            out.extend_from_slice(s.as_bytes());
            return;
        }
        let mut jis = JisState::Ascii;
        let mut chars: VecDeque<u32> = s.chars().map(|c| c as u32).collect();
        while let Some(mut cp) = chars.pop_front() {
            if name != "ISO-2022-JP" && cp < 0x80 {
                out.push(cp as u8);
                continue;
            }
            match name {
                "x-user-defined" => {
                    if (0xF780..=0xF7FF).contains(&cp) {
                        out.push((cp - 0xF780 + 0x80) as u8);
                    } else {
                        on_error(cp, out);
                    }
                }
                "GBK" | "gb18030" => {
                    if cp == 0xE5E5 {
                        on_error(cp, out);
                        continue;
                    }
                    if name == "GBK" && cp == 0x20AC {
                        out.push(0x80);
                        continue;
                    }
                    if let Some(&p) = rev_gb18030().get(&cp) {
                        let lead = p / 190 + 0x81;
                        let trail = p % 190;
                        let offset = if trail < 0x3F { 0x40 } else { 0x41 };
                        out.push(lead as u8);
                        out.push((trail + offset) as u8);
                        continue;
                    }
                    if name == "GBK" {
                        on_error(cp, out);
                        continue;
                    }
                    let mut p = gb18030_ranges_pointer(cp);
                    let b1 = p / (10 * 126 * 10);
                    p %= 10 * 126 * 10;
                    let b2 = p / (10 * 126);
                    p %= 10 * 126;
                    let b3 = p / 10;
                    let b4 = p % 10;
                    out.extend_from_slice(&[(b1 + 0x81) as u8, (b2 + 0x30) as u8, (b3 + 0x81) as u8, (b4 + 0x30) as u8]);
                }
                "Big5" => match rev_big5().get(&cp) {
                    Some(&p) => {
                        let lead = p / 157 + 0x81;
                        let trail = p % 157;
                        let offset = if trail < 0x3F { 0x40 } else { 0x62 };
                        out.push(lead as u8);
                        out.push((trail + offset) as u8);
                    }
                    None => on_error(cp, out),
                },
                "EUC-JP" => {
                    if cp == 0xA5 {
                        out.push(0x5C);
                        continue;
                    }
                    if cp == 0x203E {
                        out.push(0x7E);
                        continue;
                    }
                    if (0xFF61..=0xFF9F).contains(&cp) {
                        out.push(0x8E);
                        out.push((cp - 0xFF61 + 0xA1) as u8);
                        continue;
                    }
                    if cp == 0x2212 {
                        cp = 0xFF0D;
                    }
                    match rev_jis0208().get(&cp) {
                        Some(&p) => {
                            out.push((p / 94 + 0xA1) as u8);
                            out.push((p % 94 + 0xA1) as u8);
                        }
                        None => on_error(cp, out),
                    }
                }
                "Shift_JIS" => {
                    if cp == 0x80 {
                        out.push(0x80);
                        continue;
                    }
                    if cp == 0xA5 {
                        out.push(0x5C);
                        continue;
                    }
                    if cp == 0x203E {
                        out.push(0x7E);
                        continue;
                    }
                    if (0xFF61..=0xFF9F).contains(&cp) {
                        out.push((cp - 0xFF61 + 0xA1) as u8);
                        continue;
                    }
                    if cp == 0x2212 {
                        cp = 0xFF0D;
                    }
                    match rev_shift_jis().get(&cp) {
                        Some(&p) => {
                            let lead = p / 188;
                            let lead_offset = if lead < 0x1F { 0x81 } else { 0xC1 };
                            let trail = p % 188;
                            let offset = if trail < 0x3F { 0x40 } else { 0x41 };
                            out.push((lead + lead_offset) as u8);
                            out.push((trail + offset) as u8);
                        }
                        None => on_error(cp, out),
                    }
                }
                "EUC-KR" => match rev_euckr().get(&cp) {
                    Some(&p) => {
                        out.push((p / 190 + 0x81) as u8);
                        out.push((p % 190 + 0x41) as u8);
                    }
                    None => on_error(cp, out),
                },
                "ISO-2022-JP" => {
                    if (jis == JisState::Ascii || jis == JisState::Roman) && matches!(cp, 0x0E | 0x0F | 0x1B) {
                        on_error(0xFFFD, out);
                        continue;
                    }
                    if jis == JisState::Ascii && cp < 0x80 {
                        out.push(cp as u8);
                        continue;
                    }
                    if jis == JisState::Roman && ((cp < 0x80 && cp != 0x5C && cp != 0x7E) || cp == 0xA5 || cp == 0x203E) {
                        out.push(match cp {
                            0xA5 => 0x5C,
                            0x203E => 0x7E,
                            c => c as u8,
                        });
                        continue;
                    }
                    if cp < 0x80 && jis != JisState::Ascii {
                        chars.push_front(cp);
                        jis = JisState::Ascii;
                        out.extend_from_slice(&[0x1B, 0x28, 0x42]);
                        continue;
                    }
                    if (cp == 0xA5 || cp == 0x203E) && jis != JisState::Roman {
                        chars.push_front(cp);
                        jis = JisState::Roman;
                        out.extend_from_slice(&[0x1B, 0x28, 0x4A]);
                        continue;
                    }
                    if cp == 0x2212 {
                        cp = 0xFF0D;
                    }
                    if (0xFF61..=0xFF9F).contains(&cp) {
                        cp = u16_at(OFF_KATAKANA, (cp - 0xFF61) as usize);
                    }
                    let Some(&p) = rev_jis0208().get(&cp) else {
                        if jis == JisState::Jis0208 {
                            chars.push_front(cp);
                            jis = JisState::Ascii;
                            out.extend_from_slice(&[0x1B, 0x28, 0x42]);
                        } else {
                            on_error(cp, out);
                        }
                        continue;
                    };
                    if jis != JisState::Jis0208 {
                        chars.push_front(cp);
                        jis = JisState::Jis0208;
                        out.extend_from_slice(&[0x1B, 0x24, 0x42]);
                        continue;
                    }
                    out.push((p / 94 + 0x21) as u8);
                    out.push((p % 94 + 0x21) as u8);
                }
                _ => {
                    // Single-byte encodings.
                    let t = enc.single_byte_table().unwrap_or(0);
                    match (0..128).find(|&i| u16_at(t * 256, i) == cp) {
                        Some(i) => out.push((i + 0x80) as u8),
                        None => on_error(cp, out),
                    }
                }
            }
        }
        if name == "ISO-2022-JP" && jis != JisState::Ascii {
            out.extend_from_slice(&[0x1B, 0x28, 0x42]);
        }
    }

    /// Encode with the "html" error mode (`&#NNNN;` for unencodable code
    /// points), as form submission does.
    pub fn encode_html(self, s: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(s.len());
        self.encode_with(s, &mut out, |cp, out| out.extend_from_slice(format!("&#{cp};").as_bytes()));
        out
    }
}

/// "Percent-encode after encoding" (URL §1.3) for the query state: encode
/// with `enc` (HTML error mode) and percent-encode bytes in `set`.
pub fn percent_encode_after_encoding(enc: Encoding, s: &str, set: impl Fn(u8) -> bool, space_as_plus: bool) -> String {
    let mut bytes = Vec::with_capacity(s.len());
    let mut errors: Vec<(usize, u32)> = Vec::new();
    enc.encode_with(s, &mut bytes, |cp, out| errors.push((out.len(), cp)));
    let mut out = String::with_capacity(bytes.len());
    let mut e = errors.iter().peekable();
    for (i, &b) in bytes.iter().enumerate() {
        while let Some(&&(at, cp)) = e.peek() {
            if at != i {
                break;
            }
            out.push_str(&format!("%26%23{cp}%3B"));
            e.next();
        }
        if space_as_plus && b == b' ' {
            out.push('+');
        } else if set(b) {
            out.push_str(&format!("%{b:02X}"));
        } else {
            out.push(b as char);
        }
    }
    for &(_, cp) in e {
        out.push_str(&format!("%26%23{cp}%3B"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(label: &str) -> Encoding {
        Encoding::for_label(label).unwrap()
    }

    #[test]
    fn labels() {
        assert_eq!(enc(" latin1 ").name(), "windows-1252");
        assert_eq!(enc("SJIS").name(), "Shift_JIS");
        assert_eq!(enc("unicode-1-1-utf-8").name(), "UTF-8");
        assert!(Encoding::for_label("utf-7").is_none());
    }

    #[test]
    fn decoders_stream_and_errors() {
        let mut d = enc("utf-8").new_decoder();
        let mut out = String::new();
        d.decode(&[0xE3, 0x81], false, false, &mut out).unwrap();
        d.decode(&[0x82, 0xFF], true, false, &mut out).unwrap();
        assert_eq!(out, "あ\u{fffd}");
        let mut d = enc("utf-8").new_decoder();
        assert!(d.decode(&[0xC3], true, true, &mut String::new()).is_err());
        assert_eq!(enc("utf-16le").decode_without_bom(&[0x3D, 0xD8, 0x00, 0xDE]), "😀");
        assert_eq!(enc("shift_jis").decode_without_bom(&[0x82, 0xA0, 0x41]), "あA");
        assert_eq!(enc("big5").decode_without_bom(&[0x88, 0x62]), "\u{00CA}\u{0304}");
        assert_eq!(enc("x-user-defined").decode_without_bom(&[0x80]), "\u{F780}");
    }

    #[test]
    fn encoders() {
        let e = |l: &str, s: &str| enc(l).encode_html(s);
        assert_eq!(e("shift_jis", "あ"), vec![0x82, 0xA0]);
        assert_eq!(e("euc-jp", "あ¥"), vec![0xA4, 0xA2, 0x5C]);
        assert_eq!(e("gb18030", "\u{80}"), vec![0x81, 0x30, 0x81, 0x30]);
        assert_eq!(e("gbk", "€"), vec![0x80]);
        assert_eq!(e("iso-2022-jp", "aあb"), vec![b'a', 0x1B, 0x24, 0x42, 0x24, 0x22, 0x1B, 0x28, 0x42, b'b']);
        assert_eq!(e("windows-1252", "é☃"), b"\xe9&#9731;".to_vec());
        assert_eq!(e("utf-16le", "é"), "é".as_bytes().to_vec());
        let q = percent_encode_after_encoding(enc("euc-jp"), "あ ☃", |b| b < 0x21 || b > 0x7E, true);
        assert_eq!(q, "%A4%A2+%26%239731%3B");
    }
}
