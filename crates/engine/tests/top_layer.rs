//! The top layer: `dialog.showModal()` paints the dialog above everything
//! (positioned elements with any z-index included) over its `::backdrop`,
//! which takes the clicks outside the dialog; Escape cancels it.

use rabbithole_engine::gfx::Canvas;
use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Page, Viewport};

fn engine() -> Engine {
    let mut e = Engine::new_bundled_fonts_only();
    e.scripting = true;
    e.loader.load_scripts = true;
    e
}

fn eval(engine: &Engine, page: &mut Page, js: &str) -> String {
    engine.eval_in_page(page, js).trim_matches(|c| c == '\'' || c == '"').to_string()
}

fn rgb(c: &Canvas, x: u32, y: u32) -> (u8, u8, u8) {
    let p = c.data[(y * c.width + x) as usize];
    (((p >> 16) & 0xff) as u8, ((p >> 8) & 0xff) as u8, (p & 0xff) as u8)
}

fn render(e: &Engine, page: &mut Page) -> Canvas {
    e.tick(page);
    let mut c = Canvas::new(800, 600);
    e.render(page, &mut c, 0.0, 0.0);
    c
}

#[test]
fn modal_dialog_is_painted_above_everything_over_its_backdrop() {
    let mut e = engine();
    let url = Url::parse("https://example.test/").unwrap();
    let mut page = e.load_html(
        r#"<!doctype html><style>
        body { margin: 0 }
        #overlay { position: fixed; inset: 0; z-index: 50; background: rgb(255, 0, 0) }
        #d { width: 200px; height: 100px; padding: 0; border: 0; background: rgb(0, 255, 0) }
        #d::backdrop { background: rgb(0, 0, 255) }
        </style>
        <div id="overlay"></div>
        <dialog id="d"><p>hello</p></dialog>
        <script>
        window.log = [];
        var d = document.getElementById('d');
        d.addEventListener('cancel', function () { log.push('cancel'); });
        d.addEventListener('close', function () { log.push('close'); });
        </script>"#,
        url,
        Viewport { width: 800.0, height: 600.0 },
    );
    let c = render(&e, &mut page);
    assert_eq!(rgb(&c, 400, 300), (255, 0, 0), "closed dialog: the overlay shows");
    assert_eq!(eval(&e, &mut page, "d.showModal(); [d.open, d.matches(':modal'), document.querySelectorAll(':modal').length].join()"), "true,true,1");
    let c = render(&e, &mut page);
    // Centered in the viewport, above the z-index 50 overlay.
    assert_eq!(rgb(&c, 400, 300), (0, 255, 0));
    assert_eq!(rgb(&c, 10, 10), (0, 0, 255), "::backdrop covers the page");
    let dialog = page.doc.element_by_id("d").unwrap();
    assert_eq!(page.node_at_viewport(10.0, 10.0), Some(dialog), "clicks on the backdrop target the dialog");
    // showModal() on an open modal dialog is a no-op; on a non-modal open
    // one it throws.
    assert_eq!(eval(&e, &mut page, "d.showModal(); document.querySelectorAll(':modal').length"), "1");
    // Escape: cancel, then close.
    assert!(e.cancel_modal(&mut page).is_some());
    assert_eq!(eval(&e, &mut page, "[d.open, d.matches(':modal'), log.join('+')].join()"), "false,false,cancel+close");
    let c = render(&e, &mut page);
    assert_eq!(rgb(&c, 10, 10), (255, 0, 0));
    assert!(e.cancel_modal(&mut page).is_none());
    assert_eq!(eval(&e, &mut page, "d.show(); var r; try { d.showModal(); r = 'no'; } catch (x) { r = x.name; } d.close(); r"), "InvalidStateError");
}

