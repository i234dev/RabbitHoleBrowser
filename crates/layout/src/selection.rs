//! The document selection over laid out text: which characters of each
//! text fragment come from which offsets of its DOM text node, and which
//! part of each fragment is selected.
//!
//! Nothing here runs unless something is selected (or the user agent maps
//! a point to a DOM position), so unselected pages pay nothing for it.

use crate::tree::{Fragment, FragmentKind, LayoutBox};
use rabbithole_dom::{Document, NodeId};
use rabbithole_style::highlight::SelectionStyles;
use rabbithole_style::{StyledTree, UserSelect};
use std::collections::HashMap;

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c' | '\u{a0}')
}

/// Text fragments of the text nodes accepted by `want`, per node, in
/// tree (and line) order.
fn collect<'a>(b: &'a LayoutBox, want: &dyn Fn(NodeId) -> bool, out: &mut HashMap<NodeId, Vec<&'a Fragment>>) {
    for line in &b.lines {
        for f in &line.fragments {
            if let FragmentKind::Text { src: Some(n), .. } = &f.kind {
                if want(*n) {
                    out.entry(*n).or_default().push(f);
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
        for (i, &c) in chars.iter().enumerate() {
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
            while pos < src.len() && (is_space(src[pos]) || src[pos] == '\u{ad}') && !(src[pos] == '\u{ad}' && c == '-') {
                pos += 1;
            }
            let inserted_hyphen = matches!(c, '-' | '\u{2010}') && i + 1 == chars.len() && src.get(pos).is_none_or(|&s| s != c && s != '\u{ad}');
            offs.push(u16_at[pos.min(src.len())]);
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

/// The selected part of each text fragment, for painting.
pub(crate) struct SelectionPaint<'s> {
    /// Selected byte range of the fragment's text, by fragment address.
    pub frags: HashMap<usize, (usize, usize)>,
    pub styles: Option<&'s SelectionStyles>,
}

/// What the current (non-collapsed) selection covers, if anything.
pub(crate) fn selection_paint<'s>(root: &LayoutBox, doc: &Document, tree: &StyledTree, styles: Option<&'s SelectionStyles>) -> Option<SelectionPaint<'s>> {
    let sel = doc.selection().filter(|s| !s.is_collapsed())?;
    // `user-select: none` text is never shown selected.
    let selectable = |t: NodeId| doc.parent(t).and_then(|p| tree.get(p)).is_none_or(|s| s.user_select != UserSelect::None);
    let texts: HashMap<NodeId, (usize, usize)> = doc.text_in_range(sel.start, sel.end).into_iter().filter(|(n, _, _)| selectable(*n)).map(|(n, s, e)| (n, (s, e))).collect();
    if texts.is_empty() {
        return None;
    }
    let mut per_node: HashMap<NodeId, Vec<&Fragment>> = HashMap::new();
    collect(root, &|n| texts.contains_key(&n), &mut per_node);
    let mut frags = HashMap::new();
    for (node, list) in per_node {
        let Some(data) = doc.text(node) else { continue };
        let (s, e) = texts[&node];
        let shown: Vec<&str> = list
            .iter()
            .map(|f| match &f.kind {
                FragmentKind::Text { text, .. } => &**text,
                _ => "",
            })
            .collect();
        for ((f, text), offs) in list.iter().zip(shown.iter()).zip(align_fragments(data, &shown)) {
            if let Some(r) = selected_bytes(text, &offs, s as u32, e as u32) {
                frags.insert(*f as *const Fragment as usize, r);
            }
        }
    }
    Some(SelectionPaint { frags, styles })
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
