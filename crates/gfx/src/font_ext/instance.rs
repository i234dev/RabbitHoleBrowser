//! Static instances of TrueType variable fonts. The rasterizer draws the
//! default outlines of a font, so a variable face is turned into a plain
//! `glyf` font at the axis values its `@font-face` descriptors select
//! (font services serve one variable file for every weight of a family).

use super::sfnt::{find_table, table_directory};
use rustybuzz::ttf_parser::{self, GlyphId, OutlineBuilder, Tag};

/// Tables whose data no longer applies once the outlines are instanced:
/// variations, hinting programs and per-size metric caches.
const DROPPED: [&[u8; 4]; 14] = [b"fvar", b"gvar", b"avar", b"HVAR", b"VVAR", b"MVAR", b"cvar", b"STAT", b"hdmx", b"LTSH", b"VDMX", b"fpgm", b"prep", b"cvt "];

/// Outline points of one glyph: contours of (x, y, on-curve).
#[derive(Default)]
struct Contours {
    contours: Vec<Vec<(f32, f32, bool)>>,
    current: Vec<(f32, f32, bool)>,
    cubic: bool,
}

impl Contours {
    fn finish(&mut self) {
        let mut c = std::mem::take(&mut self.current);
        // The closing segment back to the start point is implicit.
        if c.len() > 1 {
            let (first, last) = (c[0], c[c.len() - 1]);
            if last.2 && first.0 == last.0 && first.1 == last.1 {
                c.pop();
            }
        }
        if !c.is_empty() {
            self.contours.push(c);
        }
    }
}

impl OutlineBuilder for Contours {
    fn move_to(&mut self, x: f32, y: f32) {
        self.finish();
        self.current.push((x, y, true));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.current.push((x, y, true));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.current.push((x1, y1, false));
        self.current.push((x, y, true));
    }
    fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {
        self.cubic = true;
    }
    fn close(&mut self) {
        self.finish();
    }
}

/// A simple `glyf` glyph from contours (no instructions), with its bounding
/// box (xMin, yMin, xMax, yMax). Empty glyphs have no data.
pub(crate) fn encode_glyph(contours: &[Vec<(i16, i16, bool)>]) -> (Vec<u8>, [i16; 4]) {
    let points: Vec<&(i16, i16, bool)> = contours.iter().flatten().collect();
    if points.is_empty() {
        return (Vec::new(), [0; 4]);
    }
    let mut bbox = [i16::MAX, i16::MAX, i16::MIN, i16::MIN];
    for &&(x, y, _) in &points {
        bbox = [bbox[0].min(x), bbox[1].min(y), bbox[2].max(x), bbox[3].max(y)];
    }
    let mut out = Vec::with_capacity(12 + points.len() * 5);
    out.extend_from_slice(&(contours.len() as i16).to_be_bytes());
    for v in bbox {
        out.extend_from_slice(&v.to_be_bytes());
    }
    let mut end = 0usize;
    for c in contours {
        end += c.len();
        out.extend_from_slice(&((end - 1) as u16).to_be_bytes());
    }
    out.extend_from_slice(&0u16.to_be_bytes());
    for &&(_, _, on) in &points {
        out.push(on as u8);
    }
    for axis in 0..2 {
        let mut prev = 0i32;
        for &&(x, y, _) in &points {
            let v = if axis == 0 { x } else { y } as i32;
            out.extend_from_slice(&((v - prev) as i16).to_be_bytes());
            prev = v;
        }
    }
    while out.len() % 4 != 0 {
        out.push(0);
    }
    (out, bbox)
}

