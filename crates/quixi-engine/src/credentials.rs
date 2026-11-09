//! Authenticated credential encryption backed by the OS vault or a master password.
use aes_gcm::{
    aead::{rand_core::RngCore, Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{engine::general_purpose::STANDARD, Engine};
use parking_lot::Mutex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};
use zeroize::Zeroizing;

const MASTER_CHECK: &[u8] = b"TSCDesk master vault v1";
const ARGON_MEMORY_KIB: u32 = 19_456;
const ARGON_ITERATIONS: u32 = 2;
const ARGON_PARALLELISM: u32 = 1;
static MASTER_KEYS: OnceLock<Mutex<HashMap<PathBuf, Zeroizing<Vec<u8>>>>> = OnceLock::new();

fn master_keys() -> &'static Mutex<HashMap<PathBuf, Zeroizing<Vec<u8>>>> {
    MASTER_KEYS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn master_path(path: &Path) -> PathBuf {
    path.with_file_name("credentials-master.json")
}

fn setup_marker(path: &Path) -> PathBuf {
    path.with_file_name("master-password-setup-required")
}

fn derive_wrapping_key(password: &str, salt: &[u8]) -> Result<Zeroizing<Vec<u8>>, String> {
    if password.chars().count() < 12 {
        return Err("Master password must contain at least 12 characters.".into());
    }
    let mut key = Zeroizing::new(vec![0; 32]);
    let params = Params::new(ARGON_MEMORY_KIB, ARGON_ITERATIONS, ARGON_PARALLELISM, Some(32))
        .map_err(|_| "Invalid master-password parameters".to_string())?;
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|_| "Could not derive the master encryption key".to_string())?;
    Ok(key)
}

fn encrypt_master_key(password: &str, credential_key: &[u8]) -> Result<Value, String> {
    let mut salt = [0; 16];
    let mut nonce = [0; 12];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce);
    let wrapping_key = derive_wrapping_key(password, &salt)?;
    let cipher = Aes256Gcm::new_from_slice(&wrapping_key).map_err(|_| "Invalid master key")?;
    let mut payload = credential_key.to_vec();
    payload.extend_from_slice(MASTER_CHECK);
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), payload.as_ref())
        .map_err(|_| "Could not protect the local credential vault")?;
    Ok(
        json!({"version":1,"kdf":"argon2id","memoryKiB":ARGON_MEMORY_KIB,
        "iterations":ARGON_ITERATIONS,"parallelism":ARGON_PARALLELISM,"salt":STANDARD.encode(salt),
        "nonce":STANDARD.encode(nonce),"wrappedKey":STANDARD.encode(ciphertext)}),
    )
}

fn decrypt_master_key(path: &Path, password: &str) -> Result<Zeroizing<Vec<u8>>, String> {
    let document: Value = serde_json::from_slice(
        &fs::read(master_path(path))
            .map_err(|_| "Master password is not configured.".to_string())?,
    )
    .map_err(|_| "The local credential vault metadata is invalid.".to_string())?;
    if document["version"] != 1 || document["kdf"] != "argon2id"
        || document["memoryKiB"] != ARGON_MEMORY_KIB
        || document["iterations"] != ARGON_ITERATIONS
        || document["parallelism"] != ARGON_PARALLELISM
    {
        return Err("Unsupported local credential vault format.".into());
    }
    let decode = |name: &str| {
        STANDARD
            .decode(document[name].as_str().unwrap_or_default())
            .map_err(|_| "The local credential vault metadata is invalid.".to_string())
    };
    let salt = decode("salt")?;
    let nonce = decode("nonce")?;
    if salt.len() != 16 || nonce.len() != 12 {
        return Err("The local credential vault metadata is invalid.".into());
    }
    let wrapping_key = derive_wrapping_key(password, &salt)?;
    let cipher = Aes256Gcm::new_from_slice(&wrapping_key).map_err(|_| "Invalid master key")?;
    let clear = Zeroizing::new(
        cipher
            .decrypt(Nonce::from_slice(&nonce), decode("wrappedKey")?.as_ref())
            .map_err(|_| "Incorrect master password.".to_string())?,
    );
    if clear.len() != 32 + MASTER_CHECK.len() || &clear[32..] != MASTER_CHECK {
        return Err("Incorrect master password.".into());
    }
    Ok(Zeroizing::new(clear[..32].to_vec()))
}

/// Reports whether the local fallback is configured and unlocked.
pub(crate) fn master_status(path: &Path) -> Value {
    json!({
        "configured": master_path(path).is_file(),
        "unlocked": master_keys().lock().contains_key(path),
        "setupRequired": setup_marker(path).is_file(),
    })
}

