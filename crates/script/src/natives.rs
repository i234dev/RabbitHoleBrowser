//! `__rhNative`: internal helpers the JavaScript-implemented Web APIs (the
//! `js/*.js` files) use to reach the DOM / engine. The global is removed
//! once those files have captured it.

use crate::env::with_env;
use rabbithole_js::V;
use crate::host::*;
use rabbithole_dom::Document;
use rabbithole_js::{JsResult, Realm, Value};

/// State of the "update the rendering" hook (Intersection/Resize observers).
#[derive(Default)]
pub struct RenderState {
    pub hook: Option<Value>,
    pub active: bool,
    pub dirty: bool,
    pub last_run: f64,
    pub last_scroll: (f32, f32),
    pub last_viewport: (f32, f32),
}

/// Run the rendering hook if observers are active and something changed
/// (DOM, scroll, viewport) at most once per frame. Returns whether it ran.
pub(crate) fn update_rendering(realm: &mut Realm, now: f64) -> bool {
    let (hook, due) = {
        let st = state();
        let st = st.borrow();
        let r = &st.render;
        (r.hook.clone(), r.active && now - r.last_run >= 16.0)
    };
    let Some(hook) = hook else { return false };
    if !due {
        return false;
    }
    let (scroll, viewport) = with_env(|env| (env.scroll(), env.viewport()));
    {
        let st = state();
        let mut st = st.borrow_mut();
        let r = &mut st.render;
        let changed = r.dirty || scroll != r.last_scroll || viewport != r.last_viewport;
        if !changed {
            return false;
        }
        r.dirty = false;
        r.last_run = now;
        r.last_scroll = scroll;
        r.last_viewport = viewport;
    }
    call_logged(realm, &hook, Value::Undefined, &[], "update the rendering");
    true
}

/// When the rendering hook will next want to run (if pending).
pub(crate) fn render_due(st: &HostState) -> Option<f64> {
    let r = &st.render;
    (r.active && r.hook.is_some() && r.dirty).then(|| r.last_run + 16.0)
}

/// The DOM node a `Document` / `ShadowRoot` / element argument refers to.
fn target_node(realm: &mut Realm, v: &Value) -> JsResult<NodeRef> {
    match node_of(realm, v) {
        Some(n) => Ok(n),
        None => Err(realm.throw_type_error("expected a node")),
    }
}

fn string_list(realm: &mut Realm, v: &Value) -> JsResult<Vec<String>> {
    let mut out = Vec::new();
    if !v.is_object() {
        return Ok(out);
    }
    let len = realm.get_str(v.clone(), "length")?;
    let len = realm.to_number(len)? as usize;
    for i in 0..len.min(100_000) {
        let s = realm.get_str(v.clone(), &i.to_string())?;
        match s.v() {
            V::String(js) if js.len() >= CACHED_TEXT_MIN => out.push(cached_text(js)),
            _ => out.push(realm.to_rust_string(s)?),
        }
    }
    Ok(out)
}

/// Strings at least this long go through [`cached_text`].
const CACHED_TEXT_MIN: usize = 1024;

