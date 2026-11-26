//! Cascade origin of whitespace formatting, queried only during editing.
use super::*;

type Key = (Rank, Specificity, u32);

fn relevant(name: &str, component: &str) -> bool {
    matches!(name, "white-space" | "all") || name == component ||
        (component == "text-wrap-mode" && name == "text-wrap")
}

// Project shorthand and 'all' declarations onto the queried component so
// that the normal cascade rollback algorithm sees a single property.
fn winner(doc: &Document, node: NodeId, cx: &MatchCtx, component: &str) -> Option<bool> {
    let mut decls: Vec<(Key, Declaration)> = Vec::new();
    let mut add = |d: &Declaration, rank: Rank, specificity: Specificity, order| {
        if relevant(&d.name, component) && supports_declaration(&d.name, &d.value) != Some(false) {
            decls.push(((rank, specificity, order), Declaration {
                name: component.to_owned(), value: d.value.clone(), important: d.important,
            }));
        }
    };
    let mut sheet = |r: &IndexedRule, order, shadow| {
        for d in r.decls.iter() {
            let rank = match (r.is_ua, shadow, d.important) {
                (true, _, false) => Rank::UaNormal,
                (true, _, true) => Rank::UaImportant,
                (_, true, false) => Rank::ShadowNormal,
                (_, true, true) => Rank::ShadowImportant,
                (_, false, false) => Rank::AuthorNormal,
                (_, false, true) => Rank::AuthorImportant,
            };
            add(d, rank, layered(r.specificity, r.layer, d.important), order);
        }
    };
    let index = cx.scopes.for_scope(doc.containing_shadow_root(node));
    let mut candidates = Vec::new();
    index.candidates(doc, node, &mut candidates);
    for i in candidates {
        let r = &index.rules[i];
        if r.selector.pseudo_element.is_none() && r.decls.iter().any(|d| relevant(&d.name, component)) &&
            cx.matcher.matches(&r.selector, node) && r.container_matches(doc, node, cx) {
            sheet(r, i as u32 + 1, false);
        }
    }
    if let Some(root) = doc.shadow_root(node) {
        let ix = cx.scopes.for_scope(Some(root));
        for &i in &ix.host {
            let r = &ix.rules[i];
            if r.selector.pseudo_element.is_none() && cx.matcher.matches(&r.selector, node) && r.container_matches(doc, node, cx) {
                sheet(r, i as u32 + 1, true);
            }
        }
    }
    if let Some(slot) = doc.assigned_slot(node) {
        let ix = cx.scopes.for_scope(doc.containing_shadow_root(slot));
        for &i in &ix.slotted {
            let r = &ix.rules[i];
            if matches_slotted(&r.selector, doc, node, slot, cx.opts.state) && r.container_matches(doc, node, cx) {
                sheet(r, i as u32 + 1, true);
            }
        }
    }
    if let Some(css) = presentational_css(doc, node) {
        for d in parse_declaration_list(&css) { add(&d, Rank::AuthorNormal, Specificity(0, 0, 0), 0); }
    }
    if let Some(css) = doc.attr(node, "style") {
        for d in parse_declaration_list(css) {
            add(&d, if d.important { Rank::AuthorImportant } else { Rank::AuthorNormal }, Specificity(u32::MAX, 0, 0), u32::MAX);
        }
    }
    decls.sort_by_key(|(key, _)| *key);
    let mut matched: Vec<_> = decls.iter().map(|((rank, specificity, order), decl)| Matched {
        rank: *rank, specificity: *specificity, order: *order, decl, base: None, pseudo: None,
    }).collect();
    if matched.iter().any(|m| rollback_keyword(m.decl).is_some()) { roll_back(&mut matched); }
    let m = matched.last()?;
    if matches!(wide_keyword(&m.decl.value), Some(Wide::Inherit | Wide::Unset)) { return None; }
    Some(!matches!(m.rank, Rank::UaNormal | Rank::UaImportant))
}

pub(crate) fn author_white_space(doc: &Document, node: NodeId, cx: &MatchCtx) -> bool {
    let mut collapse = None;
    let mut wrap = None;
    let mut current = Some(node);
    while let Some(el) = current.filter(|&e| doc.element(e).is_some()) {
        collapse = collapse.or_else(|| winner(doc, el, cx, "white-space-collapse"));
        wrap = wrap.or_else(|| winner(doc, el, cx, "text-wrap-mode"));
        if collapse == Some(true) || wrap == Some(true) { return true; }
        if collapse.is_some() && wrap.is_some() { break; }
        current = doc.flat_parent(el);
    }
    false
}
