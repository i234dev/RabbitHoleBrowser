//! Keyboard default actions shared by the GUI, frame routing and automation.
use crate::scripting::{input_event_init, EngineEnv, ScriptOutcome};
use crate::{Engine, Modifiers, Page};
use rabbithole_script::EventDetail;

impl Engine {
    /// Called after keydown. A handled key consumes the GUI default even
    /// if a beforeinput listener prevents the edit or moves focus away.
    pub fn editable_key_default(&self, page: &mut Page, key: &str, mods: Modifiers) -> Option<ScriptOutcome> {
        let focused = page.state.focused?;
        if matches!(page.doc.tag(focused), Some("input" | "textarea" | "select" | "button")) { return None; }
        let target = page.doc.editing_host(focused)?;
        let shortcut = mods.ctrl || mods.meta;
        if mods.alt { return None; }
        let (command, input_type, text) = match key {
            "Enter" if !shortcut && mods.shift => ("insertlinebreak", "insertLineBreak", None),
            "Enter" if !shortcut => ("insertparagraph", "insertParagraph", None),
            "Backspace" if !shortcut => ("delete", "deleteContentBackward", None),
            "Delete" if !shortcut => ("forwarddelete", "deleteContentForward", None),
            "b" | "B" if shortcut => ("bold", "formatBold", None),
            "i" | "I" if shortcut => ("italic", "formatItalic", None),
            "u" | "U" if shortcut => ("underline", "formatUnderline", None),
            k if !shortcut && k.chars().count() == 1 && !k.chars().any(char::is_control) => ("inserttext", "insertText", Some(k)),
            _ => return None,
        };
        let Some(selection) = page.doc.selection() else { return Some(ScriptOutcome::default()); };
        if [selection.start, selection.end].iter().any(|p| page.doc.editing_host(p.node) != Some(target)) {
            return Some(ScriptOutcome::default());
        }
        self.ensure_script_host(page);
        let mut init = input_event_init(input_type, text, true);
        if let Some(EventDetail::Input { target_range, .. }) = &mut init.detail {
            let (mut start, mut end) = (selection.start, selection.end);
            if start == end && matches!(command, "delete" | "forwarddelete") {
                let forward = command == "forwarddelete";
                let next = page.moved_position(start, forward, false);
                let cell = |n| page.doc.ancestor_or_self(n, |p| matches!(page.doc.tag(p), Some("td" | "th")));
                if page.doc.editing_host(next.node) == Some(target) && cell(next.node) == cell(start.node) {
                    if forward { end = next; } else { start = next; }
                }
            }
            if matches!(command, "delete" | "forwarddelete") {
                self.widen_deletion(page, &mut start, &mut end);
            }
            *target_range = Some((start, end));
        }
        let mut out = self.dispatch_event(page, target, "beforeinput", init);
        if out.default_prevented || out.navigation.is_some() || page.state.focused != Some(focused) || page.doc.editing_host(focused) != Some(target) {
            return Some(out);
        }
        let mut host = page.script.host.take()?;
        let changed = {
            let mut env = EngineEnv { engine: self, page };
            host.realm().begin_task();
            let result = host.user_edit(&mut env, target, command, text.unwrap_or(""));
            host.run_jobs(&mut env);
            host.realm().set_deadline(None);
            result
        };
        page.script.host = Some(host);
        if changed {
            out.merge(self.dispatch_event(page, target, "input", input_event_init(input_type, text, false)));
        } else {
            out.merge(self.settle(page, false));
        }
        Some(out)
    }

    /// Like other browsers, a deletion's target range covers the collapsible
    /// spaces that the edit removes because they end up invisible at the
    /// edge of a block, and a selection that ends before the only line break
    /// of a block covers it.
    fn widen_deletion(&self, page: &mut Page, start: &mut rabbithole_dom::BoundaryPoint, end: &mut rabbithole_dom::BoundaryPoint) {
        let is_space = |u: u16| u == 0x20 || u == 0x09 || u == 0x0a;
        // A text node that starts (or ends) a block with collapsible spaces.
        let at_block_edge = |engine: &Self, page: &mut Page, text: rabbithole_dom::NodeId, leading: bool| -> bool {
            let sibling = if leading { page.doc.prev_sibling(text) } else { page.doc.next_sibling(text) };
            if sibling.is_some() {
                return false;
            }
            let Some(parent) = page.doc.parent(text) else { return false };
            let Some(style) = engine.computed_style_now(page, parent) else { return false };
            style.display.is_block_level() && style.white_space.collapses()
        };
        if let Some(units) = page.doc.text(start.node).map(|t| t.encode_utf16().collect::<Vec<u16>>()) {
            if start.offset > 0 && start.offset <= units.len() && units[..start.offset].iter().all(|&u| is_space(u))
                && at_block_edge(self, page, start.node, true) {
                start.offset = 0;
            }
        }
        if let Some(units) = page.doc.text(end.node).map(|t| t.encode_utf16().collect::<Vec<u16>>()) {
            let len = units.len();
            if end.offset < len && units[end.offset..].iter().all(|&u| is_space(u)) && at_block_edge(self, page, end.node, false) {
                end.offset = len;
            } else if end.offset > 0 && end.offset < len && units[..end.offset].iter().all(|&u| is_space(u))
                && is_space(units[end.offset]) && at_block_edge(self, page, end.node, true) {
                // Leading spaces of a block joined to the previous one.
                let mut offset = end.offset;
                while offset < len && is_space(units[offset]) {
                    offset += 1;
                }
                end.offset = offset;
            }
        } else if page.doc.text(end.node).is_none() {
            let children: Vec<_> = page.doc.children(end.node).collect();
            let joins = *start != *end && start.node != end.node && !page.doc.ancestor_or_self(start.node, |a| a == end.node).is_some();
            if end.offset + 1 == children.len() && page.doc.tag(children[end.offset]) == Some("br") && joins {
                end.offset += 1;
            }
        }
    }
}
