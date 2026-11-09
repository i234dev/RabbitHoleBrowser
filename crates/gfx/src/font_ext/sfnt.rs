//! Minimal SFNT (TrueType / OpenType) container parsing: table directory,
//! `name`, `OS/2` and `head` tables, and (re)assembly of table sets.

#[derive(Debug, Clone, PartialEq)]
pub struct TableRecord {
    pub tag: [u8; 4],
    pub offset: u32,
    pub length: u32,
}

/// Face descriptors parsed from the font headers.
#[derive(Debug, Clone, PartialEq)]
pub struct SfntInfo {
    pub family: String,
    pub subfamily: String,
    pub full_name: String,
    pub weight: u16,
    pub italic: bool,
}

pub fn rd16(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*d.get(o)?, *d.get(o + 1)?]))
}

pub fn rd32(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes([*d.get(o)?, *d.get(o + 1)?, *d.get(o + 2)?, *d.get(o + 3)?]))
}

/// Offset of the SFNT header for face `index` (0 for single fonts).
pub fn face_offset(data: &[u8], index: u32) -> Option<usize> {
    if data.len() >= 4 && &data[0..4] == b"ttcf" {
        let n = rd32(data, 8)?;
        if index >= n {
            return None;
        }
        Some(rd32(data, 12 + 4 * index as usize)? as usize)
    } else {
        Some(0)
    }
}

/// Table directory of the face at `base`.
pub fn table_directory(data: &[u8], base: usize) -> Option<Vec<TableRecord>> {
    let tag = rd32(data, base)?;
    if !matches!(tag, 0x00010000 | 0x4F54544F | 0x74727565) {
        return None;
    }
    let n = rd16(data, base + 4)? as usize;
    if n == 0 || n > 512 {
        return None;
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let o = base + 12 + i * 16;
        let tag = [*data.get(o)?, *data.get(o + 1)?, *data.get(o + 2)?, *data.get(o + 3)?];
        let offset = rd32(data, o + 8)?;
        let length = rd32(data, o + 12)?;
        out.push(TableRecord { tag, offset, length });
    }
    Some(out)
}

pub fn find_table<'a>(dir: &'a [TableRecord], tag: &[u8; 4]) -> Option<&'a TableRecord> {
    dir.iter().find(|t| &t.tag == tag)
}

/// Extract face `index` of a collection (or the single font) as a
/// standalone SFNT byte stream.
pub fn extract_face(data: &[u8], index: u32) -> Option<Vec<u8>> {
    let base = face_offset(data, index)?;
    if base == 0 && !(data.len() >= 4 && &data[0..4] == b"ttcf") {
        return Some(data.to_vec());
    }
    let dir = table_directory(data, base)?;
    let flavor = rd32(data, base)?;
    let tables: Vec<([u8; 4], &[u8])> = dir
        .iter()
        .filter_map(|t| {
            let start = t.offset as usize;
            let end = start.checked_add(t.length as usize)?;
            data.get(start..end).map(|d| (t.tag, d))
        })
        .collect();
    Some(build_sfnt(flavor, &tables))
}

/// Assemble tables into an SFNT file (directory sorted by tag, 4-byte
/// aligned data, checksums computed).
pub fn build_sfnt(flavor: u32, tables: &[([u8; 4], &[u8])]) -> Vec<u8> {
    let mut sorted: Vec<&([u8; 4], &[u8])> = tables.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let n = sorted.len() as u16;
    let mut entry_selector = 0u16;
    while (2u16 << entry_selector) <= n && entry_selector < 15 {
        entry_selector += 1;
    }
    let search_range = (1u16 << entry_selector) * 16;
    let range_shift = n.saturating_mul(16).saturating_sub(search_range);
    let mut out = Vec::new();
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&n.to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&range_shift.to_be_bytes());
    let dir_len = 12 + 16 * sorted.len();
    let mut offset = dir_len;
    let mut entries = Vec::with_capacity(sorted.len());
    for (tag, d) in &sorted {
        let aligned_len = (d.len() + 3) & !3;
        entries.push((*tag, checksum(d), offset as u32, d.len() as u32));
        offset += aligned_len;
    }
    for (tag, sum, off, len) in &entries {
        out.extend_from_slice(tag);
        out.extend_from_slice(&sum.to_be_bytes());
        out.extend_from_slice(&off.to_be_bytes());
        out.extend_from_slice(&len.to_be_bytes());
    }
    for (_, d) in &sorted {
        out.extend_from_slice(d);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    out
}

fn checksum(d: &[u8]) -> u32 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i < d.len() {
        let mut w = [0u8; 4];
        for (k, b) in w.iter_mut().enumerate() {
            *b = *d.get(i + k).unwrap_or(&0);
        }
        sum = sum.wrapping_add(u32::from_be_bytes(w));
        i += 4;
    }
    sum
}

