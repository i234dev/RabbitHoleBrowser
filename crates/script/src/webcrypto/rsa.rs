//! RSA key material belongs to AWS-LC. WebCrypto supplies the digest,
//! padding scheme, PSS salt length and OAEP label for each operation.
use super::owned::Owned;
use crate::fetch::{buffer_source_bytes, make_array_buffer};
use crate::host::{arg, arg_str};
use crate::misc::dom_exception;
use aws_lc_sys as lc;
use rabbithole_js::{JsResult, ObjectId, Realm, Value};
use std::{ptr, slice};

struct Key {
    rsa: Owned<lc::RSA>,
    private: bool,
    extractable: bool,
}

fn key_id(realm: &mut Realm, args: &[Value]) -> JsResult<ObjectId> {
    arg(args, 0)
        .as_object()
        .filter(|id| realm.host_data::<Key>(*id).is_some())
        .ok_or_else(|| realm.throw_type_error("Expected an RSA key"))
}

fn import(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let der = buffer_source_bytes(realm, &arg(args, 0), "RSA key")?;
    let private = realm.to_boolean(&arg(args, 1));
    let extractable = realm.to_boolean(&arg(args, 2));
    // The PKCS #1 decoder consumes the whole input and owns its result.
    let rsa = unsafe {
        if private {
            lc::RSA_private_key_from_bytes(der.as_ptr(), der.len())
        } else {
            lc::RSA_public_key_from_bytes(der.as_ptr(), der.len())
        }
    };
    let key = Owned::new(rsa, lc::RSA_free)
        .filter(|k| unsafe { lc::RSA_check_key(k.get()) == 1 })
        .ok_or_else(|| dom_exception(realm, "DataError", "Invalid RSA key material"))?;
    let id = realm.new_object();
    realm.set_host_data(
        id,
        Box::new(Key {
            rsa: key,
            private,
            extractable,
        }),
    );
    Ok(Value::Object(id))
}

fn encode(rsa: &Owned<lc::RSA>, private: bool) -> Option<Vec<u8>> {
    let mut data = ptr::null_mut();
    let mut len = 0;
    // Both encoders allocate the result with OPENSSL_malloc on success.
    unsafe {
        let ok = if private {
            lc::RSA_private_key_to_bytes(&mut data, &mut len, rsa.get())
        } else {
            lc::RSA_public_key_to_bytes(&mut data, &mut len, rsa.get())
        };
        if ok != 1 {
            return None;
        }
        let bytes = slice::from_raw_parts(data, len).to_vec();
        if private {
            lc::OPENSSL_cleanse(data.cast(), len);
        }
        lc::OPENSSL_free(data.cast());
        Some(bytes)
    }
}

fn export(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let id = key_id(realm, args)?;
    let private = realm.to_boolean(&arg(args, 1));
    let key = realm.host_data::<Key>(id).unwrap();
    if private && !(key.private && key.extractable) {
        return Err(dom_exception(
            realm,
            "InvalidAccessError",
            "RSA private material cannot be exported",
        ));
    }
    let bytes = encode(&key.rsa, private)
        .ok_or_else(|| dom_exception(realm, "OperationError", "RSA key encoding failed"))?;
    Ok(make_array_buffer(realm, bytes))
}

#[derive(Clone, Copy, PartialEq)]
enum Operation {
    Sign,
    Verify,
    Encrypt,
    Decrypt,
}
enum Output {
    Bytes(Vec<u8>),
    Verified(bool),
}

