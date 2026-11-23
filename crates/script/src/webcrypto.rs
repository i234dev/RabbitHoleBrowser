//! Native hash and HMAC operations shared by WebCrypto key derivation and
//! signatures. A PBKDF2 operation can yield between batches of PRF calls.
use crate::fetch::{buffer_source_bytes, make_array_buffer};
use crate::host::{arg, arg_str};
use rabbithole_js::{JsResult, ObjectId, Realm, Value, V};
use ring::{digest, hmac};

fn algorithms(realm: &mut Realm, args: &[Value]) -> JsResult<(&'static digest::Algorithm, hmac::Algorithm)> {
    Ok(match arg_str(realm, args, 0)?.as_str() {
        "SHA-1" => (&digest::SHA1_FOR_LEGACY_USE_ONLY, hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY),
        "SHA-256" => (&digest::SHA256, hmac::HMAC_SHA256),
        "SHA-384" => (&digest::SHA384, hmac::HMAC_SHA384),
        "SHA-512" => (&digest::SHA512, hmac::HMAC_SHA512),
        _ => return Err(realm.throw_type_error("Unsupported hash")),
    })
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

pub(crate) fn install(realm: &mut Realm, internals: ObjectId) {
    realm.define_method(internals, "cryptoDigest", 2, hash);
    realm.define_method(internals, "cryptoHmac", 3, mac);
    realm.define_method(internals, "pbkdfStart", 5, pbkdf_start);
    realm.define_method(internals, "pbkdfStep", 1, pbkdf_step);
}
