//! Keyboard default actions shared by the GUI, frame routing and automation.
use crate::scripting::{input_event_init, EngineEnv, ScriptOutcome};
use crate::{Engine, Modifiers, Page};
use rabbithole_dom::BoundaryPoint;

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
        if page.doc.selection().is_none() {
            let point = BoundaryPoint { node: focused, offset: 0 };
            page.doc.set_selection(point, point);
        }
        let selection = page.doc.selection()?;
        if [selection.start, selection.end].iter().any(|p| page.doc.editing_host(p.node) != Some(target)) {
            return Some(ScriptOutcome::default());
        }
        self.ensure_script_host(page);
        let mut out = self.dispatch_event(page, target, "beforeinput", input_event_init(input_type, text, true));
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
}
