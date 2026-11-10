//! WebSocket and EventSource connections from the network stack
//! (`HostEnv::open_websocket` / `open_event_source`), polled from the event
//! loop; events are handed to the JavaScript interfaces in `js/net.js`
//! through a hook.

use crate::env::with_env;
use crate::host::*;
use rabbithole_js::{JsResult, ObjectId, Realm, Value};
use rabbithole_net::{EventSource, SseEvent, WebSocket, WsEvent, WsMessage};
use std::collections::HashMap;

pub enum Conn {
    Ws(WebSocket),
    Sse(EventSource),
}

#[derive(Default)]
pub struct Sockets {
    conns: HashMap<u32, (Conn, Value)>,
    next: u32,
    hook: Option<Value>,
}

fn with_sockets<R>(f: impl FnOnce(&mut Sockets) -> R) -> R {
    let st = state();
    let mut st = st.borrow_mut();
    f(&mut st.sockets)
}

fn add(realm: &mut Realm, conn: Conn, obj: Value) -> Value {
    root_value(realm, &obj);
    let id = with_sockets(|s| {
        s.next += 1;
        s.conns.insert(s.next, (conn, obj));
        s.next
    });
    Value::Number(id as f64)
}

/// `wsOpen(obj, url, protocols)` → id, `null` (no WebSocket support) or a
/// string error (connection refused synchronously).
fn ws_open(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let obj = arg(args, 0);
    let url = arg_str(realm, args, 1)?;
    let Some(u) = rabbithole_net::Url::parse_absolute(&url) else { return Ok(Value::str("invalid URL")) };
    let protos = arg(args, 2);
    let mut protocols = Vec::new();
    if protos.is_object() {
        for v in realm.iterate_to_vec(protos)? {
            protocols.push(realm.to_rust_string(v)?);
        }
    }
    match with_env(|env| env.open_websocket(u, protocols)) {
        None => Ok(Value::Null),
        Some(Err(e)) => Ok(Value::str(&e)),
        Some(Ok(ws)) => Ok(add(realm, Conn::Ws(ws), obj)),
    }
}

/// `sseOpen(obj, url, withCredentials)` → id, `null` or error string.
fn sse_open(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let obj = arg(args, 0);
    let url = arg_str(realm, args, 1)?;
    let creds = realm.to_boolean(&arg(args, 2));
    let Some(u) = rabbithole_net::Url::parse_absolute(&url) else { return Ok(Value::str("invalid URL")) };
    match with_env(|env| env.open_event_source(u, creds)) {
        None => Ok(Value::Null),
        Some(Err(e)) => Ok(Value::str(&e)),
        Some(Ok(es)) => Ok(add(realm, Conn::Sse(es), obj)),
    }
}

fn id_of(realm: &mut Realm, args: &[Value]) -> JsResult<u32> {
    Ok(realm.to_number(arg(args, 0))? as u32)
}

/// `connInfo(id)` → { readyState, bufferedAmount, protocol, extensions }.
fn conn_info(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = id_of(realm, args)?;
    let info = with_sockets(|s| {
        s.conns.get(&id).map(|(c, _)| match c {
            Conn::Ws(ws) => (ws.ready_state(), ws.buffered_amount(), ws.protocol(), ws.extensions()),
            Conn::Sse(es) => (es.ready_state(), 0, String::new(), String::new()),
        })
    });
    match info {
        Some((state, buffered, protocol, ext)) => Ok(object_from(realm, vec![("readyState", Value::Number(state as f64)), ("bufferedAmount", Value::Number(buffered as f64)), ("protocol", Value::str(&protocol)), ("extensions", Value::str(&ext))])),
        None => Ok(Value::Null),
    }
}

/// `wsSend(id, data, isText)`: text (string) or binary (ArrayBuffer/view/Blob bytes).
fn ws_send(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = id_of(realm, args)?;
    let text = realm.to_boolean(&arg(args, 2));
    let payload = if text { Err(arg_str(realm, args, 1)?) } else { Ok(crate::fetch::body_bytes_from_value(realm, &arg(args, 1))?.0) };
    let r = with_sockets(|s| match s.conns.get(&id) {
        Some((Conn::Ws(ws), _)) => match &payload {
            Err(t) => ws.send_text(t),
            Ok(b) => ws.send_binary(b),
        },
        _ => Ok(()),
    });
    if let Err(e) = r {
        return Err(crate::misc::dom_exception(realm, "InvalidStateError", &e));
    }
    Ok(Value::Undefined)
}

