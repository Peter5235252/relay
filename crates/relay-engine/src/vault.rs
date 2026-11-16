//! Local encryption keys and sealed secrets.
//!
//! Relay generates one random 256-bit master key on first launch and stores it
//! in the OS keyring. The master key is expanded with HKDF-SHA256 into
//! independent keys for the SQLCipher database and for sealing API keys, so no
//! secret is written to disk in plaintext. `RELAY_MASTER_KEY` can supply the
//! master key in headless environments where a keyring is unavailable.

use std::sync::OnceLock;

use relay_store::DatabaseKey;
use ring::{
    aead::{self, Aad, LessSafeKey, Nonce, UnboundKey},
    hkdf,
};
use zeroize::{Zeroize, Zeroizing};

/// Keyring service name shared by the master key and provider credentials.
pub const SERVICE: &str = "relay";

const MASTER_ACCOUNT: &str = "vault-master-key-v1";
const MASTER_ENV: &str = "RELAY_MASTER_KEY";
const MASTER_PREFIX: &str = "v1:";
const SEALED_PREFIX: &str = "sealed:v1:";
const HKDF_SALT: &[u8] = b"relay vault hkdf v1";
const API_INFO: &[u8] = b"relay api key v1";
const DATABASE_INFO: &[u8] = b"relay database v1";
const API_AAD: &[u8] = b"relay api key sealed v1";

static VAULT: OnceLock<Result<Vault, String>> = OnceLock::new();

/// Process-wide encryption keys derived from the local master key.
pub struct Vault {
    master: [u8; 32],
}

impl std::fmt::Debug for Vault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Vault(<redacted>)")
    }
}

