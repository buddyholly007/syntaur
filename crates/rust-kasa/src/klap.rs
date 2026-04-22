//! KLAP v2 handshake + symmetric session crypto.

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use sha1::{Digest, Sha1};
use sha2::Sha256;

use crate::KasaError;

type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;
type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

/// KLAP v2 auth hash: `sha256(sha1(username) || sha1(password))`.
pub fn auth_hash_v2(username: &str, password: &str) -> [u8; 32] {
    let u = Sha1::digest(username.as_bytes());
    let p = Sha1::digest(password.as_bytes());
    let mut h = Sha256::new();
    h.update(u);
    h.update(p);
    let d = h.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&d);
    out
}

/// SHA-256 of three concatenated buffers.
fn sha256_3(a: &[u8], b: &[u8], c: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(a);
    h.update(b);
    h.update(c);
    let mut out = [0u8; 32];
    out.copy_from_slice(&h.finalize());
    out
}

/// Hash that handshake1 expects from the device:
/// `sha256(local_seed || remote_seed || auth_hash)`.
pub fn h1_server_hash(local_seed: &[u8; 16], remote_seed: &[u8; 16], auth_hash: &[u8; 32]) -> [u8; 32] {
    sha256_3(local_seed, remote_seed, auth_hash)
}

/// Hash we send in handshake2: `sha256(remote_seed || local_seed || auth_hash)`.
pub fn h2_send_hash(remote_seed: &[u8; 16], local_seed: &[u8; 16], auth_hash: &[u8; 32]) -> [u8; 32] {
    sha256_3(remote_seed, local_seed, auth_hash)
}

/// AES-128 + IV base + rolling seq + signing prefix. Derived from the two
/// seeds + auth_hash after handshake2 succeeds.
pub struct KlapSession {
    key: [u8; 16],
    iv_base: [u8; 12],
    /// Signed 32-bit counter; incremented BEFORE each outbound request.
    pub seq: i32,
    sig: [u8; 28],
}

impl KlapSession {
    /// Derive a fresh session from the handshake material.
    pub fn derive(local_seed: &[u8; 16], remote_seed: &[u8; 16], auth_hash: &[u8; 32]) -> Self {
        // key = sha256("lsk" || ls || rs || ah)[..16]
        let mut kh = Sha256::new();
        kh.update(b"lsk");
        kh.update(local_seed);
        kh.update(remote_seed);
        kh.update(auth_hash);
        let kd = kh.finalize();
        let mut key = [0u8; 16];
        key.copy_from_slice(&kd[..16]);

        // iv_full = sha256("iv" || ...); iv_base = iv_full[..12]; seq = BE i32 of iv_full[28..32]
        let mut ih = Sha256::new();
        ih.update(b"iv");
        ih.update(local_seed);
        ih.update(remote_seed);
        ih.update(auth_hash);
        let iv_full = ih.finalize();
        let mut iv_base = [0u8; 12];
        iv_base.copy_from_slice(&iv_full[..12]);
        let seq = i32::from_be_bytes([iv_full[28], iv_full[29], iv_full[30], iv_full[31]]);

        // sig = sha256("ldk" || ...)[..28]
        let mut sh = Sha256::new();
        sh.update(b"ldk");
        sh.update(local_seed);
        sh.update(remote_seed);
        sh.update(auth_hash);
        let sd = sh.finalize();
        let mut sig = [0u8; 28];
        sig.copy_from_slice(&sd[..28]);

        Self {
            key,
            iv_base,
            seq,
            sig,
        }
    }

    /// Increment seq, encrypt, prepend signature. Returns `(signature||ciphertext, seq_used)`.
    pub fn encrypt(&mut self, plaintext: &[u8]) -> (Vec<u8>, i32) {
        self.seq = self.seq.wrapping_add(1);
        let seq_used = self.seq;
        let iv = self.iv_with_seq(seq_used);

        // PKCS7-pad to block boundary, then encrypt in place.
        let block = 16;
        let pad = block - (plaintext.len() % block);
        let mut buf = Vec::with_capacity(plaintext.len() + pad);
        buf.extend_from_slice(plaintext);
        buf.resize(plaintext.len() + pad, 0);
        let ct_len = Aes128CbcEnc::new(&self.key.into(), &iv.into())
            .encrypt_padded_mut::<Pkcs7>(&mut buf, plaintext.len())
            .expect("AES-CBC encrypt")
            .len();
        buf.truncate(ct_len);

        // signature = sha256(sig_prefix || seq(BE i32) || ciphertext)
        let mut sh = Sha256::new();
        sh.update(self.sig);
        sh.update(seq_used.to_be_bytes());
        sh.update(&buf);
        let sig_bytes = sh.finalize();

        let mut out = Vec::with_capacity(32 + buf.len());
        out.extend_from_slice(&sig_bytes);
        out.extend_from_slice(&buf);
        (out, seq_used)
    }

    /// Decrypt a response body at the current `seq` (don't increment here
    /// — the outer caller already incremented when building the request).
    pub fn decrypt(&self, body: &[u8]) -> Result<Vec<u8>, KasaError> {
        if body.len() < 32 + 16 {
            return Err(KasaError::ResponseTooShort { got: body.len() });
        }
        let ct = &body[32..];
        let iv = self.iv_with_seq(self.seq);
        let mut buf = ct.to_vec();
        let pt_len = Aes128CbcDec::new(&self.key.into(), &iv.into())
            .decrypt_padded_mut::<Pkcs7>(&mut buf)
            .map_err(|_| KasaError::BadPadding)?
            .len();
        buf.truncate(pt_len);
        Ok(buf)
    }

    fn iv_with_seq(&self, seq: i32) -> [u8; 16] {
        let mut iv = [0u8; 16];
        iv[..12].copy_from_slice(&self.iv_base);
        iv[12..].copy_from_slice(&seq.to_be_bytes());
        iv
    }
}
