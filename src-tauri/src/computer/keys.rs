//! Per-Bot SSH keypairs: Ed25519 generation, encryption at rest.
//!
//! Each Bot VM is provisioned with its own Ed25519 keypair so the
//! Tauri side can SSH into the VM and SFTP files into it without
//! sharing a key across Bots. The private key never leaves MaxBot
//! unencrypted: it's sealed with `chacha20poly1305` keyed by
//! 32 bytes derived from the user's `Settings.computer_passphrase`
//! via `Argon2id`. Decrypt-on-demand happens the first time a Bot
//! is used; the decrypted handle is cached in `SshPool` for the
//! rest of the process's lifetime.
//!
//! Key layout on disk (in `ssh_keys` table):
//! - `id`            — uuid
//! - `public_key`    — OpenSSH wire-format public key string
//! - `private_key_encrypted` — ciphertext blob:
//!     [12-byte nonce | N-byte ciphertext | 16-byte tag]
//!     ciphertext = ChaCha20-Poly1305(plaintext, key, nonce)
//!     key = Argon2id(passphrase, salt, m_cost=64MB, t_cost=3, p_cost=1, 32)
//!     salt = SHA-256("maxbot.computer.ssh_keys.v1")
//!     plaintext = PKCS#8 DER bytes of the Ed25519 secret key
//!
//! The fixed salt lets us re-derive the same key across launches
//! without storing it (the user only sets the passphrase once).
//! It does mean that two installations of MaxBot using the same
//! passphrase produce the same key — that's fine because the
//! key only ever lives on the user's disk. Argon2id's job here is
//! to make brute-forcing a low-entropy passphrase expensive, not
//! to add entropy.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use ed25519_dalek::pkcs8::{DecodePrivateKey, EncodePrivateKey};
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Number of bytes we cut off the front of the Argon2id output to
/// use as the ChaCha20-Poly1305 key. Argon2's `Params::output_len`
/// is set to 32; we only need 32 bytes.
const KEY_LEN: usize = 32;
/// Number of bytes prepended to the ciphertext as the nonce.
const NONCE_LEN: usize = 12;

/// Stable salt. Distinct from a random per-key salt so we can
/// re-derive the same key across process restarts. SHA-256 of a
/// static string gives a fixed 32 bytes, which Argon2id accepts
/// as the salt.
const SALT_INPUT: &[u8] = b"maxbot.computer.ssh_keys.v1";

/// Wraps the failure modes the rest of the ComputerManager needs
/// to distinguish on. Storage / cipher errors collapse to a
/// single variant because the renderer never needs to know
/// "Argon2 OOM" vs "Poly1305 tag mismatch" — both mean "the
/// passphrase is wrong or the key blob is corrupt".
#[derive(Debug, Error)]
pub enum KeyError {
    #[error("passphrase is empty; set Settings.computer_passphrase first")]
    EmptyPassphrase,
    #[error("encryption failed: {0}")]
    Encrypt(String),
    #[error("decryption failed: passphrase is wrong or the stored key is corrupt")]
    Decrypt,
    #[error("key generation failed: {0}")]
    Generate(String),
    #[error("pkcs8 decode failed: {0}")]
    Decode(String),
}

/// Generate a fresh Ed25519 keypair. The secret is returned in
/// PKCS#8 DER form so it can be encrypted byte-for-byte (no need
/// to walk the rustcrypto struct). The public key is returned as
/// an OpenSSH wire-format string, ready to drop into a cloud-init
/// `ssh_authorized_keys` block.
pub fn generate_keypair() -> Result<(Vec<u8>, String), KeyError> {
    let mut csprng = OsRng;
    let signing = SigningKey::generate(&mut csprng);
    let pkcs8 = signing
        .to_pkcs8_der()
        .map_err(|e| KeyError::Generate(e.to_string()))?;
    let public = format_openssh_public(&signing);
    Ok((pkcs8.as_bytes().to_vec(), public))
}

/// Re-derive an Ed25519 secret key from PKCS#8 DER bytes (the
/// inverse of the generation step). Used when decrypting a stored
/// key so we can hand the raw `SigningKey` to `russh`.
pub fn decode_pkcs8(der: &[u8]) -> Result<SigningKey, KeyError> {
    SigningKey::from_pkcs8_der(der).map_err(|e| KeyError::Decode(e.to_string()))
}

/// Encrypt a PKCS#8 private key blob with a passphrase. Output
/// layout: `[nonce(12) | ciphertext | tag(16)]`. Tag is appended
/// automatically by `ChaCha20Poly1305`.
pub fn encrypt_private(der: &[u8], passphrase: &str) -> Result<Vec<u8>, KeyError> {
    if passphrase.is_empty() {
        return Err(KeyError::EmptyPassphrase);
    }
    let key = derive_key(passphrase)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
    let mut nonce_bytes = [0u8; NONCE_LEN];
    use rand::RngCore;
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = cipher
        .encrypt(nonce, der)
        .map_err(|e| KeyError::Encrypt(e.to_string()))?;
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Decrypt a previously-encrypted private key blob. Returns
/// `Err(Decrypt)` on tag mismatch (wrong passphrase or corrupt
/// blob) — we deliberately don't leak the difference.
pub fn decrypt_private(blob: &[u8], passphrase: &str) -> Result<Vec<u8>, KeyError> {
    if passphrase.is_empty() {
        return Err(KeyError::EmptyPassphrase);
    }
    if blob.len() < NONCE_LEN + 16 {
        return Err(KeyError::Decrypt);
    }
    let key = derive_key(passphrase)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
    let (nonce_bytes, ct) = blob.split_at(NONCE_LEN);
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, ct)
        .map_err(|_| KeyError::Decrypt)
}

