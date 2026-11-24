//! ML-KEM (FIPS 203), ML-DSA (FIPS 204) and the hybrid KEMs of
//! draft-irtf-cfrg-concrete-hybrid-kems (MLKEM768-P256, MLKEM768-X25519,
//! MLKEM1024-P384). Private keys are kept as their seeds by the caller and
//! expanded by AWS-LC for each operation.
use super::keccak;
use super::owned::Owned;
use crate::fetch::{buffer_source_bytes, make_array_buffer};
use crate::host::{arg, arg_str};
use crate::misc::dom_exception;
use aws_lc_sys as lc;
use rabbithole_js::{JsResult, ObjectId, Realm, Value};
use ring::rand::{SecureRandom, SystemRandom};
use std::ptr;

// Deterministic ML-KEM key generation (FIPS 203 KeyGen_internal) is part
// of AWS-LC's experimental KEM API, which the generated aws-lc-sys bindings
// of every target do not list. The symbol carries the crate's version
// prefix (aws-lc-sys is pinned by Cargo.lock).
extern "C" {
    #[link_name = "aws_lc_0_47_0_EVP_PKEY_keygen_deterministic"]
    fn EVP_PKEY_keygen_deterministic(ctx: *mut lc::EVP_PKEY_CTX, out: *mut *mut lc::EVP_PKEY, seed: *const u8, seed_len: *mut usize) -> std::os::raw::c_int;
}

fn kem_nid(name: &str) -> Option<(i32, usize, usize)> {
    // (nid, public key length, ciphertext length)
    Some(match name {
        "ML-KEM-512" => (lc::NID_MLKEM512, 800, 768),
        "ML-KEM-768" => (lc::NID_MLKEM768, 1184, 1088),
        "ML-KEM-1024" => (lc::NID_MLKEM1024, 1568, 1568),
        _ => return None,
    })
}

fn dsa_nid(name: &str) -> Option<i32> {
    Some(match name {
        "ML-DSA-44" => lc::NID_MLDSA44,
        "ML-DSA-65" => lc::NID_MLDSA65,
        "ML-DSA-87" => lc::NID_MLDSA87,
        _ => return None,
    })
}

fn random(len: usize) -> Option<Vec<u8>> {
    let mut out = vec![0u8; len];
    SystemRandom::new().fill(&mut out).ok()?;
    Some(out)
}

fn raw_public(pkey: &Owned<lc::EVP_PKEY>) -> Option<Vec<u8>> {
    unsafe {
        let mut len = 0usize;
        if lc::EVP_PKEY_get_raw_public_key(pkey.get(), ptr::null_mut(), &mut len) != 1 { return None; }
        let mut out = vec![0u8; len];
        if lc::EVP_PKEY_get_raw_public_key(pkey.get(), out.as_mut_ptr(), &mut len) != 1 { return None; }
        out.truncate(len);
        Some(out)
    }
}

/// ML-KEM.KeyGen_internal(d, z) for a 64-byte seed d || z.
fn kem_keypair(nid: i32, seed: &[u8]) -> Option<Owned<lc::EVP_PKEY>> {
    if seed.len() != 64 { return None; }
    unsafe {
        let ctx = Owned::new(lc::EVP_PKEY_CTX_new_id(lc::EVP_PKEY_KEM, ptr::null_mut()), lc::EVP_PKEY_CTX_free)?;
        if lc::EVP_PKEY_keygen_init(ctx.get()) != 1 || lc::EVP_PKEY_CTX_kem_set_params(ctx.get(), nid) != 1 { return None; }
        let mut pkey = ptr::null_mut();
        let mut len = seed.len();
        if EVP_PKEY_keygen_deterministic(ctx.get(), &mut pkey, seed.as_ptr(), &mut len) != 1 { return None; }
        Owned::new(pkey, lc::EVP_PKEY_free)
    }
}

/// FIPS 203 section 7.2 encapsulation key check: every coefficient of the
/// encoded vector is reduced modulo q.
fn kem_public_valid(pk: &[u8], len: usize) -> bool {
    if pk.len() != len { return false; }
    pk[..len - 32].chunks_exact(3).all(|c| {
        let a = c[0] as u16 | ((c[1] as u16 & 0x0f) << 8);
        let b = (c[1] as u16 >> 4) | ((c[2] as u16) << 4);
        a < 3329 && b < 3329
    })
}

