//! Incremental style recalculation.
//!
//! A [`StyleEngine`] keeps the rule index of the active stylesheets, an
//! invalidation map (which selectors depend on which classes, ids,
//! attributes and pseudo-classes, and how: the element itself, its
//! descendants or its following siblings), and per-element cascade data
//! (custom properties, `inherit` dependencies) between restyles.
//!
//! [`StyleEngine::update`] turns the DOM mutation log and dynamic-state
//! changes (hover/focus/active/target) into a set of elements to restyle,
//! then walks down from the root only into subtrees that contain them.
//! An element whose recomputed style equals the previous one keeps the old
//! `Rc` (so later stages can detect "unchanged" by pointer), and its
//! children are only revisited when its inherited properties, custom
//! properties or flex/grid-ness changed (or a child depends on it through
//! an explicit `inherit`).
//!
//! Rule matching uses an ancestor bloom filter to reject descendant/child
//! selectors whose required ancestors cannot be present.

use crate::cascade::{compute_element, DeclCache, ElementOut, MatchCtx, Matched, ScopeIndex, ShareCache, Vars};
use crate::computed::*;
use crate::{FontFace, MediaEnv, StyleOptions, StyledTree, Stylesheet};
use rabbithole_css::selector::{Combinator, Compound, Pseudo, Selector, SelectorMatcher};
use rabbithole_css::MatchState;
use rabbithole_dom::{Document, Mutation, MutationLog, NodeId};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

// ------------------------------------------------------------ bloom filter

/// Hash of a selector feature: `kind` distinguishes tags, ids and classes.
fn feature_hash(kind: u8, s: &str) -> u32 {
    // FNV-1a.
    let mut h: u32 = 0x811c_9dc5 ^ kind as u32;
    h = h.wrapping_mul(0x0100_0193);
    for b in s.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h | 1 // never 0 (0 marks an unused slot)
}

const BLOOM_BITS: usize = 1 << 12;

/// Counting bloom filter of the tags, ids and classes of the ancestors of
/// the element being styled.
pub struct AncestorBloom {
    counts: Vec<u8>,
}

impl Default for AncestorBloom {
    fn default() -> Self {
        AncestorBloom { counts: vec![0; BLOOM_BITS] }
    }
}

impl AncestorBloom {
    fn slots(h: u32) -> (usize, usize) {
        ((h as usize) & (BLOOM_BITS - 1), ((h >> 12) as usize) & (BLOOM_BITS - 1))
    }

    fn add_hash(&mut self, h: u32, delta: i8) {
        let (a, b) = Self::slots(h);
        for i in [a, b] {
            let c = &mut self.counts[i];
            if delta > 0 {
                *c = c.saturating_add(1);
            } else if *c != u8::MAX {
                // A saturated counter stays set (it may be shared).
                *c = c.saturating_sub(1);
            }
        }
    }

    fn element(&mut self, doc: &Document, node: NodeId, delta: i8) {
        let Some(el) = doc.element(node) else { return };
        self.add_hash(feature_hash(b't', &el.tag), delta);
        if let Some(id) = el.id() {
            self.add_hash(feature_hash(b'#', id), delta);
        }
        if let Some(c) = el.attr("class") {
            for cls in c.split_ascii_whitespace() {
                self.add_hash(feature_hash(b'.', cls), delta);
            }
        }
    }

    pub fn push(&mut self, doc: &Document, node: NodeId) {
        self.element(doc, node, 1);
    }

    pub fn pop(&mut self, doc: &Document, node: NodeId) {
        self.element(doc, node, -1);
    }

    fn contains(&self, h: u32) -> bool {
        let (a, b) = Self::slots(h);
        self.counts[a] != 0 && self.counts[b] != 0
    }

    /// Can a selector whose ancestor compounds hash to `hashes` match?
    pub fn may_match(&self, hashes: &[u32; 4]) -> bool {
        hashes.iter().all(|&h| h == 0 || self.contains(h))
    }
}

/// Hashes of features that ancestors of the subject must have: for every
/// compound followed by a descendant/child combinator, its id, else its
/// first class, else its tag (at most four).
pub(crate) fn ancestor_hashes(sel: &Selector) -> [u32; 4] {
    let mut out = [0u32; 4];
    let mut n = 0;
    let last = sel.compounds.len().saturating_sub(1);
    for i in (0..last).rev() {
        if n == 4 {
            break;
        }
        if !matches!(sel.combinators[i], Combinator::Descendant | Combinator::Child) {
            continue;
        }
        let c = &sel.compounds[i];
        let h = if let Some(id) = &c.id {
            feature_hash(b'#', id)
        } else if let Some(cls) = c.classes.first() {
            feature_hash(b'.', cls)
        } else if let Some(t) = c.tag.as_deref().filter(|t| *t != "*") {
            feature_hash(b't', t)
        } else {
            continue;
        };
        out[n] = h;
        n += 1;
    }
    out
}

// ------------------------------------------------------------ invalidation

/// The changed element itself must be restyled.
const R_SELF: u8 = 1;
/// Its whole subtree must be restyled.
const R_DESC: u8 = 2;
/// Its following siblings (and their subtrees) must be restyled.
const R_SIB: u8 = 4;
/// Anything may change (`:has()`): restyle the document.
const R_GLOBAL: u8 = 8;
/// Inside `:has()`: the match of an ancestor or preceding sibling (the
/// `:has()` subject) may flip.
const R_HAS: u8 = 16;

