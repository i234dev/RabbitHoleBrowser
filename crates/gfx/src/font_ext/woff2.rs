//! WOFF 2.0 decoding: brotli-compressed table stream, `glyf`/`loca`
//! reconstruction (transform version 0) and `hmtx` reconstruction.

use super::sfnt::{build_sfnt, rd16, rd32};

const KNOWN_TAGS: [&[u8; 4]; 63] = [
    b"cmap", b"head", b"hhea", b"hmtx", b"maxp", b"name", b"OS/2", b"post", b"cvt ", b"fpgm", b"glyf", b"loca", b"prep", b"CFF ", b"VORG", b"EBDT",
    b"EBLC", b"gasp", b"hdmx", b"kern", b"LTSH", b"PCLT", b"VDMX", b"vhea", b"vmtx", b"BASE", b"GDEF", b"GPOS", b"GSUB", b"EBSC", b"JSTF", b"MATH",
    b"CBDT", b"CBLC", b"COLR", b"CPAL", b"SVG ", b"sbix", b"acnt", b"avar", b"bdat", b"bloc", b"bsln", b"cvar", b"fdsc", b"feat", b"fmtx", b"fvar",
    b"gvar", b"hsty", b"just", b"lcar", b"mort", b"morx", b"opbd", b"prop", b"trak", b"Zapf", b"Silf", b"Glat", b"Gloc", b"Feat", b"Sill",
];

struct Entry {
    tag: [u8; 4],
    orig_len: u32,
    transform_len: Option<u32>,
    transformed: bool,
}

struct Reader<'a> {
    d: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.d.get(self.pos)?;
        self.pos += 1;
        Some(v)
    }
    fn u16(&mut self) -> Option<u16> {
        let v = rd16(self.d, self.pos)?;
        self.pos += 2;
        Some(v)
    }
    fn i16(&mut self) -> Option<i16> {
        self.u16().map(|v| v as i16)
    }
    fn u32(&mut self) -> Option<u32> {
        let v = rd32(self.d, self.pos)?;
        self.pos += 4;
        Some(v)
    }
    fn bytes(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.d.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s)
    }
    /// UIntBase128
    fn base128(&mut self) -> Option<u32> {
        let mut acc: u32 = 0;
        for i in 0..5 {
            let b = self.u8()?;
            if i == 0 && b == 0x80 {
                return None;
            }
            if acc & 0xFE00_0000 != 0 {
                return None;
            }
            acc = (acc << 7) | (b & 0x7f) as u32;
            if b & 0x80 == 0 {
                return Some(acc);
            }
        }
        None
    }
    /// 255UInt16
    fn u255(&mut self) -> Option<u16> {
        let b = self.u8()?;
        Some(match b {
            253 => self.u16()?,
            254 => self.u8()? as u16 + 506,
            255 => self.u8()? as u16 + 253,
            _ => b as u16,
        })
    }
}