fn kem_encaps(nid: i32, pk_len: usize, pk: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    if !kem_public_valid(pk, pk_len) { return None; }
    unsafe {
        let pkey = Owned::new(lc::EVP_PKEY_kem_new_raw_public_key(nid, pk.as_ptr(), pk.len()), lc::EVP_PKEY_free)?;
        let ctx = Owned::new(lc::EVP_PKEY_CTX_new(pkey.get(), ptr::null_mut()), lc::EVP_PKEY_CTX_free)?;
        let (mut ct_len, mut ss_len) = (0usize, 0usize);
        if lc::EVP_PKEY_encapsulate(ctx.get(), ptr::null_mut(), &mut ct_len, ptr::null_mut(), &mut ss_len) != 1 { return None; }
        let (mut ct, mut ss) = (vec![0u8; ct_len], vec![0u8; ss_len]);
        if lc::EVP_PKEY_encapsulate(ctx.get(), ct.as_mut_ptr(), &mut ct_len, ss.as_mut_ptr(), &mut ss_len) != 1 { return None; }
        ct.truncate(ct_len);
        ss.truncate(ss_len);
        Some((ct, ss))
    }
}

fn kem_decaps(nid: i32, ct_len: usize, seed: &[u8], ct: &[u8]) -> Option<Vec<u8>> {
    if ct.len() != ct_len { return None; }
    let pkey = kem_keypair(nid, seed)?;
    unsafe {
        let ctx = Owned::new(lc::EVP_PKEY_CTX_new(pkey.get(), ptr::null_mut()), lc::EVP_PKEY_CTX_free)?;
        let mut ss = vec![0u8; 32];
        let mut ss_len = ss.len();
        if lc::EVP_PKEY_decapsulate(ctx.get(), ss.as_mut_ptr(), &mut ss_len, ct.as_ptr(), ct.len()) != 1 { return None; }
        ss.truncate(ss_len);
        Some(ss)
    }
}

// ------------------------------------------------------------- hybrid KEMs

#[derive(Clone, Copy)]
enum Group { P256, P384, X25519 }

struct Hybrid {
    nid: i32,
    pk_len: usize,
    ct_len: usize,
    group: Group,
    label: &'static [u8],
}

fn hybrid(name: &str) -> Option<Hybrid> {
    Some(match name {
        "MLKEM768-P256" => Hybrid { nid: lc::NID_MLKEM768, pk_len: 1184, ct_len: 1088, group: Group::P256, label: b"MLKEM768-P256" },
        "MLKEM768-X25519" => Hybrid { nid: lc::NID_MLKEM768, pk_len: 1184, ct_len: 1088, group: Group::X25519, label: b"\\.//^\\" },
        "MLKEM1024-P384" => Hybrid { nid: lc::NID_MLKEM1024, pk_len: 1568, ct_len: 1568, group: Group::P384, label: b"MLKEM1024-P384" },
        _ => return None,
    })
}

const P256_ORDER: [u8; 32] = [
    0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xbc, 0xe6, 0xfa, 0xad, 0xa7, 0x17, 0x9e, 0x84, 0xf3, 0xb9, 0xca, 0xc2, 0xfc, 0x63, 0x25, 0x51,
];
const P384_ORDER: [u8; 48] = [
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xc7, 0x63, 0x4d, 0x81, 0xf4, 0x37, 0x2d, 0xdf,
    0x58, 0x1a, 0x0d, 0xb2, 0x48, 0xb0, 0xa7, 0x7a, 0xec, 0xec, 0x19, 0x6a, 0xcc, 0xc5, 0x29, 0x73,
];