/// Creates a local vault, or rewraps its stable key after validating the old password.
pub(crate) fn configure_master(
    path: &Path,
    password: &str,
    current: Option<&str>,
) -> Result<(), String> {
    let credential_key = if master_path(path).exists() {
        decrypt_master_key(
            path,
            current.ok_or("Enter the current master password before replacing it.")?,
        )?
    } else {
        match native_key(path, true) {
            Ok(key) => Zeroizing::new(key),
            Err(_) => {
                let mut key = Zeroizing::new(vec![0; 32]);
                OsRng.fill_bytes(&mut key);
                key
            }
        }
    };
    let document = encrypt_master_key(password, &credential_key)?;
    super::storage::atomic_write(
        &master_path(path),
        &serde_json::to_vec_pretty(&document).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("Could not save the local credential vault: {e}"))?;
    master_keys()
        .lock()
        .insert(path.to_path_buf(), credential_key);
    let _ = fs::remove_file(setup_marker(path));
    Ok(())
}

/// Unlocks the stable credential key only for the lifetime of this process.
pub(crate) fn unlock_master(path: &Path, password: &str) -> Result<(), String> {
    let key = decrypt_master_key(path, password)?;
    master_keys().lock().insert(path.to_path_buf(), key);
    Ok(())
}

pub(crate) fn lock_master(path: &Path) {
    master_keys().lock().remove(path);
}

/// Explains how to recover secure storage without weakening credential protection.
fn credential_store_error(detail: &str) -> String {
    #[cfg(target_os = "linux")]
    {
        return format!(
            "Linux Secret Service could not persist the credential. It remains available only for this session. On Debian/ChromeOS Linux, open Passwords and Keys (seahorse), create or unlock the Login keyring, ensure `gnome-keyring-daemon --start --components=secrets` runs in the same D-Bus session, then restart TSCDesk and save again. No plaintext fallback was written. Detail: {detail}"
        );
    }
    #[cfg(not(target_os = "linux"))]
    {
        format!("Unlock your system credential store and try again: {detail}")
    }
}

fn native_key(path: &Path, create: bool) -> Result<Vec<u8>, String> {
    // Disposable test profiles never read or change the real account's vault.
    if cfg!(test) || quixi_proto::paths::test_profile().is_some() {
        let p = path.with_file_name("test-credentials.key");
        if p.exists() {
            return fs::read(p).map_err(|e| e.to_string());
        }
        if !create {
            return Err("Test credential key is missing".into());
        }
        let mut k = vec![0; 32];
        OsRng.fill_bytes(&mut k);
        super::storage::atomic_write(&p, &k).map_err(|e| e.to_string())?;
        return Ok(k);
    }
    let identity = format!("{:x}", Sha256::digest(path.to_string_lossy().as_bytes()));
    let entry = keyring::Entry::new("QuixiDesk credentials", &identity)
        .map_err(|e| credential_store_error(&e.to_string()))?;
    match entry.get_password() {
        Ok(s) => STANDARD.decode(s).map_err(|e| e.to_string()),
        Err(keyring::Error::NoEntry) if create => {
            let mut k = vec![0; 32];
            OsRng.fill_bytes(&mut k);
            entry
                .set_password(&STANDARD.encode(&k))
                .map_err(|e| credential_store_error(&e.to_string()))?;
            Ok(k)
        }
        Err(e) => Err(credential_store_error(&e.to_string())),
    }
}

fn key_for_seal(path: &Path) -> Result<(Vec<u8>, &'static str), String> {
    let master = master_keys().lock().get(path).map(|key| (**key).clone());
    match native_key(path, true) {
        Ok(native) if master.as_ref().is_some_and(|key| key == &native) => {
            Ok((native, "hybrid"))
        }
        Ok(_) if master.is_some() => Ok((master.unwrap(), "master")),
        Ok(native) => Ok((native, "native")),
        Err(native_error) => master
            .map(|key| (key, "master"))
            .ok_or_else(|| format!("{native_error} Configure or unlock the TSCDesk master-password vault to persist credentials securely.")),
    }
}