fn apply(
    key: &Key,
    op: Operation,
    hash: &str,
    salt: Option<i32>,
    input: &[u8],
    extra: &[u8],
) -> Option<Output> {
    use Operation::*;
    if key.private != matches!(op, Sign | Decrypt) {
        return None;
    }
    // The context holds a reference to pkey, which in turn retains the RSA key.
    // Input slices and the library-owned digest descriptor outlive the EVP call.
    unsafe {
        let md = match hash {
            "SHA-1" => lc::EVP_sha1(),
            "SHA-256" => lc::EVP_sha256(),
            "SHA-384" => lc::EVP_sha384(),
            "SHA-512" => lc::EVP_sha512(),
            _ => return None,
        };
        let pkey = Owned::new(lc::EVP_PKEY_new(), lc::EVP_PKEY_free)?;
        if lc::EVP_PKEY_set1_RSA(pkey.get(), key.rsa.get()) != 1 {
            return None;
        }
        let ctx = Owned::new(
            lc::EVP_PKEY_CTX_new(pkey.get(), ptr::null_mut()),
            lc::EVP_PKEY_CTX_free,
        )?;
        let ctx = ctx.get();
        let initialized = match op {
            Sign => lc::EVP_PKEY_sign_init(ctx),
            Verify => lc::EVP_PKEY_verify_init(ctx),
            Encrypt => lc::EVP_PKEY_encrypt_init(ctx),
            Decrypt => lc::EVP_PKEY_decrypt_init(ctx),
        };
        if initialized != 1 {
            return None;
        }
        if matches!(op, Sign | Verify) {
            let padding = if salt.is_some() {
                lc::RSA_PKCS1_PSS_PADDING
            } else {
                lc::RSA_PKCS1_PADDING
            };
            if lc::EVP_PKEY_CTX_set_rsa_padding(ctx, padding) != 1
                || lc::EVP_PKEY_CTX_set_signature_md(ctx, md) != 1
            {
                return None;
            }
            if let Some(n) = salt {
                if n < 0 || lc::EVP_PKEY_CTX_set_rsa_pss_saltlen(ctx, n) != 1 {
                    return None;
                }
            }
        } else {
            if lc::EVP_PKEY_CTX_set_rsa_padding(ctx, lc::RSA_PKCS1_OAEP_PADDING) != 1
                || lc::EVP_PKEY_CTX_set_rsa_oaep_md(ctx, md) != 1
                || lc::EVP_PKEY_CTX_set_rsa_mgf1_md(ctx, md) != 1
            {
                return None;
            }
            if !extra.is_empty() {
                let label = lc::OPENSSL_memdup(extra.as_ptr().cast(), extra.len());
                if label.is_null() {
                    return None;
                }
                // set0 owns the label only on success.
                if lc::EVP_PKEY_CTX_set0_rsa_oaep_label(ctx, label.cast(), extra.len()) != 1 {
                    lc::OPENSSL_free(label);
                    return None;
                }
            }
        }
        if op == Verify {
            return Some(Output::Verified(
                lc::EVP_PKEY_verify(
                    ctx,
                    extra.as_ptr(),
                    extra.len(),
                    input.as_ptr(),
                    input.len(),
                ) == 1,
            ));
        }
        let mut len = lc::RSA_size(key.rsa.get()) as usize;
        let mut out = vec![0; len];
        let call = match op {
            Sign => lc::EVP_PKEY_sign,
            Encrypt => lc::EVP_PKEY_encrypt,
            Decrypt => lc::EVP_PKEY_decrypt,
            Verify => unreachable!(),
        };
        if call(ctx, out.as_mut_ptr(), &mut len, input.as_ptr(), input.len()) != 1 {
            return None;
        }
        out.truncate(len);
        Some(Output::Bytes(out))
    }
}

fn operate(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let id = key_id(realm, args)?;
    let op = match arg_str(realm, args, 1)?.as_str() {
        "sign" => Operation::Sign,
        "verify" => Operation::Verify,
        "encrypt" => Operation::Encrypt,
        "decrypt" => Operation::Decrypt,
        _ => return Err(realm.throw_type_error("Invalid RSA operation")),
    };
    let hash = arg_str(realm, args, 2)?;
    let salt = if arg(args, 3).is_nullish() {
        None
    } else {
        let n = realm.to_number(arg(args, 3))?;
        if !(0.0..=i32::MAX as f64).contains(&n) || n.fract() != 0.0 {
            if op == Operation::Verify {
                return Ok(Value::Bool(false));
            }
            return Err(dom_exception(
                realm,
                "OperationError",
                "Invalid RSA-PSS salt length",
            ));
        }
        Some(n as i32)
    };
    let input = buffer_source_bytes(realm, &arg(args, 4), "RSA input")?;
    let extra = buffer_source_bytes(realm, &arg(args, 5), "RSA label or signature")?;
    match apply(
        realm.host_data::<Key>(id).unwrap(),
        op,
        &hash,
        salt,
        &input,
        &extra,
    ) {
        Some(Output::Verified(ok)) => Ok(Value::Bool(ok)),
        Some(Output::Bytes(bytes)) => Ok(make_array_buffer(realm, bytes)),
        None => Err(dom_exception(
            realm,
            "OperationError",
            "RSA operation failed",
        )),
    }
}

// Prime generation can take hundreds of milliseconds. Run jobs outside the
// realm and skip queued requests if navigation has already collected the job.
struct PrivateDer(Vec<u8>);
impl Drop for PrivateDer {
    fn drop(&mut self) {
        unsafe {
            lc::OPENSSL_cleanse(self.0.as_mut_ptr().cast(), self.0.len());
        }
    }
}

