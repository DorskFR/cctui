use std::fmt::Write;

/// XOR-obfuscate a string with a key, encoding result as hex
pub fn obfuscate(plaintext: &str, key: &[u8]) -> String {
    if key.is_empty() {
        return plaintext.to_string();
    }
    let mut result = String::new();
    for (i, b) in plaintext.bytes().enumerate() {
        let _ = write!(result, "{:02x}", b ^ key[i % key.len()]);
    }
    result
}

pub fn deobfuscate(ciphertext: &str, key: &[u8]) -> Option<String> {
    if key.is_empty() {
        return Some(ciphertext.to_string());
    }
    if ciphertext.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::new();
    for i in (0..ciphertext.len()).step_by(2) {
        bytes.push(u8::from_str_radix(&ciphertext[i..i + 2], 16).ok()?);
    }
    let result: Vec<u8> = bytes.iter().enumerate().map(|(i, &b)| b ^ key[i % key.len()]).collect();
    String::from_utf8(result).ok()
}

pub fn vault_key() -> Vec<u8> {
    if let Ok(k) = std::env::var("CCTUI_VAULT_KEY") {
        let mut bytes = Vec::new();
        for i in (0..k.len()).step_by(2) {
            if let Ok(b) = u8::from_str_radix(&k[i..i + 2], 16) {
                bytes.push(b);
            } else {
                return b"cctui-default-vault-key-change-me".to_vec();
            }
        }
        bytes
    } else {
        b"cctui-default-vault-key-change-me".to_vec()
    }
}
