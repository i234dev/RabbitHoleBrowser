//! The document selection over laid out text: which characters of each
//! text fragment come from which offsets of its DOM text node, and which
//! part of each fragment is selected.
//!
//! Nothing here runs unless something is selected (or the user agent maps
//! a point to a DOM position), so unselected pages pay nothing for it.

use crate::tree::{Fragment, FragmentKind, LayoutBox};
use rabbithole_dom::{Document, NodeId};
use rabbithole_style::highlight::{PaintHighlights, SelectionStyles};
use rabbithole_style::{StyledTree, UserSelect};
use std::collections::HashMap;

/// Combining marks (the common blocks).
fn is_mark(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c' | '\u{a0}')
}

/// Text fragments of the text nodes accepted by `want`, per node, in
/// tree (and line) order.
fn collect<'a>(b: &'a LayoutBox, want: &dyn Fn(NodeId) -> bool, out: &mut HashMap<NodeId, Vec<&'a Fragment>>) {
    for line in &b.lines {
        for f in &line.fragments {
            if let FragmentKind::Text { src, .. } = &f.kind {
                if let Some(n) = crate::tree::text_src(*src).filter(|&n| want(n)) {
                    out.entry(n).or_default().push(f);
                }
            }
        }
    }
    for c in &b.children {
        collect(c, want, out);
    }
}

/// Align the texts shown for one text node with its data: for each
/// fragment, the UTF-16 offset in `data` of every character boundary of
/// its text (one more entry than characters). Collapsed white space,
/// soft hyphens and inserted hyphens are accounted for.
pub fn align_fragments(data: &str, shown: &[&str]) -> Vec<Vec<u32>> {
    let src: Vec<char> = data.chars().collect();
    let mut u16_at = Vec::with_capacity(src.len() + 1);
    let mut acc = 0u32;
    for c in &src {
        u16_at.push(acc);
        acc += c.len_utf16() as u32;
    }
    u16_at.push(acc);
    let mut pos = 0usize;
    let mut out = Vec::with_capacity(shown.len());
    for text in shown {
        let chars: Vec<char> = text.chars().collect();
        let mut offs = Vec::with_capacity(chars.len() + 1);
        let mut next = 0;
        while next < chars.len() {
            let i = next;
            let c = chars[i];
            next += 1;
            if is_space(c) {
                if pos < src.len() && is_space(src[pos]) {
                    offs.push(u16_at[pos]);
                    pos += 1;
                    // A collapsed run shows as one space.
                    let next_is_space = chars.get(i + 1).is_some_and(|&n| is_space(n));
                    if !next_is_space {
                        while pos < src.len() && is_space(src[pos]) && src[pos] != '\u{a0}' {
                            pos += 1;
                        }
                    }
                } else {
                    offs.push(u16_at[pos.min(src.len())]);
                }
                continue;
            }
            // Collapsed spaces, soft hyphens and combining marks a
            // transform removed are not shown.
            while pos < src.len() && (is_space(src[pos]) || src[pos] == '\u{ad}' || (is_mark(src[pos]) && !is_mark(c) && src[pos] != c)) && !(src[pos] == '\u{ad}' && c == '-') {
                pos += 1;
            }
            let inserted_hyphen = matches!(c, '-' | '\u{2010}') && i + 1 == chars.len() && src.get(pos).is_none_or(|&s| s != c && s != '\u{ad}');
            offs.push(u16_at[pos.min(src.len())]);
            // A character that text-transform expands (ß -> SS): its
            // extra characters map to it too.
            if let Some(&sc) = src.get(pos).filter(|&&sc| sc != c) {
                for exp in [sc.to_uppercase().collect::<Vec<char>>(), sc.to_lowercase().collect::<Vec<char>>()] {
                    if exp.len() > 1 && chars[i..].starts_with(&exp) {
                        for _ in 1..exp.len() {
                            offs.push(u16_at[pos]);
                        }
                        next += exp.len() - 1;
                        break;
                    }
                }
            }
            if !inserted_hyphen && pos < src.len() {
                pos += 1;
            }
        }
        offs.push(u16_at[pos.min(src.len())]);
        out.push(offs);
    }
    out
}

/// Source offsets of every text fragment of the nodes accepted by `want`
/// (keyed by fragment address; see [`align_fragments`]).
pub fn fragment_sources(root: &LayoutBox, doc: &Document, want: &dyn Fn(NodeId) -> bool) -> HashMap<usize, Vec<u32>> {
    let mut per_node: HashMap<NodeId, Vec<&Fragment>> = HashMap::new();
    collect(root, want, &mut per_node);
    let mut out = HashMap::new();
    for (node, frags) in per_node {
        let Some(data) = doc.text(node) else { continue };
        let texts: Vec<&str> = frags
            .iter()
            .map(|f| match &f.kind {
                FragmentKind::Text { text, .. } => &**text,
                _ => "",
            })
            .collect();
        for (f, offs) in frags.iter().zip(align_fragments(data, &texts)) {
            out.insert(*f as *const Fragment as usize, offs);
        }
    }
    out
}

/// One highlight overlay: the selection or a custom highlight.
pub(crate) struct Overlay<'s> {
    pub styles: Option<&'s SelectionStyles>,
    /// The selection: unstyled text gets the default highlight colors.
    pub is_selection: bool,
}

/// The highlighted parts of text fragments, for painting.
pub(crate) struct HighlightPaint<'s> {
    /// Overlays in painting order (lowest first; the selection last).
    pub overlays: Vec<Overlay<'s>>,
    /// Highlighted byte ranges of each fragment's text (by fragment
    /// address): (start, end, overlay index), in overlay order.
    pub frags: HashMap<usize, Vec<(usize, usize, usize)>>,
}

