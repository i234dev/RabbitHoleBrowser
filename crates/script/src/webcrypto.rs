//! Native hash and HMAC operations shared by WebCrypto key derivation and
//! signatures. A PBKDF2 operation can yield between batches of PRF calls.
mod ecdh;
mod ecdsa;
mod gcm;
mod x25519;

use crate::fetch::{buffer_source_bytes, make_array_buffer};
use crate::host::{arg, arg_str};
use rabbithole_js::{JsResult, ObjectId, Realm, Value, V};
use ring::{digest, hmac, rand::{SecureRandom, SystemRandom}, signature::{self, Ed25519KeyPair, KeyPair}};

fn algorithms(realm: &mut Realm, args: &[Value]) -> JsResult<(&'static digest::Algorithm, hmac::Algorithm)> {
    Ok(match arg_str(realm, args, 0)?.as_str() {
        "SHA-1" => (&digest::SHA1_FOR_LEGACY_USE_ONLY, hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY),
        "SHA-256" => (&digest::SHA256, hmac::HMAC_SHA256),
        "SHA-384" => (&digest::SHA384, hmac::HMAC_SHA384),
        "SHA-512" => (&digest::SHA512, hmac::HMAC_SHA512),
        _ => return Err(realm.throw_type_error("Unsupported hash")),
    })
}

// WebCrypto uses Web IDL's copy of a BufferSource, including the empty
// sequence for detached buffers. Read internal slots without invoking script.
fn copy_buffer(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let value = arg(args, 0);
    if let V::Object(object) = value.v() {
        use rabbithole_js::ObjectKind;
        let buffer = match &realm.obj(object).kind {
            ObjectKind::TypedArray(view) | ObjectKind::DataView(view) => view.buffer,
            _ => object,
        };
        if matches!(&realm.obj(buffer).kind, ObjectKind::ArrayBuffer(b) if b.shared) {
            return Err(realm.throw_type_error("WebCrypto requires a non-shared buffer"));
        }
    }
    let bytes = buffer_source_bytes(realm, &value, "WebCrypto")?;
    Ok(make_array_buffer(realm, bytes))
}

fn hash(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let (algorithm, _) = algorithms(realm, args)?;
    let data = buffer_source_bytes(realm, &arg(args, 1), "digest")?;
    Ok(make_array_buffer(realm, digest::digest(algorithm, &data).as_ref().to_vec()))
}

fn mac(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let (_, algorithm) = algorithms(realm, args)?;
    let key = hmac::Key::new(algorithm, &buffer_source_bytes(realm, &arg(args, 1), "HMAC")?);
    let data = buffer_source_bytes(realm, &arg(args, 2), "HMAC")?;
    if args.len() > 3 {
        let sig = buffer_source_bytes(realm, &arg(args, 3), "HMAC")?;
        return Ok(Value::Bool(hmac::verify(&key, &data, &sig).is_ok()));
    }
    Ok(make_array_buffer(realm, hmac::sign(&key, &data).as_ref().to_vec()))
}

struct Pbkdf2 {
    key: hmac::Key,
    salt: Vec<u8>,
    iterations: u32,
    length: usize,
    block: u32,
    round: u32,
    previous: Option<hmac::Tag>,
    accumulated: [u8; 64],
    output: Vec<u8>,
}

impl Pbkdf2 {
    fn step(&mut self) -> bool {
        // Limit both the work done by one script task and the memory used
        // before another task can cancel a long-running derivation.
        for _ in 0..2048 {
            let tag = if let Some(previous) = self.previous {
                hmac::sign(&self.key, previous.as_ref())
            } else {
                let mut ctx = hmac::Context::with_key(&self.key);
                ctx.update(&self.salt);
                ctx.update(&self.block.to_be_bytes());
                ctx.sign()
            };
            for (byte, next) in self.accumulated.iter_mut().zip(tag.as_ref()) { *byte ^= next; }
            self.round += 1;
            if self.round == self.iterations {
                let count = (self.length - self.output.len()).min(tag.as_ref().len());
                self.output.extend_from_slice(&self.accumulated[..count]);
                if self.output.len() == self.length { return true; }
                self.block += 1;
                self.round = 0;
                self.previous = None;
                self.accumulated.fill(0);
            } else {
                self.previous = Some(tag);
            }
        }
        false
    }
}