pub fn decode(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 48 || &data[0..4] != b"wOF2" {
        return None;
    }
    let flavor = rd32(data, 4)?;
    let num_tables = rd16(data, 12)? as usize;
    let total_compressed = rd32(data, 20)? as usize;
    if num_tables == 0 || num_tables > 512 {
        return None;
    }
    let mut r = Reader { d: data, pos: 48 };
    let mut entries = Vec::with_capacity(num_tables);
    for _ in 0..num_tables {
        let flags = r.u8()?;
        let tag_idx = (flags & 0x3f) as usize;
        let tag = if tag_idx == 63 {
            let b = r.bytes(4)?;
            [b[0], b[1], b[2], b[3]]
        } else {
            *KNOWN_TAGS[tag_idx]
        };
        let version = (flags >> 6) & 0x03;
        let orig_len = r.base128()?;
        let transformed = match &tag {
            b"glyf" | b"loca" => version == 0,
            b"hmtx" => version == 1,
            _ => false,
        };
        let transform_len = if transformed { Some(r.base128()?) } else { None };
        entries.push(Entry { tag, orig_len, transform_len, transformed });
    }
    if flavor == 0x74746366 {
        // Collections: not supported (take the whole table set as one font
        // would need the collection directory; rare on the web).
        return None;
    }
    let comp = data.get(r.pos..r.pos.checked_add(total_compressed)?)?;
    let mut out: Vec<u8> = Vec::new();
    {
        let mut input = comp;
        brotli_decompressor::BrotliDecompress(&mut input, &mut out).ok()?;
    }
    // Split into tables.
    let mut raw: Vec<([u8; 4], &[u8], bool)> = Vec::with_capacity(entries.len());
    let mut pos = 0usize;
    for e in &entries {
        let len = e.transform_len.unwrap_or(e.orig_len) as usize;
        let slice = out.get(pos..pos.checked_add(len)?)?;
        raw.push((e.tag, slice, e.transformed));
        pos += len;
    }
    let mut tables: Vec<([u8; 4], Vec<u8>)> = Vec::with_capacity(raw.len());
    let mut glyf_bboxes: Option<Vec<[i16; 4]>> = None;
    let mut index_format: u16 = 0;
    let mut num_glyphs: u16 = 0;
    // First pass: reconstruct glyf/loca if transformed.
    let glyf = raw.iter().find(|(t, _, _)| t == b"glyf");
    if let Some((_, gdata, true)) = glyf {
        let (glyf_out, loca_out, bboxes, fmt, n) = reconstruct_glyf(gdata)?;
        glyf_bboxes = Some(bboxes);
        index_format = fmt;
        num_glyphs = n;
        tables.push((*b"glyf", glyf_out));
        tables.push((*b"loca", loca_out));
    }
    let num_hmetrics = raw.iter().find(|(t, _, _)| t == b"hhea").and_then(|(_, d, _)| rd16(d, 34)).unwrap_or(0);
    if num_glyphs == 0 {
        num_glyphs = raw.iter().find(|(t, _, _)| t == b"maxp").and_then(|(_, d, _)| rd16(d, 4)).unwrap_or(0);
    }
    for (tag, d, transformed) in &raw {
        match tag {
            b"glyf" | b"loca" if *transformed || glyf_bboxes.is_some() => {
                if !*transformed && tag == b"loca" {
                    // loca listed as transformed is rebuilt above; untransformed loca alongside transformed glyf is invalid
                }
            }
            b"hmtx" if *transformed => {
                let bboxes = glyf_bboxes.as_deref();
                tables.push((*b"hmtx", reconstruct_hmtx(d, num_glyphs, num_hmetrics, bboxes)?));
            }
            b"head" if glyf_bboxes.is_some() => {
                let mut h = d.to_vec();
                if h.len() >= 52 {
                    h[50] = (index_format >> 8) as u8;
                    h[51] = index_format as u8;
                }
                tables.push((*b"head", h));
            }
            _ => tables.push((*tag, d.to_vec())),
        }
    }
    let refs: Vec<([u8; 4], &[u8])> = tables.iter().map(|(t, d)| (*t, d.as_slice())).collect();
    Some(build_sfnt(flavor, &refs))
}

