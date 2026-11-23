//! Native peer handles are owned by the realm and closed on explicit close
//! or when the wrapper is collected. No JS callback runs while a handle is borrowed.
use crate::fetch::{buffer_source_bytes, make_array_buffer};
use crate::host::{arg, arg_str};
use rabbithole_js::{JsResult, ObjectId, Realm, Value};
use rabbithole_rtc::{ChannelConfig, IceConnectionState, Peer, PeerEvent, Reliability};
use std::io;
use std::net::{IpAddr, Ipv4Addr};

fn operation<T>(realm: &mut Realm, result: io::Result<T>) -> JsResult<T> {
    result.map_err(|e| crate::misc::dom_exception(realm, "OperationError", &e.to_string()))
}

fn with_peer<T>(
    realm: &mut Realm,
    args: &[Value],
    f: impl FnOnce(&mut Peer) -> io::Result<T>,
) -> JsResult<T> {
    let peer = arg(args, 0)
        .as_object()
        .and_then(|id| realm.host_data_mut::<Option<Peer>>(id))
        .and_then(Option::as_mut);
    let Some(peer) = peer else {
        return Err(crate::misc::dom_exception(
            realm,
            "InvalidStateError",
            "The peer is closed",
        ));
    };
    let result = f(peer);
    operation(realm, result)
}

fn create(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    // Until the browser has mDNS masking or an interface disclosure permission,
    // only advertise loopback host candidates. Relay-only disables host ICE.
    let ips = [IpAddr::V4(Ipv4Addr::LOCALHOST)];
    let addresses = if realm.to_boolean(&arg(args, 0)) {
        &ips[..]
    } else {
        &[]
    };
    let peer = operation(realm, Peer::bind(addresses))?;
    let id = realm.new_object();
    realm.set_host_data(id, Box::new(Some(peer)));
    Ok(Value::Object(id))
}

fn close(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    if let Some(p) = arg(args, 0)
        .as_object()
        .and_then(|id| realm.host_data_mut::<Option<Peer>>(id))
    {
        if let Some(mut peer) = p.take() { let _ = peer.close(); }
    }
    Ok(Value::Undefined)
}

fn channel(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let label = arg_str(realm, args, 1)?;
    let protocol = arg_str(realm, args, 2)?;
    let ordered = realm.to_boolean(&arg(args, 3));
    let optional = |realm: &mut Realm, i| -> JsResult<Option<u16>> {
        let v = arg(args, i);
        if v.is_nullish() {
            Ok(None)
        } else {
            Ok(Some(realm.to_number(v)? as u16))
        }
    };
    let negotiated = optional(realm, 4)?;
    let reliability = match (optional(realm, 5)?, optional(realm, 6)?) {
        (Some(lifetime), None) => Reliability::MaxPacketLifetime { lifetime },
        (None, Some(retransmits)) => Reliability::MaxRetransmits { retransmits },
        (None, None) => Reliability::Reliable,
        _ => return Err(realm.throw_type_error("Conflicting reliability options")),
    };
    let id = with_peer(realm, args, |p| {
        p.add_channel(ChannelConfig {
            label,
            protocol,
            ordered,
            negotiated,
            reliability,
        })
    })?;
    Ok(Value::Number(id as f64))
}

fn offer(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::str(&with_peer(realm, args, Peer::create_offer)?))
}
fn answer(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let sdp = arg_str(realm, args, 1)?;
    Ok(Value::str(&with_peer(realm, args, |p| {
        p.answer_offer(&sdp)
    })?))
}
fn accept(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let sdp = arg_str(realm, args, 1)?;
    with_peer(realm, args, |p| p.accept_answer(&sdp))?;
    Ok(Value::Undefined)
}
fn candidate(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let text = arg_str(realm, args, 1)?;
    with_peer(realm, args, |p| p.add_candidate(&text))?;
    Ok(Value::Undefined)
}
fn send(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let handle = realm.to_number(arg(args, 1))? as u32;
    let binary = realm.to_boolean(&arg(args, 2));
    let bytes = buffer_source_bytes(realm, &arg(args, 3), "RTCDataChannel data")?;
    Ok(Value::Bool(with_peer(realm, args, |p| {
        p.send(handle, binary, &bytes)
    })?))
}
fn close_channel(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let id = realm.to_number(arg(args, 1))? as u32;
    with_peer(realm, args, |p| {
        p.close_channel(id);
        Ok(())
    })?;
    Ok(Value::Undefined)
}

fn poll(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let mut events = Vec::new();
    for e in with_peer(realm, args, Peer::poll)? {
        let index = |n: u32| Value::Number(n as f64);
        let tuple = match e {
            PeerEvent::Ice(state) => vec![
                Value::str("ice"),
                Value::str(match state {
                    IceConnectionState::New => "new",
                    IceConnectionState::Checking => "checking",
                    IceConnectionState::Connected => "connected",
                    IceConnectionState::Completed => "completed",
                    IceConnectionState::Disconnected => "disconnected",
                }),
            ],
            PeerEvent::Closed => vec![Value::str("closed")],
            PeerEvent::SelectedPair { local, remote } => vec![
                Value::str("pair"), Value::str(&local.ip().to_string()), index(local.port().into()),
                Value::str(&remote.ip().to_string()), index(remote.port().into()),
            ],
            PeerEvent::Connected => vec![Value::str("connected")],
            PeerEvent::Open {
                handle,
                stream,
                config,
            } => {
                let (life, retries) = match config.reliability {
                    Reliability::Reliable => (Value::Null, Value::Null),
                    Reliability::MaxPacketLifetime { lifetime } => {
                        (index(lifetime.into()), Value::Null)
                    }
                    Reliability::MaxRetransmits { retransmits } => {
                        (Value::Null, index(retransmits.into()))
                    }
                };
                vec![
                    Value::str("open"),
                    index(handle),
                    index(stream.into()),
                    Value::str(&config.label),
                    Value::str(&config.protocol),
                    Value::Bool(config.ordered),
                    life,
                    retries,
                ]
            }
            PeerEvent::Message {
                handle,
                binary,
                data,
            } => vec![
                Value::str("message"),
                index(handle),
                Value::Bool(binary),
                make_array_buffer(realm, data),
            ],
            PeerEvent::Close(handle) => vec![Value::str("close"), index(handle)],
            PeerEvent::BufferedLow(handle) => vec![Value::str("bufferedamountlow"), index(handle)],
        };
        events.push(realm.new_array_value(tuple));
    }
    Ok(realm.new_array_value(events))
}

pub(crate) fn install(realm: &mut Realm, o: ObjectId) {
    realm.define_property(o, "rtcMessageCapacity", Value::Number(rabbithole_rtc::MESSAGE_CAPACITY.into()), 0);
    realm.define_method(o, "rtcCreate", 1, create);
    realm.define_method(o, "rtcClose", 1, close);
    realm.define_method(o, "rtcChannel", 7, channel);
    realm.define_method(o, "rtcOffer", 1, offer);
    realm.define_method(o, "rtcAnswer", 2, answer);
    realm.define_method(o, "rtcAccept", 2, accept);
    realm.define_method(o, "rtcCandidate", 2, candidate);
    realm.define_method(o, "rtcSend", 4, send);
    realm.define_method(o, "rtcCloseChannel", 2, close_channel);
    realm.define_method(o, "rtcPoll", 1, poll);
}