type Generated = std::sync::Mutex<Option<Result<PrivateDer, ()>>>;
struct Pending {
    result: std::sync::Arc<Generated>,
    extractable: bool,
}
struct Generate {
    bits: i32,
    exponent: Vec<u8>,
    result: std::sync::Weak<Generated>,
}
fn generate_der(bits: i32, exponent: &[u8]) -> Option<PrivateDer> {
    // All three values belong to this worker and are dropped before it
    // handles another job. AWS-LC may round unsupported sizes down.
    unsafe {
        let e = Owned::new(
            lc::BN_bin2bn(exponent.as_ptr(), exponent.len(), ptr::null_mut()),
            lc::BN_free,
        )?;
        let rsa = Owned::new(lc::RSA_new(), lc::RSA_free)?;
        if lc::RSA_generate_key_ex(rsa.get(), bits, e.get(), ptr::null_mut()) != 1
            || lc::RSA_bits(rsa.get()) != bits as u32
        {
            return None;
        }
        encode(&rsa, true).map(PrivateDer)
    }
}
fn generator() -> Option<&'static std::sync::mpsc::Sender<Generate>> {
    use std::sync::{mpsc, OnceLock};
    static WORKER: OnceLock<Option<mpsc::Sender<Generate>>> = OnceLock::new();
    WORKER
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel::<Generate>();
            std::thread::Builder::new()
                .name("webcrypto-rsa".into())
                .spawn(move || {
                    for job in rx {
                        if job.result.strong_count() == 0 {
                            continue;
                        }
                        let key = generate_der(job.bits, &job.exponent).ok_or(());
                        if let Some(result) = job.result.upgrade() {
                            *result.lock().unwrap() = Some(key);
                        }
                    }
                })
                .ok()?;
            Some(tx)
        })
        .as_ref()
}
fn exponent(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    use rabbithole_js::ObjectKind;
    if !arg(args, 0).as_object().is_some_and(|id| {
        matches!(&realm.obj(id).kind,
        ObjectKind::TypedArray(view) if view.kind.name() == "Uint8Array")
    }) {
        return Err(realm.throw_type_error("RSA publicExponent requires a Uint8Array"));
    }
    super::copy_buffer(realm, this, args)
}
fn generate(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let bits = realm.to_number(arg(args, 0))?;
    let exponent = buffer_source_bytes(realm, &arg(args, 1), "RSA public exponent")?;
    let extractable = realm.to_boolean(&arg(args, 2));
    // Limit sizes to AWS-LC's supported RSA modulus and public exponent.
    let first = exponent
        .iter()
        .position(|b| *b != 0)
        .unwrap_or(exponent.len());
    let exponent = &exponent[first..];
    if !(512.0..=16384.0).contains(&bits)
        || bits % 128.0 != 0.0
        || exponent.len() > 4
        || exponent.last().is_none_or(|b| b & 1 == 0)
        || exponent == [1]
    {
        return Err(dom_exception(
            realm,
            "OperationError",
            "Unsupported RSA generation parameters",
        ));
    }
    let result = std::sync::Arc::new(Generated::default());
    let job = Generate {
        bits: bits as i32,
        exponent: exponent.to_vec(),
        result: std::sync::Arc::downgrade(&result),
    };
    if !generator().is_some_and(|tx| tx.send(job).is_ok()) {
        return Err(dom_exception(
            realm,
            "OperationError",
            "RSA key generation is unavailable",
        ));
    }
    let id = realm.new_object();
    realm.set_host_data(
        id,
        Box::new(Pending {
            result,
            extractable,
        }),
    );
    Ok(Value::Object(id))
}
fn generated(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(job) = arg(args, 0)
        .as_object()
        .and_then(|id| realm.host_data::<Pending>(id))
    else {
        return Err(realm.throw_type_error("Expected pending RSA generation"));
    };
    let extractable = job.extractable;
    let Some(result) = job.result.lock().unwrap().take() else {
        return Ok(Value::Null);
    };
    let der =
        result.map_err(|_| dom_exception(realm, "OperationError", "RSA key generation failed"))?;
    let rsa = Owned::new(
        unsafe { lc::RSA_private_key_from_bytes(der.0.as_ptr(), der.0.len()) },
        lc::RSA_free,
    )
    .ok_or_else(|| dom_exception(realm, "OperationError", "RSA key decoding failed"))?;
    let id = realm.new_object();
    realm.set_host_data(
        id,
        Box::new(Key {
            rsa,
            private: true,
            extractable,
        }),
    );
    Ok(Value::Object(id))
}

pub(super) fn install(realm: &mut Realm, internals: ObjectId) {
    realm.define_method(internals, "rsaExponent", 1, exponent);
    realm.define_method(internals, "rsaGenerate", 3, generate);
    realm.define_method(internals, "rsaGenerated", 1, generated);
    realm.define_method(internals, "rsaImport", 3, import);
    realm.define_method(internals, "rsaExport", 2, export);
    realm.define_method(internals, "rsaOperate", 6, operate);
}
