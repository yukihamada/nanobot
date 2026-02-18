//! Credential Vault — AES-256-GCM encrypted credential storage in DynamoDB.
//!
//! Stores user credentials (e.g., website logins) with per-user derived keys.
//! Passwords never leave the server decrypted — they are injected directly into
//! browser automation actions.
//!
//! DynamoDB schema:
//!   PK: VAULT#{user_id}   SK: CRED#{service_name}
//!   Fields: encrypted_username, encrypted_password, nonce_u, nonce_p,
//!           service_url, display_name, created_at, updated_at

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use hkdf::Hkdf;
use sha2::Sha256;

/// Errors from vault operations.
#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("VAULT_MASTER_KEY not set or invalid (must be 64 hex chars)")]
    MasterKeyMissing,
    #[error("Encryption error: {0}")]
    Encryption(String),
    #[error("Decryption error: {0}")]
    Decryption(String),
    #[error("DynamoDB error: {0}")]
    Dynamo(String),
    #[error("Credential not found: {0}")]
    NotFound(String),
}

/// A stored credential (decrypted form, for internal use only).
#[derive(Debug, Clone)]
pub struct Credential {
    pub service_name: String,
    pub username: String,
    pub password: String,
    pub service_url: Option<String>,
    pub display_name: Option<String>,
}

/// A credential listing entry (no secrets).
#[derive(Debug, Clone, serde::Serialize)]
pub struct CredentialEntry {
    pub service_name: String,
    pub username: String,
    pub service_url: Option<String>,
    pub display_name: Option<String>,
    pub created_at: String,
}

/// Derive a per-user AES-256 key from the master key.
fn derive_user_key(master_key: &[u8; 32], user_id: &str) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(None, master_key);
    let mut okm = [0u8; 32];
    // info = "vault-credential-<user_id>"
    let info = format!("vault-credential-{user_id}");
    hk.expand(info.as_bytes(), &mut okm)
        .expect("HKDF expand should not fail for 32-byte output");
    okm
}

/// Encrypt a plaintext string with AES-256-GCM, returning (ciphertext_b64, nonce_b64).
fn encrypt_field(key: &[u8; 32], plaintext: &str) -> Result<(String, String), VaultError> {
    use aes_gcm::aead::OsRng;
    use aes_gcm::AeadCore;

    let cipher = Aes256Gcm::new(key.into());
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|e| VaultError::Encryption(e.to_string()))?;

    Ok((
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &ciphertext),
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &nonce),
    ))
}

/// Decrypt a (ciphertext_b64, nonce_b64) pair.
fn decrypt_field(
    key: &[u8; 32],
    ciphertext_b64: &str,
    nonce_b64: &str,
) -> Result<String, VaultError> {
    use base64::Engine;

    let ciphertext = base64::engine::general_purpose::STANDARD
        .decode(ciphertext_b64)
        .map_err(|e| VaultError::Decryption(format!("base64 ciphertext: {e}")))?;
    let nonce_bytes = base64::engine::general_purpose::STANDARD
        .decode(nonce_b64)
        .map_err(|e| VaultError::Decryption(format!("base64 nonce: {e}")))?;

    if nonce_bytes.len() != 12 {
        return Err(VaultError::Decryption(format!(
            "nonce must be 12 bytes, got {}",
            nonce_bytes.len()
        )));
    }

    let cipher = Aes256Gcm::new(key.into());
    let nonce = Nonce::from_slice(&nonce_bytes);
    let plaintext = cipher
        .decrypt(nonce, ciphertext.as_ref())
        .map_err(|e| VaultError::Decryption(e.to_string()))?;

    String::from_utf8(plaintext).map_err(|e| VaultError::Decryption(e.to_string()))
}

/// Parse the VAULT_MASTER_KEY env var (64 hex chars → 32 bytes).
pub fn load_master_key() -> Result<[u8; 32], VaultError> {
    let hex_str = std::env::var("VAULT_MASTER_KEY").map_err(|_| VaultError::MasterKeyMissing)?;
    let hex_str = hex_str.trim();
    if hex_str.len() != 64 {
        return Err(VaultError::MasterKeyMissing);
    }
    let bytes =
        hex::decode(hex_str).map_err(|_| VaultError::MasterKeyMissing)?;
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(key)
}