/// Decode the transformed `glyf` table; returns (glyf, loca, bboxes,
/// indexToLocFormat, numGlyphs).
pub fn reconstruct_glyf(d: &[u8]) -> Option<(Vec<u8>, Vec<u8>, Vec<[i16; 4]>, u16, u16)> {
    let mut r = Reader { d, pos: 0 };
    let _version = r.u32()?;
    let num_glyphs = r.u16()?;
    let _index_format = r.u16()?;
    let sizes: Vec<usize> = (0..7).map(|_| r.u32().map(|v| v as usize)).collect::<Option<_>>()?;
    let mut streams: Vec<&[u8]> = Vec::with_capacity(7);
    let mut pos = r.pos;
    for s in &sizes {
        let slice = d.get(pos..pos.checked_add(*s)?)?;
        streams.push(slice);
        pos += s;
    }
    let mut n_contour = Reader { d: streams[0], pos: 0 };
    let mut n_points = Reader { d: streams[1], pos: 0 };
    let mut flags = Reader { d: streams[2], pos: 0 };
    let mut glyph_s = Reader { d: streams[3], pos: 0 };
    let mut composite = Reader { d: streams[4], pos: 0 };
    let bbox_bitmap_len = 4 * ((num_glyphs as usize + 31) / 32);
    let bbox_bitmap = streams[5].get(..bbox_bitmap_len)?;
    let mut bbox_s = Reader { d: streams[5], pos: bbox_bitmap_len };
    let mut instr = Reader { d: streams[6], pos: 0 };

    let mut glyf = Vec::new();
    let mut loca_offsets: Vec<u32> = Vec::with_capacity(num_glyphs as usize + 1);
    let mut bboxes = Vec::with_capacity(num_glyphs as usize);
    for gi in 0..num_glyphs as usize {
        loca_offsets.push(glyf.len() as u32);
        let nc = n_contour.i16()?;
        let has_bbox = bbox_bitmap[gi >> 3] & (0x80 >> (gi & 7)) != 0;
        if nc == 0 {
            bboxes.push([0, 0, 0, 0]);
            continue;
        }
        if nc > 0 {
            // Simple glyph
            let mut end_pts: Vec<u16> = Vec::with_capacity(nc as usize);
            let mut total = 0u32;
            for _ in 0..nc {
                let np = n_points.u255()? as u32;
                total += np;
                end_pts.push((total.checked_sub(1)?) as u16);
            }
            let total = total as usize;
            let mut xs: Vec<i16> = Vec::with_capacity(total);
            let mut ys: Vec<i16> = Vec::with_capacity(total);
            let mut on_curve: Vec<bool> = Vec::with_capacity(total);
            let (mut x, mut y) = (0i32, 0i32);
            for _ in 0..total {
                let f = flags.u8()?;
                let on = f & 0x80 == 0;
                let f = f & 0x7f;
                let (dx, dy) = decode_triplet(f, &mut glyph_s)?;
                x += dx;
                y += dy;
                xs.push(x as i16);
                ys.push(y as i16);
                on_curve.push(on);
            }
            let ilen = glyph_s.u255()? as usize;
            let instructions = instr.bytes(ilen)?;
            let bbox = if has_bbox {
                [bbox_s.i16()?, bbox_s.i16()?, bbox_s.i16()?, bbox_s.i16()?]
            } else {
                let xmin = xs.iter().copied().min().unwrap_or(0);
                let ymin = ys.iter().copied().min().unwrap_or(0);
                let xmax = xs.iter().copied().max().unwrap_or(0);
                let ymax = ys.iter().copied().max().unwrap_or(0);
                [xmin, ymin, xmax, ymax]
            };
            bboxes.push(bbox);
            glyf.extend_from_slice(&nc.to_be_bytes());
            for v in bbox {
                glyf.extend_from_slice(&v.to_be_bytes());
            }
            for e in &end_pts {
                glyf.extend_from_slice(&e.to_be_bytes());
            }
            glyf.extend_from_slice(&(ilen as u16).to_be_bytes());
            glyf.extend_from_slice(instructions);
            // Flags: on-curve bit only; all coordinates as int16 deltas.
            for on in &on_curve {
                glyf.push(if *on { 0x01 } else { 0x00 });
            }
            let mut px = 0i16;
            for v in &xs {
                glyf.extend_from_slice(&(v.wrapping_sub(px)).to_be_bytes());
                px = *v;
            }
            let mut py = 0i16;
            for v in &ys {
                glyf.extend_from_slice(&(v.wrapping_sub(py)).to_be_bytes());
                py = *v;
            }
        } else {
            // Composite glyph
            let bbox = if has_bbox { [bbox_s.i16()?, bbox_s.i16()?, bbox_s.i16()?, bbox_s.i16()?] } else { return None };
            bboxes.push(bbox);
            glyf.extend_from_slice(&nc.to_be_bytes());
            for v in bbox {
                glyf.extend_from_slice(&v.to_be_bytes());
            }
            let mut have_instructions = false;
            loop {
                let flags_v = composite.u16()?;
                glyf.extend_from_slice(&flags_v.to_be_bytes());
                let glyph_index = composite.u16()?;
                glyf.extend_from_slice(&glyph_index.to_be_bytes());
                let mut n = if flags_v & 0x0001 != 0 { 4 } else { 2 };
                if flags_v & 0x0008 != 0 {
                    n += 2;
                } else if flags_v & 0x0040 != 0 {
                    n += 4;
                } else if flags_v & 0x0080 != 0 {
                    n += 8;
                }
                glyf.extend_from_slice(composite.bytes(n)?);
                if flags_v & 0x0100 != 0 {
                    have_instructions = true;
                }
                if flags_v & 0x0020 == 0 {
                    break;
                }
            }
            if have_instructions {
                let ilen = glyph_s.u255()? as usize;
                glyf.extend_from_slice(&(ilen as u16).to_be_bytes());
                glyf.extend_from_slice(instr.bytes(ilen)?);
            }
        }
        while glyf.len() % 4 != 0 {
            glyf.push(0);
        }
    }
    loca_offsets.push(glyf.len() as u32);
    // Always emit the long loca format (and patch head accordingly).
    let mut loca = Vec::with_capacity(loca_offsets.len() * 4);
    for o in &loca_offsets {
        loca.extend_from_slice(&o.to_be_bytes());
    }
    Some((glyf, loca, bboxes, 1, num_glyphs))
}

