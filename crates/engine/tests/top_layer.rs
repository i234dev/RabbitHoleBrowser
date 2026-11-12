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