impl Group {
    /// (Nseed, Nelem)
    fn sizes(self) -> (usize, usize) {
        match self { Group::P256 => (128, 65), Group::P384 => (48, 97), Group::X25519 => (32, 32) }
    }
    /// RandomScalar: big-endian candidates of the order's size, rejecting
    /// zero and values not below the order.
    fn scalar(self, seed: &[u8]) -> Option<Vec<u8>> {
        let order: &[u8] = match self { Group::P256 => &P256_ORDER, Group::P384 => &P384_ORDER, Group::X25519 => return Some(seed.to_vec()) };
        seed.chunks_exact(order.len())
            .find(|c| c.iter().any(|&b| b != 0) && *c < order)
            .map(|c| c.to_vec())
    }
    fn agreement(self) -> &'static aws_lc_rs::agreement::Algorithm {
        if matches!(self, Group::P384) { &aws_lc_rs::agreement::ECDH_P384 } else { &aws_lc_rs::agreement::ECDH_P256 }
    }
    fn public(self, scalar: &[u8]) -> Option<Vec<u8>> {
        if let Group::X25519 = self {
            let secret = x25519_dalek::StaticSecret::from(<[u8; 32]>::try_from(scalar).ok()?);
            return Some(x25519_dalek::PublicKey::from(&secret).as_bytes().to_vec());
        }
        let key = aws_lc_rs::agreement::PrivateKey::from_private_key(self.agreement(), scalar).ok()?;
        Some(key.compute_public_key().ok()?.as_ref().to_vec())
    }
    fn exp(self, scalar: &[u8], element: &[u8]) -> Option<Vec<u8>> {
        if let Group::X25519 = self {
            let secret = x25519_dalek::StaticSecret::from(<[u8; 32]>::try_from(scalar).ok()?);
            let peer = x25519_dalek::PublicKey::from(<[u8; 32]>::try_from(element).ok()?);
            return Some(secret.diffie_hellman(&peer).as_bytes().to_vec());
        }
        use aws_lc_rs::agreement::{agree, PrivateKey, UnparsedPublicKey};
        let key = PrivateKey::from_private_key(self.agreement(), scalar).ok()?;
        agree(&key, UnparsedPublicKey::new(self.agreement(), element), (), |s| Ok(s.to_vec())).ok()
    }
}

/// expandDecapsKeyG: (seed_PQ, dk_T) from the 32-byte hybrid seed.
fn hybrid_expand(h: &Hybrid, seed: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    if seed.len() != 32 { return None; }
    let (n_seed, _) = h.group.sizes();
    let full = keccak::shake(256, seed, 64 + n_seed);
    let scalar = h.group.scalar(&full[64..])?;
    Some((full[..64].to_vec(), scalar))
}

fn combine(h: &Hybrid, ss_pq: &[u8], ss_t: &[u8], ct_t: &[u8], ek_t: &[u8]) -> Vec<u8> {
    keccak::sha3(256, &[ss_pq, ss_t, ct_t, ek_t, h.label].concat())
}

fn hybrid_public(h: &Hybrid, seed: &[u8]) -> Option<Vec<u8>> {
    let (seed_pq, dk_t) = hybrid_expand(h, seed)?;
    let mut pk = raw_public(&kem_keypair(h.nid, &seed_pq)?)?;
    pk.extend(h.group.public(&dk_t)?);
    Some(pk)
}

fn hybrid_encaps(h: &Hybrid, pk: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let (n_seed, n_elem) = h.group.sizes();
    if pk.len() != h.pk_len + n_elem { return None; }
    let (ek_pq, ek_t) = pk.split_at(h.pk_len);
    let (mut ct, ss_pq) = kem_encaps(h.nid, h.pk_len, ek_pq)?;
    let sk_e = h.group.scalar(&random(n_seed)?)?;
    let ct_t = h.group.public(&sk_e)?;
    let ss_t = h.group.exp(&sk_e, ek_t)?;
    let ss = combine(h, &ss_pq, &ss_t, &ct_t, ek_t);
    ct.extend_from_slice(&ct_t);
    Some((ct, ss))
}

