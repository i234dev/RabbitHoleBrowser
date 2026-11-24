//! Native hash and HMAC operations shared by WebCrypto key derivation and
//! signatures. A PBKDF2 operation can yield between batches of PRF calls.
mod argon2;
mod curve448;
mod ecdh;
mod ecdsa;
mod gcm;
mod keccak;
mod ocb;
mod owned;
mod pq;
mod rsa;
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
    // A public key or R that is not a valid point or has small order never
    // verifies (RFC 8032 decoding with the small-order check of Web Crypto).
    let valid_point = |bytes: &[u8]| <[u8; 32]>::try_from(bytes).ok()
        .and_then(|b| curve25519_dalek::edwards::CompressedEdwardsY(b).decompress())
        .is_some_and(|p| !p.is_small_order());
    if sig.len() != 64 || !valid_point(&public) || !valid_point(&sig[..32]) { return Ok(Value::Bool(false)); }
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


fn bytes_at(realm: &mut Realm, args: &[Value], i: usize) -> JsResult<Vec<u8>> {
    let value = arg(args, i);
    if value.is_nullish() { return Ok(Vec::new()); }
    buffer_source_bytes(realm, &value, "WebCrypto")
}

fn length_at(realm: &mut Realm, args: &[Value], i: usize) -> JsResult<usize> {
    let n = realm.to_number(arg(args, i))?;
    if !(0.0..=(1u64 << 32) as f64).contains(&n) { return Err(crate::misc::dom_exception(realm, "OperationError", "Invalid output length")); }
    Ok(n as usize)
}

/// cryptoKeccak(name, data, outputBytes, a, b): SHA-3, cSHAKE (a = function
/// name, b = customization), TurboSHAKE (a = domain separation byte) and
/// KangarooTwelve (a = customization).
fn keccak_digest(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let name = arg_str(realm, args, 0)?;
    let data = bytes_at(realm, args, 1)?;
    let len = length_at(realm, args, 2)?;
    let out = match name.as_str() {
        "SHA3-256" => keccak::sha3(256, &data),
        "SHA3-384" => keccak::sha3(384, &data),
        "SHA3-512" => keccak::sha3(512, &data),
        "cSHAKE128" | "cSHAKE256" => {
            let (n, s) = (bytes_at(realm, args, 3)?, bytes_at(realm, args, 4)?);
            keccak::cshake(if name == "cSHAKE128" { 128 } else { 256 }, &data, len, &n, &s)
        }
        "TurboSHAKE128" | "TurboSHAKE256" => {
            let d = realm.to_number(arg(args, 3))? as u8;
            keccak::turboshake(if name == "TurboSHAKE128" { 128 } else { 256 }, &data, d, len)
        }
        "KT128" | "KT256" => {
            let c = bytes_at(realm, args, 3)?;
            keccak::kangaroo(if name == "KT128" { 128 } else { 256 }, &data, &c, len)
        }
        _ => return Err(crate::misc::dom_exception(realm, "NotSupportedError", "Unrecognized digest")),
    };
    Ok(make_array_buffer(realm, out))
}

/// cryptoKmac(level, key, data, outputBytes, customization)
fn kmac(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let level = realm.to_number(arg(args, 0))? as u32;
    let key = bytes_at(realm, args, 1)?;
    let data = bytes_at(realm, args, 2)?;
    let len = length_at(realm, args, 3)?;
    let custom = bytes_at(realm, args, 4)?;
    Ok(make_array_buffer(realm, keccak::kmac(level, &key, &data, len, &custom)))
}

fn aes_ocb(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let key = bytes_at(realm, args, 0)?;
    let iv = bytes_at(realm, args, 1)?;
    let data = bytes_at(realm, args, 2)?;
    let aad = bytes_at(realm, args, 3)?;
    let tag_len = realm.to_number(arg(args, 4))? as usize;
    let decrypt = realm.to_boolean(&arg(args, 5));
    let out = ocb::crypt(&key, &iv, &aad, &data, tag_len, decrypt)
        .ok_or_else(|| crate::misc::dom_exception(realm, "OperationError", "AES-OCB operation failed"))?;
    Ok(make_array_buffer(realm, out))
}