fn pbkdf_start(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let (_, algorithm) = algorithms(realm, args)?;
    let key = hmac::Key::new(algorithm, &buffer_source_bytes(realm, &arg(args, 1), "PBKDF2")?);
    let salt = buffer_source_bytes(realm, &arg(args, 2), "PBKDF2")?;
    let iterations = realm.to_number(arg(args, 3))? as u32;
    let length = realm.to_number(arg(args, 4))? as usize;
    if iterations == 0 || length == 0 { return Err(realm.throw_type_error("Invalid PBKDF2 parameters")); }
    let id = realm.new_object();
    realm.set_host_data(id, Box::new(Pbkdf2 { key, salt, iterations, length, block: 1,
        round: 0, previous: None, accumulated: [0; 64], output: Vec::new() }));
    Ok(Value::Object(id))
}

fn pbkdf_step(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let V::Object(id) = arg(args, 0).v() else { return Err(realm.throw_type_error("Invalid PBKDF2 operation")); };
    let Some(op) = realm.host_data_mut::<Pbkdf2>(id) else { return Err(realm.throw_type_error("Invalid PBKDF2 operation")); };
    if !op.step() { return Ok(Value::Null); }
    let output = std::mem::take(&mut op.output);
    Ok(make_array_buffer(realm, output))
}

struct EdSigningKey {
    pair: Ed25519KeyPair,
    exportable_seed: Option<[u8; 32]>,
}

fn ed_key(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let mut seed = [0u8; 32];
    if arg(args, 0).is_nullish() {
        SystemRandom::new().fill(&mut seed).map_err(|_| crate::misc::dom_exception(realm, "OperationError", "Secure random generation failed"))?;
    } else {
        seed = buffer_source_bytes(realm, &arg(args, 0), "Ed25519 seed")?.try_into()
            .map_err(|_| crate::misc::dom_exception(realm, "DataError", "Wrong Ed25519 seed length"))?;
    }
    let pair = if arg(args, 1).is_nullish() {
        Ed25519KeyPair::from_seed_unchecked(&seed)
    } else {
        let public = buffer_source_bytes(realm, &arg(args, 1), "Ed25519 public key")?;
        Ed25519KeyPair::from_seed_and_public_key(&seed, &public)
    }.map_err(|_| crate::misc::dom_exception(realm, "DataError", "Invalid Ed25519 key pair"))?;
    let exportable_seed = realm.to_boolean(&arg(args, 2)).then_some(seed);
    let id = realm.new_object();
    realm.set_host_data(id, Box::new(EdSigningKey { pair, exportable_seed }));
    Ok(Value::Object(id))
}

fn ed_key_bytes(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let private = realm.to_boolean(&arg(args, 1));
    let V::Object(id) = arg(args, 0).v() else { return Err(realm.throw_type_error("Expected Ed25519 key")) };
    let Some(key) = realm.host_data::<EdSigningKey>(id) else { return Err(realm.throw_type_error("Expected Ed25519 key")) };
    let bytes = if private {
        let Some(seed) = key.exportable_seed else { return Err(crate::misc::dom_exception(realm, "InvalidAccessError", "Key is not extractable")) };
        seed.to_vec()
    } else { key.pair.public_key().as_ref().to_vec() };
    Ok(make_array_buffer(realm, bytes))
}

fn ed_sign(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let message = buffer_source_bytes(realm, &arg(args, 1), "Ed25519 message")?;
    let V::Object(id) = arg(args, 0).v() else { return Err(realm.throw_type_error("Expected Ed25519 key")) };
    let Some(key) = realm.host_data::<EdSigningKey>(id) else { return Err(realm.throw_type_error("Expected Ed25519 key")) };
    let bytes = key.pair.sign(&message).as_ref().to_vec();
    Ok(make_array_buffer(realm, bytes))
}