fn hybrid_decaps(h: &Hybrid, seed: &[u8], ct: &[u8]) -> Option<Vec<u8>> {
    let (_, n_elem) = h.group.sizes();
    if ct.len() != h.ct_len + n_elem { return None; }
    let (seed_pq, dk_t) = hybrid_expand(h, seed)?;
    let (ct_pq, ct_t) = ct.split_at(h.ct_len);
    let ss_pq = kem_decaps(h.nid, h.ct_len, &seed_pq, ct_pq)?;
    let ss_t = h.group.exp(&dk_t, ct_t)?;
    let ek_t = h.group.public(&dk_t)?;
    Some(combine(h, &ss_pq, &ss_t, ct_t, &ek_t))
}

// ------------------------------------------------------------------ ML-DSA

fn dsa_key(nid: i32, seed: &[u8]) -> Option<Owned<lc::EVP_PKEY>> {
    if seed.len() != 32 { return None; }
    unsafe { Owned::new(lc::EVP_PKEY_pqdsa_new_raw_private_key(nid, seed.as_ptr(), seed.len()), lc::EVP_PKEY_free) }
}

fn dsa_sign(nid: i32, seed: &[u8], msg: &[u8], context: &[u8]) -> Option<Vec<u8>> {
    let pkey = dsa_key(nid, seed)?;
    unsafe {
        let md = Owned::new(lc::EVP_MD_CTX_new(), lc::EVP_MD_CTX_free)?;
        let mut pctx = ptr::null_mut();
        if lc::EVP_DigestSignInit(md.get(), &mut pctx, ptr::null(), ptr::null_mut(), pkey.get()) != 1 { return None; }
        if !context.is_empty() && lc::EVP_PKEY_CTX_set1_signature_context_string(pctx, context.as_ptr(), context.len()) != 1 { return None; }
        let mut len = 0usize;
        if lc::EVP_DigestSign(md.get(), ptr::null_mut(), &mut len, msg.as_ptr(), msg.len()) != 1 { return None; }
        let mut sig = vec![0u8; len];
        if lc::EVP_DigestSign(md.get(), sig.as_mut_ptr(), &mut len, msg.as_ptr(), msg.len()) != 1 { return None; }
        sig.truncate(len);
        Some(sig)
    }
}

fn dsa_verify(nid: i32, pk: &[u8], msg: &[u8], sig: &[u8], context: &[u8]) -> bool {
    unsafe {
        let Some(pkey) = Owned::new(lc::EVP_PKEY_pqdsa_new_raw_public_key(nid, pk.as_ptr(), pk.len()), lc::EVP_PKEY_free) else { return false };
        let Some(md) = Owned::new(lc::EVP_MD_CTX_new(), lc::EVP_MD_CTX_free) else { return false };
        let mut pctx = ptr::null_mut();
        if lc::EVP_DigestVerifyInit(md.get(), &mut pctx, ptr::null(), ptr::null_mut(), pkey.get()) != 1 { return false; }
        if !context.is_empty() && lc::EVP_PKEY_CTX_set1_signature_context_string(pctx, context.as_ptr(), context.len()) != 1 { return false; }
        let ok = lc::EVP_DigestVerify(md.get(), sig.as_ptr(), sig.len(), msg.as_ptr(), msg.len()) == 1;
        lc::ERR_clear_error();
        ok
    }
}

// ---------------------------------------------------------------- bindings

fn bytes(realm: &mut Realm, args: &[Value], i: usize) -> JsResult<Vec<u8>> {
    buffer_source_bytes(realm, &arg(args, i), "WebCrypto")
}

fn failed(realm: &mut Realm, what: &str) -> Value {
    unsafe { lc::ERR_clear_error() };
    dom_exception(realm, "OperationError", &format!("{what} failed"))
}

fn unknown(realm: &mut Realm) -> Value {
    dom_exception(realm, "NotSupportedError", "Unrecognized algorithm")
}

/// pqPublic(name, seed): the public key of a ML-KEM, hybrid or ML-DSA seed.
fn public(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let name = arg_str(realm, args, 0)?;
    let seed = bytes(realm, args, 1)?;
    let out = if let Some((nid, _, _)) = kem_nid(&name) {
        kem_keypair(nid, &seed).and_then(|k| raw_public(&k))
    } else if let Some(nid) = dsa_nid(&name) {
        dsa_key(nid, &seed).and_then(|k| raw_public(&k))
    } else if let Some(h) = hybrid(&name) {
        hybrid_public(&h, &seed)
    } else {
        return Err(unknown(realm));
    };
    let out = out.ok_or_else(|| failed(realm, "Key generation"))?;
    Ok(make_array_buffer(realm, out))
}

