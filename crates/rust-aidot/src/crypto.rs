//! AES-128-ECB with PKCS7 padding — what aidot's wire protocol uses.
//!
//! Chosen by the vendor; we only replicate it. ECB is the wrong choice for
//! anything security-critical (no IV, identical plaintext blocks produce
//! identical ciphertext) but we're bound to the vendor's format here.

use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use aes::Aes128;

use crate::AidotError;

const BLOCK: usize = 16;

/// Right-pad an ASCII key to 16 bytes with NULs, matching
/// python-aidot's `bytearray(16); key[:len(src)] = src`.
pub fn pad_key_to_16(src: &[u8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    let n = src.len().min(16);
    out[..n].copy_from_slice(&src[..n]);
    out
}

/// AES-128-ECB encrypt with PKCS7 padding — matches python-aidot's `aes_encrypt`.
pub fn encrypt_body(plaintext: &[u8], key: &[u8; 16]) -> Vec<u8> {
    let cipher = Aes128::new(key.into());
    let pad = BLOCK - (plaintext.len() % BLOCK);
    let mut padded = Vec::with_capacity(plaintext.len() + pad);
    padded.extend_from_slice(plaintext);
    padded.extend(std::iter::repeat(pad as u8).take(pad));

    for block in padded.chunks_mut(BLOCK) {
        let b: &mut [u8; BLOCK] = block.try_into().unwrap();
        cipher.encrypt_block(b.into());
    }
    padded
}

/// AES-128-ECB decrypt + PKCS7 unpad — matches python-aidot's `aes_decrypt`.
pub fn decrypt_body(ciphertext: &[u8], key: &[u8; 16]) -> Result<Vec<u8>, AidotError> {
    if ciphertext.is_empty() || ciphertext.len() % BLOCK != 0 {
        return Err(AidotError::Decrypt);
    }
    let cipher = Aes128::new(key.into());
    let mut plain = ciphertext.to_vec();
    for block in plain.chunks_mut(BLOCK) {
        let b: &mut [u8; BLOCK] = block.try_into().unwrap();
        cipher.decrypt_block(b.into());
    }
    // PKCS7 unpad
    let pad = *plain.last().ok_or(AidotError::Decrypt)? as usize;
    if pad == 0 || pad > BLOCK || pad > plain.len() {
        return Err(AidotError::Decrypt);
    }
    let new_len = plain.len() - pad;
    if !plain[new_len..].iter().all(|b| *b as usize == pad) {
        return Err(AidotError::Decrypt);
    }
    plain.truncate(new_len);
    Ok(plain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_short() {
        let key = pad_key_to_16(b"abcdef");
        let pt = b"hello, aidot";
        let ct = encrypt_body(pt, &key);
        assert_eq!(ct.len() % BLOCK, 0);
        let back = decrypt_body(&ct, &key).unwrap();
        assert_eq!(back, pt);
    }

    #[test]
    fn roundtrip_block_aligned() {
        // PKCS7 always adds a full block when input is block-aligned.
        let key = pad_key_to_16(b"key");
        let pt = b"exactly-16-bytes";
        assert_eq!(pt.len(), 16);
        let ct = encrypt_body(pt, &key);
        assert_eq!(ct.len(), 32);
        let back = decrypt_body(&ct, &key).unwrap();
        assert_eq!(back, pt);
    }

    #[test]
    fn pad_respects_actual_aidot_key() {
        // python-aidot harvests e.g. "d9x6TOSmcgddkdd1" (16 chars exactly).
        let k = pad_key_to_16(b"d9x6TOSmcgddkdd1");
        assert_eq!(&k[..], b"d9x6TOSmcgddkdd1");
    }
}
