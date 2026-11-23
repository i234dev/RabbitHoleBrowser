//! AES-GCM with the nonce and tag lengths exposed by Web Crypto (SP 800-38D).
use aes::cipher::{consts::U16, BlockEncrypt, KeyInit};
use ghash::{universal_hash::UniversalHash, Block, GHash};
use subtle::ConstantTimeEq;

fn authenticate(mut hash: GHash, associated_data: &[u8], ciphertext: &[u8]) -> Block {
    hash.update_padded(associated_data);
    hash.update_padded(ciphertext);
    let mut lengths = Block::default();
    lengths[..8].copy_from_slice(&((associated_data.len() as u64) * 8).to_be_bytes());
    lengths[8..].copy_from_slice(&((ciphertext.len() as u64) * 8).to_be_bytes());
    hash.update(&[lengths]);
    hash.finalize()
}

fn process<C: BlockEncrypt<BlockSize = U16>>(
    aes: C, iv: &[u8], aad: &[u8], mut data: Vec<u8>, tag_len: usize, decrypt: bool,
) -> Option<Vec<u8>> {
    let mut h = Block::default();
    aes.encrypt_block(&mut h);
    let hash = GHash::new(&h);
    let mut counter = if iv.len() == 12 {
        let mut block = Block::default();
        block[..12].copy_from_slice(iv);
        block[15] = 1;
        block
    } else {
        authenticate(hash.clone(), &[], iv)
    };
    let mut mask = counter;
    aes.encrypt_block(&mut mask);
    let tag = |data: &[u8]| {
        let mut t = authenticate(hash.clone(), aad, data);
        for (byte, k) in t.iter_mut().zip(mask) { *byte ^= k; }
        t
    };
    if decrypt {
        let split = data.len().checked_sub(tag_len)?;
        let expected = tag(&data[..split]);
        if !bool::from(expected[..tag_len].ct_eq(&data[split..])) { return None; }
        data.truncate(split);
    }
    if data.len() as u64 > (1u64 << 36) - 32 { return None; }
    let mut count = u32::from_be_bytes(counter[12..].try_into().ok()?);
    for chunk in data.chunks_mut(16) {
        count = count.wrapping_add(1);
        counter[12..].copy_from_slice(&count.to_be_bytes());
        let mut stream = counter;
        aes.encrypt_block(&mut stream);
        for (byte, k) in chunk.iter_mut().zip(stream) { *byte ^= k; }
    }
    if !decrypt { data.extend_from_slice(&tag(&data)[..tag_len]); }
    Some(data)
}

pub(super) fn crypt(key: &[u8], iv: &[u8], aad: &[u8], data: Vec<u8>, tag_len: usize, decrypt: bool) -> Option<Vec<u8>> {
    if iv.is_empty() || !matches!(tag_len, 4 | 8 | 12..=16) { return None; }
    match key.len() {
        16 => process(aes::Aes128::new_from_slice(key).ok()?, iv, aad, data, tag_len, decrypt),
        24 => process(aes::Aes192::new_from_slice(key).ok()?, iv, aad, data, tag_len, decrypt),
        32 => process(aes::Aes256::new_from_slice(key).ok()?, iv, aad, data, tag_len, decrypt),
        _ => None,
    }
}