#[test]
fn popover_activation_shares_state_with_selectors_paint_and_hit_testing() {
    let mut e = engine();
    let mut page = e.load_html(
        r#"<!doctype html><style>
          body { margin: 0 }
          #under { position:fixed; inset:0; z-index:50; background:red }
          #panel { width:100px; height:60px; border:0; padding:0; background:lime }
        </style><div id=under></div><button id=show type=button></button>
        <div popover id=panel><span>Menu</span></div><svg><rect id=graphic popover /></svg>"#,
        Url::parse("http://localhost/").unwrap(),
        Viewport { width:800.0, height:600.0 },
    );
    assert_eq!(eval(&e, &mut page, r#"var changes=[];
        panel.onbeforetoggle = e => changes.push([e.oldState,e.newState,e.source === show,e.cancelable].join('/'));
        show.popoverTargetElement = panel;
        [show.getAttribute('popovertarget'),panel.popover,getComputedStyle(panel).display,
         getComputedStyle(graphic).display === 'none'].join('|')"#), "|auto|none|false");
    let button = page.doc.element_by_id("show").unwrap();
    e.click_element(&mut page, button, None, Default::default());
    assert_eq!(eval(&e, &mut page, "[panel.matches(':popover-open'),panel.matches(':modal'),changes.join()].join('|')"),
        "true|false|closed/open/true/true");
    let canvas = render(&e, &mut page);
    assert_eq!(rgb(&canvas, 400, 300), (0, 255, 0));
    assert_eq!(page.node_at_viewport(10.0, 10.0), page.doc.element_by_id("under"));
    assert!(e.cancel_modal(&mut page).is_some());
    assert_eq!(eval(&e, &mut page, r#"
        show.click();
        const opened = panel.matches(':popover-open');
        show.setAttribute('popovertarget','');
        const detached = panel; detached.remove(); document.body.append(detached);
        [opened,panel.matches(':popover-open'),show.popoverTargetElement,changes.join(',')].join('|')"#),
        "true|false||closed/open/true/true,open/closed/false/false,closed/open/true/true");
}

#[test]
fn popover_beforetoggle_can_cancel_and_toggle_events_coalesce() {
    let mut e = engine();
    let mut page = e.load_html(
        "<main popover id=outer><p popover=hint id=inner></p></main><aside popover id=other></aside>",
        Url::parse("http://localhost/").unwrap(), Viewport { width:800.0, height:600.0 },
    );
    assert_eq!(eval(&e, &mut page, r#"var toggles = [];
        outer.addEventListener('beforetoggle', e => e.preventDefault(), {once:true});
        outer.ontoggle = e => toggles.push(e.oldState + '/' + e.newState);
        const canceled = outer.togglePopover(true);
        outer.showPopover(); inner.showPopover();
        const nested = document.querySelectorAll(':popover-open').length;
        other.showPopover();
        [canceled,nested,outer.matches(':popover-open'),inner.matches(':popover-open'),
          other.matches(':popover-open'),toggles.length].join('|')"#), "false|2|false|false|true|0");
    e.tick(&mut page);
    assert_eq!(eval(&e, &mut page, "toggles.join()"), "closed/closed");
}

#[test]
fn outside_pointer_release_dismisses_a_popover_before_listeners_even_if_press_was_cancelled() {
    let mut e = engine();
    let mut page = e.load_html(
        r#"<style>body{margin:0} #outside{position:fixed;inset:0} #menu{width:80px;height:40px}</style>
        <button id=outside>Outside</button><div id=menu popover></div>"#,
        Url::parse("http://localhost/").unwrap(), Viewport { width:800.0,height:600.0 },
    );
    eval(&e,&mut page,r#"var log=[];
        menu.showPopover();
        menu.onbeforetoggle=e=>log.push(e.newState);
        outside.onpointerdown=e=>{log.push(e.type);e.preventDefault()};
        outside.onpointerup=e=>log.push(e.type);
        outside.onclick=e=>log.push(e.type);"#);
    render(&e,&mut page);
    e.pointer_down(&mut page,5.0,5.0,0,Default::default());
    assert_eq!(eval(&e,&mut page,"menu.matches(':popover-open')"),"true");
    e.pointer_up(&mut page,5.0,5.0,0,Default::default());
    assert_eq!(eval(&e,&mut page,"[menu.matches(':popover-open'),...log].join('|')"),"false|pointerdown|closed|pointerup|click");
}

#[test]
fn toggle_event_converts_dictionary_states_and_retargets_its_source() {
    let mut e = engine();
    let mut page = e.load_html("<div id=host></div>",Url::parse("http://localhost/").unwrap(),Viewport { width:800.0,height:600.0 });
    let result = eval(&e,&mut page,r#"(() => {
        const root=host.attachShadow({mode:'open'}), button=document.createElement('button');
        root.appendChild(button);
        const options=Object.create({oldState:42,newState:null,source:button,bubbles:true,composed:true});
        const event=new ToggleEvent('toggle',options), log=[];
        button.addEventListener('toggle', e=>log.push(e.source===button));
        host.addEventListener('toggle', e=>log.push(e.source===host));
        button.dispatchEvent(event);
        let invalid=false;
        try { new ToggleEvent('toggle',{source:{}}); } catch(e) { invalid=e instanceof TypeError; }
        return [event.oldState,event.newState,typeof event.oldState,event.source===host,invalid,...log].join('|');
    })()"#);
    assert_eq!(result,"42|null|string|true|true|true|true");
}

#[test]
fn opening_a_dialog_hides_unrelated_popovers_and_preserves_its_ancestor() {
    let mut e = engine();
    let mut page = e.load_html(
        "<div id=menu popover><dialog id=inner></dialog><div id=sub popover=hint></div></div><dialog id=outside></dialog><div id=manual popover=manual></div>",
        Url::parse("https://example.test/").unwrap(), Viewport { width: 800.0, height: 600.0 });
    assert_eq!(eval(&e,&mut page,r#"(() => {
        menu.showPopover(); sub.showPopover(); manual.showPopover();
        inner.showModal();
        const keep=[menu.matches(':popover-open'),sub.matches(':popover-open')];
        inner.close();
        let nested;
        menu.onbeforetoggle=() => { try { sub.showPopover(); } catch (e) { nested=e.name; } };
        outside.show();
        return [...keep,menu.matches(':popover-open'),manual.matches(':popover-open'),nested].join('|');
    })()"#),"true|false|false|true|InvalidStateError");
}
