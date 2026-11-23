//! Reusable WebCrypto X25519 private keys and contribution-checked agreement.
use crate::fetch::{buffer_source_bytes, make_array_buffer};
use crate::host::arg;
use crate::misc::dom_exception;
use rabbithole_js::{JsResult, ObjectId, Realm, Value};
use ring::rand::{SecureRandom, SystemRandom};
use x25519_dalek::{PublicKey, StaticSecret};

struct Key {
    secret: StaticSecret,
    public: PublicKey,
    extractable: bool,
}

fn bytes32(realm: &mut Realm, value: &Value) -> JsResult<[u8; 32]> {
    buffer_source_bytes(realm, value, "X25519 key")?.try_into()
        .map_err(|_| dom_exception(realm, "DataError", "X25519 keys must contain 32 bytes"))
}

fn key(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let input = arg(args, 0);
    let seed = if input.is_nullish() {
        let mut bytes = [0u8; 32];
        SystemRandom::new().fill(&mut bytes)
            .map_err(|_| dom_exception(realm, "OperationError", "Secure key generation failed"))?;
        bytes
    } else {
        bytes32(realm, &input)?
    };
    let secret = StaticSecret::from(seed);
    let public = PublicKey::from(&secret);
    if !arg(args, 1).is_nullish() && bytes32(realm, &arg(args, 1))? != public.to_bytes() {
        return Err(dom_exception(realm, "DataError", "X25519 public and private keys do not match"));
    }
    let extractable = realm.to_boolean(&arg(args, 2));
    let id = realm.new_object();
    realm.set_host_data(id, Box::new(Key { secret, public, extractable }));
    Ok(Value::Object(id))
}

fn key_id(realm: &mut Realm, value: Value) -> JsResult<ObjectId> {
    value.as_object().filter(|id| realm.host_data::<Key>(*id).is_some())
        .ok_or_else(|| realm.throw_type_error("Expected an X25519 private key"))
}

fn key_bytes(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let id = key_id(realm, arg(args, 0))?;
    let private = realm.to_boolean(&arg(args, 1));
    let key = realm.host_data::<Key>(id).unwrap();
    let bytes = if private {
        if !key.extractable { return Err(dom_exception(realm, "InvalidAccessError", "Key is not extractable")); }
        key.secret.as_bytes()
    } else {
        key.public.as_bytes()
    }.to_vec();
    Ok(make_array_buffer(realm, bytes))
}

fn derive(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let public = PublicKey::from(bytes32(realm, &arg(args, 1))?);
    let id = key_id(realm, arg(args, 0))?;
    let key = realm.host_data::<Key>(id).unwrap();
    let shared = key.secret.diffie_hellman(&public);
    if !shared.was_contributory() {
        return Err(dom_exception(realm, "OperationError", "The peer contributed no key material"));
    }
    Ok(make_array_buffer(realm, shared.as_bytes().to_vec()))
}

pub(super) fn install(realm: &mut Realm, internals: ObjectId) {
    realm.define_method(internals, "x25519Key", 3, key);
    realm.define_method(internals, "x25519KeyBytes", 2, key_bytes);
    realm.define_method(internals, "x25519Derive", 2, derive);
}
