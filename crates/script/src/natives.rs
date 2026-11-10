//! `__rhNative`: internal helpers the JavaScript-implemented Web APIs (the
//! `js/*.js` files) use to reach the DOM / engine. The global is removed
//! once those files have captured it.

use crate::env::with_env;
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
        out.push(realm.to_rust_string(s)?);
    }
    Ok(out)
}

/// `setAdoptedStyles(documentOrShadowRoot, [cssText...])`
fn set_adopted_styles(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let n = target_node(realm, &arg(args, 0))?;
    let texts = string_list(realm, &arg(args, 1))?;
    with_doc(n.doc, |doc| {
        if texts.is_empty() {
            doc.adopted_styles.remove(&n.node);
        } else {
            doc.adopted_styles.insert(n.node, texts);
        }
    });
    if n.doc == 0 {
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
    let Value::Object(target) = arg(args, 0) else {
        return Err(realm.throw_type_error("installHandlers: object expected"));
    };
    let names = string_list(realm, &arg(args, 1))?;
    let leaked: Vec<&'static str> = names.into_iter().map(|n| &*Box::leak(n.into_boxed_str())).collect();
    crate::events::install_handler_props(realm, target, &leaked);
    Ok(Value::Undefined)
}

/// `registerProto(name, prototype)`: make a JS-defined interface the
/// prototype natives use for `name` (wrappers, events).
fn register_proto(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let name = arg_str(realm, args, 0)?;
    let Value::Object(p) = arg(args, 1) else {
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

pub(crate) fn install(realm: &mut Realm) {
    let o = realm.new_object();
    realm.define_method(o, "setCanvasImage", 4, set_canvas_image);
    realm.define_method(o, "imagePixels", 1, image_pixels);
    realm.define_method(o, "installHandlers", 2, install_handlers);
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
    let g = realm.global();
    realm.define_property(g, "__rhNative", Value::Object(o), rabbithole_js::CONFIGURABLE | rabbithole_js::WRITABLE);
}
