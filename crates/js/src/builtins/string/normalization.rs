//! Unicode normalization over ECMAScript UTF-16 strings (UAX #15).
//!
//! Decompose recursively, reorder combining marks stably, then compose without
//! crossing a blocked mark. Unpaired surrogates remain code units and act as
//! starters; no lossy UTF-8 conversion is used.

#[path = "normalization_data.rs"]
mod data;

const S_BASE: u32 = 0xAC00;
const L_BASE: u32 = 0x1100;
const V_BASE: u32 = 0x1161;
const T_BASE: u32 = 0x11A7;
const L_COUNT: u32 = 19;
const V_COUNT: u32 = 21;
const T_COUNT: u32 = 28;
const N_COUNT: u32 = V_COUNT * T_COUNT;
const S_COUNT: u32 = L_COUNT * N_COUNT;

fn combining_class(cp: u32) -> u8 {
    if cp < 0x300 { return 0; }
    let i = data::COMBINING.partition_point(|&(_, end, _)| end < cp);
    match data::COMBINING.get(i) {
        Some(&(start, _, class)) if start <= cp => class,
        _ => 0,
    }
}

fn decompose(cp: u32, compatibility: bool, out: &mut Vec<u32>) {
    if (S_BASE..S_BASE + S_COUNT).contains(&cp) {
        let s = cp - S_BASE;
        out.push(L_BASE + s / N_COUNT);
        out.push(V_BASE + (s % N_COUNT) / T_COUNT);
        let tail = s % T_COUNT;
        if tail != 0 { out.push(T_BASE + tail); }
        return;
    }
    if let Ok(i) = data::DECOMPOSITIONS.binary_search_by_key(&cp, |entry| entry.0) {
        let (_, offset, len, compat) = data::DECOMPOSITIONS[i];
        if !compat || compatibility {
            for &part in &data::DECOMPOSED[offset as usize..offset as usize + len as usize] {
                decompose(part, compatibility, out);
            }
            return;
        }
    }
    out.push(cp);
}

fn compose_pair(starter: u32, next: u32) -> Option<u32> {
    if (L_BASE..L_BASE + L_COUNT).contains(&starter) && (V_BASE..V_BASE + V_COUNT).contains(&next) {
        return Some(S_BASE + ((starter - L_BASE) * V_COUNT + next - V_BASE) * T_COUNT);
    }
    if (S_BASE..S_BASE + S_COUNT).contains(&starter) && (starter - S_BASE) % T_COUNT == 0
        && (T_BASE + 1..T_BASE + T_COUNT).contains(&next) {
        return Some(starter + next - T_BASE);
    }
    let key = ((starter as u64) << 21) | next as u64;
    data::COMPOSITIONS.binary_search_by_key(&key, |entry| entry.0).ok().map(|i| data::COMPOSITIONS[i].1)
}