impl Vault {
    /// Load the master key once for the lifetime of the process.
    pub fn init() -> Result<&'static Self, String> {
        match VAULT.get_or_init(Self::load) {
            Ok(vault) => Ok(vault),
            Err(error) => Err(error.clone()),
        }
    }

    fn load() -> Result<Self, String> {
        if let Ok(value) = std::env::var(MASTER_ENV) {
            let value = value.trim();
            if value.is_empty() {
                return Err(format!("{MASTER_ENV} is empty"));
            }
            return Self::from_encoded(value)
                .map_err(|_| format!("{MASTER_ENV} must be 64 hex characters"));
        }

        let entry = keyring::Entry::new(SERVICE, MASTER_ACCOUNT)
            .map_err(|error| format!("could not open the OS keyring: {error}"))?;
        match entry.get_password() {
            Ok(value) => Self::from_encoded(value.trim())
                .map_err(|_| "the stored master key is not valid".to_owned()),
            Err(keyring::Error::NoEntry) => {
                let mut master = [0u8; 32];
                getrandom::fill(&mut master)
                    .map_err(|error| format!("could not generate a master key: {error}"))?;
                let mut hex_key = hex::encode(master);
                let mut encoded = String::with_capacity(MASTER_PREFIX.len() + hex_key.len());
                encoded.push_str(MASTER_PREFIX);
                encoded.push_str(&hex_key);
                hex_key.zeroize();
                entry.set_password(&encoded).map_err(|error| {
                    format!("could not save the master key to the OS keyring: {error}")
                })?;
                encoded.zeroize();
                Ok(Self { master })
            }
            Err(error) => Err(format!(
                "could not read the master key from the OS keyring: {error}"
            )),
        }
    }

    fn from_encoded(value: &str) -> Result<Self, ()> {
        let value = value.strip_prefix(MASTER_PREFIX).unwrap_or(value);
        let bytes = Zeroizing::new(hex::decode(value).map_err(|_| ())?);
        if bytes.len() != 32 {
            return Err(());
        }
        let mut master = [0u8; 32];
        master.copy_from_slice(&bytes);
        Ok(Self { master })
    }

    #[cfg(test)]
    fn from_master(master: [u8; 32]) -> Self {
        Self { master }
    }

    fn derive(&self, info: &[u8]) -> Result<[u8; 32], String> {
        let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, HKDF_SALT);
        let prk = salt.extract(&self.master);
        let info = [info];
        let okm = prk
            .expand(&info, hkdf::HKDF_SHA256)
            .map_err(|_| "could not derive an encryption key".to_owned())?;
        let mut key = [0u8; 32];
        okm.fill(&mut key)
            .map_err(|_| "could not derive an encryption key".to_owned())?;
        Ok(key)
    }

    /// Key used by SQLCipher for the local database.
    pub fn database_key(&self) -> Result<DatabaseKey, String> {
        let mut key = self.derive(DATABASE_INFO)?;
        let database_key = DatabaseKey::new(key);
        key.zeroize();
        Ok(database_key)
    }

    /// Whether a stored value uses Relay's sealed-secret envelope.
    pub fn is_sealed(value: &str) -> bool {
        value.starts_with(SEALED_PREFIX)
    }

    /// Seal an API key before it is written to the OS keyring.
    pub fn seal(&self, plaintext: &str) -> Result<String, String> {
        let mut key_bytes = self.derive(API_INFO)?;
        let unbound = UnboundKey::new(&aead::CHACHA20_POLY1305, &key_bytes)
            .map_err(|_| "could not initialize encryption".to_owned())?;
        key_bytes.zeroize();
        let key = LessSafeKey::new(unbound);

        let mut nonce = [0u8; aead::NONCE_LEN];
        getrandom::fill(&mut nonce)
            .map_err(|error| format!("could not generate an encryption nonce: {error}"))?;
        let mut sealed = plaintext.as_bytes().to_vec();
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(API_AAD),
            &mut sealed,
        )
        .map_err(|_| "could not encrypt the secret".to_owned())?;

        let mut blob = Vec::with_capacity(aead::NONCE_LEN + sealed.len());
        blob.extend_from_slice(&nonce);
        blob.extend_from_slice(&sealed);
        Ok(format!("{SEALED_PREFIX}{}", hex::encode(blob)))
    }

    /// Open a secret sealed by [`Vault::seal`].
    pub fn open(&self, blob: &str) -> Result<String, String> {
        let encoded = blob
            .strip_prefix(SEALED_PREFIX)
            .ok_or_else(|| "unsupported secret format".to_owned())?;
        let mut bytes = hex::decode(encoded).map_err(|_| "invalid secret encoding".to_owned())?;
        if bytes.len() <= aead::NONCE_LEN {
            return Err("the encrypted secret is truncated".to_owned());
        }

        let mut nonce = [0u8; aead::NONCE_LEN];
        nonce.copy_from_slice(&bytes[..aead::NONCE_LEN]);
        let mut key_bytes = self.derive(API_INFO)?;
        let unbound = UnboundKey::new(&aead::CHACHA20_POLY1305, &key_bytes)
            .map_err(|_| "could not initialize decryption".to_owned())?;
        key_bytes.zeroize();
        let key = LessSafeKey::new(unbound);

        let plaintext = key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(API_AAD),
                &mut bytes[aead::NONCE_LEN..],
            )
            .map_err(|_| "could not decrypt the secret".to_owned())?;
        let plaintext = plaintext.to_vec();
        bytes.zeroize();
        match String::from_utf8(plaintext) {
            Ok(plaintext) => Ok(plaintext),
            Err(error) => {
                let mut bytes = error.into_bytes();
                bytes.zeroize();
                Err("the decrypted secret is not valid UTF-8".to_owned())
            }
        }
    }
}

impl Drop for Vault {
    fn drop(&mut self) {
        self.master.zeroize();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::Vault;

    #[test]
    fn seals_and_opens_secrets() {
        let vault = Vault::from_master([7u8; 32]);
        let sealed = vault.seal("sk-test-secret").expect("seal");
        assert_ne!(sealed, "sk-test-secret");
        assert!(!sealed.contains("sk-test-secret"));
        assert_eq!(vault.open(&sealed).expect("open"), "sk-test-secret");
    }

    #[test]
    fn rejects_tampered_secrets_and_wrong_keys() {
        let vault = Vault::from_master([7u8; 32]);
        let mut sealed = vault.seal("sk-test-secret").expect("seal");
        sealed.push('0');
        assert!(vault.open(&sealed).is_err());

        let other = Vault::from_master([9u8; 32]);
        let sealed = vault.seal("sk-test-secret").expect("seal");
        assert!(other.open(&sealed).is_err());
    }

    #[test]
    fn derives_a_database_key() {
        assert!(Vault::from_master([1u8; 32]).database_key().is_ok());
    }
}