/// Store a credential in DynamoDB (encrypted).
#[cfg(feature = "dynamodb-backend")]
pub async fn store_credential(
    dynamo: &aws_sdk_dynamodb::Client,
    table: &str,
    user_id: &str,
    service_name: &str,
    username: &str,
    password: &str,
    service_url: Option<&str>,
    display_name: Option<&str>,
) -> Result<(), VaultError> {
    let master_key = load_master_key()?;
    let user_key = derive_user_key(&master_key, user_id);

    let (enc_username, nonce_u) = encrypt_field(&user_key, username)?;
    let (enc_password, nonce_p) = encrypt_field(&user_key, password)?;

    let now = chrono::Utc::now().to_rfc3339();

    use aws_sdk_dynamodb::types::AttributeValue as AV;
    let mut item = std::collections::HashMap::new();
    item.insert("pk".to_string(), AV::S(format!("VAULT#{user_id}")));
    item.insert("sk".to_string(), AV::S(format!("CRED#{service_name}")));
    item.insert("encrypted_username".to_string(), AV::S(enc_username));
    item.insert("encrypted_password".to_string(), AV::S(enc_password));
    item.insert("nonce_u".to_string(), AV::S(nonce_u));
    item.insert("nonce_p".to_string(), AV::S(nonce_p));
    item.insert("service_name".to_string(), AV::S(service_name.to_string()));
    item.insert("created_at".to_string(), AV::S(now.clone()));
    item.insert("updated_at".to_string(), AV::S(now));

    if let Some(url) = service_url {
        item.insert("service_url".to_string(), AV::S(url.to_string()));
    }
    if let Some(name) = display_name {
        item.insert("display_name".to_string(), AV::S(name.to_string()));
    }

    dynamo
        .put_item()
        .table_name(table)
        .set_item(Some(item))
        .send()
        .await
        .map_err(|e| VaultError::Dynamo(e.to_string()))?;

    tracing::info!(
        "Stored credential for user={} service={}",
        user_id,
        service_name
    );
    Ok(())
}

/// Retrieve and decrypt a credential from DynamoDB.
#[cfg(feature = "dynamodb-backend")]
pub async fn get_credential(
    dynamo: &aws_sdk_dynamodb::Client,
    table: &str,
    user_id: &str,
    service_name: &str,
) -> Result<Credential, VaultError> {
    use aws_sdk_dynamodb::types::AttributeValue as AV;

    let result = dynamo
        .get_item()
        .table_name(table)
        .key("pk", AV::S(format!("VAULT#{user_id}")))
        .key("sk", AV::S(format!("CRED#{service_name}")))
        .send()
        .await
        .map_err(|e| VaultError::Dynamo(e.to_string()))?;

    let item = result
        .item()
        .ok_or_else(|| VaultError::NotFound(service_name.to_string()))?;

    let master_key = load_master_key()?;
    let user_key = derive_user_key(&master_key, user_id);

    let enc_username = item
        .get("encrypted_username")
        .and_then(|v| v.as_s().ok())
        .ok_or_else(|| VaultError::Decryption("missing encrypted_username".into()))?;
    let nonce_u = item
        .get("nonce_u")
        .and_then(|v| v.as_s().ok())
        .ok_or_else(|| VaultError::Decryption("missing nonce_u".into()))?;
    let enc_password = item
        .get("encrypted_password")
        .and_then(|v| v.as_s().ok())
        .ok_or_else(|| VaultError::Decryption("missing encrypted_password".into()))?;
    let nonce_p = item
        .get("nonce_p")
        .and_then(|v| v.as_s().ok())
        .ok_or_else(|| VaultError::Decryption("missing nonce_p".into()))?;

    let username = decrypt_field(&user_key, enc_username, nonce_u)?;
    let password = decrypt_field(&user_key, enc_password, nonce_p)?;

    let service_url = item
        .get("service_url")
        .and_then(|v| v.as_s().ok())
        .map(|s| s.to_string());
    let display_name = item
        .get("display_name")
        .and_then(|v| v.as_s().ok())
        .map(|s| s.to_string());

    Ok(Credential {
        service_name: service_name.to_string(),
        username,
        password,
        service_url,
        display_name,
    })
}

