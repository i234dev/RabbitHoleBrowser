//! WebCrypto selects the hash independently of the NIST curve and uses
//! fixed-width r || s. AWS-LC performs all elliptic-curve and signing operations.
use aws_lc_sys as lc;
use std::{mem::MaybeUninit, ptr::NonNull};

struct Owned<T> {
    ptr: NonNull<T>,
    free: unsafe extern "C" fn(*mut T),
}
impl<T> Owned<T> {
    fn new(ptr: *mut T, free: unsafe extern "C" fn(*mut T)) -> Option<Self> {
        Some(Self { ptr: NonNull::new(ptr)?, free })
    }
    fn get(&self) -> *mut T { self.ptr.as_ptr() }
    fn transfer(self) { std::mem::forget(self); }
}
impl<T> Drop for Owned<T> {
    fn drop(&mut self) {
        // Every constructor supplies the matching destructor for this owned reference.
        unsafe { (self.free)(self.get()) }
    }
}

fn key(der: &[u8], private: bool) -> Option<Owned<lc::EC_KEY>> {
    // CBS borrows der throughout both parse calls. get1 acquires an independent reference.
    unsafe {
        let mut reader = MaybeUninit::uninit();
        lc::CBS_init(reader.as_mut_ptr(), der.as_ptr(), der.len());
        let mut reader = reader.assume_init();
        let parsed = if private { lc::EVP_parse_private_key(&mut reader) } else { lc::EVP_parse_public_key(&mut reader) };
        let parsed = Owned::new(parsed, lc::EVP_PKEY_free)?;
        if lc::CBS_len(&reader) != 0 { return None; }
        Owned::new(lc::EVP_PKEY_get1_EC_KEY(parsed.get()), lc::EC_KEY_free)
    }
}

fn width(key: &Owned<lc::EC_KEY>) -> Option<usize> {
    // The group belongs to key and stays alive for the duration of this call.
    let group = NonNull::new(unsafe { lc::EC_KEY_get0_group(key.get()) }.cast_mut())?;
    Some((unsafe { lc::EC_GROUP_get_degree(group.as_ptr()) } as usize).div_ceil(8))
}

pub(super) fn sign(der: &[u8], digest: &[u8]) -> Option<Vec<u8>> {
    let key = key(der, true)?;
    let width = width(&key)?;
    let mut result = vec![0; 2 * width];
    // The two numbers belong to sig. Each output slice holds exactly width bytes.
    unsafe {
        let sig = Owned::new(lc::ECDSA_do_sign(digest.as_ptr(), digest.len(), key.get()), lc::ECDSA_SIG_free)?;
        let (r, s) = result.split_at_mut(width);
        if lc::BN_bn2bin_padded(r.as_mut_ptr(), width, lc::ECDSA_SIG_get0_r(sig.get())) != 1 ||
           lc::BN_bn2bin_padded(s.as_mut_ptr(), width, lc::ECDSA_SIG_get0_s(sig.get())) != 1 { return None; }
    }
    Some(result)
}

pub(super) fn verify(der: &[u8], digest: &[u8], signature: &[u8]) -> Option<bool> {
    let key = key(der, false)?;
    let width = width(&key)?;
    if signature.len() != 2 * width { return Some(false); }
    let (r, s) = signature.split_at(width);
    // set0 transfers both BIGNUMs on success. On every earlier return their owners free them.
    unsafe {
        let sig = Owned::new(lc::ECDSA_SIG_new(), lc::ECDSA_SIG_free)?;
        let r = Owned::new(lc::BN_bin2bn(r.as_ptr(), width, std::ptr::null_mut()), lc::BN_free)?;
        let s = Owned::new(lc::BN_bin2bn(s.as_ptr(), width, std::ptr::null_mut()), lc::BN_free)?;
        if lc::ECDSA_SIG_set0(sig.get(), r.get(), s.get()) != 1 { return None; }
        r.transfer();
        s.transfer();
        Some(lc::ECDSA_do_verify(digest.as_ptr(), digest.len(), sig.get(), key.get()) == 1)
    }
}