/// pqEncaps(name, publicKey): ciphertext || 32-byte shared key.
fn encaps(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let name = arg_str(realm, args, 0)?;
    let pk = bytes(realm, args, 1)?;
    let out = if let Some((nid, pk_len, _)) = kem_nid(&name) {
        kem_encaps(nid, pk_len, &pk)
    } else if let Some(h) = hybrid(&name) {
        hybrid_encaps(&h, &pk)
    } else {
        return Err(unknown(realm));
    };
    let (mut ct, ss) = out.ok_or_else(|| failed(realm, "Encapsulation"))?;
    ct.extend_from_slice(&ss);
    Ok(make_array_buffer(realm, ct))
}

/// pqDecaps(name, seed, ciphertext): the shared key.
fn decaps(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let name = arg_str(realm, args, 0)?;
    let seed = bytes(realm, args, 1)?;
    let ct = bytes(realm, args, 2)?;
    let out = if let Some((nid, _, ct_len)) = kem_nid(&name) {
        kem_decaps(nid, ct_len, &seed, &ct)
    } else if let Some(h) = hybrid(&name) {
        hybrid_decaps(&h, &seed, &ct)
    } else {
        return Err(unknown(realm));
    };
    let ss = out.ok_or_else(|| failed(realm, "Decapsulation"))?;
    Ok(make_array_buffer(realm, ss))
}

/// pqSign(name, seed, message, context)
fn sign(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let name = arg_str(realm, args, 0)?;
    let nid = dsa_nid(&name).ok_or_else(|| unknown(realm))?;
    let (seed, msg, ctx) = (bytes(realm, args, 1)?, bytes(realm, args, 2)?, bytes(realm, args, 3)?);
    let sig = dsa_sign(nid, &seed, &msg, &ctx).ok_or_else(|| failed(realm, "Signing"))?;
    Ok(make_array_buffer(realm, sig))
}

/// pqVerify(name, publicKey, message, signature, context)
fn verify(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let name = arg_str(realm, args, 0)?;
    let nid = dsa_nid(&name).ok_or_else(|| unknown(realm))?;
    let (pk, msg, sig, ctx) = (bytes(realm, args, 1)?, bytes(realm, args, 2)?, bytes(realm, args, 3)?, bytes(realm, args, 4)?);
    Ok(Value::Bool(dsa_verify(nid, &pk, &msg, &sig, &ctx)))
}

pub(super) fn install(realm: &mut Realm, internals: ObjectId) {
    realm.define_method(internals, "pqPublic", 2, public);
    realm.define_method(internals, "pqEncaps", 2, encaps);
    realm.define_method(internals, "pqDecaps", 3, decaps);
    realm.define_method(internals, "pqSign", 4, sign);
    realm.define_method(internals, "pqVerify", 5, verify);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }

    #[test]
    fn hybrid_zero_seed_vector() {
        // draft-irtf-cfrg-concrete-hybrid-kems-04 B.1: MLKEM768-P256, seed = 0^32.
        let h = hybrid("MLKEM768-P256").unwrap();
        let pk = hybrid_public(&h, &[0; 32]).unwrap();
        assert!(hex(&pk).starts_with("3d209f716752f6408e7f89bceef97ac3885300453779"));
        let (ct, ss) = hybrid_encaps(&h, &pk).unwrap();
        assert_eq!(hybrid_decaps(&h, &[0; 32], &ct).unwrap(), ss);
    }

    #[test]
    fn mldsa_round_trip() {
        let nid = dsa_nid("ML-DSA-44").unwrap();
        let pk = raw_public(&dsa_key(nid, &[7; 32]).unwrap()).unwrap();
        let sig = dsa_sign(nid, &[7; 32], b"message", b"ctx").unwrap();
        assert!(dsa_verify(nid, &pk, b"message", &sig, b"ctx"));
        assert!(!dsa_verify(nid, &pk, b"message", &sig, b""));
    }
}