/// Parse family/style information from the face at `base` in `data`.
/// `fetch` must return the bytes for an (offset, length) range (so callers
/// can read lazily from a file).
pub fn parse_info_with(dir: &[TableRecord], mut fetch: impl FnMut(u32, u32) -> Option<Vec<u8>>) -> Option<SfntInfo> {
    let name_rec = find_table(dir, b"name")?;
    let name = fetch(name_rec.offset, name_rec.length)?;
    let names = parse_name_table(&name);
    let family = names.get(&16).or_else(|| names.get(&1)).cloned()?;
    let subfamily = names.get(&17).or_else(|| names.get(&2)).cloned().unwrap_or_default();
    let full_name = names.get(&4).cloned().unwrap_or_else(|| format!("{family} {subfamily}").trim().to_string());
    let mut weight = 400u16;
    let mut italic = false;
    if let Some(os2) = find_table(dir, b"OS/2") {
        if let Some(d) = fetch(os2.offset, os2.length.min(96)) {
            if let Some(w) = rd16(&d, 4) {
                if (1..=1000).contains(&w) {
                    weight = w;
                }
            }
            if let Some(fs) = rd16(&d, 62) {
                italic = fs & 0x01 != 0 || fs & 0x100 != 0;
                if fs & 0x20 != 0 && weight == 400 {
                    weight = 700;
                }
            }
        }
    }
    if let Some(head) = find_table(dir, b"head") {
        if let Some(d) = fetch(head.offset, head.length.min(54)) {
            if let Some(mac) = rd16(&d, 44) {
                if mac & 0x02 != 0 {
                    italic = true;
                }
                if mac & 0x01 != 0 && weight < 600 {
                    weight = 700;
                }
            }
        }
    }
    let sub = subfamily.to_ascii_lowercase();
    if sub.contains("italic") || sub.contains("oblique") {
        italic = true;
    }
    if weight == 400 {
        if sub.contains("black") || sub.contains("heavy") {
            weight = 900;
        } else if sub.contains("extrabold") || sub.contains("ultrabold") {
            weight = 800;
        } else if sub.contains("semibold") || sub.contains("demibold") {
            weight = 600;
        } else if sub.contains("bold") {
            weight = 700;
        } else if sub.contains("medium") {
            weight = 500;
        } else if sub.contains("extralight") || sub.contains("ultralight") {
            weight = 200;
        } else if sub.contains("light") {
            weight = 300;
        } else if sub.contains("thin") || sub.contains("hairline") {
            weight = 100;
        }
    }
    Some(SfntInfo { family, subfamily, full_name, weight, italic })
}

/// Parse an in-memory face's information.
pub fn parse_info(data: &[u8], index: u32) -> Option<SfntInfo> {
    let base = face_offset(data, index)?;
    let dir = table_directory(data, base)?;
    parse_info_with(&dir, |off, len| data.get(off as usize..(off as usize).checked_add(len as usize)?).map(|d| d.to_vec()))
}

/// Decode the `name` table into nameID → string (preferring Windows
/// Unicode English, then any Windows/Unicode record, then Mac Roman).
pub fn parse_name_table(d: &[u8]) -> std::collections::HashMap<u16, String> {
    let mut out: std::collections::HashMap<u16, (u8, String)> = std::collections::HashMap::new();
    let Some(count) = rd16(d, 2) else { return Default::default() };
    let Some(string_off) = rd16(d, 4) else { return Default::default() };
    for i in 0..count as usize {
        let o = 6 + i * 12;
        let (Some(platform), Some(encoding), Some(language), Some(name_id), Some(len), Some(off)) =
            (rd16(d, o), rd16(d, o + 2), rd16(d, o + 4), rd16(d, o + 6), rd16(d, o + 8), rd16(d, o + 10))
        else {
            break;
        };
        let start = string_off as usize + off as usize;
        let Some(bytes) = d.get(start..start + len as usize) else { continue };
        let (priority, text) = match (platform, encoding) {
            (3, 1) | (3, 10) | (0, _) => {
                let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                let p = if platform == 3 && language == 0x409 { 3 } else { 2 };
                (p, String::from_utf16_lossy(&units))
            }
            (1, 0) => (1, bytes.iter().map(|&b| mac_roman(b)).collect()),
            _ => continue,
        };
        let text = text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        match out.get(&name_id) {
            Some((p, _)) if *p >= priority => {}
            _ => {
                out.insert(name_id, (priority, text));
            }
        }
    }
    out.into_iter().map(|(k, (_, v))| (k, v)).collect()
}

fn mac_roman(b: u8) -> char {
    if b < 0x80 {
        return b as char;
    }
    const HI: [char; 128] = [
        'Ä', 'Å', 'Ç', 'É', 'Ñ', 'Ö', 'Ü', 'á', 'à', 'â', 'ä', 'ã', 'å', 'ç', 'é', 'è', 'ê', 'ë', 'í', 'ì', 'î', 'ï', 'ñ', 'ó', 'ò', 'ô', 'ö', 'õ', 'ú', 'ù', 'û', 'ü',
        '†', '°', '¢', '£', '§', '•', '¶', 'ß', '®', '©', '™', '´', '¨', '≠', 'Æ', 'Ø', '∞', '±', '≤', '≥', '¥', 'µ', '∂', '∑', '∏', 'π', '∫', 'ª', 'º', 'Ω', 'æ', 'ø',
        '¿', '¡', '¬', '√', 'ƒ', '≈', '∆', '«', '»', '…', '\u{a0}', 'À', 'Ã', 'Õ', 'Œ', 'œ', '–', '—', '“', '”', '‘', '’', '÷', '◊', 'ÿ', 'Ÿ', '⁄', '€', '‹', '›', 'ﬁ', 'ﬂ',
        '‡', '·', '‚', '„', '‰', 'Â', 'Ê', 'Á', 'Ë', 'È', 'Í', 'Î', 'Ï', 'Ì', 'Ó', 'Ô', '\u{f8ff}', 'Ò', 'Ú', 'Û', 'Ù', 'ı', 'ˆ', '˜', '¯', '˘', '˙', '˚', '¸', '˝', '˛', 'ˇ',
    ];
    HI[(b - 0x80) as usize]
}