/// Derive a 32-byte ChaCha20-Poly1305 key from the user's
/// passphrase via Argon2id. The salt is fixed (see `SALT_INPUT`)
/// so the same passphrase reproduces the same key across
/// launches — required for round-tripping through SQLite.
fn derive_key(passphrase: &str) -> Result<[u8; KEY_LEN], KeyError> {
    // Build a 32-byte fixed salt by hashing the constant input.
    let mut hasher = Sha256::new();
    hasher.update(SALT_INPUT);
    let salt: [u8; 32] = hasher.finalize().into();

    // 64 MiB / 3 iters / 1 lane — matches the OWASP "moderate"
    // recommendation for interactive use. Disk-encryption-style
    // operations want more, but this runs on every Bot key
    // decrypt and shouldn't add 200ms to the first tool call.
    let params = Params::new(64 * 1024, 3, 1, Some(KEY_LEN))
        .map_err(|e| KeyError::Encrypt(format!("argon2 params: {e}")))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = [0u8; KEY_LEN];
    argon
        .hash_password_into(passphrase.as_bytes(), &salt, &mut out)
        .map_err(|e| KeyError::Encrypt(format!("argon2 derive: {e}")))?;
    Ok(out)
}

/// Format the public half as an OpenSSH wire string:
/// `ssh-ed25519 <base64> <comment>`. Cloud-init's
/// `ssh_authorized_keys` parser accepts this directly.
fn format_openssh_public(signing: &SigningKey) -> String {
    use base64::Engine as _;
    // The OpenSSH wire format for Ed25519 public keys is the
    // fixed string "ssh-ed25519" followed by the 32-byte public
    // key bytes, all length-prefixed. We build it manually
    // because `ed25519_dalek` doesn't ship a wire-format helper.
    let mut blob = Vec::with_capacity(51);
    let tag = b"ssh-ed25519";
    blob.extend_from_slice(&(tag.len() as u32).to_be_bytes());
    blob.extend_from_slice(tag);
    let pk_bytes = signing.verifying_key().to_bytes();
    blob.extend_from_slice(&(pk_bytes.len() as u32).to_be_bytes());
    blob.extend_from_slice(&pk_bytes);
    let b64 = base64::engine::general_purpose::STANDARD.encode(&blob);
    format!("ssh-ed25519 {b64} maxbot-bot")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_tp_encrypt_decrypt_preserves_bytes() {
        let (der, pubkey) = generate_keypair().expect("generate");
        assert!(pubkey.starts_with("ssh-ed25519 "), "got {pubkey}");
        let passphrase = "correct horse battery staple";
        let ct = encrypt_private(&der, passphrase).expect("encrypt");
        // Layout: nonce(12) + ct(N) + tag(16). ct is plaintext
        // length, so total = 12 + der.len() + 16.
        assert_eq!(ct.len(), NONCE_LEN + der.len() + 16);
        let pt = decrypt_private(&ct, passphrase).expect("decrypt");
        assert_eq!(pt, der, "round-tripped plaintext must match");
    }

    #[test]
    fn wrong_passphrase_fails_to_decrypt() {
        let (der, _) = generate_keypair().expect("generate");
        let ct = encrypt_private(&der, "right").expect("encrypt");
        let err = decrypt_private(&ct, "wrong").expect_err("must reject");
        assert!(matches!(err, KeyError::Decrypt));
    }

    #[test]
    fn empty_passphrase_is_rejected() {
        let (der, _) = generate_keypair().expect("generate");
        assert!(matches!(encrypt_private(&der, ""), Err(KeyError::EmptyPassphrase)));
        let ct = encrypt_private(&der, "good").expect("encrypt");
        assert!(matches!(decrypt_private(&ct, ""), Err(KeyError::EmptyPassphrase)));
    }

    #[test]
    fn argon2_round_trip_produces_stable_key() {
        // The salt is fixed, so two derives with the same
        // passphrase must return the same key. If they didn't,
        // every relaunch would corrupt the key store.
        let a = derive_key("hunter2").expect("derive a");
        let b = derive_key("hunter2").expect("derive b");
        assert_eq!(a, b);
        let c = derive_key("hunter3").expect("derive c");
        assert_ne!(a, c, "different passphrases must produce different keys");
    }

    #[test]
    fn public_key_format_is_parseable_open_ssh() {
        // Sanity check: the public half is a 68-byte base64 blob
        // (4 byte length + 11 byte "ssh-ed25519" + 4 byte length
        // + 32 byte pk) = 51 bytes raw, 68 chars base64.
        let (_, pubkey) = generate_keypair().expect("generate");
        let parts: Vec<&str> = pubkey.split_whitespace().collect();
        assert_eq!(parts.len(), 3, "expected '<tag> <b64> <comment>'");
        assert_eq!(parts[0], "ssh-ed25519");
        assert_eq!(parts[2], "maxbot-bot");
        use base64::Engine as _;
        let b64_bytes = base64::engine::general_purpose::STANDARD
            .decode(parts[1])
            .expect("b64 decodes");
        assert_eq!(b64_bytes.len(), 51, "OpenSSH Ed25519 blob is 51 bytes");
    }
}
