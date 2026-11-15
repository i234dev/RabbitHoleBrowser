//! HTML option selectedness/dirtiness. Attributes represent defaults only.
use super::{Document, Mutation, NodeId};

#[derive(Clone, Copy)]
pub(super) struct OptionState {
    selected: bool,
    dirty: bool,
}

impl Document {
    /// The HTML list of options excludes nested select/datalist/option subtrees.
    pub fn select_options(&self, select: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack: Vec<_> = self.children(select).collect();
        stack.reverse();
        while let Some(n) = stack.pop() {
            match self.tag(n) {
                Some("option") => out.push(n),
                Some("select" | "datalist" | "hr") => continue,
                _ => {
                    let mut children: Vec<_> = self.children(n).collect();
                    children.reverse();
                    stack.extend(children);
                }
            }
        }
        out
    }

    fn option_select(&self, option: NodeId) -> Option<NodeId> {
        let mut p = self.parent(option);
        while let Some(n) = p {
            match self.tag(n) {
                Some("select") => return Some(n),
                Some("option" | "datalist" | "hr") => return None,
                _ => p = self.parent(n),
            }
        }
        None
    }

    pub fn is_option_selected(&self, option: NodeId) -> bool {
        self.option_states.get(&option).map_or_else(|| self.attr(option, "selected").is_some(), |s| s.selected)
    }

    pub fn option_disabled(&self, option: NodeId) -> bool {
        self.attr(option, "disabled").is_some()
            || self.parent(option).is_some_and(|p| self.is_element(p, "optgroup") && self.attr(p, "disabled").is_some())
    }

    fn write_option_selected(&mut self, option: NodeId, selected: bool, dirty: bool) {
        let old = self.is_option_selected(option);
        let state = self.option_states.entry(option).or_insert(OptionState { selected: old, dirty: false });
        state.selected = selected;
        state.dirty |= dirty;
        if old != selected {
            // Same invalidation signal as checkedness; no attribute is changed.
            self.record(Mutation::Attr { node: option, name: "selected".into(), old: old.then(String::new) });
        }
    }

    /// Option() overrides initial selectedness without setting dirtiness.
    pub fn initialize_option_selected(&mut self, option: NodeId, selected: bool) {
        self.write_option_selected(option, selected, false);
    }

    fn deselect_other_options(&mut self, select: NodeId, chosen: NodeId) {
        if self.attr(select, "multiple").is_none() {
            for n in self.select_options(select) {
                if n != chosen { self.write_option_selected(n, false, false); }
            }
        }
    }

    /// The option.selected setter (also used by native multiple-selection UI).
    pub fn set_option_selected(&mut self, option: NodeId, selected: bool) {
        self.write_option_selected(option, selected, true);
        if let Some(select) = self.option_select(option) {
            if selected { self.deselect_other_options(select, option); }
            self.normalize_select(select);
        }
    }

    /// select.value/selectedIndex select only the first match, even on multiple.
    /// A missing match leaves no selection; automatic fallback must not run here.
    pub fn set_select_index(&mut self, select: NodeId, index: Option<usize>) {
        for (i, n) in self.select_options(select).into_iter().enumerate() {
            let chosen = Some(i) == index;
            self.write_option_selected(n, chosen, chosen);
        }
    }

    pub fn reset_select(&mut self, select: NodeId) {
        for n in self.select_options(select) {
            let selected = self.attr(n, "selected").is_some();
            self.write_option_selected(n, selected, false);
            self.option_states.get_mut(&n).unwrap().dirty = false;
        }
        self.normalize_select(select);
    }

    fn normalize_select(&mut self, select: NodeId) {
        if self.attr(select, "multiple").is_some() { return; }
        let options = self.select_options(select);
        let mut last = None;
        for &n in &options {
            if self.is_option_selected(n) {
                if let Some(previous) = last { self.write_option_selected(previous, false, false); }
                last = Some(n);
            }
        }
        let size = self.attr(select, "size").and_then(|s| s.parse::<u32>().ok()).filter(|&s| s > 0).unwrap_or(1);
        if last.is_none() && size == 1 {
            if let Some(n) = options.into_iter().find(|&n| !self.option_disabled(n)) {
                self.write_option_selected(n, true, false);
            }
        }
    }

    pub(super) fn select_tree_changed(&mut self, parent: NodeId, child: NodeId, inserted: bool) {
        let select = if self.is_element(parent, "select") { Some(parent) } else { self.option_select(parent) };
        if let Some(select) = select {
            if !self.descendants(child).any(|n| self.is_element(n, "option")) { return; }
            if inserted {
                let root = child;
                let selected: Vec<_> = self.select_options(select).into_iter()
                    .filter(|&n| (n == root || self.is_ancestor(root, n)) && self.is_option_selected(n)).collect();
                for n in selected { self.deselect_other_options(select, n); }
            }
            self.normalize_select(select);
        }
    }

    pub(super) fn select_attribute_changed(&mut self, node: NodeId, name: &str, had: bool, has: bool) {
        if self.is_element(node, "option") && name == "selected" && had != has {
            let dirty = self.option_states.get(&node).is_some_and(|s| s.dirty);
            if !dirty {
                self.write_option_selected(node, has, false);
                if has {
                    if let Some(select) = self.option_select(node) { self.deselect_other_options(select, node); }
                }
            }
            if let Some(select) = self.option_select(node) { self.normalize_select(select); }
        } else if self.is_element(node, "select") && matches!(name, "multiple" | "size") {
            self.normalize_select(node);
        } else if matches!(self.tag(node), Some("option" | "optgroup")) && name == "disabled" {
            if let Some(select) = self.option_select(node) { self.normalize_select(select); }
        }
    }
}