/// x448(scalar, u): the X448 function; u defaults to the base point.
fn x448(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let scalar = bytes_at(realm, args, 0)?;
    let out = if arg(args, 1).is_nullish() { curve448::x448_public(&scalar) } else {
        let u = bytes_at(realm, args, 1)?;
        curve448::x448(&scalar, &u)
    }.ok_or_else(|| crate::misc::dom_exception(realm, "DataError", "Invalid X448 key"))?;
    Ok(make_array_buffer(realm, out.to_vec()))
}

fn ed448_public(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let secret = bytes_at(realm, args, 0)?;
    let out = curve448::ed448_public(&secret).ok_or_else(|| crate::misc::dom_exception(realm, "DataError", "Invalid Ed448 key"))?;
    Ok(make_array_buffer(realm, out.to_vec()))
}

fn ed448_sign(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let (secret, msg, ctx) = (bytes_at(realm, args, 0)?, bytes_at(realm, args, 1)?, bytes_at(realm, args, 2)?);
    let out = curve448::ed448_sign(&secret, &msg, &ctx).ok_or_else(|| crate::misc::dom_exception(realm, "OperationError", "Ed448 signing failed"))?;
    Ok(make_array_buffer(realm, out))
}

fn ed448_verify(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let (public, msg, sig, ctx) = (bytes_at(realm, args, 0)?, bytes_at(realm, args, 1)?, bytes_at(realm, args, 2)?, bytes_at(realm, args, 3)?);
    Ok(Value::Bool(curve448::ed448_verify(&public, &msg, &sig, &ctx)))
}

/// argon2(type, password, nonce, secret, associatedData, parallelism, memory, passes, tagBytes)
fn argon2(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let kind = realm.to_number(arg(args, 0))? as u32;
    let (password, salt, secret, data) = (bytes_at(realm, args, 1)?, bytes_at(realm, args, 2)?, bytes_at(realm, args, 3)?, bytes_at(realm, args, 4)?);
    let lanes = realm.to_number(arg(args, 5))? as u32;
    let memory = realm.to_number(arg(args, 6))? as u32;
    let passes = realm.to_number(arg(args, 7))? as u32;
    let tag_len = length_at(realm, args, 8)?;
    let params = argon2::Params { password: &password, salt: &salt, secret: &secret, data: &data, lanes, memory, passes, kind, tag_len };
    let out = argon2::argon2(&params).ok_or_else(|| crate::misc::dom_exception(realm, "OperationError", "Argon2 derivation failed"))?;
    Ok(make_array_buffer(realm, out))
}

/// cryptoImpl(): the lazily compiled WebCrypto implementation (a factory
/// function taking the natives).
fn implementation(realm: &mut Realm, _: Value, _: &[Value]) -> JsResult<Value> {
    realm.eval_builtin(include_str!("js/crypto_impl.js"), "rabbithole:crypto_impl.js")
}

pub(crate) fn install(realm: &mut Realm, internals: ObjectId) {
    ecdh::install(realm, internals);
    rsa::install(realm, internals);
    x25519::install(realm, internals);
    pq::install(realm, internals);
    realm.define_method(internals, "cryptoImpl", 0, implementation);
    realm.define_method(internals, "cryptoKeccak", 5, keccak_digest);
    realm.define_method(internals, "cryptoKmac", 5, kmac);
    realm.define_method(internals, "aesOcb", 6, aes_ocb);
    realm.define_method(internals, "x448", 2, x448);
    realm.define_method(internals, "ed448Public", 1, ed448_public);
    realm.define_method(internals, "ed448Sign", 3, ed448_sign);
    realm.define_method(internals, "ed448Verify", 4, ed448_verify);
    realm.define_method(internals, "argon2", 9, argon2);
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
