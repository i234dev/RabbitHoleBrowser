//! Boxless controls must not inherit a container's geometry in CSSOM.
use rabbithole_engine::dom::NodeData;
use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Page, Viewport};

fn load(body: &str) -> (Engine, Page) {
    let mut engine = Engine::new_bundled_fonts_only();
    let page = engine.load_html(
        &format!("<!doctype html><style>html,body{{margin:0;padding:0;font:16px/20px sans-serif}}</style>{body}"),
        Url::parse("https://example.test/controls").unwrap(),
        Viewport { width: 800., height: 600. },
    );
    (engine, page)
}

fn eval(engine: &Engine, page: &mut Page, script: &str) -> String {
    engine.eval_in_page(page, script).trim_matches(|c| c == '\'' || c == '"').to_string()
}

#[test]
fn boxless_controls_and_contents_return_no_client_rects() {
    let (engine, mut page) = load(r#"
      <form id=container style="width:380px;height:96px">
        <div id=password style="display:none"><input id=hiddenChild type=password></div>
        <input id=hiddenInput type=hidden>
        <span id=hiddenInline style="display:none">hidden</span>
        <div id=contents style="display:contents"><div id=child style="width:20px;height:30px"></div></div>
      </form>
      <div id=invisible style="visibility:hidden;width:40px;height:17px"></div>
      <div id=transparent style="opacity:0;width:35px;height:19px"></div>
      <div id=empty style="position:absolute;left:33px;top:45px;width:0;height:0"></div>
      <div style="height:2000px"></div>
    "#);
    for id in ["password", "hiddenChild", "hiddenInput", "hiddenInline", "contents"] {
        let node = page.doc.element_by_id(id).unwrap();
        assert!(page.layout_rect_of(node).is_none(), "#{id}");
        assert!(page.client_rects_of(node).is_empty(), "#{id}");
        assert!(page.client_rect_of(node).is_none(), "#{id}");
    }
    // The visual fallback remains available for existing GUI/anchor callers.
    let hidden = page.doc.element_by_id("password").unwrap();
    let container = page.doc.element_by_id("container").unwrap();
    assert_eq!(page.rect_of(hidden), page.rect_of(container));
    let result = eval(&engine, &mut page, r#"
      function zero(id) {
        const element=document.getElementById(id),r=element.getBoundingClientRect();
        return [r.x,r.y,r.width,r.height,element.getClientRects().length,element.offsetWidth,element.offsetHeight].join(',');
      }
      ['password','hiddenChild','hiddenInput','hiddenInline','contents'].map(zero).join('|')
    "#);
    assert_eq!(result, ["0,0,0,0,0,0,0"; 5].join("|"));
    let result = eval(&engine, &mut page, r#"
      ['child','invisible','transparent'].map(id=>{
        const e=document.getElementById(id),r=e.getBoundingClientRect();
        return [r.width,r.height,e.getClientRects().length].join(',');
      }).join('|')
    "#);
    assert_eq!(result, "20,30,1|40,17,1|35,19,1");
    assert_eq!(eval(&engine, &mut page, "const r=document.getElementById('empty').getBoundingClientRect();[r.x,r.y,r.width,r.height,document.getElementById('empty').getClientRects().length].join(',')"), "33,45,0,0,1");
    assert_eq!(eval(&engine, &mut page, "window.scrollTo(0,300); ['password','hiddenChild','hiddenInput','hiddenInline','contents'].map(zero).join('|')"), ["0,0,0,0,0,0,0"; 5].join("|"));
}

#[test]
fn client_geometry_tracks_detachment_and_display_changes() {
    let (engine, mut page) = load("<div id=host><input id=field style='display:block;width:80px;height:20px;padding:0;border:0'></div>");
    let result = eval(&engine, &mut page, r#"
      const field=document.getElementById('field'),host=document.getElementById('host');
      function box(){const r=field.getBoundingClientRect();return [r.width,r.height,field.getClientRects().length].join(',');}
      const results=[box()];
      host.style.display='none';results.push(box());
      host.style.display='block';results.push(box());
      field.remove();results.push(box());
      host.appendChild(field);results.push(box());
      field.style.display='none';results.push(box());
      results.join('|')
    "#);
    assert_eq!(result, "80,20,1|0,0,0|80,20,1|0,0,0|80,20,1|0,0,0");
}

#[test]
fn svg_fallback_stays_within_a_rendered_svg_subtree() {
    let (engine, mut page) = load(r#"
      <div style="width:380px;height:96px">
        <svg id=icon width=120 height=60><rect id=shape x=1 y=2 width=20 height=10/><g style="display:none"><rect id=hiddenShape width=10 height=10/></g></svg>
        <svg id=hiddenSvg style="display:none" width=120 height=60><rect id=hiddenSvgChild width=20 height=10/></svg>
      </div>
    "#);
    let result = eval(&engine, &mut page, r#"
      const icon=document.getElementById('icon').getBoundingClientRect(),shape=document.getElementById('shape').getBoundingClientRect();
      [icon.width===120,icon.height===60,shape.width>0&&shape.width<=icon.width,shape.height>0&&shape.height<=icon.height,
       ...['hiddenShape','hiddenSvg','hiddenSvgChild'].map(id=>{const e=document.getElementById(id),r=e.getBoundingClientRect();return r.width===0&&r.height===0&&e.getClientRects().length===0;})].join(',')
    "#);
    assert_eq!(result, "true,true,true,true,true,true,true");
}

#[test]
fn text_nodes_keep_the_geometry_of_their_own_fragments() {
    let (_engine, page) = load("<div id=container style='width:380px'><span id=inline>short text</span></div>");
    let inline = page.doc.element_by_id("inline").unwrap();
    let text = page.doc.descendants(inline).find(|&n| matches!(&page.doc.node(n).data, NodeData::Text(_))).unwrap();
    let rect = page.layout_rect_of(text).unwrap();
    assert!(rect.w > 0. && rect.w < 380., "{rect:?}");
    assert!(rect.h > 0.);
    assert_eq!(page.layout_rect_of(inline), Some(rect));
}