/// A static TrueType font from variable font `sfnt` with `axes` set (tag,
/// user-space value; axes the font lacks are ignored). `None` when the font
/// is not a variable `glyf` font, or when no given axis applies.
pub fn instantiate(sfnt: &[u8], axes: &[([u8; 4], f32)]) -> Option<Vec<u8>> {
    let dir = table_directory(sfnt, 0)?;
    find_table(&dir, b"fvar")?;
    find_table(&dir, b"glyf")?;
    let mut face = ttf_parser::Face::parse(sfnt, 0).ok()?;
    let mut applied = false;
    let available: Vec<Tag> = face.variation_axes().into_iter().map(|a| a.tag).collect();
    for (tag, value) in axes {
        let tag = Tag::from_bytes(tag);
        if available.contains(&tag) {
            applied |= face.set_variation(tag, *value).is_some();
        }
    }
    if !applied {
        return None;
    }
    let num_glyphs = face.number_of_glyphs();
    let mut glyf = Vec::new();
    let mut loca: Vec<u8> = Vec::with_capacity((num_glyphs as usize + 1) * 4);
    let mut hmtx = Vec::with_capacity(num_glyphs as usize * 4);
    let (mut max_points, mut max_contours, mut max_advance) = (0usize, 0usize, 0u16);
    for gid in 0..num_glyphs {
        let mut b = Contours::default();
        face.outline_glyph(GlyphId(gid), &mut b);
        b.finish();
        if b.cubic {
            return None;
        }
        let rounded: Vec<Vec<(i16, i16, bool)>> = b
            .contours
            .iter()
            .map(|c| c.iter().map(|&(x, y, on)| (x.round().clamp(-32768.0, 32767.0) as i16, y.round().clamp(-32768.0, 32767.0) as i16, on)).collect())
            .collect();
        max_points = max_points.max(rounded.iter().map(Vec::len).sum());
        max_contours = max_contours.max(rounded.len());
        let (data, bbox) = encode_glyph(&rounded);
        loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
        glyf.extend_from_slice(&data);
        let advance = face.glyph_hor_advance(GlyphId(gid)).unwrap_or(0);
        max_advance = max_advance.max(advance);
        hmtx.extend_from_slice(&advance.to_be_bytes());
        hmtx.extend_from_slice(&bbox[0].to_be_bytes());
    }
    loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());

    let table = |tag: &[u8; 4]| -> Option<Vec<u8>> {
        let t = find_table(&dir, tag)?;
        sfnt.get(t.offset as usize..(t.offset as usize).checked_add(t.length as usize)?).map(<[u8]>::to_vec)
    };
    let mut head = table(b"head")?;
    let mut hhea = table(b"hhea")?;
    let mut maxp = table(b"maxp")?;
    if head.len() < 54 || hhea.len() < 36 || maxp.len() < 6 {
        return None;
    }
    head[50..52].copy_from_slice(&1i16.to_be_bytes());
    hhea[10..12].copy_from_slice(&max_advance.to_be_bytes());
    hhea[34..36].copy_from_slice(&num_glyphs.to_be_bytes());
    if maxp.len() >= 32 {
        maxp[6..8].copy_from_slice(&(max_points.min(u16::MAX as usize) as u16).to_be_bytes());
        maxp[8..10].copy_from_slice(&(max_contours.min(u16::MAX as usize) as u16).to_be_bytes());
        // No composite glyphs or instructions remain.
        for o in [10, 12, 26, 28, 30] {
            maxp[o..o + 2].copy_from_slice(&0u16.to_be_bytes());
        }
    }
    let mut os2 = table(b"OS/2");
    if let (Some(os2), Some(w)) = (os2.as_mut(), axes.iter().find(|a| &a.0 == b"wght")) {
        if os2.len() >= 6 {
            os2[4..6].copy_from_slice(&(w.1.round().clamp(1.0, 1000.0) as u16).to_be_bytes());
        }
    }
    let replaced: [(&[u8; 4], &Vec<u8>); 5] = [(b"glyf", &glyf), (b"loca", &loca), (b"hmtx", &hmtx), (b"head", &head), (b"hhea", &hhea)];
    let mut tables: Vec<([u8; 4], Vec<u8>)> = Vec::with_capacity(dir.len());
    for t in &dir {
        if DROPPED.contains(&&t.tag) {
            continue;
        }
        let data = if let Some((_, d)) = replaced.iter().find(|(tag, _)| **tag == t.tag) {
            (*d).clone()
        } else if &t.tag == b"maxp" {
            maxp.clone()
        } else if &t.tag == b"OS/2" {
            os2.clone()?
        } else {
            table(&t.tag)?
        };
        tables.push((t.tag, data));
    }
    let refs: Vec<([u8; 4], &[u8])> = tables.iter().map(|(t, d)| (*t, d.as_slice())).collect();
    Some(super::sfnt::build_sfnt(0x00010000, &refs))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-glyph variable font: `I` is a 100-unit wide bar with a 300-unit
    /// advance at wght 400; at wght 900 its right edge and advance grow by
    /// 200 units.
    pub(crate) fn variable_bar_font() -> Vec<u8> {
        let mut head = vec![0u8; 54];
        head[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        head[12..16].copy_from_slice(&0x5F0F_3CF5u32.to_be_bytes());
        head[18..20].copy_from_slice(&1000u16.to_be_bytes());
        head[40..42].copy_from_slice(&300i16.to_be_bytes());
        head[42..44].copy_from_slice(&700i16.to_be_bytes());
        head[50..52].copy_from_slice(&1i16.to_be_bytes());
        let mut hhea = vec![0u8; 36];
        hhea[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        hhea[4..6].copy_from_slice(&800i16.to_be_bytes());
        hhea[6..8].copy_from_slice(&(-200i16).to_be_bytes());
        hhea[10..12].copy_from_slice(&300u16.to_be_bytes());
        hhea[18..20].copy_from_slice(&1i16.to_be_bytes());
        hhea[34..36].copy_from_slice(&2u16.to_be_bytes());
        let mut maxp = vec![0u8; 32];
        maxp[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        maxp[4..6].copy_from_slice(&2u16.to_be_bytes());
        maxp[6..8].copy_from_slice(&4u16.to_be_bytes());
        maxp[8..10].copy_from_slice(&1u16.to_be_bytes());
        maxp[14..16].copy_from_slice(&2u16.to_be_bytes());
        let (bar, _) = encode_glyph(&[vec![(100, 0, true), (100, 700, true), (200, 700, true), (200, 0, true)]]);
        let glyf = bar.clone();
        let mut loca = Vec::new();
        for o in [0u32, 0, bar.len() as u32] {
            loca.extend_from_slice(&o.to_be_bytes());
        }
        let mut hmtx = Vec::new();
        for (adv, lsb) in [(300u16, 0i16), (300, 100)] {
            hmtx.extend_from_slice(&adv.to_be_bytes());
            hmtx.extend_from_slice(&lsb.to_be_bytes());
        }
        // cmap format 12: 'I' -> glyph 1.
        let mut cmap = vec![0, 0, 0, 1, 0, 3, 0, 10, 0, 0, 0, 12];
        cmap.extend_from_slice(&12u16.to_be_bytes());
        cmap.extend_from_slice(&0u16.to_be_bytes());
        for n in [28u32, 0, 1, 'I' as u32, 'I' as u32, 1] {
            cmap.extend_from_slice(&n.to_be_bytes());
        }
        // fvar: one axis, wght 100..900, default 400.
        let mut fvar = Vec::new();
        for v in [1u16, 0, 16, 2, 1, 20, 0, 8] {
            fvar.extend_from_slice(&v.to_be_bytes());
        }
        fvar.extend_from_slice(b"wght");
        for v in [100u32, 400, 900] {
            fvar.extend_from_slice(&(v << 16).to_be_bytes());
        }
        fvar.extend_from_slice(&0u16.to_be_bytes());
        fvar.extend_from_slice(&256u16.to_be_bytes());
        // gvar: glyph 1 has one tuple peaking at wght max: points 2 and 3
        // and the right phantom point move 200 units right.
        let mut data = Vec::new();
        data.push(0); // all points
        data.push(0x40 | 7); // 8 x deltas as words
        for d in [0i16, 0, 200, 200, 0, 200, 0, 0] {
            data.extend_from_slice(&d.to_be_bytes());
        }
        data.push(0x80 | 7); // 8 zero y deltas
        let mut var = Vec::new();
        var.extend_from_slice(&1u16.to_be_bytes()); // one tuple
        var.extend_from_slice(&10u16.to_be_bytes()); // data offset
        var.extend_from_slice(&(data.len() as u16).to_be_bytes());
        var.extend_from_slice(&(0x8000u16 | 0x2000).to_be_bytes()); // embedded peak, private points
        var.extend_from_slice(&0x4000u16.to_be_bytes()); // peak 1.0
        var.extend_from_slice(&data);
        while var.len() % 2 != 0 {
            var.push(0);
        }
        let mut gvar = Vec::new();
        for v in [1u16, 0, 1, 0] {
            gvar.extend_from_slice(&v.to_be_bytes());
        }
        gvar.extend_from_slice(&32u32.to_be_bytes()); // shared tuples
        gvar.extend_from_slice(&2u16.to_be_bytes()); // glyph count
        gvar.extend_from_slice(&1u16.to_be_bytes()); // long offsets
        gvar.extend_from_slice(&32u32.to_be_bytes()); // data array
        for o in [0u32, 0, var.len() as u32] {
            gvar.extend_from_slice(&o.to_be_bytes());
        }
        gvar.extend_from_slice(&var);
        let tables: Vec<([u8; 4], &[u8])> = vec![
            (*b"head", &head),
            (*b"hhea", &hhea),
            (*b"maxp", &maxp),
            (*b"hmtx", &hmtx),
            (*b"cmap", &cmap),
            (*b"glyf", &glyf),
            (*b"loca", &loca),
            (*b"fvar", &fvar),
            (*b"gvar", &gvar),
        ];
        super::super::sfnt::build_sfnt(0x00010000, &tables)
    }

    fn bar_metrics(sfnt: &[u8]) -> (u16, i16) {
        let face = ttf_parser::Face::parse(sfnt, 0).unwrap();
        let g = face.glyph_index('I').unwrap();
        (face.glyph_hor_advance(g).unwrap(), face.glyph_bounding_box(g).unwrap().x_max)
    }

    #[test]
    fn instances_apply_glyph_and_advance_variations() {
        let font = variable_bar_font();
        assert_eq!(bar_metrics(&font), (300, 200));
        let bold = instantiate(&font, &[(*b"wght", 900.0)]).unwrap();
        assert_eq!(bar_metrics(&bold), (500, 400));
        // 700 is 60% of the way from the default to the maximum.
        let semi = instantiate(&font, &[(*b"wght", 700.0)]).unwrap();
        assert_eq!(bar_metrics(&semi), (420, 320));
        let face = ttf_parser::Face::parse(&semi, 0).unwrap();
        assert!(!face.is_variable());
        // Axes the font lacks, and static fonts, are left alone.
        assert!(instantiate(&font, &[(*b"wdth", 75.0)]).is_none());
        let sans = include_bytes!("../../../../assets/fonts/DejaVuSans.ttf");
        assert!(instantiate(sans, &[(*b"wght", 700.0)]).is_none());
    }
}

#[cfg(test)]
pub(crate) use tests::variable_bar_font;
