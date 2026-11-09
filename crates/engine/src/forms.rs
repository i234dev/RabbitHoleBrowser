//! HTML forms: the form data set, submission requests (GET/POST,
//! urlencoded/multipart) and control state stored in the DOM.

use crate::Page;
use rabbithole_dom::{Document, NodeId};
use rabbithole_net::{Body, Request};

/// A file chosen for an `<input type=file>` (kept outside the DOM).
#[derive(Debug, Clone)]
pub struct ChosenFile {
    pub name: String,
    pub mime: String,
    pub data: Vec<u8>,
}

/// Nearest ancestor `<form>` of a control (or the form referenced by its
/// `form` attribute).
pub fn form_of(doc: &Document, node: NodeId) -> Option<NodeId> {
    if let Some(id) = doc.attr(node, "form") {
        if let Some(f) = doc.element_by_id(id) {
            if doc.is_element(f, "form") {
                return Some(f);
            }
        }
    }
    doc.ancestor_or_self(node, |n| doc.is_element(n, "form"))
}

/// Is this element a form control whose value is submitted?
pub fn is_control(doc: &Document, node: NodeId) -> bool {
    matches!(doc.tag(node), Some("input" | "select" | "textarea" | "button"))
}

/// Is this element something the user can type into / toggle / click?
pub fn is_interactive(doc: &Document, node: NodeId) -> bool {
    match doc.tag(node) {
        Some("input") => {
            let t = doc.attr(node, "type").map(|t| t.to_ascii_lowercase()).unwrap_or_default();
            t != "hidden"
        }
        Some("textarea" | "select" | "button") => true,
        _ => false,
    }
}

/// Control kinds relevant to interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlKind {
    Text,
    Password,
    Checkbox,
    Radio,
    Submit,
    Reset,
    Button,
    File,
    Select,
    TextArea,
    Hidden,
    Other,
}

pub fn control_kind(doc: &Document, node: NodeId) -> ControlKind {
    match doc.tag(node) {
        Some("textarea") => ControlKind::TextArea,
        Some("select") => ControlKind::Select,
        Some("button") => match doc.attr(node, "type").map(|t| t.to_ascii_lowercase()).as_deref() {
            Some("button") => ControlKind::Button,
            Some("reset") => ControlKind::Reset,
            _ => ControlKind::Submit,
        },
        Some("input") => {
            let t = doc.attr(node, "type").map(|t| t.trim().to_ascii_lowercase()).unwrap_or_else(|| "text".into());
            match t.as_str() {
                "checkbox" => ControlKind::Checkbox,
                "radio" => ControlKind::Radio,
                "submit" | "image" => ControlKind::Submit,
                "reset" => ControlKind::Reset,
                "button" => ControlKind::Button,
                "file" => ControlKind::File,
                "hidden" => ControlKind::Hidden,
                "password" => ControlKind::Password,
                "text" | "search" | "email" | "url" | "tel" | "number" | "date" | "time" | "datetime-local" | "month" | "week"
                | "color" | "range" | "" => ControlKind::Text,
                _ => ControlKind::Other,
            }
        }
        _ => ControlKind::Other,
    }
}

fn is_disabled(doc: &Document, node: NodeId) -> bool {
    doc.ancestor_or_self(node, |n| {
        doc.attr(n, "disabled").is_some() && (n == node || doc.is_element(n, "fieldset"))
    })
    .is_some()
}

/// Value of a `<select>`: selected options (respecting `multiple`).
fn select_values(doc: &Document, select: NodeId) -> Vec<String> {
    let multiple = doc.attr(select, "multiple").is_some();
    let mut out = Vec::new();
    let mut first: Option<String> = None;
    for n in doc.descendants(select) {
        if !doc.is_element(n, "option") {
            continue;
        }
        if doc.attr(n, "disabled").is_some() {
            continue;
        }
        let value = doc.attr(n, "value").map(|v| v.to_string()).unwrap_or_else(|| doc.text_content(n).trim().to_string());
        if first.is_none() {
            first = Some(value.clone());
        }
        if doc.attr(n, "selected").is_some() {
            out.push(value);
            if !multiple {
                break;
            }
        }
    }
    if out.is_empty() && !multiple {
        if let Some(f) = first {
            // Single selects always have a selected option (the first).
            if doc.attr(select, "size").and_then(|s| s.parse::<u32>().ok()).unwrap_or(1) <= 1 {
                out.push(f);
            }
        }
    }
    out
}