/// What the selection and the custom highlights cover, if anything.
pub(crate) fn highlight_paint<'s>(root: &LayoutBox, doc: &Document, tree: &StyledTree, highlights: Option<&'s PaintHighlights>) -> Option<HighlightPaint<'s>> {
    let sel = doc.selection().filter(|s| !s.is_collapsed());
    let custom = highlights.map(|h| &h.custom[..]).unwrap_or(&[]);
    if sel.is_none() && custom.is_empty() {
        return None;
    }
    let mut overlays = Vec::new();
    // Per overlay: text node -> highlighted UTF-16 ranges.
    let mut covered: Vec<HashMap<NodeId, Vec<(usize, usize)>>> = Vec::new();
    let add = |ranges: &mut dyn Iterator<Item = (rabbithole_dom::BoundaryPoint, rabbithole_dom::BoundaryPoint)>, filter: &dyn Fn(NodeId) -> bool| {
        let mut map: HashMap<NodeId, Vec<(usize, usize)>> = HashMap::new();
        for (a, b) in ranges {
            for (n, s, e) in doc.text_in_range(a, b) {
                if filter(n) {
                    map.entry(n).or_default().push((s, e));
                }
            }
        }
        map
    };
    for (ranges, styles) in custom {
        if !styles.any {
            continue; // no ::highlight() rule: nothing to paint
        }
        overlays.push(Overlay { styles: Some(styles), is_selection: false });
        covered.push(add(&mut ranges.iter().copied(), &|_| true));
    }
    if let Some(sel) = sel {
        // `user-select: none` text is never shown selected.
        let selectable = |t: NodeId| doc.parent(t).and_then(|p| tree.get(p)).is_none_or(|s| s.user_select != UserSelect::None);
        overlays.push(Overlay { styles: highlights.and_then(|h| h.selection.as_ref()), is_selection: true });
        covered.push(add(&mut std::iter::once((sel.start, sel.end)), &selectable));
    }
    if covered.iter().all(|m| m.is_empty()) {
        return None;
    }
    let mut per_node: HashMap<NodeId, Vec<&Fragment>> = HashMap::new();
    collect(root, &|n| covered.iter().any(|m| m.contains_key(&n)), &mut per_node);
    let mut frags: HashMap<usize, Vec<(usize, usize, usize)>> = HashMap::new();
    for (node, list) in per_node {
        let Some(data) = doc.text(node) else { continue };
        let shown: Vec<&str> = list
            .iter()
            .map(|f| match &f.kind {
                FragmentKind::Text { text, .. } => &**text,
                _ => "",
            })
            .collect();
        for ((f, text), offs) in list.iter().zip(shown.iter()).zip(align_fragments(data, &shown)) {
            for (oi, map) in covered.iter().enumerate() {
                for &(s, e) in map.get(&node).map(|v| &v[..]).unwrap_or(&[]) {
                    if let Some((b0, b1)) = selected_bytes(text, &offs, s as u32, e as u32) {
                        frags.entry(*f as *const Fragment as usize).or_default().push((b0, b1, oi));
                    }
                }
            }
        }
    }
    Some(HighlightPaint { overlays, frags })
}

/// Byte range of `text` whose characters start inside `[s, e)` of the
/// source (offsets from [`align_fragments`]).
pub fn selected_bytes(text: &str, offs: &[u32], s: u32, e: u32) -> Option<(usize, usize)> {
    let mut first = None;
    let mut last = 0;
    for (i, (b, ch)) in text.char_indices().enumerate() {
        let o = offs.get(i).copied().unwrap_or(u32::MAX);
        if o >= s && o < e {
            if first.is_none() {
                first = Some(b);
            }
            last = b + ch.len_utf8();
        }
    }
    first.map(|f| (f, last))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapsed_space_and_lines() {
        // "  Hello   big\n world" shown as "Hello big " / "world".
        let offs = align_fragments("  Hello   big\n world", &["Hello big ", "world"]);
        assert_eq!(offs[0][0], 2); // H
        assert_eq!(offs[0][5], 7); // the collapsed space
        assert_eq!(offs[0][6], 10); // b
        assert_eq!(offs[0][9], 13); // the line-end space
        assert_eq!(offs[1][0], 15); // w
        assert_eq!(*offs[1].last().unwrap(), 20);
        assert_eq!(selected_bytes("Hello big ", &offs[0], 3, 11), Some((1, 7)));
    }

    #[test]
    fn hyphens_and_utf16() {
        let offs = align_fragments("hy\u{ad}phen 😀x", &["hy-", "phen 😀x"]);
        assert_eq!(offs[0], vec![0, 1, 2, 3]);
        assert_eq!(offs[1][0], 3);
        assert_eq!(offs[1][6], 10); // x after the surrogate pair
        let sharp = align_fragments("straße x", &["STRASSE X"]);
        assert_eq!(sharp[0], vec![0, 1, 2, 3, 4, 4, 5, 6, 7, 8]);
        let auto = align_fragments("hyphen", &["hy-", "phen"]);
        assert_eq!(auto[0], vec![0, 1, 2, 2]);
        assert_eq!(auto[1][0], 2);
    }

    #[test]
    fn preserved_spaces() {
        let offs = align_fragments("a  b", &["a  b"]);
        assert_eq!(offs[0], vec![0, 1, 2, 3, 4]);
    }
}
