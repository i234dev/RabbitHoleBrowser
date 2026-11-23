//! Reusable NIST curve agreement keys. Private scalars stay in the native
//! key handle unless the importer or generator granted extractability.
use crate::fetch::{buffer_source_bytes, make_array_buffer};
use crate::host::{arg, arg_str};
use crate::misc::dom_exception;
use aws_lc_rs::agreement::{self, Algorithm, ParsedPublicKey, PrivateKey, UnparsedPublicKey};
use aws_lc_rs::encoding::{AsBigEndian, AsDer, EcPrivateKeyBin, Pkcs8V1Der};
use rabbithole_js::{JsResult, ObjectId, Realm, Value};

struct Key {
    private: PrivateKey,
    extractable: bool,
    signing: bool,
}

fn curve(realm: &mut Realm, args: &[Value]) -> JsResult<&'static Algorithm> {
    match arg_str(realm, args, 0)?.as_str() {
        "P-256" => Ok(&agreement::ECDH_P256),
        "P-384" => Ok(&agreement::ECDH_P384),
        "P-521" => Ok(&agreement::ECDH_P521),
        _ => Err(dom_exception(realm, "NotSupportedError", "Unsupported ECDH curve")),
    }
}

fn create(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let algorithm = curve(realm, args)?;
    let format = arg_str(realm, args, 1)?;
    let private = if format == "generate" {
        PrivateKey::generate(algorithm)
            .map_err(|_| dom_exception(realm, "OperationError", "ECDH key generation failed"))?
    } else {
        let bytes = buffer_source_bytes(realm, &arg(args, 2), "ECDH private key")?;
        match format.as_str() {
            "pkcs8" => PrivateKey::from_private_key_der(algorithm, &bytes),
            "scalar" => PrivateKey::from_private_key(algorithm, &bytes),
            _ => return Err(realm.throw_type_error("Invalid ECDH private key format")),
        }.map_err(|_| dom_exception(realm, "DataError", "Invalid ECDH private key"))?
    };
    let extractable = realm.to_boolean(&arg(args, 3));
    let signing = realm.to_boolean(&arg(args, 4));
    let id = realm.new_object();
    realm.set_host_data(id, Box::new(Key { private, extractable, signing }));
    Ok(Value::Object(id))
}

fn handle(realm: &mut Realm, args: &[Value]) -> JsResult<ObjectId> {
    arg(args, 0).as_object().filter(|id| realm.host_data::<Key>(*id).is_some())
        .ok_or_else(|| realm.throw_type_error("Expected an ECDH private key"))
}

fn export(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let format = arg_str(realm, args, 1)?;
    let id = handle(realm, args)?;
    let key = realm.host_data::<Key>(id).unwrap();
    if format != "public" && !key.extractable {
        return Err(dom_exception(realm, "InvalidAccessError", "Key is not extractable"));
    }
    let bytes = match format.as_str() {
        "public" => key.private.compute_public_key().map(|p| p.as_ref().to_vec()),
        "pkcs8" => key.private.as_der().map(|p: Pkcs8V1Der<'_>| p.as_ref().to_vec()),
        "scalar" => key.private.as_be_bytes().map(|p: EcPrivateKeyBin<'_>| p.as_ref().to_vec()),
        _ => return Err(realm.throw_type_error("Invalid ECDH export format")),
    }.map_err(|_| dom_exception(realm, "OperationError", "Could not encode ECDH key"))?;
    Ok(make_array_buffer(realm, bytes))
}

fn validate(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let algorithm = curve(realm, args)?;
    let bytes = buffer_source_bytes(realm, &arg(args, 1), "ECDH public point")?;
    if bytes.first() != Some(&4) ||
        ParsedPublicKey::try_from(UnparsedPublicKey::new(algorithm, &bytes)).is_err() {
        return Err(dom_exception(realm, "DataError", "Invalid uncompressed ECDH public point"));
    }
    Ok(Value::Undefined)
}

fn derive(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let bytes = buffer_source_bytes(realm, &arg(args, 1), "ECDH public key")?;
    let id = handle(realm, args)?;
    let key = realm.host_data::<Key>(id).unwrap();
    if key.signing { return Err(dom_exception(realm, "InvalidAccessError", "Expected an agreement key")); }
    let private = &key.private;
    let peer = UnparsedPublicKey::new(private.algorithm(), bytes);
    let result = agreement::agree(private, peer, (), |secret| Ok(secret.to_vec()))
        .map_err(|_| dom_exception(realm, "OperationError", "ECDH agreement failed"))?;
    Ok(make_array_buffer(realm, result))
}

fn sign(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let digest = buffer_source_bytes(realm, &arg(args, 1), "ECDSA digest")?;
    let id = handle(realm, args)?;
    let key = realm.host_data::<Key>(id).unwrap();
    if !key.signing { return Err(dom_exception(realm, "InvalidAccessError", "Expected a signing key")); }
    let result = key.private.as_der().ok().and_then(|der: Pkcs8V1Der<'_>| super::ecdsa::sign(der.as_ref(), &digest))
        .ok_or_else(|| dom_exception(realm, "OperationError", "ECDSA signing failed"))?;
    Ok(make_array_buffer(realm, result))
}

fn verify(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let public = buffer_source_bytes(realm, &arg(args, 0), "ECDSA public key")?;
    let digest = buffer_source_bytes(realm, &arg(args, 1), "ECDSA digest")?;
    let signature = buffer_source_bytes(realm, &arg(args, 2), "ECDSA signature")?;
    super::ecdsa::verify(&public, &digest, &signature).map(Value::Bool)
        .ok_or_else(|| dom_exception(realm, "OperationError", "ECDSA verification failed"))
}

pub(super) fn install(realm: &mut Realm, internals: ObjectId) {
    realm.define_method(internals, "ecdhKey", 5, create);
    realm.define_method(internals, "ecdhExport", 2, export);
    realm.define_method(internals, "ecdhValidate", 2, validate);
    realm.define_method(internals, "ecdsaSign", 2, sign);
    realm.define_method(internals, "ecdsaVerify", 3, verify);
    realm.define_method(internals, "ecdhDerive", 2, derive);
}