/// Build the form data set (HTML §4.10.21.4) as (name, value) pairs plus
/// files. `submitter` contributes its name/value when it has one.
pub fn form_data_set(
    doc: &Document,
    form: NodeId,
    submitter: Option<NodeId>,
    files: &dyn Fn(NodeId) -> Vec<ChosenFile>,
) -> (Vec<(String, String)>, Vec<(String, String, String, Vec<u8>)>) {
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut file_parts = Vec::new();
    let form_id = doc.attr(form, "id").map(|s| s.to_string());

    // Controls are descendants of the form, plus any element whose `form`
    // attribute points at it.
    let mut controls: Vec<NodeId> = doc.descendants(form).filter(|&n| is_control(doc, n)).collect();
    if let Some(id) = &form_id {
        for n in doc.descendants(Document::ROOT) {
            if is_control(doc, n) && doc.attr(n, "form") == Some(id.as_str()) && !controls.contains(&n) {
                controls.push(n);
            }
        }
    }
    for n in controls {
        if is_disabled(doc, n) {
            continue;
        }
        let kind = control_kind(doc, n);
        let name = doc.attr(n, "name").unwrap_or("").to_string();
        match kind {
            ControlKind::Submit | ControlKind::Button | ControlKind::Reset => {
                if Some(n) == submitter && !name.is_empty() {
                    let is_image = doc.attr(n, "type").map(|t| t.eq_ignore_ascii_case("image")).unwrap_or(false);
                    if is_image {
                        fields.push((format!("{name}.x"), "0".into()));
                        fields.push((format!("{name}.y"), "0".into()));
                    } else {
                        fields.push((name, doc.attr(n, "value").unwrap_or("").to_string()));
                    }
                }
            }
            ControlKind::Checkbox | ControlKind::Radio => {
                if name.is_empty() || doc.attr(n, "checked").is_none() {
                    continue;
                }
                fields.push((name, doc.attr(n, "value").unwrap_or("on").to_string()));
            }
            ControlKind::Select => {
                if name.is_empty() {
                    continue;
                }
                for v in select_values(doc, n) {
                    fields.push((name.clone(), v));
                }
            }
            ControlKind::TextArea => {
                if name.is_empty() {
                    continue;
                }
                // The DOM `value` attribute (set when the user types) wins
                // over the initial text content.
                let value = doc.attr(n, "value").map(|v| v.to_string()).unwrap_or_else(|| doc.text_content(n));
                fields.push((name, value.replace('\n', "\r\n")));
            }
            ControlKind::File => {
                if name.is_empty() {
                    continue;
                }
                let chosen = files(n);
                if chosen.is_empty() {
                    file_parts.push((name, String::new(), "application/octet-stream".into(), Vec::new()));
                } else {
                    for f in chosen {
                        file_parts.push((name.clone(), f.name, f.mime, f.data));
                    }
                }
            }
            ControlKind::Hidden | ControlKind::Text | ControlKind::Password | ControlKind::Other => {
                if name.is_empty() {
                    continue;
                }
                let mut value = doc.attr(n, "value").unwrap_or("").to_string();
                if name == "_charset_" && kind == ControlKind::Hidden && value.is_empty() {
                    value = "UTF-8".into();
                }
                fields.push((name, value));
            }
        }
    }
    (fields, file_parts)
}