thread_local! {
    /// Recently converted large strings (stylesheet texts handed over by
    /// `adoptedStyleSheets` again and again), most recent last.
    static TEXT_CACHE: std::cell::RefCell<Vec<(rabbithole_js::JsStr, std::rc::Rc<str>)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The Rust form of a large string, converting each distinct JS string
/// (by identity, then by contents) from UTF-16 only once.
fn cached_text(js: &rabbithole_js::JsStr) -> String {
    const MAX_ENTRIES: usize = 64;
    const MAX_UNITS: usize = 16 << 20;
    TEXT_CACHE.with(|c| {
        let mut c = c.borrow_mut();
        let hit = c.iter().rposition(|(k, _)| k.ptr_eq(js)).or_else(|| {
            let units = js.as_units();
            c.iter().rposition(|(k, _)| k.len() == units.len() && k.as_units() == units)
        });
        if let Some(i) = hit {
            let e = c.remove(i);
            let text = e.1.to_string();
            c.push((js.clone(), e.1));
            return text;
        }
        let text = js.to_rust_string();
        c.push((js.clone(), std::rc::Rc::from(text.as_str())));
        let mut total: usize = c.iter().map(|(k, _)| k.len()).sum();
        while c.len() > MAX_ENTRIES || (total > MAX_UNITS && c.len() > 1) {
            total -= c[0].0.len();
            c.remove(0);
        }
        text
    })
}

/// `setAdoptedStyles(documentOrShadowRoot, [cssText...])`
fn set_adopted_styles(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let n = target_node(realm, &arg(args, 0))?;
    let texts = string_list(realm, &arg(args, 1))?;
    let changed = with_doc(n.doc, |doc| {
        if texts.is_empty() {
            doc.adopted_styles.remove(&n.node).is_some()
        } else if doc.adopted_styles.get(&n.node) == Some(&texts) {
            // Same sheets again (frameworks re-adopt on every render):
            // nothing to restyle.
            false
        } else {
            doc.adopted_styles.insert(n.node, texts);
            true
        }
    });
    if n.doc == 0 && changed {
        mark_dirty();
    }
    Ok(Value::Undefined)
}

/// `setSheetText(styleOrLinkElement, cssText | null)`: the CSSOM of an
/// element's sheet was modified (insertRule, ...): render `cssText` instead
/// of the element's own source while that source is unchanged.
fn set_sheet_text(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let n = target_node(realm, &arg(args, 0))?;
    let text = arg(args, 1);
    let text = if text.is_nullish() { None } else { Some(realm.to_rust_string(text)?) };
    with_doc(n.doc, |doc| {
        let source = doc.text_content(n.node);
        match text {
            Some(t) => {
                doc.sheet_overrides.insert(n.node, (source, t));
            }
            None => {
                doc.sheet_overrides.remove(&n.node);
            }
        }
    });
    if n.doc == 0 {
        mark_dirty();
    }
    Ok(Value::Undefined)
}

/// `fetchText(url)`: synchronous GET through the page's fetcher (cached
/// stylesheets). Returns the body text or null.
fn fetch_text(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let url = arg_str(realm, args, 0)?;
    let Some(u) = resolve_url(&url) else { return Ok(Value::Null) };
    let r = with_env(|env| env.fetch(rabbithole_net::Request::get(u)));
    match r {
        Ok(resp) if resp.status < 400 => Ok(Value::str(&rabbithole_net::decode_text(&resp.body, resp.charset.as_deref()))),
        _ => Ok(Value::Null),
    }
}

/// `isShadowRoot(node)`
fn is_shadow_root(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(node_of(realm, &arg(args, 0)).map(crate::shadow::is_shadow_root).unwrap_or(false)))
}

/// `nodeKey(node)`: a stable number identifying a node (for JS maps).
fn node_key(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(match node_of(realm, &arg(args, 0)) {
        Some(n) => Value::Number((n.doc as f64) * 4294967296.0 + n.node as f64),
        None => Value::Null,
    })
}

/// `rootNode(node)`: the node's tree root (document or shadow root).
fn document_root(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(wrap_main(realm, Document::ROOT))
}

/// `setRenderHook(fn)`: called during "update the rendering" (observers).
fn set_render_hook(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let f = arg(args, 0);
    root_value(realm, &f);
    state().borrow_mut().render.hook = Some(f);
    Ok(Value::Undefined)
}

/// `setRenderObservers(active)`: whether any IntersectionObserver /
/// ResizeObserver is observing (the host only runs the hook then).
fn set_render_observers(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let on = realm.to_boolean(&arg(args, 0));
    state().borrow_mut().render.active = on;
    Ok(Value::Undefined)
}

/// `requestRenderUpdate()`: run the hook at the next rendering opportunity.
fn request_render_update(_realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    state().borrow_mut().render.dirty = true;
    Ok(Value::Undefined)
}

