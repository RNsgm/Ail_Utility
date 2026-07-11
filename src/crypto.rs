use std::fs;
use std::path::Path;

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use rand::Rng;

pub const KEY_FILE: &str = "key.bin";
pub const KEY_SIZE: usize = 32;

#[derive(Debug)]
pub enum CryptoError {
    InvalidKeyLength,
    DataTooShort,
    EncryptionFailed,
    DecryptionFailed(String),
}

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CryptoError::InvalidKeyLength => write!(f, "invalid AES key length"),
            CryptoError::DataTooShort => write!(f, "encrypted data too short"),
            CryptoError::EncryptionFailed => write!(f, "encryption failed"),
            CryptoError::DecryptionFailed(msg) => write!(f, "decryption failed: {msg}"),
        }
    }
}

impl std::error::Error for CryptoError {}

pub fn get_or_create_key() -> Vec<u8> {
    let path = Path::new(KEY_FILE);
    if path.exists() {
        if let Ok(key) = fs::read(path) {
            if key.len() == KEY_SIZE {
                return key;
            }
        }
    }

    let mut key = vec![0u8; KEY_SIZE];
    rand::rng().fill_bytes(&mut key);
    let _ = fs::write(path, &key);
    key
}

pub fn encrypt(data: &[u8], key: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| CryptoError::InvalidKeyLength)?;
    let mut nonce_bytes = [0u8; 12];
    rand::rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, data)
        .map_err(|_| CryptoError::EncryptionFailed)?;

    let mut result = nonce_bytes.to_vec();
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

pub fn decrypt(data: &[u8], key: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if data.len() < 12 {
        return Err(CryptoError::DataTooShort);
    }

    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| CryptoError::InvalidKeyLength)?;
    let (nonce_bytes, ciphertext) = data.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| CryptoError::DecryptionFailed(e.to_string()))
}
