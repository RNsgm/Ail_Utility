use std::fs;
use std::path::Path;
use rand::Rng;
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce
};

pub const KEY_FILE: &str = "key.bin";
pub const KEY_SIZE: usize = 32; // AES-256

pub fn get_or_create_key() -> Vec<u8> {
    let path = Path::new(KEY_FILE);
    if path.exists() {
        if let Ok(key) = fs::read(path) {
            if key.len() == KEY_SIZE {
                return key;
            }
        }
    }

    // Генерируем новый ключ
    let mut key = vec![0u8; KEY_SIZE];
    rand::rng().fill_bytes(&mut key);
    let _ = fs::write(path, &key);
    key
}

pub fn encrypt(data: &[u8], key: &[u8]) -> Result<Vec<u8>, String> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| e.to_string())?;
    let mut nonce_bytes = [0u8; 12];
    rand::rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher.encrypt(nonce, data).map_err(|e| e.to_string())?;
    
    // Результат: nonce (12 байт) + зашифрованные данные
    let mut result = nonce_bytes.to_vec();
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

pub fn decrypt(data: &[u8], key: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() < 12 {
        return Err("Data too short".to_string());
    }

    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| e.to_string())?;
    let (nonce_bytes, ciphertext) = data.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    cipher.decrypt(nonce, ciphertext).map_err(|e| e.to_string())
}