/// `connClose(id, code?, reason?)`
fn conn_close(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = id_of(realm, args)?;
    let code = match arg(args, 1) {
        Value::Undefined | Value::Null => None,
        v => Some(realm.to_number(v)? as u16),
    };
    let reason = arg_str_opt(realm, args, 2)?.unwrap_or_default();
    let sse = with_sockets(|s| match s.conns.get(&id) {
        Some((Conn::Ws(ws), _)) => {
            let _ = ws.close(code, &reason);
            false
        }
        Some((Conn::Sse(es), _)) => {
            es.close();
            true
        }
        None => false,
    });
    if sse {
        // A closed EventSource dispatches nothing more.
        if let Some((_, obj)) = with_sockets(|s| s.conns.remove(&id)) {
            unroot_value(realm, &obj);
        }
    }
    Ok(Value::Undefined)
}

fn set_hook(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let f = arg(args, 0);
    root_value(realm, &f);
    with_sockets(|s| s.hook = Some(f));
    Ok(Value::Undefined)
}

/// Poll every connection and dispatch its events (event loop turn).
/// Returns whether anything was delivered.
pub(crate) fn poll(realm: &mut Realm) -> bool {
    let (hook, batch): (Option<Value>, Vec<(u32, Value, Vec<Event>)>) = with_sockets(|s| {
        let batch = s
            .conns
            .iter()
            .map(|(id, (c, obj))| {
                let evs: Vec<Event> = match c {
                    Conn::Ws(ws) => ws.poll().into_iter().map(Event::Ws).collect(),
                    Conn::Sse(es) => es.poll().into_iter().map(Event::Sse).collect(),
                };
                (*id, obj.clone(), evs)
            })
            .filter(|(_, _, e)| !e.is_empty())
            .collect();
        (s.hook.clone(), batch)
    });
    let Some(hook) = hook else { return false };
    let mut any = false;
    for (id, obj, events) in batch {
        for ev in events {
            any = true;
            let args: Vec<Value> = match ev {
                Event::Ws(WsEvent::Open { protocol, extensions }) => vec![obj.clone(), Value::str("open"), Value::str(&protocol), Value::str(&extensions)],
                Event::Ws(WsEvent::Message(WsMessage::Text(t))) => vec![obj.clone(), Value::str("message"), Value::str(&t)],
                Event::Ws(WsEvent::Message(WsMessage::Binary(b))) => {
                    let ab = crate::fetch::make_array_buffer(realm, b);
                    vec![obj.clone(), Value::str("binary"), ab]
                }
                Event::Ws(WsEvent::Error(e)) => vec![obj.clone(), Value::str("error"), Value::str(&e)],
                Event::Ws(WsEvent::Close { code, reason, was_clean }) => {
                    if let Some((_, o)) = with_sockets(|s| s.conns.remove(&id)) {
                        unroot_value(realm, &o);
                    }
                    vec![obj.clone(), Value::str("close"), Value::Number(code as f64), Value::str(&reason), Value::Bool(was_clean)]
                }
                Event::Sse(SseEvent::Open) => vec![obj.clone(), Value::str("open")],
                Event::Sse(SseEvent::Message { event, data, last_event_id, origin }) => vec![obj.clone(), Value::str("sse"), Value::str(&event), Value::str(&data), Value::str(&last_event_id), Value::str(&origin)],
                Event::Sse(SseEvent::Error { reconnecting, message }) => {
                    if !reconnecting {
                        if let Some((_, o)) = with_sockets(|s| s.conns.remove(&id)) {
                            unroot_value(realm, &o);
                        }
                    }
                    vec![obj.clone(), Value::str("error"), Value::str(&message), Value::Bool(reconnecting)]
                }
            };
            call_logged(realm, &hook, Value::Undefined, &args, "socket event");
        }
    }
    any
}

enum Event {
    Ws(WsEvent),
    Sse(SseEvent),
}

pub(crate) fn has_connections() -> bool {
    with_sockets(|s| !s.conns.is_empty())
}

pub(crate) fn install(realm: &mut Realm, o: ObjectId) {
    realm.define_method(o, "wsOpen", 3, ws_open);
    realm.define_method(o, "sseOpen", 3, sse_open);
    realm.define_method(o, "connInfo", 1, conn_info);
    realm.define_method(o, "wsSend", 3, ws_send);
    realm.define_method(o, "connClose", 3, conn_close);
    realm.define_method(o, "setSocketHook", 1, set_hook);
}

impl HostState {
    /// No open connection to poll.
    pub fn sockets_idle(&self) -> bool {
        self.sockets.conns.is_empty()
    }
}