/// `setFactory(name, fn)`: JS constructors the natives use to build
/// objects (e.g. "rect" for DOMRect).
fn set_factory(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let name = arg_str(realm, args, 0)?;
    let f = arg(args, 1);
    root_value(realm, &f);
    state().borrow_mut().factories.insert(name, f);
    Ok(Value::Undefined)
}

/// `installHandlers(prototype, ["click", ...])`: `onclick`-style IDL
/// event handler attributes (shared with the native ones).
fn install_handlers(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let V::Object(target) = (arg(args, 0)).v() else {
        return Err(realm.throw_type_error("installHandlers: object expected"));
    };
    let names = string_list(realm, &arg(args, 1))?;
    let leaked: Vec<&'static str> = names.into_iter().map(|n| &*Box::leak(n.into_boxed_str())).collect();
    crate::events::install_handler_props(realm, target, &leaked);
    Ok(Value::Undefined)
}

/// `markHTMLDDA(object)`: give it [[IsHTMLDDA]] (`document.all`).
fn mark_htmldda(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if let Some(o) = arg(args, 0).as_object() {
        realm.make_htmldda(o);
    }
    Ok(Value::Undefined)
}

/// `registerProto(name, prototype)`: make a JS-defined interface the
/// prototype natives use for `name` (wrappers, events).
fn register_proto(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let name = arg_str(realm, args, 0)?;
    let V::Object(p) = (arg(args, 1)).v() else {
        return Err(realm.throw_type_error("registerProto: object expected"));
    };
    realm.extra_roots.push(Value::Object(p));
    state().borrow_mut().protos.insert(Box::leak(name.into_boxed_str()), p);
    Ok(Value::Undefined)
}

/// `setCanvasImage(canvas, width, height, rgbaBytes | null)`: the bitmap
/// to render for a `<canvas>`.
fn set_canvas_image(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let n = target_node(realm, &arg(args, 0))?;
    if n.doc != 0 {
        return Ok(Value::Undefined);
    }
    let w = realm.to_number(arg(args, 1))? as u32;
    let h = realm.to_number(arg(args, 2))? as u32;
    let data = arg(args, 3);
    let rgba = if data.is_nullish() { None } else { Some(crate::fetch::body_bytes_from_value(realm, &data)?.0) };
    let rgba = rgba.filter(|b| b.len() == (w as usize) * (h as usize) * 4 && w > 0 && h > 0);
    with_env(|env| env.set_canvas_image(n.node, w, h, rgba));
    Ok(Value::Undefined)
}

/// `imagePixels(img)` → `{ width, height, data: ArrayBuffer }` or null.
fn image_pixels(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(n) = node_of(realm, &arg(args, 0)) else { return Ok(Value::Null) };
    if n.doc != 0 {
        return Ok(Value::Null);
    }
    match with_env(|env| env.image_pixels(n.node)) {
        Some((w, h, rgba)) => {
            let ab = crate::fetch::make_array_buffer(realm, rgba);
            Ok(object_from(realm, vec![("width", Value::Number(w as f64)), ("height", Value::Number(h as f64)), ("data", ab)]))
        }
        None => Ok(Value::Null),
    }
}

/// `setScriptFontFaces(cssText)`: @font-face rules of `document.fonts`.
fn set_script_font_faces(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let text = arg_str(realm, args, 0)?;
    with_doc(0, |doc| {
        if text.is_empty() {
            doc.adopted_styles.remove(&Document::SCRIPT_FONT_FACES);
        } else {
            doc.adopted_styles.insert(Document::SCRIPT_FONT_FACES, vec![text]);
        }
    });
    mark_dirty();
    Ok(Value::Undefined)
}

/// Compression formats of `CompressionStream` / `DecompressionStream`.
fn compression_format(realm: &mut Realm, v: &Value) -> JsResult<u8> {
    match realm.to_rust_string(v.clone())?.as_str() {
        "gzip" => Ok(0),
        "deflate" => Ok(1),
        "deflate-raw" => Ok(2),
        f => Err(realm.throw_type_error(&format!("Unsupported compression format: '{f}'"))),
    }
}

