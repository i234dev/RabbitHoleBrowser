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

/// SEC 1 point (compressed or uncompressed) as an uncompressed point.
fn uncompressed(curve: &str, bytes: &[u8]) -> Option<Vec<u8>> {
    use aws_lc_sys as lc;
    use std::ptr::null_mut;
    let nid = match curve {
        "P-256" => lc::NID_X9_62_prime256v1,
        "P-384" => lc::NID_secp384r1,
        "P-521" => lc::NID_secp521r1,
        _ => return None,
    };
    if !matches!(bytes.first(), Some(2..=4)) { return None; }
    // The named groups are static; the point is owned and freed here.
    unsafe {
        let group = lc::EC_GROUP_new_by_curve_name(nid);
        if group.is_null() { return None; }
        let point = super::owned::Owned::new(lc::EC_POINT_new(group), lc::EC_POINT_free)?;
        if lc::EC_POINT_oct2point(group, point.get(), bytes.as_ptr(), bytes.len(), null_mut()) != 1 {
            lc::ERR_clear_error();
            return None;
        }
        let form = lc::point_conversion_form_t::POINT_CONVERSION_UNCOMPRESSED;
        let len = lc::EC_POINT_point2oct(group, point.get(), form, null_mut(), 0, null_mut());
        let mut out = vec![0u8; len];
        if len == 0 || lc::EC_POINT_point2oct(group, point.get(), form, out.as_mut_ptr(), len, null_mut()) != len { return None; }
        Some(out)
    }
}

/// ecdhValidate(curve, point): the validated public point, uncompressed.
fn validate(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let algorithm = curve(realm, args)?;
    let name = arg_str(realm, args, 0)?;
    let bytes = buffer_source_bytes(realm, &arg(args, 1), "ECDH public point")?;
    let point = uncompressed(&name, &bytes)
        .filter(|p| ParsedPublicKey::try_from(UnparsedPublicKey::new(algorithm, p)).is_ok())
        .ok_or_else(|| dom_exception(realm, "DataError", "Invalid ECDH public point"))?;
    Ok(make_array_buffer(realm, point))
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