/// Build the HTTP request for submitting `form` (HTML §4.10.21.3).
pub fn submission_request(page: &Page, form: NodeId, submitter: Option<NodeId>, files: &dyn Fn(NodeId) -> Vec<ChosenFile>) -> Option<Request> {
    let doc = &page.doc;
    let attr = |name: &str| -> Option<String> {
        // Submitter attributes (formaction, formmethod, formenctype) override.
        submitter
            .and_then(|s| doc.attr(s, &format!("form{name}")).map(|v| v.to_string()))
            .or_else(|| doc.attr(form, name).map(|v| v.to_string()))
    };
    let action = attr("action").unwrap_or_default();
    let mut url = if action.trim().is_empty() { page.url.clone() } else { page.resolve(&action)? };
    let method = attr("method").map(|m| m.trim().to_ascii_lowercase()).unwrap_or_else(|| "get".into());
    let enctype = attr("enctype").map(|e| e.trim().to_ascii_lowercase()).unwrap_or_default();
    let (fields, file_parts) = form_data_set(doc, form, submitter, files);

    let req = match method.as_str() {
        "post" if url.is_http() => {
            let body = if enctype == "multipart/form-data" || !file_parts.is_empty() {
                Body::multipart(&fields, &file_parts)
            } else if enctype == "text/plain" {
                let text = fields.iter().map(|(k, v)| format!("{k}={v}\r\n")).collect::<String>();
                Body::text("text/plain", &text)
            } else {
                Body::form(&fields)
            };
            Request::post(url, body)
        }
        "dialog" => return None,
        _ => {
            // GET: replace the query with the urlencoded data set.
            let query = fields
                .iter()
                .map(|(k, v)| format!("{}={}", rabbithole_net::form_encode(k), rabbithole_net::form_encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            url.query = Some(query);
            url.fragment = None;
            Request::get(url)
        }
    };
    Some(req.with_referer(page.url.clone()))
}

/// Mutations of control state, stored in the DOM so layout, selectors
/// (`:checked`) and submission all see them.
impl Page {
    pub fn form_of(&self, node: NodeId) -> Option<NodeId> {
        form_of(&self.doc, node)
    }

    /// The interactive control at or above `node`.
    pub fn control_of(&self, node: NodeId) -> Option<NodeId> {
        self.doc.ancestor_or_self(node, |n| is_interactive(&self.doc, n))
    }

    pub fn control_kind(&self, node: NodeId) -> ControlKind {
        control_kind(&self.doc, node)
    }

    /// Current text of a text-like control.
    pub fn control_text(&self, node: NodeId) -> String {
        match control_kind(&self.doc, node) {
            ControlKind::TextArea => self.doc.attr(node, "value").map(|v| v.to_string()).unwrap_or_else(|| self.doc.text_content(node)),
            _ => self.doc.attr(node, "value").unwrap_or("").to_string(),
        }
    }

    /// Set the text of a text-like control (`value` attribute).
    pub fn set_control_text(&mut self, node: NodeId, text: &str) {
        set_attr(&mut self.doc, node, "value", Some(text));
    }

    /// Toggle a checkbox, or select a radio button (unchecking its group).
    pub fn toggle_control(&mut self, node: NodeId) {
        match control_kind(&self.doc, node) {
            ControlKind::Checkbox => {
                let checked = self.doc.attr(node, "checked").is_some();
                set_attr(&mut self.doc, node, "checked", if checked { None } else { Some("") });
            }
            ControlKind::Radio => {
                let name = self.doc.attr(node, "name").map(|s| s.to_string());
                let form = form_of(&self.doc, node);
                if let Some(name) = name {
                    let group: Vec<NodeId> = self
                        .doc
                        .descendants(form.unwrap_or(Document::ROOT))
                        .filter(|&n| n != node && control_kind(&self.doc, n) == ControlKind::Radio && self.doc.attr(n, "name") == Some(name.as_str()))
                        .collect();
                    for g in group {
                        set_attr(&mut self.doc, g, "checked", None);
                    }
                }
                set_attr(&mut self.doc, node, "checked", Some(""));
            }
            _ => {}
        }
    }

    /// Options of a `<select>` as (label, value, selected, disabled).
    pub fn select_options(&self, select: NodeId) -> Vec<(String, String, bool, bool)> {
        self.doc
            .descendants(select)
            .filter(|&n| self.doc.is_element(n, "option"))
            .map(|n| {
                let label = self.doc.attr(n, "label").map(|l| l.to_string()).unwrap_or_else(|| self.doc.text_content(n).trim().to_string());
                let value = self.doc.attr(n, "value").map(|v| v.to_string()).unwrap_or_else(|| label.clone());
                (label, value, self.doc.attr(n, "selected").is_some(), self.doc.attr(n, "disabled").is_some())
            })
            .collect()
    }

    /// Select the option at `index` (single select semantics unless
    /// `multiple`, where it toggles).
    pub fn select_option(&mut self, select: NodeId, index: usize) {
        let multiple = self.doc.attr(select, "multiple").is_some();
        let options: Vec<NodeId> = self.doc.descendants(select).filter(|&n| self.doc.is_element(n, "option")).collect();
        for (i, &o) in options.iter().enumerate() {
            if i == index {
                let already = self.doc.attr(o, "selected").is_some();
                set_attr(&mut self.doc, o, "selected", if multiple && already { None } else { Some("") });
            } else if !multiple {
                set_attr(&mut self.doc, o, "selected", None);
            }
        }
    }

    /// Reset all controls of a form to their initial state (attributes
    /// named `data-rh-initial-*` are not kept; we approximate by clearing
    /// user-set values: `value` on inputs falls back to `defaultvalue`).
    pub fn reset_form(&mut self, form: NodeId) {
        let controls: Vec<NodeId> = self.doc.descendants(form).filter(|&n| is_control(&self.doc, n)).collect();
        for n in controls {
            match control_kind(&self.doc, n) {
                ControlKind::Text | ControlKind::Password | ControlKind::Other => {
                    let default = self.doc.attr(n, "data-rh-default").map(|v| v.to_string());
                    set_attr(&mut self.doc, n, "value", default.as_deref());
                }
                ControlKind::TextArea => set_attr(&mut self.doc, n, "value", None),
                ControlKind::Checkbox | ControlKind::Radio => {
                    let default = self.doc.attr(n, "data-rh-default-checked").is_some();
                    set_attr(&mut self.doc, n, "checked", if default { Some("") } else { None });
                }
                _ => {}
            }
        }
    }

    /// Remember initial control values so `reset_form` can restore them.
    /// Called once after loading.
    pub fn snapshot_form_defaults(&mut self) {
        let controls: Vec<NodeId> = self.doc.descendants(Document::ROOT).filter(|&n| is_control(&self.doc, n)).collect();
        for n in controls {
            match control_kind(&self.doc, n) {
                ControlKind::Text | ControlKind::Password | ControlKind::Other => {
                    if let Some(v) = self.doc.attr(n, "value").map(|v| v.to_string()) {
                        set_attr(&mut self.doc, n, "data-rh-default", Some(&v));
                    }
                }
                ControlKind::Checkbox | ControlKind::Radio => {
                    if self.doc.attr(n, "checked").is_some() {
                        set_attr(&mut self.doc, n, "data-rh-default-checked", Some(""));
                    }
                }
                _ => {}
            }
        }
    }

    /// The submit button that `Enter` in a text field would trigger: the
    /// form's first submit control.
    pub fn default_submitter(&self, form: NodeId) -> Option<NodeId> {
        self.doc
            .descendants(form)
            .find(|&n| control_kind(&self.doc, n) == ControlKind::Submit && !is_disabled(&self.doc, n))
    }

    /// Build the submission request for a form.
    pub fn submit_request(&self, form: NodeId, submitter: Option<NodeId>, files: &dyn Fn(NodeId) -> Vec<ChosenFile>) -> Option<Request> {
        submission_request(self, form, submitter, files)
    }

    /// Placeholder text of a control.
    pub fn placeholder(&self, node: NodeId) -> Option<&str> {
        self.doc.attr(node, "placeholder")
    }

    /// `maxlength` of a text control.
    pub fn max_length(&self, node: NodeId) -> Option<usize> {
        self.doc.attr(node, "maxlength").and_then(|m| m.parse().ok())
    }
}

/// Set (`Some`) or remove (`None`) an attribute.
pub fn set_attr(doc: &mut Document, node: NodeId, name: &str, value: Option<&str>) {
    if let Some(el) = doc.element_mut(node) {
        match value {
            Some(v) => match el.attrs.iter_mut().find(|(k, _)| k == name) {
                Some(slot) => slot.1 = v.to_string(),
                None => el.attrs.push((name.to_string(), v.to_string())),
            },
            None => el.attrs.retain(|(k, _)| k != name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_set_and_requests() {
        let html = r#"<form action="/search" method="get"><input name="q" value="rabbits"><input type="checkbox" name="safe" checked><input type="checkbox" name="off"><input type="radio" name="r" value="a"><input type="radio" name="r" value="b" checked><select name="s"><option>one</option><option value="2" selected>two</option></select><textarea name="t">hello
world</textarea><input type="hidden" name="h" value="1"><button name="go" value="now">Go</button><input type="submit" name="other" value="x" disabled></form>"#;
        let doc = rabbithole_html::parse(html);
        let form = doc.find_element("form").unwrap();
        let button = doc.find_element("button").unwrap();
        let (fields, files) = form_data_set(&doc, form, Some(button), &|_| Vec::new());
        assert!(files.is_empty());
        let pairs: Vec<(String, String)> = fields;
        assert_eq!(
            pairs,
            vec![
                ("q".to_string(), "rabbits".to_string()),
                ("safe".into(), "on".into()),
                ("r".into(), "b".into()),
                ("s".into(), "2".into()),
                ("t".into(), "hello\r\nworld".into()),
                ("h".into(), "1".into()),
                ("go".into(), "now".into()),
            ]
        );
        let body = Body::form(&pairs);
        assert_eq!(String::from_utf8(body.data).unwrap(), "q=rabbits&safe=on&r=b&s=2&t=hello%0D%0Aworld&h=1&go=now");
    }
}