/// `__rhNative.inflate(bytes, format)`: the decompressed bytes as an
/// ArrayBuffer (TypeError on corrupt or truncated data).
fn native_inflate(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    use std::io::Read;
    let (data, _) = crate::fetch::body_bytes_from_value(realm, &arg(args, 0))?;
    let format = compression_format(realm, &arg(args, 1))?;
    let mut out = Vec::new();
    let r = match format {
        0 => flate2::read::MultiGzDecoder::new(&data[..]).read_to_end(&mut out),
        1 => flate2::read::ZlibDecoder::new(&data[..]).read_to_end(&mut out),
        _ => flate2::read::DeflateDecoder::new(&data[..]).read_to_end(&mut out),
    };
    if let Err(e) = r {
        return Err(realm.throw_type_error(&format!("Failed to decompress data: {e}")));
    }
    Ok(crate::fetch::make_array_buffer(realm, out))
}

/// `__rhNative.deflate(bytes, format)`: the compressed bytes as an
/// ArrayBuffer.
fn native_deflate(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    use std::io::Write;
    let (data, _) = crate::fetch::body_bytes_from_value(realm, &arg(args, 0))?;
    let format = compression_format(realm, &arg(args, 1))?;
    let level = flate2::Compression::default();
    let out = match format {
        0 => {
            let mut e = flate2::write::GzEncoder::new(Vec::new(), level);
            e.write_all(&data).and_then(|_| e.finish())
        }
        1 => {
            let mut e = flate2::write::ZlibEncoder::new(Vec::new(), level);
            e.write_all(&data).and_then(|_| e.finish())
        }
        _ => {
            let mut e = flate2::write::DeflateEncoder::new(Vec::new(), level);
            e.write_all(&data).and_then(|_| e.finish())
        }
    };
    match out {
        Ok(bytes) => Ok(crate::fetch::make_array_buffer(realm, bytes)),
        Err(e) => Err(realm.throw_type_error(&format!("Failed to compress data: {e}"))),
    }
}

pub(crate) fn install(realm: &mut Realm) {
    let o = realm.new_object();
    realm.define_method(o, "inflate", 2, native_inflate);
    realm.define_method(o, "deflate", 2, native_deflate);
    crate::sockets::install(realm, o);
    crate::media::install(realm, o);
    crate::canvas2d::install(realm, o);
    realm.define_method(o, "setScriptFontFaces", 1, set_script_font_faces);
    realm.define_method(o, "setCanvasImage", 4, set_canvas_image);
    realm.define_method(o, "imagePixels", 1, image_pixels);
    realm.define_method(o, "installHandlers", 2, install_handlers);
    realm.define_method(o, "markHTMLDDA", 1, mark_htmldda);
    realm.define_method(o, "registerProto", 2, register_proto);
    realm.define_method(o, "setRenderHook", 1, set_render_hook);
    realm.define_method(o, "setRenderObservers", 1, set_render_observers);
    realm.define_method(o, "requestRenderUpdate", 0, request_render_update);
    realm.define_method(o, "setFactory", 2, set_factory);
    realm.define_method(o, "setAdoptedStyles", 2, set_adopted_styles);
    realm.define_method(o, "setSheetText", 2, set_sheet_text);
    realm.define_method(o, "fetchText", 1, fetch_text);
    realm.define_method(o, "isShadowRoot", 1, is_shadow_root);
    realm.define_method(o, "nodeKey", 1, node_key);
    realm.define_method(o, "documentNode", 0, document_root);
    crate::workers::install_natives(realm, o);
    crate::live::install(realm, o);
    realm.define_method(o, "createDoctype", 4, crate::document::create_doctype);
    realm.define_method(o, "createXMLDocument", 0, crate::document::create_xml_document);
    realm.define_method(o, "validateQName", 1, crate::document::validate_qname);
    let g = realm.global();
    realm.define_property(g, "__rhNative", Value::Object(o), rabbithole_js::CONFIGURABLE | rabbithole_js::WRITABLE);
}
