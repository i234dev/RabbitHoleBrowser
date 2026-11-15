//! Live option state must agree across script, native selection, rendering and submission.
use rabbithole_engine::{Engine, Viewport};
use rabbithole_engine::net::Url;
const VIEW: Viewport = Viewport { width: 800.0, height: 600.0 };
const HTML: &str = "<!doctype html><form id=f><select id=s name=choice><option value=a selected>Alpha</option><option value=b>Beta</option><option value=c>Gamma</option></select></form>";

fn setup(html: &str) -> (Engine, rabbithole_engine::Page) {
    let mut engine = Engine::new_bundled_fonts_only();
    let page = engine.load_html(html, Url::parse("https://example.test/select").unwrap(), VIEW);
    (engine, page)
}

#[test]
fn selected_attribute_changes_do_not_override_dirty_option_and_reset_restores_defaults() {
    let (engine, mut p) = setup(HTML);
    assert_eq!(engine.eval_in_page(&mut p, "var s=document.getElementById('s'),f=document.getElementById('f');s.selectedIndex=2;s.options[2].setAttribute('selected','');s.options[2].removeAttribute('selected');s.selectedIndex"), "2");
    assert_eq!(engine.eval_in_page(&mut p, "s.options[2].defaultSelected=true;s.options[2].defaultSelected=false;s.selectedIndex"), "2");
    assert_eq!(engine.eval_in_page(&mut p, "s.value+'|'+s.selectedOptions.length+'|'+s.selectedOptions[0].value+'|'+new FormData(f).get('choice')+'|'+s.options[0].defaultSelected"), "'c|1|c|c|true'");
    assert_eq!(engine.eval_in_page(&mut p, "f.reset();s.value+'|'+s.selectedIndex+'|'+s.options[2].selected"), "'a|0|false'");
    assert_eq!(engine.eval_in_page(&mut p, "s.options[2].defaultSelected=true;s.selectedIndex"), "2");
}

#[test]
fn native_selection_and_script_selection_share_rendered_and_submitted_state() {
    let (engine, mut p) = setup(HTML);
    let s = p.doc.element_by_id("s").unwrap();
    p.select_option(s, 2);
    assert_eq!(p.control_text(s), "c");
    engine.restyle(&mut p);
    assert!(p.text_runs().iter().any(|r| &*r.text == "Gamma"));
    assert_eq!(engine.eval_in_page(&mut p, "var s=document.getElementById('s');s.value+'|'+s.selectedIndex+'|'+new FormData(document.getElementById('f')).get('choice')"), "'c|2|c'");
    let f = p.doc.element_by_id("f").unwrap();
    let (fields, _) = rabbithole_engine::forms::form_data_set(&p.doc, f, None, &|_| vec![]);
    assert_eq!(fields, vec![("choice".to_string(), "c".to_string())]);
    assert_eq!(engine.eval_in_page(&mut p, "s.selectedIndex=-1;s.value+'|'+s.selectedIndex+'|'+s.selectedOptions.length+'|'+new FormData(document.getElementById('f')).has('choice')"), "'|-1|0|false'");
    assert!(p.select_options(s).iter().all(|o| !o.2));
    engine.restyle(&mut p);
    assert!(!p.text_runs().iter().any(|r| &*r.text == "Alpha"));
    p.reset_form(f);
    assert!(p.select_options(s)[0].2);
}

#[test]
fn multiple_duplicate_values_option_constructor_and_canceled_reset() {
    let (engine, mut p) = setup("<!doctype html><form id=f><select id=s name=x multiple><option value=x selected>A</option><option value=x>B</option><option value=y>C</option></select></form>");
    assert_eq!(engine.eval_in_page(&mut p, "var s=document.getElementById('s'),f=document.getElementById('f');s.options[1].selected=true;s.options[2].selected=true;s.selectedOptions.length"), "3");
    assert_eq!(engine.eval_in_page(&mut p, "s.value='x';s.selectedOptions.length+'|'+s.selectedIndex+'|'+new FormData(f).getAll('x').length"), "'1|0|1'");
    assert_eq!(engine.eval_in_page(&mut p, "s.selectedIndex=2;f.addEventListener('reset',e=>e.preventDefault());f.reset();s.selectedIndex"), "2");
    assert_eq!(engine.eval_in_page(&mut p, "var o=new Option('Test','v',true);o.defaultSelected+'|'+o.selected"), "'true|false'");
    assert_eq!(engine.eval_in_page(&mut p, "var o2=new Option('Test','v',false,true);o2.defaultSelected+'|'+o2.selected"), "'false|true'");
}

#[test]
fn checked_and_default_pseudo_classes_observe_distinct_option_states() {
    let (engine, mut p) = setup(HTML);
    assert_eq!(engine.eval_in_page(&mut p, "var s=document.getElementById('s');s.selectedIndex=2;s.options[2].matches(':checked')+'|'+s.options[0].matches(':checked')+'|'+s.options[0].matches(':default')+'|'+s.options[2].matches('[selected]')"), "'true|false|true|false'");
}