fn ed_verify(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let public = buffer_source_bytes(realm, &arg(args, 0), "Ed25519 public key")?;
    let message = buffer_source_bytes(realm, &arg(args, 1), "Ed25519 message")?;
    let sig = buffer_source_bytes(realm, &arg(args, 2), "Ed25519 signature")?;
    Ok(Value::Bool(signature::UnparsedPublicKey::new(&signature::ED25519, public).verify(&message, &sig).is_ok()))
}

fn aes_gcm(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let key = buffer_source_bytes(realm, &arg(args, 0), "AES-GCM key")?;
    let iv = buffer_source_bytes(realm, &arg(args, 1), "AES-GCM IV")?;
    let data = buffer_source_bytes(realm, &arg(args, 2), "AES-GCM data")?;
    let aad = buffer_source_bytes(realm, &arg(args, 3), "AES-GCM additional data")?;
    let tag_len = realm.to_number(arg(args, 4))? as usize;
    let decrypt = realm.to_boolean(&arg(args, 5));
    let out = gcm::crypt(&key, &iv, &aad, data, tag_len, decrypt)
        .ok_or_else(|| crate::misc::dom_exception(realm, "OperationError", "AES-GCM operation failed"))?;
    Ok(make_array_buffer(realm, out))
}

fn chacha20_poly1305(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305};
    let key = buffer_source_bytes(realm, &arg(args, 0), "ChaCha20-Poly1305 key")?;
    let iv = buffer_source_bytes(realm, &arg(args, 1), "ChaCha20-Poly1305 IV")?;
    let mut data = buffer_source_bytes(realm, &arg(args, 2), "ChaCha20-Poly1305 data")?;
    let aad = buffer_source_bytes(realm, &arg(args, 3), "ChaCha20-Poly1305 additional data")?;
    let decrypt = realm.to_boolean(&arg(args, 4));
    let cipher = UnboundKey::new(&CHACHA20_POLY1305, &key).map(LessSafeKey::new)
        .map_err(|_| crate::misc::dom_exception(realm,"OperationError","Invalid ChaCha20-Poly1305 key"))?;
    let nonce = Nonce::try_assume_unique_for_key(&iv)
        .map_err(|_| crate::misc::dom_exception(realm,"OperationError","A 12-byte IV is required"))?;
    let result = if decrypt {
        cipher.open_in_place(nonce,Aad::from(&aad),&mut data).map(|plain| plain.len())
    } else {
        cipher.seal_in_place_append_tag(nonce,Aad::from(&aad),&mut data).map(|_| data.len())
    };
    let length = result.map_err(|_| crate::misc::dom_exception(realm,"OperationError","ChaCha20-Poly1305 operation failed"))?;
    data.truncate(length);
    Ok(make_array_buffer(realm,data))
}

pub(crate) fn install(realm: &mut Realm, internals: ObjectId) {
    ecdh::install(realm, internals);
    x25519::install(realm, internals);
    realm.define_method(internals, "chachaPoly1305", 5, chacha20_poly1305);
    realm.define_method(internals, "aesGcm", 6, aes_gcm);
    realm.define_method(internals, "ed25519Key", 3, ed_key);
    realm.define_method(internals, "ed25519KeyBytes", 2, ed_key_bytes);
    realm.define_method(internals, "ed25519Sign", 2, ed_sign);
    realm.define_method(internals, "ed25519Verify", 3, ed_verify);
    realm.define_method(internals, "cryptoCopyBuffer", 1, copy_buffer);
    realm.define_method(internals, "cryptoDigest", 2, hash);
    realm.define_method(internals, "cryptoHmac", 3, mac);
    realm.define_method(internals, "pbkdfStart", 5, pbkdf_start);
    realm.define_method(internals, "pbkdfStep", 1, pbkdf_step);
}