/// Which selectors depend on which features, and how.
#[derive(Default, Debug)]
struct InvalidationMap {
    classes: HashMap<String, u8>,
    ids: HashMap<String, u8>,
    attrs: HashMap<String, u8>,
    /// Compounds using `:hover` / `:active` / `:focus`, so a state change
    /// only restyles elements that can match the rest of the compound.
    hover: StateDeps,
    active: StateDeps,
    focus: StateDeps,
    target: u8,
    /// `:first-child` / `:only-child`: an insertion flips the next
    /// element sibling at most.
    first: u8,
    /// `:last-child` / `:only-child`: the previous element sibling.
    last: u8,
    /// `:nth-child()`, `:nth-of-type()`, `:*-of-type`: every following
    /// sibling.
    nth_fwd: u8,
    /// `:nth-last-child()`, `:nth-last-of-type()`, `:*-of-type`: every
    /// preceding sibling.
    nth_back: u8,
    empty: u8,
    /// Relation of compounds right of a `+`/`~` combinator (elements whose
    /// match changes when a previous sibling is inserted or removed).
    sibling_follow: u8,
    /// `:defined`.
    defined: u8,
    /// Some selector uses `:has()`.
    has_has: bool,
    /// Tags, classes, ids and attribute names in `:has()` arguments: an
    /// inserted or removed subtree with none of them cannot change what a
    /// `:has()` matches (unless `has_any`).
    /// (Indexed by kind: tags, ids, classes, attribute names.)
    has_features: [HashSet<String>; 4],
    /// Some compound in a `:has()` argument matches without any of those
    /// features (universal, pseudo-classes only, `:not()`).
    has_any: bool,
    /// Relation of the compounds carrying `:has()` to their subjects; when
    /// not just `R_SELF`, `:has()` changes restyle the whole document.
    has_outer: u8,
}

impl InvalidationMap {
    fn build(scopes: &ScopeIndex) -> InvalidationMap {
        let mut m = InvalidationMap::default();
        for r in std::iter::once(&scopes.doc).chain(scopes.shadow.iter()).flat_map(|ix| ix.rules.iter()) {
            m.add_selector(&r.selector, R_SELF, false);
        }
        m
    }

    /// Can inserting or removing the subtree at `root` change what some
    /// `:has()` matches?
    fn may_affect_has(&self, doc: &Document, root: NodeId) -> bool {
        if self.has_any {
            return true;
        }
        doc.descendants(root).any(|n| {
            let Some(el) = doc.element(n) else { return false };
            let [tags, ids, classes, attrs] = &self.has_features;
            tags.contains(el.tag.as_str())
                || el.id().is_some_and(|id| ids.contains(id))
                || el.attr("class").is_some_and(|c| c.split_ascii_whitespace().any(|c| classes.contains(c)))
                || el.attrs.iter().any(|(name, _)| attrs.contains(name.as_str()))
        })
    }

    /// Add the selectors of the shadow-scope groups `groups` (dependencies
    /// are only ever added: a map with extra entries over-approximates).
    fn add_groups(&mut self, scopes: &ScopeIndex, groups: &[usize]) {
        for &g in groups {
            for r in scopes.shadow[g].rules.iter().filter(|r| !r.is_ua) {
                self.add_selector(&r.selector, R_SELF, false);
            }
        }
    }

    /// Record the features of `sel`, whose subject relates to the element
    /// that matters as `outer` (nested selectors) — `in_has` forces global.
    fn add_selector(&mut self, sel: &Selector, outer: u8, in_has: bool) {
        let last = sel.compounds.len() - 1;
        for (i, c) in sel.compounds.iter().enumerate() {
            let own = if i == last {
                R_SELF
            } else {
                match sel.combinators[i] {
                    Combinator::Descendant | Combinator::Child => R_DESC,
                    Combinator::NextSibling | Combinator::SubsequentSibling => R_SIB,
                }
            };
            // Inside `:has()` the argument's own ancestors are matched without
            // scoping, so a change there can also affect elements below it.
            let rel = if in_has {
                if own == R_SELF { R_HAS } else { R_HAS | R_DESC }
            } else if own == R_SELF {
                outer
            } else {
                own
            };
            if i > 0 && matches!(sel.combinators[i - 1], Combinator::NextSibling | Combinator::SubsequentSibling) {
                self.sibling_follow |= rel;
            }
            self.add_compound(c, rel, in_has);
        }
    }