fn key_for_open(path: &Path, backend: &str) -> Result<Vec<u8>, String> {
    if backend == "hybrid" {
        if let Ok(key) = native_key(path, false) {
            return Ok(key);
        }
    }
    if matches!(backend, "master" | "hybrid") {
        return master_keys()
            .lock()
            .get(path)
            .map(|key| (**key).clone())
            .ok_or_else(|| {
                "Unlock the TSCDesk master-password vault to load saved provider credentials."
                    .into()
            });
    }
    native_key(path, false)
}
pub(crate) fn seal(path: &Path, mut document: Value) -> Result<Value, String> {
    let mut secrets = serde_json::Map::new();
    for provider in document["providers"].as_array_mut().into_iter().flatten() {
        let id = provider["id"].as_str().unwrap_or_default().to_string();
        let mut fields = serde_json::Map::new();
        for field in ["apiKeyRaw", "oauthAccessToken", "oauthRefreshToken"] {
            if let Some(v) = provider.as_object_mut().and_then(|p| p.remove(field)) {
                if v.as_str().is_some_and(|s| !s.is_empty()) {
                    fields.insert(field.into(), v);
                }
            }
        }
        if !fields.is_empty() {
            secrets.insert(id, Value::Object(fields));
        }
    }
    if !secrets.is_empty() {
        let (k, backend) = key_for_seal(path)?;
        let cipher = Aes256Gcm::new_from_slice(&k).map_err(|_| "Invalid credential key")?;
        let mut nonce = [0; 12];
        OsRng.fill_bytes(&mut nonce);
        let encrypted = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                serde_json::to_vec(&secrets).unwrap().as_ref(),
            )
            .map_err(|_| "Could not encrypt credentials")?;
        document["encryptedCredentials"] = json!({"version":1,"backend":backend,"nonce":STANDARD.encode(nonce),"ciphertext":STANDARD.encode(encrypted)});
    }
    Ok(document)
}
pub(crate) fn open(path: &Path, mut document: Value) -> Result<Value, String> {
    if let Some(encrypted) = document.get("encryptedCredentials") {
        if encrypted["version"] != 1 {
            return Err("Unsupported credential format".into());
        }
        let decode = |field: &str| {
            STANDARD
                .decode(encrypted[field].as_str().unwrap_or_default())
                .map_err(|_| "Invalid credential encoding".to_string())
        };
        let nonce = decode("nonce")?;
        if nonce.len() != 12 {
            return Err("Invalid credential nonce".into());
        }
        let backend = encrypted["backend"].as_str().unwrap_or("native");
        let k = key_for_open(path, backend)?;
        let cipher = Aes256Gcm::new_from_slice(&k).map_err(|_| "Invalid credential key")?;
        let bytes = cipher
            .decrypt(Nonce::from_slice(&nonce), decode("ciphertext")?.as_ref())
            .map_err(|_| "Credentials could not be decrypted. Existing file preserved.")?;
        let secrets: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        for provider in document["providers"].as_array_mut().into_iter().flatten() {
            if let Some(fields) = secrets[provider["id"].as_str().unwrap_or_default()].as_object() {
                provider.as_object_mut().unwrap().extend(fields.clone());
            }
        }
        document
            .as_object_mut()
            .unwrap()
            .remove("encryptedCredentials");
    }
    Ok(document)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_roundtrip_and_corruption_never_expose_secrets() {
        let dir = std::env::temp_dir().join(format!("quixi-vault-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("providers.json");
        let original = json!({"providers":[{"id":"custom","apiKeyRaw":"secret-api-key","oauthAccessToken":"access-token","oauthRefreshToken":"refresh-token"}]});
        let sealed = seal(&path, original.clone()).unwrap();
        let text = sealed.to_string();
        assert!(!text.contains("secret-api-key"));
        assert!(!text.contains("access-token"));
        assert_eq!(open(&path, sealed.clone()).unwrap(), original);
        let mut damaged = sealed;
        damaged["encryptedCredentials"]["ciphertext"] = json!(STANDARD.encode([0; 32]));
        assert!(open(&path, damaged).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn master_password_wraps_a_stable_key_and_can_be_changed() {
        let dir = std::env::temp_dir().join(format!("quixi-master-vault-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("providers.json");
        configure_master(&path, "first secure password", None).unwrap();
        let original = key_for_open(&path, "master").unwrap();
        lock_master(&path);
        assert!(unlock_master(&path, "incorrect password").is_err());
        unlock_master(&path, "first secure password").unwrap();
        configure_master(
            &path,
            "second secure password",
            Some("first secure password"),
        )
        .unwrap();
        assert_eq!(key_for_open(&path, "master").unwrap(), original);
        lock_master(&path);
        assert!(unlock_master(&path, "first secure password").is_err());
        unlock_master(&path, "second secure password").unwrap();
        let metadata = fs::read_to_string(master_path(&path)).unwrap();
        assert!(!metadata.contains("first secure password"));
        assert!(!metadata.contains("second secure password"));
        lock_master(&path);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn hybrid_provider_uses_native_then_master_without_exposing_secret() {
        let dir = std::env::temp_dir().join(format!("quixi-master-provider-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("providers.json");
        configure_master(&path, "a secure master password", None).unwrap();
        let key = key_for_open(&path, "master").unwrap();
        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        let nonce = [7_u8; 12];
        let secrets = json!({"custom":{"apiKeyRaw":"private-provider-key"}});
        let ciphertext = cipher.encrypt(Nonce::from_slice(&nonce), serde_json::to_vec(&secrets).unwrap().as_ref()).unwrap();
        let document = json!({
            "providers":[{"id":"custom"}],
            "encryptedCredentials":{"version":1,"backend":"hybrid","nonce":STANDARD.encode(nonce),"ciphertext":STANDARD.encode(ciphertext)}
        });
        assert!(!document.to_string().contains("private-provider-key"));
        lock_master(&path);
        assert_eq!(open(&path, document.clone()).unwrap()["providers"][0]["apiKeyRaw"], "private-provider-key");
        fs::remove_file(path.with_file_name("test-credentials.key")).unwrap();
        assert!(open(&path, document.clone()).is_err());
        unlock_master(&path, "a secure master password").unwrap();
        assert_eq!(open(&path, document).unwrap()["providers"][0]["apiKeyRaw"], "private-provider-key");
        lock_master(&path);
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_vault_error_is_actionable_without_offering_plaintext() {
        let message = credential_store_error("Secret Service: no result found");
        assert!(message.contains("seahorse"));
        assert!(message.contains("gnome-keyring-daemon"));
        assert!(message.contains("only for this session"));
        assert!(message.contains("No plaintext fallback was written"));
    }
}
