//! WOFF 1.0 decoding (zlib-compressed tables) into a plain SFNT stream.

use super::sfnt::{build_sfnt, rd16, rd32};

pub fn decode(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 44 || &data[0..4] != b"wOFF" {
        return None;
    }
    let flavor = rd32(data, 4)?;
    let num_tables = rd16(data, 12)? as usize;
    if num_tables == 0 || num_tables > 512 {
        return None;
    }
    let mut tables: Vec<([u8; 4], Vec<u8>)> = Vec::with_capacity(num_tables);
    for i in 0..num_tables {
        let o = 44 + i * 20;
        let tag = [*data.get(o)?, *data.get(o + 1)?, *data.get(o + 2)?, *data.get(o + 3)?];
        let offset = rd32(data, o + 4)? as usize;
        let comp_len = rd32(data, o + 8)? as usize;
        let orig_len = rd32(data, o + 12)? as usize;
        let comp = data.get(offset..offset.checked_add(comp_len)?)?;
        let table = if comp_len < orig_len {
            let out = miniz_oxide::inflate::decompress_to_vec_zlib(comp).ok()?;
            if out.len() != orig_len {
                return None;
            }
            out
        } else {
            comp.to_vec()
        };
        tables.push((tag, table));
    }
    let refs: Vec<([u8; 4], &[u8])> = tables.iter().map(|(t, d)| (*t, d.as_slice())).collect();
    Some(build_sfnt(flavor, &refs))
}