/// List all credentials for a user (without passwords).
#[cfg(feature = "dynamodb-backend")]
pub async fn list_credentials(
    dynamo: &aws_sdk_dynamodb::Client,
    table: &str,
    user_id: &str,
) -> Result<Vec<CredentialEntry>, VaultError> {
    use aws_sdk_dynamodb::types::AttributeValue as AV;

    let master_key = load_master_key()?;
    let user_key = derive_user_key(&master_key, user_id);

    let result = dynamo
        .query()
        .table_name(table)
        .key_condition_expression("pk = :pk AND begins_with(sk, :prefix)")
        .expression_attribute_values(":pk", AV::S(format!("VAULT#{user_id}")))
        .expression_attribute_values(":prefix", AV::S("CRED#".to_string()))
        .send()
        .await
        .map_err(|e| VaultError::Dynamo(e.to_string()))?;

    let mut entries = Vec::new();
    for item in result.items() {
        let service_name = item
            .get("service_name")
            .and_then(|v| v.as_s().ok())
            .unwrap_or(&String::new())
            .to_string();

        // Decrypt username only (not password) for listing
        let username = if let (Some(enc), Some(nonce)) = (
            item.get("encrypted_username").and_then(|v| v.as_s().ok()),
            item.get("nonce_u").and_then(|v| v.as_s().ok()),
        ) {
            decrypt_field(&user_key, enc, nonce).unwrap_or_else(|_| "***".to_string())
        } else {
            "***".to_string()
        };

        let service_url = item
            .get("service_url")
            .and_then(|v| v.as_s().ok())
            .map(|s| s.to_string());
        let display_name = item
            .get("display_name")
            .and_then(|v| v.as_s().ok())
            .map(|s| s.to_string());
        let created_at = item
            .get("created_at")
            .and_then(|v| v.as_s().ok())
            .unwrap_or(&String::new())
            .to_string();

        entries.push(CredentialEntry {
            service_name,
            username,
            service_url,
            display_name,
            created_at,
        });
    }

    Ok(entries)
}

/// Delete a credential from DynamoDB.
#[cfg(feature = "dynamodb-backend")]
pub async fn delete_credential(
    dynamo: &aws_sdk_dynamodb::Client,
    table: &str,
    user_id: &str,
    service_name: &str,
) -> Result<(), VaultError> {
    use aws_sdk_dynamodb::types::AttributeValue as AV;

    dynamo
        .delete_item()
        .table_name(table)
        .key("pk", AV::S(format!("VAULT#{user_id}")))
        .key("sk", AV::S(format!("CRED#{service_name}")))
        .send()
        .await
        .map_err(|e| VaultError::Dynamo(e.to_string()))?;

    tracing::info!(
        "Deleted credential for user={} service={}",
        user_id,
        service_name
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_user_key_deterministic() {
        let master = [0xABu8; 32];
        let key1 = derive_user_key(&master, "user123");
        let key2 = derive_user_key(&master, "user123");
        assert_eq!(key1, key2);
    }

    #[test]
    fn test_derive_user_key_different_users() {
        let master = [0xABu8; 32];
        let key1 = derive_user_key(&master, "user1");
        let key2 = derive_user_key(&master, "user2");
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = [0x42u8; 32];
        let plaintext = "my-secret-password-日本語テスト";

        let (ciphertext, nonce) = encrypt_field(&key, plaintext).unwrap();
        let decrypted = decrypt_field(&key, &ciphertext, &nonce).unwrap();

        assert_eq!(plaintext, decrypted);
    }

    #[test]
    fn test_encrypt_produces_different_ciphertext() {
        let key = [0x42u8; 32];
        let plaintext = "same-password";

        let (ct1, _) = encrypt_field(&key, plaintext).unwrap();
        let (ct2, _) = encrypt_field(&key, plaintext).unwrap();

        // Random nonce → different ciphertext each time
        assert_ne!(ct1, ct2);
    }

    #[test]
    fn test_wrong_key_fails_decryption() {
        let key1 = [0x42u8; 32];
        let key2 = [0x43u8; 32];

        let (ciphertext, nonce) = encrypt_field(&key1, "secret").unwrap();
        let result = decrypt_field(&key2, &ciphertext, &nonce);

        assert!(result.is_err());
    }

    #[test]
    fn test_empty_string_encrypt_decrypt() {
        let key = [0x42u8; 32];
        let (ciphertext, nonce) = encrypt_field(&key, "").unwrap();
        let decrypted = decrypt_field(&key, &ciphertext, &nonce).unwrap();
        assert_eq!("", decrypted);
    }
}
