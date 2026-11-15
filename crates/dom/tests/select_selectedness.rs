use rabbithole_dom::{Document, NodeId};

fn select(doc: &mut Document, multiple: bool) -> (NodeId, [NodeId; 3]) {
    let s = doc.create_element("select", if multiple { vec![("multiple".into(), "".into())] } else { vec![] });
    let options = std::array::from_fn(|i| {
        let o = doc.create_element("option", if i == 0 { vec![("selected".into(), "".into())] } else { vec![] });
        doc.append_child(s, o);
        o
    });
    (s, options)
}

#[test]
fn dirty_selectedness_survives_default_changes_and_reset_clears_dirtiness() {
    let mut d = Document::new();
    let (s, [a, _, c]) = select(&mut d, false);
    d.set_select_index(s, Some(2));
    assert!(d.is_option_selected(c));
    assert!(d.attr(a, "selected").is_some(), "interaction preserves defaults");
    d.set_attr(c, "selected", Some(""));
    d.set_attr(c, "selected", None);
    assert!(d.is_option_selected(c), "dirty option ignores attribute add/remove");
    d.reset_select(s);
    assert!(d.is_option_selected(a));
    assert!(!d.is_option_selected(c));
    d.set_attr(c, "selected", Some(""));
    assert!(d.is_option_selected(c), "reset cleared dirtiness");
    assert!(!d.is_option_selected(a));
}

#[test]
fn empty_selection_is_not_a_getter_fallback_and_tree_removal_recomputes() {
    let mut d = Document::new();
    let (s, [a, b, c]) = select(&mut d, false);
    d.set_select_index(s, None);
    assert!(d.select_options(s).iter().all(|&o| !d.is_option_selected(o)));
    let text = d.create_text("label");
    d.append_child(c, text);
    assert!(!d.is_option_selected(a), "label edits do not undo explicit empty selection");
    d.set_option_selected(c, true);
    d.detach(c);
    assert!(d.is_option_selected(a));
    d.set_attr(a, "disabled", Some(""));
    d.set_option_selected(a, false);
    assert!(d.is_option_selected(b), "fallback skips disabled options");
    d.set_attr(s, "size", Some("3"));
    d.set_option_selected(b, false);
    assert!(!d.is_option_selected(a) && !d.is_option_selected(b));
}

#[test]
fn multiple_state_clone_and_import_keep_dirty_defaults_separate() {
    let mut d = Document::new();
    let (s, [a, b, c]) = select(&mut d, true);
    d.set_option_selected(b, true);
    d.set_option_selected(c, true);
    assert!([a, b, c].iter().all(|&o| d.is_option_selected(o)));
    let clone = d.clone_subtree(c, true);
    d.set_attr(clone, "selected", Some(""));
    d.set_attr(clone, "selected", None);
    assert!(d.is_option_selected(clone));
    let mut other = Document::new();
    let imported = other.import_subtree(&d, c);
    other.set_attr(imported, "selected", Some(""));
    other.set_attr(imported, "selected", None);
    assert!(other.is_option_selected(imported));
    d.set_select_index(s, Some(1));
    assert!(!d.is_option_selected(a) && d.is_option_selected(b) && !d.is_option_selected(c));
    d.reset_select(s);
    assert!(d.is_option_selected(a) && !d.is_option_selected(b) && !d.is_option_selected(c));
}

#[test]
fn selected_insertions_and_optgroup_moves_keep_single_selection() {
    let mut d = Document::new();
    let (s, [a, _, c]) = select(&mut d, false);
    let o = d.create_element("option", vec![("selected".into(), "".into())]);
    d.insert_before(s, o, Some(a));
    assert!(d.is_option_selected(o) && !d.is_option_selected(a));
    let group = d.create_element("optgroup", vec![("disabled".into(), "".into())]);
    d.detach(c);
    d.append_child(group, c);
    assert!(d.option_disabled(c));
}
