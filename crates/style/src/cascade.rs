//! The cascade: rule matching (with an index on the rightmost compound),
//! ordering by importance/origin/specificity/order, custom properties,
//! presentational attributes, and per-property value application.

use crate::computed::*;
use crate::props::{self, ApplyCtx};
use crate::values::{self, ColorValue, LengthContext};
use crate::{FontFace, FontSource, MediaEnv, StyleOptions, StyledTree, Stylesheet};
use rabbithole_css::selector::{Compound, Selector, Specificity};
use rabbithole_css::selector::{is_host_selector, is_slotted_selector, matches_slotted};
use rabbithole_css::{matches_with, parse_declaration_list, parse_stylesheet, Declaration, PseudoElement, StyleRule, Token};
use rabbithole_dom::{Document, NodeId};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

fn ua_sheet() -> &'static Stylesheet {
    static UA: OnceLock<Stylesheet> = OnceLock::new();
    UA.get_or_init(|| parse_stylesheet(crate::ua::UA_CSS))
}

/// Origin/importance rank. Higher wins. `Shadow*` are author rules of a
/// shadow tree applied to its host (`:host`) or to slotted elements
/// (`::slotted`): the inner tree context loses to the outer one for normal
/// declarations and wins for `!important` ones (CSS Cascade 4 §6.1).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Rank {
    UaNormal = 0,
    ShadowNormal = 1,
    AuthorNormal = 2,
    AuthorImportant = 3,
    ShadowImportant = 4,
    UaImportant = 5,
}

struct IndexedRule<'a> {
    is_ua: bool,
    selector: &'a Selector,
    specificity: Specificity,
    decls: &'a [Declaration],
    /// Base URL of the sheet the rule came from.
    base: Option<&'a str>,
}

/// Rules bucketed by the key of their rightmost compound selector.
#[derive(Default)]
struct RuleIndex<'a> {
    rules: Vec<IndexedRule<'a>>,
    by_id: HashMap<String, Vec<usize>>,
    by_class: HashMap<String, Vec<usize>>,
    by_tag: HashMap<String, Vec<usize>>,
    universal: Vec<usize>,
    /// Rules whose subject is `:host...` (applied to the shadow host).
    host: Vec<usize>,
    /// `::slotted(...)` rules (applied to elements assigned to slots).
    slotted: Vec<usize>,
}

impl<'a> RuleIndex<'a> {
    fn add_sheet(&mut self, rules: &[&'a StyleRule], is_ua: bool, base: Option<&'a str>) {
        for rule in rules {
            for sel in &rule.selectors {
                let idx = self.rules.len();
                self.rules.push(IndexedRule {
                    is_ua,
                    selector: sel,
                    specificity: sel.specificity(),
                    decls: &rule.declarations,
                    base,
                });
                if is_slotted_selector(sel) {
                    self.slotted.push(idx);
                    continue;
                }
                if is_host_selector(sel) {
                    self.host.push(idx);
                    continue;
                }
                let last: &Compound = sel.compounds.last().expect("non-empty selector");
                if let Some(id) = &last.id {
                    self.by_id.entry(id.clone()).or_default().push(idx);
                } else if let Some(cls) = last.classes.first() {
                    self.by_class.entry(cls.clone()).or_default().push(idx);
                } else if let Some(tag) = &last.tag {
                    if tag != "*" {
                        self.by_tag.entry(tag.clone()).or_default().push(idx);
                    } else {
                        self.universal.push(idx);
                    }
                } else {
                    self.universal.push(idx);
                }
            }
        }
    }

    /// Candidate rule indices for an element, sorted ascending (= source order).
    fn candidates(&self, doc: &Document, node: NodeId, out: &mut Vec<usize>) {
        out.clear();
        let el = match doc.element(node) {
            Some(e) => e,
            None => return,
        };
        out.extend_from_slice(&self.universal);
        if let Some(v) = self.by_tag.get(&el.tag) {
            out.extend_from_slice(v);
        }
        if let Some(id) = el.id() {
            if let Some(v) = self.by_id.get(id) {
                out.extend_from_slice(v);
            }
        }
        if let Some(classes) = el.attr("class") {
            for c in classes.split_ascii_whitespace() {
                if let Some(v) = self.by_class.get(c) {
                    out.extend_from_slice(v);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
    }
}

/// A declaration that matched an element, with its cascade sort key.
struct Matched<'a> {
    rank: Rank,
    specificity: Specificity,
    order: u32,
    decl: &'a Declaration,
    base: Option<&'a str>,
    pseudo: Option<PseudoElement>,
}

type Vars = HashMap<String, Vec<Token>>;

/// Rule indices per tree scope: the document, and each shadow root (UA
/// rules + the sheets scoped to it). Shadow roots with the same sheets
/// share one index.
struct ScopeIndex<'a> {
    doc: RuleIndex<'a>,
    ua_only: RuleIndex<'a>,
    shadow: Vec<RuleIndex<'a>>,
    by_root: HashMap<NodeId, usize>,
}

impl<'a> ScopeIndex<'a> {
    fn build(ua_rules: &[&'a StyleRule], author: &'a [Stylesheet], author_rules: &'a [Vec<&'a StyleRule>]) -> ScopeIndex<'a> {
        let mut doc = RuleIndex::default();
        doc.add_sheet(ua_rules, true, None);
        let mut per_root: std::collections::BTreeMap<NodeId, Vec<usize>> = std::collections::BTreeMap::new();
        for (i, (sheet, rules)) in author.iter().zip(author_rules).enumerate() {
            if sheet.shadow_scopes.is_empty() {
                doc.add_sheet(rules, false, sheet.base_url.as_deref());
            } else {
                for &r in &sheet.shadow_scopes {
                    per_root.entry(r).or_default().push(i);
                }
            }
        }
        let mut ua_only = RuleIndex::default();
        if !per_root.is_empty() {
            ua_only.add_sheet(ua_rules, true, None);
        }
        let mut shadow: Vec<RuleIndex<'a>> = Vec::new();
        let mut groups: HashMap<Vec<usize>, usize> = HashMap::new();
        let mut by_root = HashMap::new();
        for (root, list) in per_root {
            let gi = match groups.get(&list) {
                Some(&g) => g,
                None => {
                    let mut ix = RuleIndex::default();
                    ix.add_sheet(ua_rules, true, None);
                    for &i in &list {
                        ix.add_sheet(&author_rules[i], false, author[i].base_url.as_deref());
                    }
                    shadow.push(ix);
                    groups.insert(list, shadow.len() - 1);
                    shadow.len() - 1
                }
            };
            by_root.insert(root, gi);
        }
        ScopeIndex { doc, ua_only, shadow, by_root }
    }

    /// The index for the tree rooted at `root` (`None`: the document).
    fn for_scope(&self, root: Option<NodeId>) -> &RuleIndex<'a> {
        match root {
            None => &self.doc,
            Some(r) => match self.by_root.get(&r) {
                Some(&i) => &self.shadow[i],
                None if self.ua_only.rules.is_empty() => &self.doc,
                None => &self.ua_only,
            },
        }
    }
}

pub fn style_tree_with(doc: &Document, author: &[Stylesheet], env: &MediaEnv, opts: &StyleOptions) -> StyledTree {
    let mut ua_rules = Vec::new();
    ua_sheet().active_rules(env, &mut ua_rules);
    let mut author_rules_store: Vec<Vec<&StyleRule>> = Vec::with_capacity(author.len());
    for sheet in author {
        let mut rules = Vec::new();
        sheet.active_rules(env, &mut rules);
        author_rules_store.push(rules);
    }
    let index = ScopeIndex::build(&ua_rules, author, &author_rules_store);

    let mut tree = StyledTree::empty();
    tree.font_faces = collect_font_faces(author, env, opts);
    for sheet in author {
        let mut kf = Vec::new();
        sheet.keyframes(&mut kf);
        for (name, frames) in kf {
            tree.keyframes.insert(
                name.to_string(),
                frames.iter().map(|f| (f.offsets.clone(), f.declarations.clone())).collect(),
            );
        }
    }

    let n = doc.nodes.len();
    tree.styles = vec![None; n];
    tree.before = vec![None; n];
    tree.after = vec![None; n];
    tree.marker = vec![None; n];

    let root_style = Rc::new(ComputedStyle::initial());
    let empty_vars: Rc<Vars> = Rc::new(HashMap::new());

    let html = match doc.html_element() {
        Some(h) => h,
        None => return tree,
    };

    let mut candidates = Vec::new();
    let mut matched: Vec<Matched> = Vec::new();
    // (node, parent style, parent vars, root font size)
    let mut stack: Vec<(NodeId, Rc<ComputedStyle>, Rc<Vars>, f32)> = vec![(html, root_style, empty_vars, 16.0)];

    while let Some((node, parent, parent_vars, root_fs)) = stack.pop() {
        if doc.element(node).is_none() {
            continue;
        }
        let out = compute_element(doc, node, &parent, &parent_vars, root_fs, env, opts, &index, &mut candidates, &mut matched);
        let is_root = node == html;
        let root_fs = if is_root { out.style.font_size } else { root_fs };
        if is_root {
            tree.root_font_size = root_fs;
        }
        let style = Rc::new(out.style);
        let hidden = style.display.is_none();
        tree.styles[node] = Some(style.clone());
        if hidden {
            continue;
        }
        tree.before[node] = out.before.map(Rc::new);
        tree.after[node] = out.after.map(Rc::new);
        tree.marker[node] = out.marker.map(Rc::new);
        // Push children in reverse so they are processed in document order.
        let kids: Vec<NodeId> = doc.flat_child_elements(node);
        for &k in kids.iter().rev() {
            stack.push((k, style.clone(), out.vars.clone(), root_fs));
        }
    }
    tree
}

/// Compute the style of a single element (and its ancestors) without
/// styling the rest of the document. Used for `getComputedStyle()` while
/// the DOM has pending changes, so script read/write loops stay cheap.
/// Returns `None` when the element is not rendered (display:none ancestor).
pub fn style_chain(doc: &Document, author: &[Stylesheet], env: &MediaEnv, opts: &StyleOptions, target: NodeId) -> Option<Rc<ComputedStyle>> {
    let html = doc.html_element()?;
    // Ancestor chain html → target (elements only).
    let mut chain = Vec::new();
    let mut cur = Some(target);
    while let Some(n) = cur {
        if doc.element(n).is_some() {
            chain.push(n);
        }
        if n == html {
            break;
        }
        cur = doc.flat_parent(n);
    }
    if chain.last() != Some(&html) {
        return None; // detached
    }
    chain.reverse();

    let mut ua_rules = Vec::new();
    ua_sheet().active_rules(env, &mut ua_rules);
    let mut author_rules_store: Vec<Vec<&StyleRule>> = Vec::with_capacity(author.len());
    for sheet in author {
        let mut rules = Vec::new();
        sheet.active_rules(env, &mut rules);
        author_rules_store.push(rules);
    }
    let index = ScopeIndex::build(&ua_rules, author, &author_rules_store);

    let mut candidates = Vec::new();
    let mut matched: Vec<Matched> = Vec::new();
    let mut parent = Rc::new(ComputedStyle::initial());
    let mut vars: Rc<Vars> = Rc::new(HashMap::new());
    let mut root_fs = 16.0;
    for &node in &chain {
        let out = compute_element(doc, node, &parent, &vars, root_fs, env, opts, &index, &mut candidates, &mut matched);
        if node == html {
            root_fs = out.style.font_size;
        }
        let style = Rc::new(out.style);
        if style.display.is_none() && node != target {
            return None;
        }
        vars = out.vars;
        parent = style;
    }
    Some(parent)
}