    fn add_compound(&mut self, c: &Compound, rel: u8, in_has: bool) {
        if in_has {
            let tag = c.tag.as_ref().filter(|t| *t != "*");
            let nested = c.pseudos.iter().any(|p| matches!(p, Pseudo::Is(_) | Pseudo::Where(_) | Pseudo::Nesting(_)));
            if tag.is_none() && c.id.is_none() && c.classes.is_empty() && c.attrs.is_empty() && !nested {
                self.has_any = true;
            }
            let [tags, ids, classes, attrs] = &mut self.has_features;
            if let Some(t) = tag {
                tags.insert(t.to_ascii_lowercase());
            }
            if let Some(id) = &c.id {
                ids.insert(id.clone());
            }
            for cls in &c.classes {
                classes.insert(cls.clone());
            }
            for a in &c.attrs {
                attrs.insert(a.name.clone());
            }
        }
        for cls in &c.classes {
            *self.classes.entry(cls.clone()).or_default() |= rel;
        }
        if let Some(id) = &c.id {
            *self.ids.entry(id.clone()).or_default() |= rel;
        }
        for a in &c.attrs {
            *self.attrs.entry(a.name.clone()).or_default() |= rel;
        }
        for p in &c.pseudos {
            match p {
                Pseudo::Hover => self.hover.add(c, rel),
                Pseudo::Active => self.active.add(c, rel),
                Pseudo::Focus | Pseudo::FocusVisible | Pseudo::FocusWithin => self.focus.add(c, rel),
                Pseudo::Target => self.target |= rel,
                Pseudo::FirstChild => self.first |= rel,
                Pseudo::LastChild => self.last |= rel,
                Pseudo::OnlyChild => {
                    self.first |= rel;
                    self.last |= rel;
                }
                Pseudo::NthChildOf(_, _, list) | Pseudo::NthLastChildOf(_, _, list) => {
                    if matches!(p, Pseudo::NthChildOf(..)) { self.nth_fwd |= rel; } else { self.nth_back |= rel; }
                    // A filter change can alter sibling membership, including
                    // complex selectors depending on ancestors or :has().
                    for s in list { self.add_selector(s, R_GLOBAL, false); }
                }
                Pseudo::NthChild(..) | Pseudo::NthOfType(..) | Pseudo::FirstOfType => self.nth_fwd |= rel,
                Pseudo::NthLastChild(..) | Pseudo::NthLastOfType(..) | Pseudo::LastOfType => self.nth_back |= rel,
                Pseudo::OnlyOfType => {
                    self.nth_fwd |= rel;
                    self.nth_back |= rel;
                }
                Pseudo::Empty => self.empty |= rel,
                Pseudo::Defined => self.defined |= rel,
                Pseudo::Link | Pseudo::AnyLink => *self.attrs.entry("href".into()).or_default() |= rel,
                Pseudo::Enabled | Pseudo::Disabled => *self.attrs.entry("disabled".into()).or_default() |= rel,
                // Form states read the value, checkedness and validation
                // attributes of the element, and (`:invalid` forms and
                // fieldsets, radio groups, `:default` buttons) of other
                // elements of its form: treated like `:has()` arguments.
                Pseudo::Indeterminate
                | Pseudo::PlaceholderShown
                | Pseudo::Required
                | Pseudo::Optional
                | Pseudo::ReadOnly
                | Pseudo::ReadWrite
                | Pseudo::Valid
                | Pseudo::Invalid
                | Pseudo::InRange
                | Pseudo::OutOfRange
                | Pseudo::Default
                | Pseudo::Modal
                | Pseudo::Open => {
                    let rel = if matches!(p, Pseudo::Valid | Pseudo::Invalid | Pseudo::Indeterminate | Pseudo::Default) { rel | R_HAS } else { rel };
                    for a in ["value", "checked", "placeholder", "required", "readonly", "disabled", "type", "min", "max", "selected", "data-rh-indeterminate", "data-rh-custom-validity", "open", "contenteditable"] {
                        *self.attrs.entry(a.into()).or_default() |= rel;
                    }
                }
                Pseudo::Checked => {
                    *self.attrs.entry("checked".into()).or_default() |= rel;
                    *self.attrs.entry("selected".into()).or_default() |= rel;
                }
                // `:lang()` looks at the nearest ancestor with `lang`.
                Pseudo::Lang(_) => *self.attrs.entry("lang".into()).or_default() |= rel | R_DESC,
                Pseudo::Not(list) | Pseudo::Is(list) | Pseudo::Where(list) => {
                    for s in list {
                        self.add_selector(s, rel, in_has);
                    }
                }
                Pseudo::Nesting(list) => {
                    for s in list.iter() { self.add_selector(s, rel, in_has); }
                }
                Pseudo::Has(list) => {
                    self.has_has = true;
                    self.has_outer |= rel;
                    for (_, s) in list {
                        self.add_selector(s, R_HAS, true);
                    }
                }
                Pseudo::Host(list) | Pseudo::HostContext(list) | Pseudo::Slotted(list) => {
                    for s in list {
                        self.add_selector(s, rel | R_DESC, in_has);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Compounds containing a dynamic pseudo-class, keyed by their tag / id /
/// classes (attributes and other pseudo-classes are ignored, which only
/// over-approximates).
#[derive(Default, Debug)]
struct StateDeps {
    deps: HashMap<(Option<String>, Option<String>, Vec<String>), u8>,
}

impl StateDeps {
    fn add(&mut self, c: &Compound, rel: u8) {
        let tag = c.tag.clone().filter(|t| t != "*");
        *self.deps.entry((tag, c.id.clone(), c.classes.clone())).or_default() |= rel;
    }

    fn is_empty(&self) -> bool {
        self.deps.is_empty()
    }

    /// Combined relation of the compounds `node` may match.
    fn relation(&self, doc: &Document, node: NodeId) -> u8 {
        let Some(el) = doc.element(node) else { return 0 };
        let mut rel = 0;
        for ((tag, id, classes), r) in &self.deps {
            if tag.as_ref().is_some_and(|t| *t != el.tag) || id.as_ref().is_some_and(|i| el.id() != Some(i.as_str())) {
                continue;
            }
            if classes.iter().all(|c| el.has_class(c)) {
                rel |= r;
            }
        }
        rel
    }
}

/// Per-element restyle flags.
const N_SELF: u8 = 1;
const N_SUBTREE: u8 = 2;
/// Some descendant has flags (the walk must go through this element).
const N_DESC: u8 = 4;

struct Marks<'d> {
    doc: &'d Document,
    need: &'d mut Vec<u8>,
    /// The document has shadow trees: marks propagate along the flat tree
    /// (the tree styles are inherited through).
    shadow: bool,
    global: bool,
    /// `:has()` subjects relate to other elements: any `:has()` change is
    /// global.
    has_global: bool,
}

impl Marks<'_> {
    /// The element `node` inherits its style from (flat tree parent).
    fn up(&self, node: NodeId) -> Option<NodeId> {
        if self.shadow {
            self.doc.flat_parent(node)
        } else {
            self.doc.parent(node)
        }
    }

    fn mark(&mut self, node: NodeId, flag: u8) {
        if node >= self.need.len() || self.doc.element(node).is_none() || (self.shadow && self.doc.is_shadow_root(node)) {
            return;
        }
        self.need[node] |= flag;
        let mut cur = self.up(node);
        while let Some(p) = cur {
            if self.need[p] & N_DESC != 0 {
                break;
            }
            self.need[p] |= N_DESC;
            cur = self.up(p);
        }
    }

    fn apply(&mut self, node: NodeId, rel: u8) {
        if rel & R_GLOBAL != 0 {
            self.global = true;
        }
        if rel & R_HAS != 0 {
            self.has_scope(node);
        }
        if rel & R_SELF != 0 {
            self.mark(node, N_SELF);
        }
        if rel & R_DESC != 0 {
            self.mark(node, N_SUBTREE);
        }
        if rel & R_SIB != 0 {
            let mut cur = self.doc.next_sibling(node);
            while let Some(s) = cur {
                self.mark(s, N_SUBTREE);
                cur = self.doc.next_sibling(s);
            }
        }
    }

    /// A `:has()` argument may have changed at `node`: restyle its
    /// ancestors and their preceding siblings (the possible subjects).
    fn has_scope(&mut self, node: NodeId) {
        if self.has_global {
            self.global = true;
            return;
        }
        let mut cur = Some(node);
        while let Some(a) = cur {
            self.mark(a, N_SELF);
            let mut sib = self.doc.prev_sibling(a);
            while let Some(s) = sib {
                self.mark(s, N_SELF);
                sib = self.doc.prev_sibling(s);
            }
            cur = self.doc.parent(a);
        }
    }

    /// After `child` was inserted: mark its following (`forward`) or
    /// preceding element siblings with `all`, and the nearest one with
    /// `nearest` as well.
    fn siblings(&mut self, child: NodeId, forward: bool, all: u8, nearest: u8) {
        if all | nearest == 0 {
            return;
        }
        if (all | nearest) & R_GLOBAL != 0 {
            self.global = true;
        }
        let doc = self.doc;
        let step = |n: NodeId| if forward { doc.next_sibling(n) } else { doc.prev_sibling(n) };
        let mut cur = step(child);
        let mut first = true;
        while let Some(s) = cur {
            cur = step(s);
            if self.doc.element(s).is_none() {
                continue;
            }
            let rel = if first { all | nearest } else { all };
            first = false;
            if rel == 0 {
                break;
            }
            self.mark(s, if rel & !R_SELF != 0 { N_SUBTREE } else { N_SELF });
        }
    }

    /// Mark the element children of `parent` with `rel` (structural and
    /// sibling-combinator dependencies after a child list change).
    fn children(&mut self, parent: NodeId, rel: u8) {
        if rel == 0 {
            return;
        }
        let flag = if rel & !R_SELF != 0 { N_SUBTREE } else { N_SELF };
        if rel & R_GLOBAL != 0 {
            self.global = true;
        }
        let kids: Vec<NodeId> = self.doc.child_elements(parent).collect();
        for k in kids {
            self.mark(k, flag);
        }
    }
}

// ------------------------------------------------------------ the engine

/// One element whose computed style (or pseudo-element styles) changed.
#[derive(Debug, Clone)]
pub struct StyleChange {
    pub node: NodeId,
    pub old: Option<Rc<ComputedStyle>>,
    pub new: Option<Rc<ComputedStyle>>,
    /// `::before` / `::after` / `::marker` changed.
    pub pseudo_changed: bool,
}

/// What [`StyleEngine::update`] did.
#[derive(Debug, Default)]
pub struct RestyleOutcome {
    /// Everything was recomputed (stylesheets, media, `:has()`, log
    /// overflow...): callers should treat every node as changed.
    pub full: bool,
    /// Elements whose style changed (empty when `full`).
    pub changes: Vec<StyleChange>,
    /// Elements whose style was recomputed (for statistics).
    pub recomputed: usize,
    /// Elements whose flat-tree children changed without a mutation of
    /// their own child list (shadow root attached, slot assignment
    /// changed): their boxes must be rebuilt with their children's.
    pub flat_changed: Vec<NodeId>,
}

/// Persistent style resolution state for one document.
pub struct StyleEngine {
    index: ScopeIndex,
    /// Upgraded custom elements when styles were computed (`:defined`).
    defined_seen: HashSet<NodeId>,
    /// Shadow host -> root when styles were computed.
    shadow_seen: HashMap<NodeId, NodeId>,
    /// Shadow roots whose sheets changed since the last update.
    pending_roots: Vec<NodeId>,
    /// `@keyframes` / `@font-face` must be collected again.
    aux_dirty: bool,
    inval: InvalidationMap,
    env: MediaEnv,
    /// Custom properties per element (inherited by children).
    vars: Vec<Option<Rc<Vars>>>,
    /// Element uses `inherit` on a non-inherited property.
    inherit_dep: Vec<bool>,
    need: Vec<u8>,
    decls: DeclCache,
    /// Dynamic state the current styles were computed with.
    state: MatchState,
    /// Styles exist for the document (a full pass ran).
    ready: bool,
}

fn env_eq(a: &MediaEnv, b: &MediaEnv) -> bool {
    a.width == b.width && a.height == b.height && a.screen == b.screen && a.scripting == b.scripting
}

fn display_class(d: Display) -> u8 {
    match d {
        Display::Flex | Display::InlineFlex => 1,
        Display::Grid | Display::InlineGrid => 2,
        _ => 0,
    }
}

/// Parent change bits passed down the walk.
const P_ANY: u8 = 1;
const P_INH: u8 = 2;
const P_DISP: u8 = 4;

impl StyleEngine {
    pub fn new(author: &[Stylesheet], env: &MediaEnv) -> StyleEngine {
        let index = ScopeIndex::build(author, env);
        let inval = InvalidationMap::build(&index);
        StyleEngine {
            index,
            defined_seen: HashSet::new(),
            shadow_seen: HashMap::new(),
            pending_roots: Vec::new(),
            aux_dirty: false,
            inval,
            env: *env,
            vars: Vec::new(),
            inherit_dep: Vec::new(),
            need: Vec::new(),
            decls: DeclCache::default(),
            state: MatchState::default(),
            ready: false,
        }
    }

    /// Does this engine match the given media environment?
    pub fn env_matches(&self, env: &MediaEnv) -> bool {
        env_eq(&self.env, env)
    }

    /// Dynamic state the current styles were computed with.
    pub fn state(&self) -> &MatchState {
        &self.state
    }

    /// Does any rule use `:hover`/`:focus`/`:active`/`:target`?
    pub fn has_dynamic_rules(&self) -> bool {
        !self.inval.hover.is_empty() || !self.inval.focus.is_empty() || !self.inval.active.is_empty() || self.inval.target != 0
    }

    /// Style the whole document from scratch.
    pub fn full(&mut self, doc: &Document, author: &[Stylesheet], opts: &StyleOptions) -> StyledTree {
        let mut tree = StyledTree::empty();
        self.collect_aux(&mut tree, author, opts);
        self.shadow_seen = doc.shadow_roots.clone();
        self.pending_roots.clear();
        let n = doc.nodes.len();
        tree.styles = vec![None; n];
        tree.before = vec![None; n];
        tree.after = vec![None; n];
        tree.marker = vec![None; n];
        self.vars = vec![None; n];
        self.inherit_dep = vec![false; n];
        self.need = vec![0; n];
        self.state = opts.state.clone();
        self.ready = true;
        self.defined_seen = doc.defined_custom.clone();
        self.walk(doc, &mut tree, opts, true, &mut Vec::new());
        tree
    }

    /// `@font-face` rules and `@keyframes` of `author`.
    fn collect_aux(&mut self, tree: &mut StyledTree, author: &[Stylesheet], opts: &StyleOptions) {
        tree.font_faces = collect_font_faces(author, &self.env, opts);
        tree.keyframes.clear();
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
        self.aux_dirty = false;
    }

    /// Is there work for [`StyleEngine::update`] that the mutation log
    /// does not show (sheets of shadow roots changed, shadow roots
    /// attached, custom elements upgraded)?
    pub fn has_pending(&self, doc: &Document) -> bool {
        !self.pending_roots.is_empty() || self.aux_dirty || doc.shadow_roots.len() != self.shadow_seen.len() || doc.defined_custom.len() != self.defined_seen.len()
    }

    /// The author sheets were collected again. When the document-level
    /// sheets are the same ones (by [`Stylesheet::source_id`]) and only
    /// shadow-root scoped sheets differ, the engine is updated in place and
    /// the hosts of the affected shadow roots are restyled by the next
    /// [`StyleEngine::update`]. Returns false when the engine cannot follow
    /// (the caller must build a new one).
    pub fn set_sheets(&mut self, author: &[Stylesheet]) -> bool {
        if !self.ready || author.iter().any(|s| s.source_id == 0) {
            return false;
        }
        let doc_sources = author.iter().filter(|s| s.shadow_scopes.is_empty()).map(|s| s.source_id);
        if !doc_sources.eq(self.index.doc_sources.iter().copied()) {
            return false;
        }
        let old = std::mem::take(&mut self.index.root_sources);
        let added = self.index.rebuild_shadow(author, &self.env);
        self.inval.add_groups(&self.index, &added);
        for (root, list) in &self.index.root_sources {
            if old.get(root) != Some(list) {
                self.pending_roots.push(*root);
            }
        }
        for root in old.keys() {
            if !self.index.root_sources.contains_key(root) {
                self.pending_roots.push(*root);
            }
        }
        self.aux_dirty = true;
        true
    }

    /// Bring `tree` up to date after the DOM mutations in `log` and a
    /// dynamic state change to `opts.state`.
    pub fn update(&mut self, doc: &Document, tree: &mut StyledTree, log: &MutationLog, opts: &StyleOptions, author: &[Stylesheet]) -> RestyleOutcome {
        if !self.ready || log.overflow {
            *tree = self.full(doc, author, opts);
            return RestyleOutcome { full: true, ..Default::default() };
        }
        if self.aux_dirty {
            self.collect_aux(tree, author, opts);
        }
        let n = doc.nodes.len();
        for v in [&mut tree.styles, &mut tree.before, &mut tree.after, &mut tree.marker] {
            if v.len() < n {
                v.resize(n, None);
            }
        }
        if self.vars.len() < n {
            self.vars.resize(n, None);
            self.inherit_dep.resize(n, false);
        }
        self.need.clear();
        self.need.resize(n, 0);
        let mut flat_changed = Vec::new();
        let global = self.invalidate(doc, tree, log, opts.state, &mut flat_changed);
        self.state = opts.state.clone();
        if global {
            *tree = self.full(doc, author, opts);
            return RestyleOutcome { full: true, ..Default::default() };
        }
        let mut changes = Vec::new();
        let recomputed = self.walk(doc, tree, opts, false, &mut changes);
        RestyleOutcome { full: false, changes, recomputed, flat_changed }
    }

    /// Turn mutations and state changes into restyle marks; returns true
    /// when everything must be recomputed. Elements whose flat-tree
    /// children changed through shadow DOM are added to `flat_changed`.
    fn invalidate(&mut self, doc: &Document, tree: &mut StyledTree, log: &MutationLog, new_state: &MatchState, flat_changed: &mut Vec<NodeId>) -> bool {
        let inval = &self.inval;
        let has_global = inval.has_outer & !R_SELF != 0;
        let shadow = !doc.shadow_roots.is_empty() || !self.shadow_seen.is_empty();
        let mut m = Marks { doc, need: &mut self.need, shadow, global: false, has_global };
        let mut removed: Vec<NodeId> = Vec::new();
        // Shadow DOM bookkeeping: hosts whose light children changed (slot
        // assignment / fallback content), hosts whose slots changed, and
        // light children whose `slot` attribute changed.
        let mut hosts_light: Vec<NodeId> = Vec::new();
        let mut hosts_slots: Vec<NodeId> = Vec::new();
        let mut reassigned: Vec<NodeId> = Vec::new();
        // Children logged more than once (moved): their old position is
        // unknown.
        let mut moved: HashMap<NodeId, u32> = HashMap::new();
        if inval.first | inval.last | inval.nth_fwd | inval.nth_back | inval.sibling_follow != 0 {
            for mu in &log.entries {
                if let Mutation::Children { child, .. } = mu {
                    *moved.entry(*child).or_default() += 1;
                }
            }
        }
        let has_slot = |n: NodeId| doc.element(n).is_some() && doc.descendants(n).any(|d| doc.is_element(d, "slot"));
        let host_of_tree = |n: NodeId| doc.containing_shadow_root(n).and_then(|r| doc.shadow_host(r));
        for mu in &log.entries {
            if shadow {
                match mu {
                    Mutation::Children { parent, child } => {
                        if let Some(host) = doc.shadow_host(*parent) {
                            if has_slot(*child) {
                                hosts_slots.push(host);
                            }
                        } else if doc.shadow_root(*parent).is_some() {
                            hosts_light.push(*parent);
                        } else if has_slot(*child) {
                            if let Some(h) = host_of_tree(*parent) {
                                hosts_slots.push(h);
                            }
                        }
                    }
                    Mutation::Attr { node, .. } | Mutation::Node { node } => {
                        let name = match mu {
                            Mutation::Attr { name, .. } => Some(name.as_str()),
                            _ => None,
                        };
                        if name.map_or(true, |n| n == "slot") {
                            if let Some(p) = doc.parent(*node).filter(|&p| doc.shadow_root(p).is_some()) {
                                hosts_light.push(p);
                                reassigned.push(*node);
                            }
                        }
                        if name.map_or(true, |n| n == "name") && doc.is_element(*node, "slot") {
                            if let Some(h) = host_of_tree(*node) {
                                hosts_slots.push(h);
                            }
                        }
                    }
                    Mutation::Text { node } => {
                        if let Some(p) = doc.parent(*node).filter(|&p| doc.shadow_root(p).is_some()) {
                            hosts_light.push(p);
                        }
                    }
                }
            }
            match mu {
                Mutation::Attr { node, name, old } => {
                    let node = *node;
                    if !doc.is_connected(node) {
                        continue;
                    }
                    m.mark(node, N_SELF);
                    let mut rel = inval.attrs.get(name.as_str()).copied().unwrap_or(0);
                    let new = doc.attr(node, name);
                    match name.as_str() {
                        "class" => {
                            let o: HashSet<&str> = old.as_deref().unwrap_or("").split_ascii_whitespace().collect();
                            let nw: HashSet<&str> = new.unwrap_or("").split_ascii_whitespace().collect();
                            for c in o.symmetric_difference(&nw) {
                                rel |= inval.classes.get(*c).copied().unwrap_or(0);
                            }
                        }
                        "id" => {
                            for id in [old.as_deref(), new].into_iter().flatten() {
                                rel |= inval.ids.get(id).copied().unwrap_or(0);
                            }
                            if new_state.target_id.is_some() {
                                rel |= inval.target;
                            }
                        }
                        _ => {}
                    }
                    m.apply(node, rel);
                }
                Mutation::Children { parent, child } => {
                    let (parent, child) = (*parent, *child);
                    if doc.is_connected(child) && doc.parent(child) == Some(parent) {
                        m.mark(child, N_SUBTREE);
                    } else {
                        removed.push(child);
                    }
                    if !doc.is_connected(parent) {
                        continue;
                    }
                    if inval.has_has && inval.may_affect_has(doc, child) {
                        // The inserted/removed subtree may satisfy or stop
                        // satisfying a `:has()` of the parent chain.
                        m.has_scope(parent);
                        m.children(parent, R_SELF);
                    }
                    if doc.element(child).is_some() {
                        if doc.parent(child) == Some(parent) && moved.get(&child).is_none_or(|&c| c < 2) {
                            // Inserted: only siblings after it change their
                            // index, only those before it their index from
                            // the end.
                            m.siblings(child, true, inval.nth_fwd | inval.sibling_follow, inval.first);
                            m.siblings(child, false, inval.nth_back, inval.last);
                        } else {
                            m.children(parent, inval.first | inval.last | inval.nth_fwd | inval.nth_back | inval.sibling_follow);
                        }
                    }
                    if inval.empty != 0 {
                        m.apply(parent, inval.empty);
                    }
                    if doc.element(child).is_none() && doc.element(parent).is_some() {
                        // Text inserted/removed: only `:empty` can notice.
                        continue;
                    }
                }
                Mutation::Text { node } => {
                    if let Some(p) = doc.parent(*node) {
                        if inval.empty != 0 && doc.is_connected(p) {
                            m.apply(p, inval.empty);
                        }
                    }
                }
                Mutation::Node { node } => {
                    let node = *node;
                    if !doc.is_connected(node) {
                        continue;
                    }
                    if doc.element(node).is_some() {
                        m.apply(node, R_SELF | R_DESC | R_SIB);
                        if inval.has_has {
                            m.has_scope(node);
                        }
                    } else if let Some(p) = doc.parent(node) {
                        if inval.empty != 0 {
                            m.apply(p, inval.empty);
                        }
                    }
                }
            }
        }
        // Dynamic state: elements whose :hover/:active/:focus match flips
        // are the ancestor chains' symmetric difference.
        let old_state = &self.state;
        for (old, new, deps) in [
            (old_state.hovered, new_state.hovered, &inval.hover),
            (old_state.active, new_state.active, &inval.active),
            (old_state.focused, new_state.focused, &inval.focus),
        ] {
            if old == new || deps.is_empty() {
                continue;
            }
            let chain = |n: Option<NodeId>| -> Vec<NodeId> {
                let mut v = Vec::new();
                let mut cur = n.filter(|&n| n < doc.nodes.len());
                while let Some(c) = cur {
                    v.push(c);
                    cur = doc.parent(c);
                }
                v
            };
            let (a, b) = (chain(old), chain(new));
            let sa: HashSet<NodeId> = a.iter().copied().collect();
            let sb: HashSet<NodeId> = b.iter().copied().collect();
            for &e in a.iter().filter(|e| !sb.contains(e)).chain(b.iter().filter(|e| !sa.contains(e))) {
                if doc.is_connected(e) {
                    let rel = deps.relation(doc, e);
                    if rel != 0 {
                        m.apply(e, rel);
                    }
                }
            }
        }
        // Links that became visited (or not): `:link` / `:visited` flip.
        if old_state.visited != new_state.visited {
            for &e in old_state.visited.symmetric_difference(&new_state.visited) {
                if e < doc.nodes.len() && doc.is_connected(e) {
                    m.apply(e, R_SELF | R_DESC | R_SIB);
                }
            }
        }
        if old_state.target_id != new_state.target_id && inval.target != 0 {
            for id in [old_state.target_id.as_deref(), new_state.target_id.as_deref()].into_iter().flatten() {
                if let Some(e) = doc.element_by_id(id) {
                    m.apply(e, inval.target);
                }
            }
        }
        if shadow {
            // Newly attached shadow roots: the host's flat children are now
            // its shadow tree (and its `:host` rules apply).
            if doc.shadow_roots.len() != self.shadow_seen.len() {
                for (&host, &root) in &doc.shadow_roots {
                    if self.shadow_seen.get(&host) != Some(&root) && doc.is_connected(host) {
                        hosts_slots.push(host);
                        m.mark(host, N_SUBTREE);
                        flat_changed.push(host);
                    }
                }
                self.shadow_seen = doc.shadow_roots.clone();
            }
            // Shadow roots whose sheets changed.
            for root in self.pending_roots.drain(..) {
                if let Some(h) = doc.shadow_host(root).filter(|&h| doc.is_connected(h)) {
                    m.mark(h, N_SUBTREE);
                }
            }
            // Slots added, removed or renamed: every light child may have
            // moved to another slot (or out of the flat tree).
            hosts_slots.sort_unstable();
            hosts_slots.dedup();
            for &h in &hosts_slots {
                if !doc.is_connected(h) {
                    continue;
                }
                reassigned.extend(doc.children(h));
                hosts_light.push(h);
            }
            for &c in &reassigned {
                if doc.element(c).is_none() || !doc.is_connected(c) {
                    continue;
                }
                if doc.assigned_slot(c).is_some() {
                    m.mark(c, N_SUBTREE);
                } else {
                    clear_styles(doc, tree, c);
                }
            }
            // Light children changed: a slot's fallback content is rendered
            // exactly when nothing is assigned to it.
            hosts_light.sort_unstable();
            hosts_light.dedup();
            for &h in &hosts_light {
                let Some(root) = doc.shadow_root(h) else { continue };
                if !doc.is_connected(h) {
                    continue;
                }
                flat_changed.push(h);
                let slots: Vec<NodeId> = doc.descendants(root).filter(|&d| doc.is_element(d, "slot")).collect();
                for s in slots {
                    flat_changed.push(s);
                    if doc.assigned_nodes(s).is_empty() {
                        m.mark(s, N_SUBTREE);
                    } else {
                        let fallback: Vec<NodeId> = doc.children(s).collect();
                        for c in fallback {
                            clear_styles(doc, tree, c);
                        }
                    }
                }
            }
        }
        // Custom elements upgraded (or failed): `:defined` flips.
        if doc.defined_custom.len() != self.defined_seen.len() || (inval.defined != 0 && doc.defined_custom != self.defined_seen) {
            if inval.defined != 0 {
                for &e in doc.defined_custom.symmetric_difference(&self.defined_seen) {
                    if e < doc.nodes.len() && doc.is_connected(e) {
                        m.apply(e, inval.defined);
                    }
                }
            }
            self.defined_seen = doc.defined_custom.clone();
        }
        let global = m.global;
        // Removed subtrees lose their styles (unless re-inserted, which
        // marked them for restyle above).
        for r in removed {
            if doc.is_connected(r) {
                continue;
            }
            if shadow {
                clear_styles(doc, tree, r);
                continue;
            }
            for d in doc.descendants(r) {
                if d < tree.styles.len() {
                    tree.styles[d] = None;
                    tree.before[d] = None;
                    tree.after[d] = None;
                    tree.marker[d] = None;
                }
            }
        }
        global
    }

    /// Walk from the root, (re)computing marked elements (all when `all`).
    /// Returns how many elements were recomputed.
    fn walk(&mut self, doc: &Document, tree: &mut StyledTree, opts: &StyleOptions, all: bool, changes: &mut Vec<StyleChange>) -> usize {
        let Some(html) = doc.html_element() else { return 0 };
        enum Step {
            Enter { node: NodeId, parent: Rc<ComputedStyle>, pvars: Rc<Vars>, root_fs: f32, force: bool, pchange: u8 },
            Exit(NodeId),
        }
        let matcher = SelectorMatcher::new(doc, opts.state);
        let mut bloom = AncestorBloom::default();
        let mut candidates = Vec::new();
        let mut matched: Vec<Matched> = Vec::new();
        let mut decls = std::mem::take(&mut self.decls);
        let mut share = ShareCache::default();
        let root_parent = Rc::new(ComputedStyle::initial());
        let mut stack = vec![Step::Enter { node: html, parent: root_parent, pvars: Rc::new(HashMap::new()), root_fs: 16.0, force: all, pchange: 0 }];
        let mut recomputed = 0;
        while let Some(step) = stack.pop() {
            let (node, parent, pvars, root_fs, force, pchange) = match step {
                Step::Exit(n) => {
                    bloom.pop(doc, n);
                    continue;
                }
                Step::Enter { node, parent, pvars, root_fs, force, pchange } => (node, parent, pvars, root_fs, force, pchange),
            };
            if doc.element(node).is_none() {
                continue;
            }
            let flags = self.need[node];
            let mut force = force || flags & N_SUBTREE != 0;
            let old = tree.styles[node].clone();
            let old_hidden = old.as_ref().map(|s| s.display.is_none()).unwrap_or(true);
            let recompute = force
                || flags & N_SELF != 0
                || old.is_none()
                || pchange & (P_INH | P_DISP) != 0
                || (pchange & P_ANY != 0 && self.inherit_dep[node]);
            let mut child_change = 0u8;
            let mut root_fs = root_fs;
            let (style, vars) = if recompute {
                recomputed += 1;
                let cx = MatchCtx { scopes: &self.index, env: &self.env, opts, matcher: &matcher, bloom: Some(&bloom) };
                let out: ElementOut = compute_element(doc, node, &parent, &pvars, root_fs, &cx, &mut decls, Some(&mut share), &mut candidates, &mut matched);
                self.inherit_dep[node] = out.inherit_dep;
                if node == html {
                    if old.as_ref().map(|o| o.font_size != out.style.font_size).unwrap_or(false) {
                        // rem units everywhere depend on it.
                        force = true;
                    }
                    root_fs = out.style.font_size;
                    tree.root_font_size = root_fs;
                }
                let style = match &old {
                    Some(o) if Rc::ptr_eq(o, &out.style) || **o == *out.style => o.clone(),
                    _ => {
                        if let Some(o) = &old {
                            child_change |= P_ANY;
                            if !o.inherited_eq(&out.style) {
                                child_change |= P_INH;
                            }
                            if display_class(o.display) != display_class(out.style.display) {
                                child_change |= P_DISP;
                            }
                        }
                        out.style
                    }
                };
                let vars = match &self.vars[node] {
                    Some(v) if Rc::ptr_eq(v, &out.vars) || **v == *out.vars => v.clone(),
                    _ => {
                        if self.vars[node].is_some() {
                            child_change |= P_INH;
                        }
                        out.vars
                    }
                };
                self.vars[node] = Some(vars.clone());
                let hidden = style.display.is_none();
                let mut pseudo_changed = false;
                for (slot, new) in [
                    (&mut tree.before[node], if hidden { None } else { out.before }),
                    (&mut tree.after[node], if hidden { None } else { out.after }),
                    (&mut tree.marker[node], if hidden { None } else { out.marker }),
                ] {
                    match (slot.as_ref(), new) {
                        (Some(o), Some(nw)) if Rc::ptr_eq(o, &nw) || **o == *nw => {}
                        (None, None) => {}
                        (_, nw) => {
                            *slot = nw;
                            pseudo_changed = true;
                        }
                    }
                }
                let changed = !old.as_ref().map(|o| Rc::ptr_eq(o, &style)).unwrap_or(false);
                if !all && (changed || pseudo_changed) {
                    changes.push(StyleChange { node, old: old.clone(), new: Some(style.clone()), pseudo_changed });
                }
                tree.styles[node] = Some(style.clone());
                (style, vars)
            } else {
                let vars = self.vars[node].clone().unwrap_or_else(|| pvars.clone());
                (old.clone().expect("visible element has a style"), vars)
            };
            if style.display.is_none() {
                if !all && (!old_hidden || flags & N_DESC != 0) {
                    // Descendants are not rendered (it just got hidden, or
                    // elements were inserted below a hidden one).
                    let below: Vec<NodeId> = if doc.shadow_roots.is_empty() { doc.descendants(node).skip(1).collect() } else { doc.shadow_including_descendants(node).into_iter().skip(1).collect() };
                    for d in below {
                        if d < tree.styles.len() && tree.styles[d].is_some() {
                            tree.styles[d] = None;
                            tree.before[d] = None;
                            tree.after[d] = None;
                            tree.marker[d] = None;
                        }
                    }
                }
                continue;
            }
            let descend = force || child_change != 0 || flags & N_DESC != 0 || old_hidden;
            if !descend {
                continue;
            }
            bloom.push(doc, node);
            stack.push(Step::Exit(node));
            // Only passing through to marked descendants: children that are
            // neither marked nor unstyled would not be recomputed.
            let only_marked = !force && child_change == 0 && !old_hidden;
            let kids: Vec<NodeId> = doc
                .flat_child_elements(node)
                .into_iter()
                .filter(|&k| !only_marked || self.need[k] != 0 || tree.styles[k].is_none())
                .collect();
            for &k in kids.iter().rev() {
                stack.push(Step::Enter { node: k, parent: style.clone(), pvars: vars.clone(), root_fs, force, pchange: child_change });
            }
        }
        self.decls = decls;
        self.need.iter_mut().for_each(|f| *f = 0);
        recomputed
    }

    /// Style of one element computed from scratch along its ancestor chain
    /// (no stored state used), also inside `display: none` subtrees (as
    /// `getComputedStyle` needs). `None` when detached.
    pub fn style_chain(&self, doc: &Document, opts: &StyleOptions, target: NodeId) -> Option<Rc<ComputedStyle>> {
        self.style_chain_from(doc, opts, target, None)
    }

    /// [`StyleEngine::style_chain`] starting below the nearest ancestor
    /// whose style is stored in `tree` (the up-to-date styles this engine
    /// computed), so only the unrendered part of the chain is computed.
    pub fn style_chain_from(&self, doc: &Document, opts: &StyleOptions, target: NodeId, tree: Option<&StyledTree>) -> Option<Rc<ComputedStyle>> {
        let html = doc.html_element()?;
        let mut chain = Vec::new();
        let mut cur = Some(target);
        let mut start: Option<(Rc<ComputedStyle>, Rc<Vars>)> = None;
        while let Some(n) = cur {
            if doc.element(n).is_some() {
                if n != target {
                    if let (Some(t), Some(Some(v))) = (tree, self.vars.get(n)) {
                        if let Some(st) = t.get(n) {
                            start = Some((st.clone(), v.clone()));
                            break;
                        }
                    }
                }
                chain.push(n);
            }
            if n == html {
                break;
            }
            cur = doc.flat_parent(n);
        }
        if start.is_none() && chain.last() != Some(&html) {
            return None; // detached
        }
        chain.reverse();
        let mut candidates = Vec::new();
        let mut matched: Vec<Matched> = Vec::new();
        let mut decls = DeclCache::default();
        let (mut parent, mut vars, mut root_fs) = match start {
            Some((st, v)) => (st, v, tree.map(|t| t.root_font_size).unwrap_or(16.0)),
            None => (Rc::new(ComputedStyle::initial()), Rc::new(HashMap::new()), 16.0),
        };
        let matcher = SelectorMatcher::new(doc, opts.state);
        let cx = MatchCtx { scopes: &self.index, env: &self.env, opts, matcher: &matcher, bloom: None };
        for &node in &chain {
            let out = compute_element(doc, node, &parent, &vars, root_fs, &cx, &mut decls, None, &mut candidates, &mut matched);
            if node == html {
                root_fs = out.style.font_size;
            }
            let style = out.style;
            vars = out.vars;
            parent = style;
        }
        Some(parent)
    }
}

/// Drop the styles of `node` and everything below it, shadow trees
/// included (it left the flat tree).
fn clear_styles(doc: &Document, tree: &mut StyledTree, node: NodeId) {
    for d in doc.shadow_including_descendants(node) {
        if d < tree.styles.len() {
            tree.styles[d] = None;
            tree.before[d] = None;
            tree.after[d] = None;
            tree.marker[d] = None;
        }
    }
}

fn collect_font_faces(author: &[Stylesheet], env: &MediaEnv, opts: &StyleOptions) -> Vec<FontFace> {
    crate::cascade::collect_font_faces(author, env, opts)
}
