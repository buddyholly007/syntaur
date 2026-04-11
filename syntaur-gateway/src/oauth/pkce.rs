//! PKCE (RFC 7636) S256 challenge generation.
//!
//! `code_verifier`: 32 cryptographic random bytes, base64url-no-pad.
//! `code_challenge`: base64url-no-pad(SHA256(code_verifier)).
//! Method: "S256" (always — we never advertise "plain").
//!
//! The verifier is sent to the token endpoint at code exchange time;
//! the challenge is sent to the authorization endpoint. If an attacker
//! intercepts the authorization code without the verifier, the exchange
//! fails.

use base64::Engine;
use rand::RngCore;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct PkcePair {
    pub verifier: String,
    pub challenge: String,
}

impl PkcePair {
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        let digest = Sha256::digest(verifier.as_bytes());
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
        Self {
            verifier,
            challenge,
        }
    }
}

/// Generate an opaque `state` value for CSRF protection. 32 bytes of
/// random base64url — same shape as a bearer token but tiny TTL.
pub fn generate_state() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}