/// Result of styling one element: its style, custom properties and any
/// generated pseudo-element styles.
struct ElementOut {
    style: ComputedStyle,
    vars: Rc<Vars>,
    before: Option<ComputedStyle>,
    after: Option<ComputedStyle>,
    marker: Option<ComputedStyle>,
}

#[allow(clippy::too_many_arguments)]
fn compute_element<'a>(
    doc: &Document,
    node: NodeId,
    parent: &ComputedStyle,
    parent_vars: &Rc<Vars>,
    root_font_size: f32,
    env: &MediaEnv,
    opts: &StyleOptions,
    scopes: &'a ScopeIndex<'a>,
    candidates: &mut Vec<usize>,
    matched: &mut Vec<Matched<'a>>,
) -> ElementOut {
    matched.clear();
    let index = scopes.for_scope(doc.containing_shadow_root(node));
    index.candidates(doc, node, candidates);
    for &ci in candidates.iter() {
        let r = &index.rules[ci];
        if matches_with(r.selector, doc, node, opts.state) {
            for d in r.decls {
                let rank = match (r.is_ua, d.important) {
                    (true, false) => Rank::UaNormal,
                    (true, true) => Rank::UaImportant,
                    (false, false) => Rank::AuthorNormal,
                    (false, true) => Rank::AuthorImportant,
                };
                matched.push(Matched {
                    rank,
                    specificity: r.specificity,
                    order: ci as u32 + 1,
                    decl: d,
                    base: r.base,
                    pseudo: r.selector.pseudo_element,
                });
            }
        }
    }
    // `:host` rules of this element's own shadow tree.
    if let Some(root) = doc.shadow_root(node) {
        let ix = scopes.for_scope(Some(root));
        for &hi in &ix.host {
            let r = &ix.rules[hi];
            if matches_with(r.selector, doc, node, opts.state) {
                for d in r.decls {
                    let rank = if d.important { Rank::ShadowImportant } else { Rank::ShadowNormal };
                    matched.push(Matched { rank, specificity: r.specificity, order: hi as u32 + 1, decl: d, base: r.base, pseudo: r.selector.pseudo_element });
                }
            }
        }
    }
    // `::slotted()` rules of the shadow tree this element is slotted into.
    if let Some(slot) = doc.assigned_slot(node) {
        let ix = scopes.for_scope(doc.containing_shadow_root(slot));
        for &si in &ix.slotted {
            let r = &ix.rules[si];
            if matches_slotted(r.selector, doc, node, slot, opts.state) {
                for d in r.decls {
                    let rank = if d.important { Rank::ShadowImportant } else { Rank::ShadowNormal };
                    matched.push(Matched { rank, specificity: r.specificity, order: si as u32 + 1, decl: d, base: r.base, pseudo: r.selector.pseudo_element });
                }
            }
        }
    }
    // Presentational attributes: lowest author priority.
    let presentational = presentational_declarations(doc, node);
    // Style attribute: beats every selector.
    let style_attr = doc.attr(node, "style").map(parse_declaration_list).unwrap_or_default();

    let mut local: Vec<Matched> = Vec::with_capacity(matched.len() + presentational.len() + style_attr.len());
    let mut pseudo_local: Vec<Matched> = Vec::new();
    for d in &presentational {
        local.push(Matched { rank: Rank::AuthorNormal, specificity: Specificity(0, 0, 0), order: 0, decl: d, base: None, pseudo: None });
    }
    for m in matched.iter() {
        let entry = Matched { rank: m.rank, specificity: m.specificity, order: m.order, decl: m.decl, base: m.base, pseudo: m.pseudo };
        if m.pseudo.is_some() {
            pseudo_local.push(entry);
        } else {
            local.push(entry);
        }
    }
    for d in &style_attr {
        local.push(Matched {
            rank: if d.important { Rank::AuthorImportant } else { Rank::AuthorNormal },
            specificity: Specificity(u32::MAX, 0, 0),
            order: u32::MAX,
            decl: d,
            base: None,
            pseudo: None,
        });
    }
    local.sort_by(|a, b| (a.rank, a.specificity, a.order).cmp(&(b.rank, b.specificity, b.order)));

    // Custom properties.
    let mut vars: Rc<Vars> = parent_vars.clone();
    for m in &local {
        if m.decl.name.starts_with("--") {
            Rc::make_mut(&mut vars).insert(m.decl.name.clone(), m.decl.value.clone());
        }
    }

    let el = doc.element(node).expect("element");
    let is_root = el.tag == "html";
    let style = apply_declarations(&local, &vars, parent, root_font_size, env, opts, is_root, false);

    let mut out = ElementOut { style, vars, before: None, after: None, marker: None };
    if out.style.display.is_none() || pseudo_local.is_empty() {
        return out;
    }
    pseudo_local.sort_by(|a, b| (a.rank, a.specificity, a.order).cmp(&(b.rank, b.specificity, b.order)));
    for which in [PseudoElement::Before, PseudoElement::After, PseudoElement::Marker] {
        let decls: Vec<Matched> = pseudo_local
            .iter()
            .filter(|m| m.pseudo == Some(which))
            .map(|m| Matched { rank: m.rank, specificity: m.specificity, order: m.order, decl: m.decl, base: m.base, pseudo: m.pseudo })
            .collect();
        if decls.is_empty() {
            continue;
        }
        let root_fs = if is_root { out.style.font_size } else { root_font_size };
        let ps = apply_declarations(&decls, &out.vars, &out.style, root_fs, env, opts, false, true);
        match which {
            PseudoElement::Marker => out.marker = Some(ps),
            PseudoElement::Before | PseudoElement::After => {
                if matches!(ps.content, Content::Items(_)) && !ps.display.is_none() {
                    if which == PseudoElement::Before {
                        out.before = Some(ps);
                    } else {
                        out.after = Some(ps);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Apply sorted declarations on top of a style inherited from `parent`.
#[allow(clippy::too_many_arguments)]
fn apply_declarations(
    local: &[Matched],
    vars: &Vars,
    parent: &ComputedStyle,
    root_font_size: f32,
    env: &MediaEnv,
    opts: &StyleOptions,
    is_root: bool,
    pseudo: bool,
) -> ComputedStyle {
    let mut style = ComputedStyle::inherit_from(parent);

    // Substitute var() references up front (shared by both passes).
    let mut resolved: Vec<(&str, std::borrow::Cow<[Token]>, Option<&str>)> = Vec::with_capacity(local.len());
    for m in local {
        if m.decl.name.starts_with("--") {
            continue;
        }
        if m.decl.value.iter().any(|t| matches!(t, Token::Function(f) if f.eq_ignore_ascii_case("var"))) {
            match substitute_vars(&m.decl.value, vars, 0) {
                Some(v) => resolved.push((m.decl.name.as_str(), std::borrow::Cow::Owned(v), m.base)),
                None => continue,
            }
        } else {
            resolved.push((m.decl.name.as_str(), std::borrow::Cow::Borrowed(&m.decl.value), m.base));
        }
    }

    // Pass 1: font-size (em resolves against the parent's font-size).
    let parent_ctx = LengthContext {
        font_size: parent.font_size,
        root_font_size,
        viewport_w: env.width,
        viewport_h: env.height,
    };
    for (name, value, _) in &resolved {
        match *name {
            "font-size" => apply_font_size(value, &mut style, parent, &parent_ctx),
            "font" => {
                if let Some(parsed) = parse_font_shorthand(value) {
                    if let Some(size) = parsed.size {
                        apply_font_size(&size, &mut style, parent, &parent_ctx);
                    }
                }
            }
            _ => {}
        }
    }

    // Pass 2: everything else.
    let len = LengthContext {
        font_size: style.font_size,
        root_font_size: if is_root { style.font_size } else { root_font_size },
        viewport_w: env.width,
        viewport_h: env.height,
    };
    // `color` first: `currentcolor` in every other property (borders,
    // backgrounds, shadows...) is the element's final color, whatever the
    // declaration order.
    for (name, value, base) in &resolved {
        if *name == "color" {
            let ctx = ApplyCtx { len, pseudo, base: *base, document_base: opts.document_base, resolve: opts.resolve_url };
            apply_property(name, value, &mut style, parent, &ctx);
        }
    }
    // `border-*-color` is not inherited; its initial value is currentcolor.
    // `border-*-width` starts as `medium`; the computed width is zero
    // where the style is none/hidden (resolved after all declarations).
    style.border_color = Edges::all(style.color);
    style.border_width = Edges::all(3.0);
    for (name, value, base) in &resolved {
        if *name == "font-size" || *name == "color" {
            continue;
        }
        let ctx = ApplyCtx { len, pseudo, base: *base, document_base: opts.document_base, resolve: opts.resolve_url };
        apply_property(name, value, &mut style, parent, &ctx);
    }

    for (w, st) in [
        (&mut style.border_width.top, style.border_style.top),
        (&mut style.border_width.right, style.border_style.right),
        (&mut style.border_width.bottom, style.border_style.bottom),
        (&mut style.border_width.left, style.border_style.left),
    ] {
        if !st.is_visible() {
            *w = 0.0;
        }
    }

    // Blockification (CSS Display §2.7): the root, out-of-flow and floated
    // boxes, and the children of flex/grid containers (flex/grid items,
    // including their ::before/::after).
    let in_flex_or_grid = !is_root && matches!(parent.display, Display::Flex | Display::InlineFlex | Display::Grid | Display::InlineGrid);
    if is_root || in_flex_or_grid || style.position == Position::Absolute || style.position == Position::Fixed || style.float != Float::None {
        style.display = match style.display {
            Display::Inline | Display::InlineBlock | Display::TableCell | Display::TableRow | Display::TableRowGroup
            | Display::TableHeaderGroup | Display::TableFooterGroup | Display::TableCaption | Display::TableColumn
            | Display::TableColumnGroup | Display::ListItem if !is_root => Display::Block,
            Display::Inline | Display::InlineBlock => Display::Block,
            Display::InlineFlex => Display::Flex,
            Display::InlineGrid => Display::Grid,
            Display::InlineTable => Display::Table,
            Display::Contents if is_root => Display::Block,
            d => d,
        };
    }
    style
}

// ---------------------------------------------------------------- font-face

fn collect_font_faces(author: &[Stylesheet], env: &MediaEnv, opts: &StyleOptions) -> Vec<FontFace> {
    let mut out = Vec::new();
    for sheet in author {
        let mut faces = Vec::new();
        sheet.font_faces(env, &mut faces);
        let base = sheet.base_url.as_deref().unwrap_or(opts.document_base);
        for decls in faces {
            if let Some(f) = parse_font_face(decls, base, opts) {
                out.push(f);
            }
        }
    }
    out
}

fn parse_font_face(decls: &[Declaration], base: &str, opts: &StyleOptions) -> Option<FontFace> {
    let mut family = None;
    let mut sources = Vec::new();
    let mut weight = (400u16, 400u16);
    let mut italic = false;
    let mut display = "auto".to_string();
    let mut unicode_range = None;
    for d in decls {
        let comps = values::components(&d.value);
        match d.name.as_str() {
            "font-family" => {
                let names = props::text::family_names(&comps);
                family = names.into_iter().next().or_else(|| {
                    comps.first().and_then(|c| match c {
                        [Token::String(s)] | [Token::Ident(s)] => Some(s.clone()),
                        _ => None,
                    })
                });
            }
            "src" => {
                for item in values::split_commas(&comps) {
                    let mut url = None;
                    let mut format = None;
                    let mut local = false;
                    for c in &item {
                        if let Some(u) = url_of(c) {
                            url = Some(u);
                        } else if let Some(f) = props::fn_name(c) {
                            match f.as_str() {
                                "local" => local = true,
                                "format" | "tech" => {
                                    if f == "format" {
                                        format = props::fn_args(c).first().and_then(|a| a.first()).and_then(|t| match t {
                                            [Token::String(s)] | [Token::Ident(s)] => Some(s.to_ascii_lowercase()),
                                            _ => None,
                                        });
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    if local {
                        continue;
                    }
                    if let Some(u) = url {
                        let abs = (opts.resolve_url)(base, &u).unwrap_or(u);
                        sources.push(FontSource { url: abs, format });
                    }
                }
            }
            "font-weight" => {
                let ws: Vec<u16> = comps
                    .iter()
                    .filter_map(|c| {
                        if let Some(n) = values::number(c) {
                            Some((n.round() as i32).clamp(1, 1000) as u16)
                        } else {
                            match values::ident(c)?.as_str() {
                                "normal" => Some(400),
                                "bold" => Some(700),
                                _ => None,
                            }
                        }
                    })
                    .collect();
                match ws.len() {
                    1 => weight = (ws[0], ws[0]),
                    2 => weight = (ws[0].min(ws[1]), ws[0].max(ws[1])),
                    _ => {}
                }
            }
            "font-style" => {
                italic = comps.iter().any(|c| matches!(values::ident(c).as_deref(), Some("italic" | "oblique")));
            }
            "font-display" => {
                if let Some(k) = comps.first().and_then(|c| values::ident(c)) {
                    display = k;
                }
            }
            "unicode-range" => {
                // Rebuild the text: the generic tokenizer splits `U+0000-00FF`
                // into idents/numbers, so re-join and parse per comma item.
                let mut text = String::new();
                for t in &d.value {
                    match t {
                        Token::Ident(s) => text.push_str(s),
                        Token::Number(n) => text.push_str(&format!("{n}")),
                        Token::Dimension(v, u) => text.push_str(&format!("{v}{u}")),
                        Token::Percentage(p) => text.push_str(&format!("{p}%")),
                        Token::Delim(c) => text.push(*c),
                        Token::Comma => text.push(','),
                        _ => {}
                    }
                }
                let ranges: Vec<(u32, u32)> = text.split(',').filter_map(parse_unicode_range).collect();
                if !ranges.is_empty() {
                    unicode_range = Some(ranges);
                }
            }
            _ => {}
        }
    }
    Some(FontFace { family: family?, sources, weight, italic, display, unicode_range })
}

/// `U+0025-00FF`, `U+4??`, `U+20AC` (also tolerates a missing `+`, which
/// the tokenizer may have swallowed into a signed number).
fn parse_unicode_range(text: &str) -> Option<(u32, u32)> {
    let t = text.trim().to_ascii_uppercase();
    let body = t.strip_prefix('U')?;
    let body = body.strip_prefix('+').unwrap_or(body);
    if body.is_empty() {
        return None;
    }
    if let Some((a, b)) = body.split_once('-') {
        return Some((u32::from_str_radix(a, 16).ok()?, u32::from_str_radix(b, 16).ok()?));
    }
    if body.contains('?') {
        let lo = body.replace('?', "0");
        let hi = body.replace('?', "F");
        return Some((u32::from_str_radix(&lo, 16).ok()?, u32::from_str_radix(&hi, 16).ok()?));
    }
    let v = u32::from_str_radix(body, 16).ok()?;
    Some((v, v))
}

// ------------------------------------------------------------ custom props

fn substitute_vars(value: &[Token], vars: &Vars, depth: u32) -> Option<Vec<Token>> {
    if depth > 8 {
        return None;
    }
    let mut out = Vec::with_capacity(value.len());
    let mut i = 0;
    while i < value.len() {
        match &value[i] {
            Token::Function(f) if f.eq_ignore_ascii_case("var") => {
                // find matching close paren
                let start = i;
                let mut depth_p = 0i32;
                let mut end = value.len();
                while i < value.len() {
                    match value[i] {
                        Token::Function(_) | Token::OpenParen => depth_p += 1,
                        Token::CloseParen => {
                            depth_p -= 1;
                            if depth_p == 0 {
                                end = i;
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                let inner = &value[start + 1..end];
                i = end + 1;
                let mut it = inner.iter().skip_while(|t| **t == Token::Whitespace);
                let name = match it.next() {
                    Some(Token::Ident(n)) if n.starts_with("--") => n.clone(),
                    _ => return None,
                };
                let rest: Vec<Token> = it.cloned().collect();
                let fallback: Option<Vec<Token>> = rest
                    .iter()
                    .position(|t| *t == Token::Comma)
                    .map(|p| rest[p + 1..].to_vec());
                match vars.get(&name) {
                    Some(v) => {
                        let sub = substitute_vars(v, vars, depth + 1)?;
                        out.extend(sub);
                    }
                    None => match fallback {
                        Some(fb) => {
                            let sub = substitute_vars(&fb, vars, depth + 1)?;
                            out.extend(sub);
                        }
                        None => return None,
                    },
                }
            }
            t => {
                out.push(t.clone());
                i += 1;
            }
        }
    }
    Some(out)
}

// ------------------------------------------------------- presentational

fn presentational_declarations(doc: &Document, node: NodeId) -> Vec<Declaration> {
    let el = match doc.element(node) {
        Some(e) => e,
        None => return Vec::new(),
    };
    let mut css = String::new();
    let tag = el.tag.as_str();
    let dim = |v: &str| -> Option<String> {
        let v = v.trim();
        if v.is_empty() {
            return None;
        }
        if let Some(p) = v.strip_suffix('%') {
            return p.trim().parse::<f32>().ok().map(|n| format!("{n}%"));
        }
        let v = v.trim_end_matches("px");
        v.parse::<f32>().ok().map(|n| format!("{n}px"))
    };
    match tag {
        "img" | "td" | "th" | "table" | "iframe" | "video" | "canvas" | "embed" | "object" | "hr" | "col" | "colgroup" | "svg" | "input" | "select" => {
            if let Some(w) = el.attr("width").and_then(dim) {
                css.push_str(&format!("width:{w};"));
            }
            if tag != "hr" {
                if let Some(h) = el.attr("height").and_then(dim) {
                    css.push_str(&format!("height:{h};"));
                }
            }
        }
        _ => {}
    }
    if let Some(c) = el.attr("bgcolor") {
        css.push_str(&format!("background-color:{};", legacy_color(c)));
    }
    if tag == "body" {
        if let Some(c) = el.attr("text") {
            css.push_str(&format!("color:{};", legacy_color(c)));
        }
        if let Some(b) = el.attr("background") {
            let b = b.trim().replace(['"', '\'', ')', '('], "");
            if !b.is_empty() {
                css.push_str(&format!("background-image:url(\"{b}\");"));
            }
        }
    }
    if tag == "a" {
        // <body link=...> colors unvisited links.
        if let Some(body) = doc.body() {
            if let Some(c) = doc.attr(body, "link") {
                if el.attr("href").is_some() {
                    css.push_str(&format!("color:{};", legacy_color(c)));
                }
            }
        }
    }
    if tag == "input" {
        let ty = el.attr("type").map(|t| t.to_ascii_lowercase()).unwrap_or_default();
        if matches!(ty.as_str(), "" | "text" | "search" | "password" | "email" | "url" | "tel" | "number") {
            if let Some(sz) = el.attr("size").and_then(|v| v.trim().parse::<f32>().ok()) {
                if sz > 0.0 {
                    css.push_str(&format!("width:calc({sz}ch + 6px);"));
                }
            }
        }
    }
    if tag == "textarea" {
        if let Some(c) = el.attr("cols").and_then(|v| v.trim().parse::<f32>().ok()) {
            if c > 0.0 {
                css.push_str(&format!("width:calc({c}ch + 6px);"));
            }
        }
        if let Some(r) = el.attr("rows").and_then(|v| v.trim().parse::<f32>().ok()) {
            if r > 0.0 {
                css.push_str(&format!("height:calc({}em + 6px);", r * 1.2));
            }
        }
    }
    if tag == "select" {
        if let Some(sz) = el.attr("size").and_then(|v| v.trim().parse::<f32>().ok()) {
            if sz > 1.0 {
                css.push_str(&format!("height:calc({}em + 4px);", sz * 1.3));
            }
        }
    }
    if tag == "hr" {
        if let Some(c) = el.attr("color") {
            css.push_str(&format!("color:{0};border-color:{0};border-style:solid;", legacy_color(c)));
        }
        if let Some(w) = el.attr("width").and_then(dim) {
            css.push_str(&format!("width:{w};"));
        }
    }
    if tag == "marquee" {
        css.push_str("display:block;overflow:hidden;white-space:nowrap;");
    }
    if tag == "font" {
        if let Some(c) = el.attr("color") {
            css.push_str(&format!("color:{};", legacy_color(c)));
        }
        if let Some(s) = el.attr("size") {
            let s = s.trim();
            let n: i32 = if let Some(r) = s.strip_prefix('+') {
                3 + r.parse::<i32>().unwrap_or(0)
            } else if let Some(r) = s.strip_prefix('-') {
                3 - r.parse::<i32>().unwrap_or(0)
            } else {
                s.parse().unwrap_or(3)
            };
            let kw = match n.clamp(1, 7) {
                1 => "x-small",
                2 => "small",
                3 => "medium",
                4 => "large",
                5 => "x-large",
                6 => "xx-large",
                _ => "xxx-large",
            };
            css.push_str(&format!("font-size:{kw};"));
        }
        if let Some(f) = el.attr("face") {
            css.push_str(&format!("font-family:{};", f.replace(';', "")));
        }
    }
    if let Some(a) = el.attr("align") {
        let a = a.to_ascii_lowercase();
        match tag {
            "img" | "iframe" | "embed" | "object" | "table" | "hr" => {
                match a.as_str() {
                    "left" | "right" if tag != "hr" => css.push_str(&format!("float:{a};")),
                    "left" => css.push_str("margin-left:0;margin-right:auto;"),
                    "right" => css.push_str("margin-left:auto;margin-right:0;"),
                    "center" | "middle" => css.push_str("margin-left:auto;margin-right:auto;"),
                    _ => {}
                }
            }
            _ => match a.as_str() {
                "left" | "right" | "center" | "justify" => css.push_str(&format!("text-align:{a};")),
                "middle" => css.push_str("text-align:center;"),
                _ => {}
            },
        }
    }
    if let Some(v) = el.attr("valign") {
        let v = v.to_ascii_lowercase();
        if matches!(v.as_str(), "top" | "middle" | "bottom" | "baseline") {
            css.push_str(&format!("vertical-align:{v};"));
        }
    }
    if el.attr("nowrap").is_some() && matches!(tag, "td" | "th") {
        css.push_str("white-space:nowrap;");
    }
    if matches!(tag, "table" | "img") {
        if let Some(b) = el.attr("border") {
            let n: f32 = b.trim().parse().unwrap_or(1.0);
            if n > 0.0 {
                css.push_str(&format!("border:{n}px solid #808080;"));
            }
        }
    }
    if tag == "img" {
        if let Some(h) = el.attr("hspace").and_then(dim) {
            css.push_str(&format!("margin-left:{h};margin-right:{h};"));
        }
        if let Some(v) = el.attr("vspace").and_then(dim) {
            css.push_str(&format!("margin-top:{v};margin-bottom:{v};"));
        }
    }
    if matches!(tag, "td" | "th") {
        // cellpadding / border from the enclosing table
        let table = doc.ancestor_or_self(node, |n| doc.is_element(n, "table"));
        if let Some(t) = table {
            if let Some(p) = doc.attr(t, "cellpadding").and_then(dim) {
                css.push_str(&format!("padding:{p};"));
            }
            if let Some(b) = doc.attr(t, "border") {
                if b.trim().parse::<f32>().map(|n| n > 0.0).unwrap_or(true) {
                    css.push_str("border:1px inset #a0a0a0;");
                }
            }
        }
        if let Some(bg) = el.attr("background") {
            let b = bg.trim().replace(['"', '\'', ')', '('], "");
            if !b.is_empty() {
                css.push_str(&format!("background-image:url(\"{b}\");"));
            }
        }
    }
    if tag == "table" {
        if let Some(cs) = el.attr("cellspacing").and_then(dim) {
            css.push_str(&format!("border-spacing:{cs};"));
        }
        if let Some(bg) = el.attr("background") {
            let b = bg.trim().replace(['"', '\'', ')', '('], "");
            if !b.is_empty() {
                css.push_str(&format!("background-image:url(\"{b}\");"));
            }
        }
    }
    if tag == "hr" {
        if let Some(s) = el.attr("size").and_then(dim) {
            css.push_str(&format!("height:{s};"));
        }
        if el.attr("noshade").is_some() {
            css.push_str("border-style:solid;");
        }
    }
    if tag == "li" || tag == "ol" {
        if let Some(t) = el.attr("type") {
            let lst = match t {
                "1" => Some("decimal"),
                "a" => Some("lower-alpha"),
                "A" => Some("upper-alpha"),
                "i" => Some("lower-roman"),
                "I" => Some("upper-roman"),
                _ => None,
            };
            if let Some(l) = lst {
                css.push_str(&format!("list-style-type:{l};"));
            }
        }
    }
    if tag == "ul" {
        if let Some(t) = el.attr("type") {
            let t = t.to_ascii_lowercase();
            if matches!(t.as_str(), "disc" | "circle" | "square") {
                css.push_str(&format!("list-style-type:{t};"));
            }
        }
    }
    if css.is_empty() {
        Vec::new()
    } else {
        parse_declaration_list(&css)
    }
}

/// `bgcolor="FFFFFF"` style values: add the missing '#'.
fn legacy_color(c: &str) -> String {
    let c = c.trim();
    if c.starts_with('#') || values::named_color(&c.to_ascii_lowercase()).is_some() {
        c.to_string()
    } else if c.len() >= 3 && c.bytes().all(|b| b.is_ascii_hexdigit()) {
        format!("#{c}")
    } else {
        c.to_string()
    }
}

// ------------------------------------------------------------ properties

#[derive(Clone, Copy, PartialEq, Eq)]
enum Wide {
    Inherit,
    Initial,
    Unset,
}

fn wide_keyword(value: &[Token]) -> Option<Wide> {
    match value {
        [Token::Ident(s)] => match s.to_ascii_lowercase().as_str() {
            "inherit" => Some(Wide::Inherit),
            "initial" => Some(Wide::Initial),
            "unset" | "revert" | "revert-layer" => Some(Wide::Unset),
            _ => None,
        },
        _ => None,
    }
}

fn is_inherited(name: &str) -> bool {
    matches!(
        name,
        "color" | "font" | "font-family" | "font-size" | "font-weight" | "font-style" | "line-height" | "letter-spacing"
            | "word-spacing" | "text-align" | "text-decoration" | "text-decoration-line" | "text-transform" | "text-indent"
            | "white-space" | "list-style" | "list-style-type" | "list-style-position" | "visibility" | "cursor"
            | "border-collapse" | "border-spacing" | "direction" | "tab-size" | "word-break" | "overflow-wrap" | "word-wrap"
            | "text-shadow" | "quotes" | "pointer-events" | "user-select" | "font-variant" | "font-variant-caps"
            | "caption-side" | "empty-cells" | "list-style-image" | "text-align-last" | "hyphens" | "font-family-x"
    )
}

/// Copy the field(s) named by `prop` from `from` into `to`.
fn copy_property(prop: &str, from: &ComputedStyle, to: &mut ComputedStyle) {
    match prop {
        "display" => to.display = from.display,
        "position" => to.position = from.position,
        "float" => to.float = from.float,
        "clear" => to.clear = from.clear,
        "flex-direction" => to.flex_direction = from.flex_direction,
        "width" => to.width = from.width,
        "height" => to.height = from.height,
        "min-width" => to.min_width = from.min_width,
        "max-width" => to.max_width = from.max_width,
        "min-height" => to.min_height = from.min_height,
        "max-height" => to.max_height = from.max_height,
        "margin" => to.margin = from.margin,
        "margin-top" => to.margin.top = from.margin.top,
        "margin-right" => to.margin.right = from.margin.right,
        "margin-bottom" => to.margin.bottom = from.margin.bottom,
        "margin-left" => to.margin.left = from.margin.left,
        "padding" => to.padding = from.padding,
        "padding-top" => to.padding.top = from.padding.top,
        "padding-right" => to.padding.right = from.padding.right,
        "padding-bottom" => to.padding.bottom = from.padding.bottom,
        "padding-left" => to.padding.left = from.padding.left,
        "border" => {
            to.border_width = from.border_width;
            to.border_style = from.border_style;
            to.border_color = from.border_color;
        }
        "border-width" => to.border_width = from.border_width,
        "border-style" => to.border_style = from.border_style,
        "border-color" => to.border_color = from.border_color,
        "border-radius" => {
            to.border_radius = from.border_radius;
            to.border_radii = from.border_radii;
        }
        "box-sizing" => to.box_sizing = from.box_sizing,
        "top" => to.inset.top = from.inset.top,
        "right" => to.inset.right = from.inset.right,
        "bottom" => to.inset.bottom = from.inset.bottom,
        "left" => to.inset.left = from.inset.left,
        "inset" => to.inset = from.inset,
        "color" => to.color = from.color,
        "background" => {
            to.background_color = from.background_color;
            to.background_image = from.background_image.clone();
            to.background_layers = from.background_layers.clone();
        }
        "background-color" => to.background_color = from.background_color,
        "opacity" => to.opacity = from.opacity,
        "font" => {
            to.font_family = from.font_family;
            to.font_family_list = from.font_family_list.clone();
            to.font_size = from.font_size;
            to.font_weight = from.font_weight;
            to.font_style = from.font_style;
            to.line_height = from.line_height;
            to.font_variant_small_caps = from.font_variant_small_caps;
        }
        "font-family" => {
            to.font_family = from.font_family;
            to.font_family_list = from.font_family_list.clone();
        }
        "font-size" => to.font_size = from.font_size,
        "font-weight" => to.font_weight = from.font_weight,
        "font-style" => to.font_style = from.font_style,
        "line-height" => to.line_height = from.line_height,
        "letter-spacing" => to.letter_spacing = from.letter_spacing,
        "word-spacing" => to.word_spacing = from.word_spacing,
        "text-align" => to.text_align = from.text_align,
        "text-decoration" => {
            to.text_decoration = from.text_decoration;
            to.text_decoration_color = from.text_decoration_color;
            to.text_decoration_style = from.text_decoration_style;
            to.text_decoration_thickness = from.text_decoration_thickness;
        }
        "text-decoration-line" => to.text_decoration = from.text_decoration,
        "text-transform" => to.text_transform = from.text_transform,
        "text-indent" => to.text_indent = from.text_indent,
        "white-space" => to.white_space = from.white_space,
        "vertical-align" => to.vertical_align = from.vertical_align,
        "list-style" => {
            to.list_style_type = from.list_style_type;
            to.list_style_position = from.list_style_position;
        }
        "list-style-type" => to.list_style_type = from.list_style_type,
        "list-style-position" => to.list_style_position = from.list_style_position,
        "overflow" => {
            to.overflow_x = from.overflow_x;
            to.overflow_y = from.overflow_y;
        }
        "overflow-x" => to.overflow_x = from.overflow_x,
        "overflow-y" => to.overflow_y = from.overflow_y,
        "visibility" => to.visibility = from.visibility,
        "z-index" => to.z_index = from.z_index,
        "cursor" => to.cursor_pointer = from.cursor_pointer,
        "flex" => {
            to.flex_grow = from.flex_grow;
            to.flex_shrink = from.flex_shrink;
            to.flex_basis = from.flex_basis;
        }
        "flex-grow" => to.flex_grow = from.flex_grow,
        "flex-shrink" => to.flex_shrink = from.flex_shrink,
        "flex-basis" => to.flex_basis = from.flex_basis,
        "flex-wrap" => to.flex_wrap = from.flex_wrap,
        "flex-flow" => {
            to.flex_direction = from.flex_direction;
            to.flex_wrap = from.flex_wrap;
        }
        "justify-content" => to.justify_content = from.justify_content,
        "align-items" => to.align_items = from.align_items,
        "align-self" => to.align_self = from.align_self,
        "align-content" => to.align_content = from.align_content,
        "justify-items" => to.justify_items = from.justify_items,
        "justify-self" => to.justify_self = from.justify_self,
        "order" => to.order = from.order,
        "gap" | "grid-gap" => {
            to.row_gap = from.row_gap;
            to.column_gap = from.column_gap;
        }
        "row-gap" | "grid-row-gap" => to.row_gap = from.row_gap,
        "column-gap" | "grid-column-gap" => to.column_gap = from.column_gap,
        "grid-template-columns" => to.grid_template_columns = from.grid_template_columns.clone(),
        "grid-template-rows" => to.grid_template_rows = from.grid_template_rows.clone(),
        "grid-template-areas" => to.grid_template_areas = from.grid_template_areas.clone(),
        "grid-template" | "grid" => {
            to.grid_template_columns = from.grid_template_columns.clone();
            to.grid_template_rows = from.grid_template_rows.clone();
            to.grid_template_areas = from.grid_template_areas.clone();
            to.grid_auto_flow = from.grid_auto_flow;
            to.grid_auto_rows = from.grid_auto_rows.clone();
            to.grid_auto_columns = from.grid_auto_columns.clone();
        }
        "grid-auto-columns" => to.grid_auto_columns = from.grid_auto_columns.clone(),
        "grid-auto-rows" => to.grid_auto_rows = from.grid_auto_rows.clone(),
        "grid-auto-flow" => to.grid_auto_flow = from.grid_auto_flow,
        "grid-row" | "grid-area" | "grid-column" => {
            to.grid_row_start = from.grid_row_start.clone();
            to.grid_row_end = from.grid_row_end.clone();
            to.grid_column_start = from.grid_column_start.clone();
            to.grid_column_end = from.grid_column_end.clone();
        }
        "grid-row-start" => to.grid_row_start = from.grid_row_start.clone(),
        "grid-row-end" => to.grid_row_end = from.grid_row_end.clone(),
        "grid-column-start" => to.grid_column_start = from.grid_column_start.clone(),
        "grid-column-end" => to.grid_column_end = from.grid_column_end.clone(),
        "background-image" | "background-size" | "background-position" | "background-repeat" | "background-attachment"
        | "background-origin" | "background-clip" => {
            to.background_layers = from.background_layers.clone();
            to.background_image = from.background_image.clone();
        }
        "border-top-left-radius" | "border-top-right-radius" | "border-bottom-right-radius" | "border-bottom-left-radius" => {
            to.border_radii = from.border_radii;
            to.border_radius = from.border_radius;
        }
        "box-shadow" => to.box_shadow = from.box_shadow.clone(),
        "text-shadow" => to.text_shadow = from.text_shadow.clone(),
        "transform" => to.transform = from.transform.clone(),
        "transform-origin" => to.transform_origin = from.transform_origin,
        "outline" | "outline-width" | "outline-style" | "outline-color" | "outline-offset" => {
            to.outline_width = from.outline_width;
            to.outline_style = from.outline_style;
            to.outline_color = from.outline_color;
            to.outline_offset = from.outline_offset;
        }
        "text-overflow" => to.text_overflow = from.text_overflow,
        "word-break" => to.word_break = from.word_break,
        "overflow-wrap" | "word-wrap" => to.overflow_wrap = from.overflow_wrap,
        "text-decoration-color" => to.text_decoration_color = from.text_decoration_color,
        "text-decoration-style" => to.text_decoration_style = from.text_decoration_style,
        "text-decoration-thickness" => to.text_decoration_thickness = from.text_decoration_thickness,
        "direction" => to.direction = from.direction,
        "tab-size" => to.tab_size = from.tab_size,
        "-webkit-line-clamp" | "line-clamp" => to.line_clamp = from.line_clamp,
        "font-variant" | "font-variant-caps" => to.font_variant_small_caps = from.font_variant_small_caps,
        "object-fit" => to.object_fit = from.object_fit,
        "object-position" => to.object_position = from.object_position,
        "aspect-ratio" => to.aspect_ratio = from.aspect_ratio,
        "image-rendering" => to.image_rendering_pixelated = from.image_rendering_pixelated,
        "content" => to.content = from.content.clone(),
        "counter-reset" | "counter-set" => to.counter_reset = from.counter_reset.clone(),
        "counter-increment" => to.counter_increment = from.counter_increment.clone(),
        "quotes" => to.quotes = from.quotes.clone(),
        "pointer-events" => to.pointer_events = from.pointer_events,
        "user-select" => to.user_select = from.user_select,
        "resize" => to.resize = from.resize,
        "appearance" | "-webkit-appearance" => to.appearance = from.appearance,
        "transition" | "transition-property" | "transition-duration" | "transition-delay" | "transition-timing-function" => {
            to.transitions = from.transitions.clone()
        }
        "animation" | "animation-name" | "animation-duration" | "animation-delay" | "animation-timing-function"
        | "animation-iteration-count" | "animation-direction" | "animation-fill-mode" => to.animations = from.animations.clone(),
        "border-collapse" => to.border_collapse = from.border_collapse,
        "border-spacing" => to.border_spacing = from.border_spacing,
        "table-layout" => to.table_layout_fixed = from.table_layout_fixed,
        "caption-side" => to.caption_side_bottom = from.caption_side_bottom,
        "empty-cells" => to.empty_cells_hide = from.empty_cells_hide,
        "scroll-margin-top" => to.scroll_margin_top = from.scroll_margin_top,
        "filter" | "isolation" | "will-change" | "contain" | "mix-blend-mode" => to.creates_stacking_context = from.creates_stacking_context,
        _ => {}
    }
}

fn apply_font_size(value: &[Token], style: &mut ComputedStyle, parent: &ComputedStyle, ctx: &LengthContext) {
    if let Some(w) = wide_keyword(value) {
        match w {
            Wide::Initial => style.font_size = 16.0,
            _ => style.font_size = parent.font_size,
        }
        return;
    }
    let comps = values::components(value);
    if comps.len() != 1 {
        return;
    }
    if let Some(kw) = values::ident(comps[0]) {
        let px = match kw.as_str() {
            "xx-small" => 9.0,
            "x-small" => 10.0,
            "small" => 13.0,
            "medium" => 16.0,
            "large" => 18.0,
            "x-large" => 24.0,
            "xx-large" => 32.0,
            "xxx-large" => 48.0,
            "smaller" => parent.font_size / 1.2,
            "larger" => parent.font_size * 1.2,
            _ => return,
        };
        style.font_size = px;
        return;
    }
    if let Some(lv) = values::parse_length(comps[0]) {
        match lv.resolve(ctx) {
            Length::Px(v) => style.font_size = v.max(0.0),
            Length::Percent(p) => style.font_size = (parent.font_size * p / 100.0).max(0.0),
            _ => {}
        }
    }
}

fn resolve_len(c: &[Token], ctx: &LengthContext) -> Option<Length> {
    values::parse_length(c).map(|l| l.resolve(ctx))
}

/// Resolve a length that may not be `auto` or negative-sensitive.
fn resolve_len_no_auto(c: &[Token], ctx: &LengthContext) -> Option<Length> {
    let l = values::parse_length(c)?;
    if l.is_auto() {
        return None;
    }
    Some(l.resolve(ctx))
}

fn color_of(c: &[Token], style: &ComputedStyle) -> Option<Color> {
    match values::parse_color_component(c)? {
        ColorValue::Color(col) => Some(col),
        ColorValue::CurrentColor => Some(style.color),
    }
}

fn parse_edges<T: Copy>(comps: &[&[Token]], f: impl Fn(&[Token]) -> Option<T>) -> Option<Edges<T>> {
    let vals: Option<Vec<T>> = comps.iter().map(|c| f(c)).collect();
    let v = vals?;
    Some(match v.len() {
        1 => Edges::all(v[0]),
        2 => Edges { top: v[0], right: v[1], bottom: v[0], left: v[1] },
        3 => Edges { top: v[0], right: v[1], bottom: v[2], left: v[1] },
        4 => Edges { top: v[0], right: v[1], bottom: v[2], left: v[3] },
        _ => return None,
    })
}

fn border_width(c: &[Token], ctx: &LengthContext) -> Option<f32> {
    if let Some(kw) = values::ident(c) {
        return match kw.as_str() {
            "thin" => Some(1.0),
            "medium" => Some(3.0),
            "thick" => Some(5.0),
            _ => None,
        };
    }
    match values::parse_length(c)?.resolve(ctx) {
        Length::Px(v) => Some(v.max(0.0)),
        _ => None,
    }
}

fn border_style(c: &[Token]) -> Option<BorderStyle> {
    Some(match values::ident(c)?.as_str() {
        "none" | "hidden" => BorderStyle::None,
        "solid" | "groove" | "ridge" | "inset" | "outset" => BorderStyle::Solid,
        "dashed" => BorderStyle::Dashed,
        "dotted" => BorderStyle::Dotted,
        "double" => BorderStyle::Double,
        _ => return None,
    })
}

fn display_of(comps: &[&[Token]]) -> Option<Display> {
    let idents: Vec<String> = comps.iter().filter_map(|c| values::ident(c)).collect();
    if idents.is_empty() || idents.len() != comps.len() {
        return None;
    }
    if idents.len() == 1 {
        return Some(match idents[0].as_str() {
            "none" => Display::None,
            "block" | "flow-root" | "-webkit-flow-root" => {
                if idents[0] == "block" {
                    Display::Block
                } else {
                    Display::FlowRoot
                }
            }
            "inline" => Display::Inline,
            "inline-block" => Display::InlineBlock,
            "list-item" => Display::ListItem,
            "flex" | "-webkit-box" | "-webkit-flex" | "-ms-flexbox" => Display::Flex,
            "inline-flex" | "-webkit-inline-box" | "-webkit-inline-flex" => Display::InlineFlex,
            "grid" | "-ms-grid" => Display::Grid,
            "inline-grid" => Display::InlineGrid,
            "table" => Display::Table,
            "inline-table" => Display::InlineTable,
            "table-row" => Display::TableRow,
            "table-row-group" => Display::TableRowGroup,
            "table-header-group" => Display::TableHeaderGroup,
            "table-footer-group" => Display::TableFooterGroup,
            "table-cell" => Display::TableCell,
            "table-caption" => Display::TableCaption,
            "table-column" => Display::TableColumn,
            "table-column-group" => Display::TableColumnGroup,
            "contents" => Display::Contents,
            "ruby" | "ruby-base" | "ruby-text" => Display::Inline,
            _ => return None,
        });
    }
    // Two-value syntax: <outer> <inner>
    let outer_inline = idents.iter().any(|i| i == "inline");
    let inner = idents.iter().find(|i| matches!(i.as_str(), "flow" | "flow-root" | "flex" | "grid" | "table" | "list-item"));
    Some(match (outer_inline, inner.map(|s| s.as_str())) {
        (true, Some("flow")) => Display::Inline,
        (true, Some("flow-root")) => Display::InlineBlock,
        (true, Some("flex")) => Display::InlineFlex,
        (true, Some("grid")) => Display::InlineGrid,
        (true, Some("table")) => Display::InlineTable,
        (false, Some("flex")) => Display::Flex,
        (false, Some("grid")) => Display::Grid,
        (false, Some("table")) => Display::Table,
        (_, Some("list-item")) => Display::ListItem,
        (false, _) => Display::Block,
        (true, _) => Display::Inline,
    })
}

pub(crate) fn family_of(comps: &[&[Token]]) -> Option<Family> {
    let lists = values::split_commas(comps);
    for fam in lists {
        let mut name = String::new();
        for c in fam {
            match c {
                [Token::Ident(s)] | [Token::String(s)] => {
                    if !name.is_empty() {
                        name.push(' ');
                    }
                    name.push_str(s);
                }
                _ => {}
            }
        }
        let n = name.to_ascii_lowercase();
        if n.is_empty() {
            continue;
        }
        if n == "monospace" || n.contains("mono") || n.contains("courier") || n.contains("menlo") || n.contains("consolas")
            || n.contains("code") || n.contains("inconsolata") || n == "ui-monospace"
        {
            return Some(Family::Mono);
        }
        if n.contains("sans") {
            return Some(Family::Sans);
        }
        if n == "serif" || n.contains("georgia") || n.contains("times") || n.contains("garamond") || n.contains("cambria")
            || n.contains("book") || n.contains("palatino") || n.contains("baskerville") || n.contains("merriweather")
            || n.contains("playfair") || n.contains("ui-serif") || n.contains("lora")
        {
            return Some(Family::Serif);
        }
        if matches!(
            n.as_str(),
            "sans-serif" | "system-ui" | "ui-sans-serif" | "arial" | "helvetica" | "verdana" | "tahoma" | "roboto" | "inter"
                | "segoe ui" | "-apple-system" | "blinkmacsystemfont" | "helvetica neue" | "open sans" | "lato"
                | "ubuntu" | "noto sans" | "dejavu sans" | "cursive" | "fantasy" | "emoji"
        ) || n.contains("sans")
        {
            return Some(Family::Sans);
        }
        // Unknown family: keep scanning for a generic one.
    }
    Some(Family::Sans)
}

fn weight_of(c: &[Token], parent: &ComputedStyle) -> Option<u16> {
    if let Some(n) = values::number(c) {
        return Some((n.round() as i32).clamp(1, 1000) as u16);
    }
    Some(match values::ident(c)?.as_str() {
        "normal" => 400,
        "bold" => 700,
        "bolder" => {
            if parent.font_weight < 350 {
                400
            } else if parent.font_weight < 550 {
                700
            } else {
                900
            }
        }
        "lighter" => {
            if parent.font_weight < 550 {
                100
            } else if parent.font_weight < 750 {
                400
            } else {
                700
            }
        }
        _ => return None,
    })
}

fn line_height_of(c: &[Token], font_size: f32, ctx: &LengthContext) -> Option<Option<f32>> {
    if values::is_ident(c, "normal") {
        return Some(None);
    }
    if let Some(n) = values::number(c) {
        return Some(Some((n * font_size).max(0.0)));
    }
    match values::parse_length(c)?.resolve(ctx) {
        Length::Px(v) => Some(Some(v.max(0.0))),
        Length::Percent(p) => Some(Some(font_size * p / 100.0)),
        _ => None,
    }
}

struct FontShorthand {
    style: Option<FontStyle>,
    small_caps: bool,
    weight: Option<Vec<Token>>,
    size: Option<Vec<Token>>,
    line_height: Option<Vec<Token>>,
    family: Vec<Token>,
}

fn is_size_component(c: &[Token]) -> bool {
    if let Some(kw) = values::ident(c) {
        return matches!(
            kw.as_str(),
            "xx-small" | "x-small" | "small" | "medium" | "large" | "x-large" | "xx-large" | "xxx-large" | "smaller" | "larger"
        );
    }
    matches!(c, [Token::Dimension(..)] | [Token::Percentage(_)] | [Token::Function(_), ..])
}

fn parse_font_shorthand(value: &[Token]) -> Option<FontShorthand> {
    let comps = values::components(value);
    let mut out = FontShorthand { style: None, small_caps: false, weight: None, size: None, line_height: None, family: Vec::new() };
    let mut i = 0;
    while i < comps.len() {
        let c = comps[i];
        if is_size_component(c) {
            break;
        }
        if let Some(kw) = values::ident(c) {
            match kw.as_str() {
                "italic" | "oblique" => out.style = Some(FontStyle::Italic),
                "normal" => {}
                "bold" | "bolder" | "lighter" => out.weight = Some(c.to_vec()),
                "small-caps" => out.small_caps = true,
                "caption" | "icon" | "menu" | "message-box" | "small-caption" | "status-bar" => return None,
                _ => return None,
            }
        } else if let Some(n) = values::number(c) {
            if (1.0..=1000.0).contains(&n) {
                out.weight = Some(c.to_vec());
            } else {
                return None;
            }
        } else {
            return None;
        }
        i += 1;
    }
    let size = comps.get(i)?;
    out.size = Some(size.to_vec());
    i += 1;
    if let Some([Token::Delim('/')]) = comps.get(i) {
        let lh = comps.get(i + 1)?;
        out.line_height = Some(lh.to_vec());
        i += 2;
    }
    // Family: remaining tokens (keep commas) — rebuild token list.
    let mut fam = Vec::new();
    for c in &comps[i..] {
        fam.extend_from_slice(c);
        fam.push(Token::Whitespace);
    }
    out.family = fam;
    Some(out)
}

/// Properties whose grammar is a single component value.
const SINGLE_VALUE: &[&str] = &[
    "position", "float", "clear", "flex-direction", "width", "height", "min-width", "min-height", "max-width", "max-height",
    "margin-top", "margin-right", "margin-bottom", "margin-left", "padding-top", "padding-right", "padding-bottom",
    "padding-left", "border-top-width", "border-right-width", "border-bottom-width", "border-left-width",
    "border-top-style", "border-right-style", "border-bottom-style", "border-left-style", "border-top-color",
    "border-right-color", "border-bottom-color", "border-left-color", "box-sizing", "top", "right", "bottom", "left",
    "color", "background-color", "opacity", "font-weight", "font-style", "line-height", "letter-spacing", "word-spacing",
    "text-align", "text-transform", "text-indent", "white-space", "vertical-align", "list-style-type",
    "list-style-position", "overflow-x", "overflow-y", "visibility", "z-index", "font-size",
    "border-collapse", "table-layout", "caption-side", "empty-cells",
];

fn apply_property(name: &str, value: &[Token], style: &mut ComputedStyle, parent: &ComputedStyle, ctx: &ApplyCtx) {
    // Logical property aliases.
    let name = match name {
        "margin-inline-start" => "margin-left",
        "margin-inline-end" => "margin-right",
        "margin-block-start" => "margin-top",
        "margin-block-end" => "margin-bottom",
        "padding-inline-start" => "padding-left",
        "padding-inline-end" => "padding-right",
        "padding-block-start" => "padding-top",
        "padding-block-end" => "padding-bottom",
        "inset-inline-start" => "left",
        "inset-inline-end" => "right",
        "inset-block-start" => "top",
        "inset-block-end" => "bottom",
        "inline-size" => "width",
        "block-size" => "height",
        "min-inline-size" => "min-width",
        "max-inline-size" => "max-width",
        "min-block-size" => "min-height",
        "max-block-size" => "max-height",
        "border-inline-start" => "border-left",
        "border-inline-end" => "border-right",
        "border-block-start" => "border-top",
        "border-block-end" => "border-bottom",
        "border-inline-start-width" => "border-left-width",
        "border-inline-end-width" => "border-right-width",
        "border-inline-start-color" => "border-left-color",
        "border-inline-end-color" => "border-right-color",
        "border-inline-start-style" => "border-left-style",
        "border-inline-end-style" => "border-right-style",
        "border-start-start-radius" | "border-start-end-radius" | "border-end-start-radius" | "border-end-end-radius" => {
            "border-top-left-radius"
        }
        n => n,
    };

    if let Some(w) = wide_keyword(value) {
        let initial = ComputedStyle::initial();
        let use_parent = match w {
            Wide::Inherit => true,
            Wide::Initial => false,
            Wide::Unset => is_inherited(name),
        };
        let from = if use_parent { parent } else { &initial };
        if name == "all" {
            let mut fresh = if use_parent { ComputedStyle::inherit_from(parent) } else { ComputedStyle::initial() };
            if w == Wide::Inherit {
                fresh = parent.clone();
            }
            // keep font-size from pass 1 semantics simple: take from `from`
            fresh.font_size = from.font_size;
            *style = fresh;
        } else {
            copy_property(name, from, style);
        }
        return;
    }

    let comps = values::components(value);
    if comps.is_empty() {
        return;
    }
    // Values containing bad tokens or stray blocks never match a property
    // grammar; single-value properties with extra components are invalid
    // as a whole (CSS Syntax 3 / CSS 2.1 §4.2: drop the declaration).
    if value.iter().any(|t| matches!(t, Token::BadString | Token::BadUrl | Token::OpenBrace | Token::CloseBrace)) {
        return;
    }
    if SINGLE_VALUE.contains(&name) && comps.len() != 1 {
        return;
    }
    let c0 = comps[0];

    match name {
        "display" => {
            if let Some(d) = display_of(&comps) {
                style.display = d;
            }
        }
        "position" => {
            if let Some(kw) = values::ident(c0) {
                style.position = match kw.as_str() {
                    "static" => Position::Static,
                    "relative" => Position::Relative,
                    "absolute" => Position::Absolute,
                    "fixed" => Position::Fixed,
                    "sticky" | "-webkit-sticky" => Position::Sticky,
                    _ => return,
                };
            }
        }
        "float" => {
            if let Some(kw) = values::ident(c0) {
                style.float = match kw.as_str() {
                    "none" => Float::None,
                    "left" | "inline-start" => Float::Left,
                    "right" | "inline-end" => Float::Right,
                    _ => return,
                };
            }
        }
        "clear" => {
            if let Some(kw) = values::ident(c0) {
                style.clear = match kw.as_str() {
                    "none" => Clear::None,
                    "left" | "inline-start" => Clear::Left,
                    "right" | "inline-end" => Clear::Right,
                    "both" => Clear::Both,
                    _ => return,
                };
            }
        }
        "flex-direction" => {
            if let Some(kw) = values::ident(c0) {
                style.flex_direction = match kw.as_str() {
                    "row" => FlexDirection::Row,
                    "row-reverse" => FlexDirection::RowReverse,
                    "column" => FlexDirection::Column,
                    "column-reverse" => FlexDirection::ColumnReverse,
                    _ => return,
                };
            }
        }
        "flex-flow" => {
            for c in &comps {
                apply_property("flex-direction", c, style, parent, ctx);
            }
        }
        "width" | "height" | "min-width" | "min-height" => {
            if comps.len() != 1 {
                return;
            }
            let l = if let Some(kw) = values::ident(c0) {
                match kw.as_str() {
                    "auto" => Length::Auto,
                    "max-content" | "-webkit-max-content" | "-moz-max-content" => Length::MaxContent,
                    "min-content" | "-webkit-min-content" | "-moz-min-content" => Length::MinContent,
                    "fit-content" | "-webkit-fit-content" | "-moz-fit-content" => Length::FitContent,
                    "-webkit-fill-available" | "-moz-available" | "fill-available" | "stretch" => Length::Auto,
                    _ => return,
                }
            } else if props::fn_name(c0).as_deref() == Some("fit-content") {
                Length::FitContent
            } else {
                match resolve_len(c0, ctx) {
                    Some(l) => l,
                    None => return,
                }
            };
            match name {
                "width" => style.width = l,
                "height" => style.height = l,
                "min-width" => style.min_width = if l.is_auto() { Length::ZERO } else { l },
                _ => style.min_height = if l.is_auto() { Length::ZERO } else { l },
            }
        }
        "max-width" | "max-height" => {
            if comps.len() != 1 {
                return;
            }
            let l = if let Some(kw) = values::ident(c0) {
                match kw.as_str() {
                    "none" | "-webkit-fill-available" | "-moz-available" | "stretch" => Length::Auto,
                    "max-content" | "-webkit-max-content" | "-moz-max-content" => Length::MaxContent,
                    "min-content" | "-webkit-min-content" | "-moz-min-content" => Length::MinContent,
                    "fit-content" | "-webkit-fit-content" | "-moz-fit-content" => Length::FitContent,
                    _ => return,
                }
            } else if props::fn_name(c0).as_deref() == Some("fit-content") {
                Length::FitContent
            } else {
                match resolve_len_no_auto(c0, ctx) {
                    Some(l) => l,
                    None => return,
                }
            };
            if name == "max-width" {
                style.max_width = l;
            } else {
                style.max_height = l;
            }
        }
        "margin" => {
            if let Some(e) = parse_edges(&comps, |c| resolve_len(c, ctx)) {
                style.margin = e;
            }
        }
        "margin-top" | "margin-right" | "margin-bottom" | "margin-left" => {
            if comps.len() == 1 {
                if let Some(l) = resolve_len(c0, ctx) {
                    match name {
                        "margin-top" => style.margin.top = l,
                        "margin-right" => style.margin.right = l,
                        "margin-bottom" => style.margin.bottom = l,
                        _ => style.margin.left = l,
                    }
                }
            }
        }
        "margin-inline" | "margin-block" => {
            if let Some(v) = two_values(&comps, |c| resolve_len(c, ctx)) {
                if name == "margin-inline" {
                    style.margin.left = v.0;
                    style.margin.right = v.1;
                } else {
                    style.margin.top = v.0;
                    style.margin.bottom = v.1;
                }
            }
        }
        "padding" => {
            if let Some(e) = parse_edges(&comps, |c| resolve_len_no_auto(c, ctx)) {
                style.padding = e;
            }
        }
        "padding-top" | "padding-right" | "padding-bottom" | "padding-left" => {
            if comps.len() == 1 {
                if let Some(l) = resolve_len_no_auto(c0, ctx) {
                    match name {
                        "padding-top" => style.padding.top = l,
                        "padding-right" => style.padding.right = l,
                        "padding-bottom" => style.padding.bottom = l,
                        _ => style.padding.left = l,
                    }
                }
            }
        }
        "padding-inline" | "padding-block" => {
            if let Some(v) = two_values(&comps, |c| resolve_len_no_auto(c, ctx)) {
                if name == "padding-inline" {
                    style.padding.left = v.0;
                    style.padding.right = v.1;
                } else {
                    style.padding.top = v.0;
                    style.padding.bottom = v.1;
                }
            }
        }
        "border" | "border-top" | "border-right" | "border-bottom" | "border-left" => {
            let mut width = None;
            let mut bstyle = None;
            let mut color = None;
            if comps.len() > 3 {
                return;
            }
            // Each of width / style / color at most once, in any order.
            for c in &comps {
                if let Some(s) = border_style(c).filter(|_| bstyle.is_none()) {
                    bstyle = Some(s);
                } else if let Some(w) = border_width(c, ctx).filter(|_| width.is_none()) {
                    width = Some(w);
                } else if let Some(col) = color_of(c, style).filter(|_| color.is_none()) {
                    color = Some(col);
                } else {
                    return;
                }
            }
            let w = width.unwrap_or(3.0);
            let s = bstyle.unwrap_or(BorderStyle::None);
            let col = color.unwrap_or(style.color);
            let sides: &[usize] = match name {
                "border" => &[0, 1, 2, 3],
                "border-top" => &[0],
                "border-right" => &[1],
                "border-bottom" => &[2],
                _ => &[3],
            };
            for &side in sides {
                set_side(&mut style.border_width, side, w);
                set_side(&mut style.border_style, side, s);
                set_side(&mut style.border_color, side, col);
            }
        }
        "border-width" => {
            if let Some(e) = parse_edges(&comps, |c| border_width(c, ctx)) {
                style.border_width = e;
            }
        }
        "border-style" => {
            if let Some(e) = parse_edges(&comps, border_style) {
                style.border_style = e;
                // A none style zeroes the used width.
            }
        }
        "border-color" => {
            if let Some(e) = parse_edges(&comps, |c| color_of(c, style)) {
                style.border_color = e;
            }
        }
        "border-top-width" | "border-right-width" | "border-bottom-width" | "border-left-width" => {
            if let Some(w) = border_width(c0, ctx) {
                set_side(&mut style.border_width, side_index(name), w);
            }
        }
        "border-top-style" | "border-right-style" | "border-bottom-style" | "border-left-style" => {
            if let Some(s) = border_style(c0) {
                set_side(&mut style.border_style, side_index(name), s);
            }
        }
        "border-top-color" | "border-right-color" | "border-bottom-color" | "border-left-color" => {
            if let Some(c) = color_of(c0, style) {
                set_side(&mut style.border_color, side_index(name), c);
            }
        }
        "box-sizing" => {
            if let Some(kw) = values::ident(c0) {
                style.box_sizing = match kw.as_str() {
                    "border-box" => BoxSizing::BorderBox,
                    "content-box" => BoxSizing::ContentBox,
                    _ => return,
                };
            }
        }
        "top" | "right" | "bottom" | "left" => {
            if comps.len() == 1 {
                if let Some(l) = resolve_len(c0, ctx) {
                    match name {
                        "top" => style.inset.top = l,
                        "right" => style.inset.right = l,
                        "bottom" => style.inset.bottom = l,
                        _ => style.inset.left = l,
                    }
                }
            }
        }
        "inset" => {
            if let Some(e) = parse_edges(&comps, |c| resolve_len(c, ctx)) {
                style.inset = e;
            }
        }
        "color" => {
            if comps.len() == 1 {
                match values::parse_color_component(c0) {
                    Some(ColorValue::Color(c)) => style.color = c,
                    Some(ColorValue::CurrentColor) => style.color = parent.color,
                    None => {}
                }
            }
        }
        "background-color" => {
            if comps.len() == 1 {
                if let Some(c) = color_of(c0, style) {
                    style.background_color = c;
                }
            }
        }
        "opacity" => {
            if let Some(n) = values::number(c0) {
                style.opacity = n.clamp(0.0, 1.0);
            } else if let [Token::Percentage(p)] = c0 {
                style.opacity = (p / 100.0).clamp(0.0, 1.0);
            }
        }
        "font-weight" => {
            if let Some(w) = weight_of(c0, parent) {
                style.font_weight = w;
            }
        }
        "font-style" => {
            if let Some(kw) = values::ident(c0) {
                style.font_style = match kw.as_str() {
                    "normal" => FontStyle::Normal,
                    "italic" | "oblique" => FontStyle::Italic,
                    _ => return,
                };
            }
        }
        "font" => {
            if let Some(f) = parse_font_shorthand(value) {
                // Size was applied in pass 1; reset the rest to initial first.
                style.font_style = f.style.unwrap_or(FontStyle::Normal);
                style.font_weight = match &f.weight {
                    Some(w) => weight_of(w, parent).unwrap_or(400),
                    None => 400,
                };
                style.line_height = match &f.line_height {
                    Some(lh) => line_height_of(lh, style.font_size, ctx).unwrap_or(None),
                    None => None,
                };
                let fam_comps = values::components(&f.family);
                if !fam_comps.is_empty() {
                    if let Some(fam) = family_of(&fam_comps) {
                        style.font_family = fam;
                    }
                    style.font_family_list = props::text::family_names(&fam_comps);
                }
                style.font_variant_small_caps = f.small_caps;
            }
        }
        "line-height" => {
            if let Some(lh) = line_height_of(c0, style.font_size, ctx) {
                style.line_height = lh;
            }
        }
        "letter-spacing" | "word-spacing" => {
            let v = if values::is_ident(c0, "normal") {
                Some(0.0)
            } else {
                match resolve_len_no_auto(c0, ctx) {
                    Some(Length::Px(v)) => Some(v),
                    _ => None,
                }
            };
            if let Some(v) = v {
                if name == "letter-spacing" {
                    style.letter_spacing = v;
                } else {
                    style.word_spacing = v;
                }
            }
        }
        "text-align" => {
            if let Some(kw) = values::ident(c0) {
                style.text_align = match kw.as_str() {
                    "left" | "start" | "-webkit-left" | "-moz-left" => TextAlign::Left,
                    "right" | "end" | "-webkit-right" | "-moz-right" => TextAlign::Right,
                    "center" | "-webkit-center" | "-moz-center" => TextAlign::Center,
                    "justify" | "justify-all" => TextAlign::Justify,
                    _ => return,
                };
            }
        }
        "text-transform" => {
            if let Some(kw) = values::ident(c0) {
                style.text_transform = match kw.as_str() {
                    "none" => TextTransform::None,
                    "uppercase" => TextTransform::Uppercase,
                    "lowercase" => TextTransform::Lowercase,
                    "capitalize" => TextTransform::Capitalize,
                    _ => return,
                };
            }
        }
        "text-indent" => {
            if let Some(l) = resolve_len_no_auto(c0, ctx) {
                style.text_indent = l;
            }
        }
        "white-space" => {
            if let Some(kw) = values::ident(c0) {
                style.white_space = match kw.as_str() {
                    "normal" => WhiteSpace::Normal,
                    "nowrap" => WhiteSpace::Nowrap,
                    "pre" => WhiteSpace::Pre,
                    "pre-wrap" | "break-spaces" => WhiteSpace::PreWrap,
                    "pre-line" => WhiteSpace::PreLine,
                    _ => return,
                };
            }
        }
        "text-wrap" | "text-wrap-mode" => {
            if let Some(kw) = values::ident(c0) {
                match kw.as_str() {
                    "nowrap" => {
                        if style.white_space == WhiteSpace::Normal {
                            style.white_space = WhiteSpace::Nowrap;
                        }
                    }
                    "wrap" => {
                        if style.white_space == WhiteSpace::Nowrap {
                            style.white_space = WhiteSpace::Normal;
                        }
                    }
                    _ => {}
                }
            }
        }
        "vertical-align" => {
            if let Some(kw) = values::ident(c0) {
                style.vertical_align = match kw.as_str() {
                    "baseline" => VerticalAlign::Baseline,
                    "middle" => VerticalAlign::Middle,
                    "top" => VerticalAlign::Top,
                    "bottom" => VerticalAlign::Bottom,
                    "text-top" => VerticalAlign::TextTop,
                    "text-bottom" => VerticalAlign::TextBottom,
                    "sub" => VerticalAlign::Sub,
                    "super" => VerticalAlign::Super,
                    _ => return,
                };
            } else if let Some(l) = resolve_len_no_auto(c0, ctx) {
                style.vertical_align = match l {
                    Length::Px(v) => VerticalAlign::Length(v),
                    Length::Percent(p) => VerticalAlign::Length(style.line_height_px() * p / 100.0),
                    _ => return,
                };
            }
        }
        "list-style" => {
            let mut ty = None;
            let mut pos = None;
            let mut none_seen = false;
            for c in &comps {
                if let Some(kw) = values::ident(c) {
                    match kw.as_str() {
                        "none" => none_seen = true,
                        "inside" => pos = Some(ListStylePosition::Inside),
                        "outside" => pos = Some(ListStylePosition::Outside),
                        k => match list_style_type(k) {
                            Some(t) => ty = Some(t),
                            // Any other identifier names a counter style (decimal fallback).
                            None => ty = Some(ListStyleType::Decimal),
                        },
                    }
                } else if url_of(c).is_none() && !matches!(c, [Token::String(_)]) && props::fn_name(c).is_none() {
                    return;
                }
            }
            style.list_style_type = ty.unwrap_or(if none_seen { ListStyleType::None } else { ListStyleType::Disc });
            style.list_style_position = pos.unwrap_or(ListStylePosition::Outside);
        }
        "list-style-type" => {
            if let Some(kw) = values::ident(c0) {
                style.list_style_type = list_style_type(&kw).unwrap_or(ListStyleType::Disc);
            } else if let [Token::String(_)] = c0 {
                style.list_style_type = ListStyleType::Disc;
            }
        }
        "list-style-position" => {
            if let Some(kw) = values::ident(c0) {
                style.list_style_position = match kw.as_str() {
                    "inside" => ListStylePosition::Inside,
                    "outside" => ListStylePosition::Outside,
                    _ => return,
                };
            }
        }
        "overflow" => {
            let vals: Vec<Overflow> = comps.iter().filter_map(|c| overflow_of(c)).collect();
            if vals.len() != comps.len() {
                return;
            }
            match vals.len() {
                1 => {
                    style.overflow_x = vals[0];
                    style.overflow_y = vals[0];
                }
                2 => {
                    style.overflow_x = vals[0];
                    style.overflow_y = vals[1];
                }
                _ => {}
            }
        }
        "overflow-x" => {
            if let Some(o) = overflow_of(c0) {
                style.overflow_x = o;
            }
        }
        "overflow-y" => {
            if let Some(o) = overflow_of(c0) {
                style.overflow_y = o;
            }
        }
        "visibility" => {
            if let Some(kw) = values::ident(c0) {
                style.visibility = match kw.as_str() {
                    "visible" => Visibility::Visible,
                    "hidden" | "collapse" => Visibility::Hidden,
                    _ => return,
                };
            }
        }
        "z-index" => {
            if values::is_ident(c0, "auto") {
                style.z_index = None;
            } else if let Some(n) = values::number(c0).filter(|n| n.fract() == 0.0) {
                style.z_index = Some(n as i32);
            }
        }
        "cursor" => {
            let last = comps.iter().rev().find_map(|c| values::ident(c));
            if let Some(kw) = last {
                style.cursor_pointer = kw == "pointer";
            }
        }
        _ => {
            props::apply(name, &comps, value, style, parent, ctx);
        }
    }
}

fn two_values<T: Copy>(comps: &[&[Token]], f: impl Fn(&[Token]) -> Option<T>) -> Option<(T, T)> {
    match comps.len() {
        1 => {
            let v = f(comps[0])?;
            Some((v, v))
        }
        2 => Some((f(comps[0])?, f(comps[1])?)),
        _ => None,
    }
}

pub(crate) fn url_of(c: &[Token]) -> Option<String> {
    match c {
        [Token::Url(u)] => Some(u.clone()),
        [Token::Function(f), rest @ ..] if f.eq_ignore_ascii_case("url") => rest.iter().find_map(|t| match t {
            Token::String(s) => Some(s.clone()),
            _ => None,
        }),
        _ => None,
    }
}

fn overflow_of(c: &[Token]) -> Option<Overflow> {
    Some(match values::ident(c)?.as_str() {
        "visible" => Overflow::Visible,
        "hidden" => Overflow::Hidden,
        "scroll" => Overflow::Scroll,
        "auto" | "overlay" => Overflow::Auto,
        "clip" => Overflow::Clip,
        _ => return None,
    })
}

pub(crate) fn list_style_type(kw: &str) -> Option<ListStyleType> {
    Some(match kw {
        "none" => ListStyleType::None,
        "disc" => ListStyleType::Disc,
        "circle" => ListStyleType::Circle,
        "square" => ListStyleType::Square,
        "decimal" => ListStyleType::Decimal,
        "decimal-leading-zero" => ListStyleType::DecimalLeadingZero,
        "lower-alpha" | "lower-latin" => ListStyleType::LowerAlpha,
        "upper-alpha" | "upper-latin" => ListStyleType::UpperAlpha,
        "lower-roman" => ListStyleType::LowerRoman,
        "upper-roman" => ListStyleType::UpperRoman,
        "disclosure-closed" | "disclosure-open" => ListStyleType::Disc,
        _ => return None,
    })
}

fn side_index(name: &str) -> usize {
    if name.starts_with("border-top") {
        0
    } else if name.starts_with("border-right") {
        1
    } else if name.starts_with("border-bottom") {
        2
    } else {
        3
    }
}

fn set_side<T: Copy>(e: &mut Edges<T>, side: usize, v: T) {
    match side {
        0 => e.top = v,
        1 => e.right = v,
        2 => e.bottom = v,
        _ => e.left = v,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style_tree;
    use rabbithole_css::parse_stylesheet;

    fn env() -> MediaEnv {
        MediaEnv { width: 1000.0, height: 800.0, screen: true, scripting: true }
    }

    fn styled(html: &str, css: &str) -> (Document, StyledTree) {
        let doc = rabbithole_html::parse(html);
        let sheets = vec![parse_stylesheet(css)];
        let tree = style_tree(&doc, &sheets, &env());
        (doc, tree)
    }

    fn style_of<'a>(doc: &Document, tree: &'a StyledTree, tag: &str) -> &'a ComputedStyle {
        let n = doc.find_element(tag).expect("element");
        tree.get(n).expect("styled")
    }

    #[test]
    fn inheritance() {
        let (doc, t) = styled("<div><span>x</span></div>", "div { color: red; margin: 10px }");
        let span = style_of(&doc, &t, "span");
        assert_eq!(span.color, Color::rgb(255, 0, 0));
        assert_eq!(span.margin.top, Length::Px(0.0));
        let div = style_of(&doc, &t, "div");
        assert_eq!(div.margin.top, Length::Px(10.0));
    }

    #[test]
    fn specificity_order_important_and_style_attr() {
        let (doc, t) = styled(
            "<p id=a class=b style='height: 5px'>x</p>",
            "p { color: red; width: 1px; height: 1px } .b { color: green } p { color: blue } #a { width: 2px } p { width: 3px !important }",
        );
        let p = style_of(&doc, &t, "p");
        assert_eq!(p.color, Color::rgb(0, 128, 0));
        assert_eq!(p.width, Length::Px(3.0));
        assert_eq!(p.height, Length::Px(5.0));
    }

    #[test]
    fn em_rem_percent() {
        let (doc, t) = styled(
            "<body><p><span>x</span></p></body>",
            "html { font-size: 20px } body { font-size: 16px } p { font-size: 2em; width: 2em; height: 50%; margin-left: 1rem } span { font-size: 150% }",
        );
        let p = style_of(&doc, &t, "p");
        assert_eq!(p.font_size, 32.0);
        assert_eq!(p.width, Length::Px(64.0));
        assert_eq!(p.height, Length::Percent(50.0));
        assert_eq!(p.margin.left, Length::Px(20.0));
        let span = style_of(&doc, &t, "span");
        assert_eq!(span.font_size, 48.0);
    }

    #[test]
    fn shorthands() {
        let (doc, t) = styled(
            "<div>x</div>",
            "div { margin: 1px 2px; padding: 1px 2px 3px; border: 2px dashed red; border-radius: 4px; background: url(a.png) #fff; overflow: hidden auto; font: italic bold 12px/1.5 Georgia, serif }",
        );
        let d = style_of(&doc, &t, "div");
        assert_eq!(d.margin, Edges { top: Length::Px(1.0), right: Length::Px(2.0), bottom: Length::Px(1.0), left: Length::Px(2.0) });
        assert_eq!(d.padding.bottom, Length::Px(3.0));
        assert_eq!(d.padding.left, Length::Px(2.0));
        assert_eq!(d.border_width, Edges::all(2.0));
        assert_eq!(d.border_style, Edges::all(BorderStyle::Dashed));
        assert_eq!(d.border_color, Edges::all(Color::rgb(255, 0, 0)));
        assert_eq!(d.border_radius, 4.0);
        assert_eq!(d.background_color, Color::WHITE);
        assert_eq!(d.background_image.as_deref(), Some("a.png"));
        assert_eq!(d.overflow_x, Overflow::Hidden);
        assert_eq!(d.overflow_y, Overflow::Auto);
        assert_eq!(d.font_style, FontStyle::Italic);
        assert_eq!(d.font_weight, 700);
        assert_eq!(d.font_size, 12.0);
        assert_eq!(d.line_height, Some(18.0));
        assert_eq!(d.font_family, Family::Serif);
    }

    #[test]
    fn display_none_subtree() {
        let (doc, t) = styled("<div><p><b>x</b></p></div>", "div { display: none }");
        let div = doc.find_element("div").unwrap();
        assert_eq!(t.get(div).unwrap().display, Display::None);
        let p = doc.find_element("p").unwrap();
        assert!(t.get(p).is_none());
        let b = doc.find_element("b").unwrap();
        assert!(t.get(b).is_none());
        // text nodes are unstyled
        let text = doc.children(b).next().unwrap();
        assert!(t.get(text).is_none());
    }

    #[test]
    fn ua_defaults() {
        let (doc, t) = styled(
            "<h1>T</h1><a href=x>l</a><ul><li>i</li></ul><script>var x;</script><p><em>e</em><code>c</code></p>",
            "",
        );
        let h1 = style_of(&doc, &t, "h1");
        assert_eq!(h1.font_size, 32.0);
        assert!(h1.is_bold());
        assert_eq!(h1.display, Display::Block);
        let a = style_of(&doc, &t, "a");
        assert!(a.text_decoration.underline);
        assert_eq!(a.color, Color::rgb(0, 0, 0xee));
        assert!(a.cursor_pointer);
        let li = style_of(&doc, &t, "li");
        assert_eq!(li.display, Display::ListItem);
        assert_eq!(li.list_style_type, ListStyleType::Disc);
        let ul = style_of(&doc, &t, "ul");
        assert_eq!(ul.padding.left, Length::Px(40.0));
        let script = style_of(&doc, &t, "script");
        assert_eq!(script.display, Display::None);
        let body = style_of(&doc, &t, "body");
        assert_eq!(body.margin, Edges::all(Length::Px(8.0)));
        let html = style_of(&doc, &t, "html");
        assert_eq!(html.display, Display::Block);
        assert!(style_of(&doc, &t, "em").is_italic());
        assert_eq!(style_of(&doc, &t, "code").font_family, Family::Mono);
    }

    #[test]
    fn media_queries_use_env() {
        let doc = rabbithole_html::parse("<p>x</p>");
        let sheets = vec![parse_stylesheet("@media (max-width: 600px) { p { color: red } } @media (min-width: 601px) { p { color: blue } }")];
        let narrow = style_tree(&doc, &sheets, &MediaEnv { width: 500.0, height: 800.0, screen: true, scripting: true });
        let wide = style_tree(&doc, &sheets, &MediaEnv { width: 900.0, height: 800.0, screen: true, scripting: true });
        let p = doc.find_element("p").unwrap();
        assert_eq!(narrow.get(p).unwrap().color, Color::rgb(255, 0, 0));
        assert_eq!(wide.get(p).unwrap().color, Color::rgb(0, 0, 255));
    }

    #[test]
    fn custom_properties() {
        let (doc, t) = styled(
            "<div><p>x</p></div>",
            ":root { --main: #123456; --gap: 4px } div { --gap: 8px } p { color: var(--main); margin: var(--gap); padding: var(--missing, 2px); width: var(--nope) }",
        );
        let p = style_of(&doc, &t, "p");
        assert_eq!(p.color, Color::rgb(0x12, 0x34, 0x56));
        assert_eq!(p.margin.top, Length::Px(8.0));
        assert_eq!(p.padding.top, Length::Px(2.0));
        assert_eq!(p.width, Length::Auto);
    }

    #[test]
    fn presentational_attributes() {
        let (doc, t) = styled(
            "<img width=100 height='50%'><table bgcolor=FF0000 cellpadding=5 border=1><tr><td align=center>c</td></tr></table><font color=red size=5>f</font>",
            "",
        );
        let img = style_of(&doc, &t, "img");
        assert_eq!(img.width, Length::Px(100.0));
        assert_eq!(img.height, Length::Percent(50.0));
        let table = style_of(&doc, &t, "table");
        assert_eq!(table.background_color, Color::rgb(255, 0, 0));
        assert_eq!(table.border_width.top, 1.0);
        let td = style_of(&doc, &t, "td");
        assert_eq!(td.padding.top, Length::Px(5.0));
        assert_eq!(td.text_align, TextAlign::Center);
        let font = style_of(&doc, &t, "font");
        assert_eq!(font.color, Color::rgb(255, 0, 0));
        assert_eq!(font.font_size, 24.0);
    }

    #[test]
    fn wide_keywords_and_blockification() {
        let (doc, t) = styled(
            "<div><p><span>x</span></p></div>",
            "div { color: red } p { color: blue } span { color: inherit; display: inline; position: absolute; margin: initial }",
        );
        let span = style_of(&doc, &t, "span");
        assert_eq!(span.color, Color::rgb(0, 0, 255));
        assert_eq!(span.display, Display::Block);
    }

    #[test]
    fn currentcolor_and_text_decoration() {
        let (doc, t) = styled("<p><u>x</u></p>", "p { color: green; border: 1px solid currentcolor } u { text-decoration: underline line-through }");
        let p = style_of(&doc, &t, "p");
        assert_eq!(p.border_color.top, Color::rgb(0, 128, 0));
        let u = style_of(&doc, &t, "u");
        assert!(u.text_decoration.underline && u.text_decoration.line_through);
    }
}