pub(super) fn normalize(units: &[u16], form: &str) -> Vec<u16> {
    let compatibility = matches!(form, "NFKC" | "NFKD");
    let composition = matches!(form, "NFC" | "NFKC");
    let mut decomposed = Vec::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        let first = units[i] as u32;
        let cp = if (0xD800..=0xDBFF).contains(&first)
            && units.get(i + 1).is_some_and(|&u| (0xDC00..=0xDFFF).contains(&u)) {
            i += 1;
            0x10000 + ((first - 0xD800) << 10) + units[i] as u32 - 0xDC00
        } else { first };
        decompose(cp, compatibility, &mut decomposed);
        i += 1;
    }
    // Each starter is a reordering barrier, as is every isolated surrogate.
    // Stable sorting keeps equal-class marks in their original order and
    // avoids quadratic insertion on adversarial runs of combining characters.
    let mut start = 0;
    for i in 0..decomposed.len() {
        if combining_class(decomposed[i]) == 0 {
            decomposed[start..i].sort_by_key(|&cp| combining_class(cp));
            start = i + 1;
        }
    }
    decomposed[start..].sort_by_key(|&cp| combining_class(cp));
    let code_points = if composition {
        let mut composed = Vec::with_capacity(decomposed.len());
        let mut starter: Option<usize> = None;
        let mut previous_class = 0;
        for cp in decomposed {
            let class = combining_class(cp);
            let pair = starter.filter(|_| previous_class == 0 || previous_class < class)
                .and_then(|index| compose_pair(composed[index], cp).map(|result| (index, result)));
            if let Some((index, result)) = pair {
                composed[index] = result;
                // The consumed mark cannot block another composition.
            } else {
                if class == 0 { starter = Some(composed.len()); }
                composed.push(cp);
                previous_class = class;
            }
        }
        composed
    } else { decomposed };
    let mut out = Vec::with_capacity(units.len());
    for cp in code_points {
        if cp <= 0xFFFF {
            out.push(cp as u16);
        } else {
            out.push(0xD800 + ((cp - 0x10000) >> 10) as u16);
            out.push(0xDC00 + ((cp - 0x10000) & 0x3FF) as u16);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::normalize;

    fn text(input: &str, form: &str) -> String {
        String::from_utf16(&normalize(&input.encode_utf16().collect::<Vec<_>>(), form)).unwrap()
    }

    #[test]
    fn composition_exclusions_reordering_and_compatibility() {
        assert_eq!(text("\u{1E9B}\u{0323}", "NFD"), "\u{017F}\u{0323}\u{0307}");
        assert_eq!(text("\u{1E9B}\u{0323}", "NFKC"), "\u{1E69}");
        assert_eq!(text("\u{1E9B}\u{0323}", "NFKD"), "s\u{0323}\u{0307}");
        assert_eq!(text("\u{0958}\u{0344}\u{2126}", "NFC"), "\u{0915}\u{093C}\u{0308}\u{0301}\u{03A9}");
        assert_eq!(text("\u{0315}\u{0300}A", "NFD"), "\u{0300}\u{0315}A");
        assert_eq!(text("A\u{0305}\u{0301}", "NFC"), "A\u{0305}\u{0301}");
        assert_eq!(text("A\u{030A}\u{0301}", "NFC"), "\u{01FA}");
        assert_eq!(text("\u{FB03}\u{FF21}", "NFKC"), "ffiA");
    }

    #[test]
    fn hangul_supplementary_and_unpaired_surrogates() {
        assert_eq!(text("\u{1100}\u{1161}\u{11A8}", "NFC"), "\u{AC01}");
        assert_eq!(text("\u{AC01}", "NFD"), "\u{1100}\u{1161}\u{11A8}");
        assert_eq!(text("\u{1D15E}", "NFC"), "\u{1D157}\u{1D165}");
        let units = [0xD800, 0x0041, 0x030A, 0xDC00, 0x0301, 0xD83D, 0xDE00];
        assert_eq!(normalize(&units, "NFC"), [0xD800, 0x00C5, 0xDC00, 0x0301, 0xD83D, 0xDE00]);
    }

    #[test]
    #[ignore = "external Unicode 17 conformance corpus; set NORMALIZATION_TEST_FILE"]
    fn unicode_normalization_conformance() {
        let file = std::env::var("NORMALIZATION_TEST_FILE").expect("NORMALIZATION_TEST_FILE");
        let source = std::fs::read_to_string(file).unwrap();
        assert!(source.starts_with("# NormalizationTest-17.0.0.txt"));
        let mut rows = 0;
        for (line_index, line) in source.lines().enumerate() {
            let line = line.split('#').next().unwrap().trim();
            if line.is_empty() || line.starts_with('@') { continue; }
            let columns: Vec<Vec<u16>> = line.split(';').take(5).map(|col| {
                col.split_whitespace().flat_map(|cp| {
                    let cp = u32::from_str_radix(cp, 16).unwrap();
                    let mut encoded = [0; 2];
                    char::from_u32(cp).unwrap().encode_utf16(&mut encoded).to_vec()
                }).collect()
            }).collect();
            assert_eq!(columns.len(), 5);
            for i in 0..5 {
                for (form, expected) in [("NFC", if i < 3 { 1 } else { 3 }), ("NFD", if i < 3 { 2 } else { 4 }), ("NFKC", 3), ("NFKD", 4)] {
                    assert_eq!(normalize(&columns[i], form), columns[expected], "line {} column {} form {}", line_index + 1, i + 1, form);
                }
            }
            rows += 1;
        }
        assert_eq!(rows, 20034, "the pinned Unicode 17 corpus must be complete");
    }
}
