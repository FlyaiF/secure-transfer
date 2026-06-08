use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use hkdf::Hkdf;
use rand::rngs::OsRng;
use sha2::Sha256;
use x25519_dalek::{EphemeralSecret, PublicKey, StaticSecret};

/// Generate a new X25519 keypair. Returns (private_key_bytes, public_key_bytes).
pub fn keygen() -> ([u8; 32], [u8; 32]) {
    let secret = StaticSecret::random_from_rng(OsRng);
    let public = PublicKey::from(&secret);
    (secret.to_bytes(), *public.as_bytes())
}

/// Derive the public key that corresponds to a receiver private key.
pub fn public_key_from_private(receiver_privkey: &[u8; 32]) -> [u8; 32] {
    let secret = StaticSecret::from(*receiver_privkey);
    let public = PublicKey::from(&secret);
    *public.as_bytes()
}

/// Encrypt file data using receiver's public key.
/// Returns encrypted blob: [ephemeral_pubkey_32B][nonce_12B][ciphertext+tag]
pub fn encrypt(data: &[u8], receiver_pubkey: &[u8; 32]) -> Result<Vec<u8>, String> {
    let receiver_public = PublicKey::from(*receiver_pubkey);

    // Ephemeral keypair for this transfer
    let ephemeral_secret = EphemeralSecret::random_from_rng(OsRng);
    let ephemeral_public = PublicKey::from(&ephemeral_secret);

    // ECDH → shared secret
    let shared_secret = ephemeral_secret.diffie_hellman(&receiver_public);

    // Derive AES key via HKDF
    let aes_key = derive_aes_key(shared_secret.as_bytes());

    // AES-256-GCM encrypt
    let cipher = Aes256Gcm::new_from_slice(&aes_key).map_err(|e| e.to_string())?;
    let nonce_bytes: [u8; 12] = rand::random();
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher.encrypt(nonce, data).map_err(|e| e.to_string())?;

    // Build blob: ephemeral_pubkey || nonce || ciphertext+tag
    let mut blob = Vec::with_capacity(32 + 12 + ciphertext.len());
    blob.extend_from_slice(ephemeral_public.as_bytes());
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ciphertext);
    Ok(blob)
}

/// Decrypt an encrypted blob using receiver's private key.
pub fn decrypt(blob: &[u8], receiver_privkey: &[u8; 32]) -> Result<Vec<u8>, String> {
    if blob.len() < 32 + 12 + 16 {
        return Err("encrypted blob too short".into());
    }

    let ephemeral_pubkey: [u8; 32] = blob[..32].try_into().unwrap();
    let nonce_bytes: [u8; 12] = blob[32..44].try_into().unwrap();
    let ciphertext = &blob[44..];

    let ephemeral_public = PublicKey::from(ephemeral_pubkey);
    let receiver_secret = StaticSecret::from(*receiver_privkey);

    // ECDH → same shared secret
    let shared_secret = receiver_secret.diffie_hellman(&ephemeral_public);

    // Derive same AES key
    let aes_key = derive_aes_key(shared_secret.as_bytes());

    // Decrypt
    let cipher = Aes256Gcm::new_from_slice(&aes_key).map_err(|e| e.to_string())?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| "decryption failed: wrong key or corrupted data".into())
}

fn derive_aes_key(shared_secret: &[u8]) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(None, shared_secret);
    let mut key = [0u8; 32];
    hk.expand(b"visual-transfer-aes-key", &mut key)
        .expect("HKDF expand failed");
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let (privkey, pubkey) = keygen();
        let data = b"Hello, visual transfer!";
        let blob = encrypt(data, &pubkey).unwrap();
        let decrypted = decrypt(&blob, &privkey).unwrap();
        assert_eq!(decrypted, data);
    }

    #[test]
    fn test_public_key_from_private_matches_keygen() {
        let (privkey, pubkey) = keygen();
        assert_eq!(public_key_from_private(&privkey), pubkey);
    }

    #[test]
    fn test_wrong_key_fails() {
        let (_privkey, pubkey) = keygen();
        let (wrong_privkey, _) = keygen();
        let data = b"secret data";
        let blob = encrypt(data, &pubkey).unwrap();
        assert!(decrypt(&blob, &wrong_privkey).is_err());
    }
}