/// Decode one coordinate triplet given its flag byte.
fn decode_triplet(flag: u8, r: &mut Reader) -> Option<(i32, i32)> {
    let flag = flag as i32;
    let with_sign = |f: i32, v: i32| if f & 1 != 0 { v } else { -v };
    Some(if flag < 10 {
        let b0 = r.u8()? as i32;
        (0, with_sign(flag, ((flag & 14) << 7) + b0))
    } else if flag < 20 {
        let b0 = r.u8()? as i32;
        (with_sign(flag, (((flag - 10) & 14) << 7) + b0), 0)
    } else if flag < 84 {
        let b0 = flag - 20;
        let b1 = r.u8()? as i32;
        (with_sign(flag, 1 + (b0 & 0x30) + (b1 >> 4)), with_sign(flag >> 1, 1 + ((b0 & 0x0c) << 2) + (b1 & 0x0f)))
    } else if flag < 120 {
        let b0 = flag - 84;
        let b1 = r.u8()? as i32;
        let b2 = r.u8()? as i32;
        (with_sign(flag, 1 + ((b0 / 12) << 8) + b1), with_sign(flag >> 1, 1 + (((b0 % 12) >> 2) << 8) + b2))
    } else if flag < 124 {
        let b1 = r.u8()? as i32;
        let b2 = r.u8()? as i32;
        let b3 = r.u8()? as i32;
        (with_sign(flag, (b1 << 4) + (b2 >> 4)), with_sign(flag >> 1, ((b2 & 0x0f) << 8) + b3))
    } else {
        let b1 = r.u8()? as i32;
        let b2 = r.u8()? as i32;
        let b3 = r.u8()? as i32;
        let b4 = r.u8()? as i32;
        (with_sign(flag, (b1 << 8) + b2), with_sign(flag >> 1, (b3 << 8) + b4))
    })
}

/// Rebuild `hmtx` from its transformed form.
pub fn reconstruct_hmtx(d: &[u8], num_glyphs: u16, num_hmetrics: u16, bboxes: Option<&[[i16; 4]]>) -> Option<Vec<u8>> {
    let mut r = Reader { d, pos: 0 };
    let flags = r.u8()?;
    let nh = num_hmetrics as usize;
    let ng = num_glyphs as usize;
    let mut advances = Vec::with_capacity(nh);
    for _ in 0..nh {
        advances.push(r.u16()?);
    }
    let mut lsbs: Vec<i16> = Vec::with_capacity(ng);
    if flags & 1 == 0 {
        for _ in 0..nh {
            lsbs.push(r.i16()?);
        }
    } else {
        let b = bboxes?;
        for g in b.iter().take(nh) {
            lsbs.push(g[0]);
        }
    }
    if flags & 2 == 0 {
        for _ in nh..ng {
            lsbs.push(r.i16()?);
        }
    } else {
        let b = bboxes?;
        for g in b.iter().skip(nh).take(ng - nh) {
            lsbs.push(g[0]);
        }
    }
    let mut out = Vec::with_capacity(nh * 4 + (ng - nh) * 2);
    for i in 0..nh {
        out.extend_from_slice(&advances[i].to_be_bytes());
        out.extend_from_slice(&lsbs.get(i).copied().unwrap_or(0).to_be_bytes());
    }
    for lsb in lsbs.iter().skip(nh) {
        out.extend_from_slice(&lsb.to_be_bytes());
    }
    Some(out)
}

#[cfg(test)]
pub(crate) fn read_255(d: &[u8]) -> Option<u16> {
    Reader { d, pos: 0 }.u255()
}

#[cfg(test)]
pub(crate) fn read_base128(d: &[u8]) -> Option<u32> {
    Reader { d, pos: 0 }.base128()
}
