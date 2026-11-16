//! OS keyring access with environment-variable fallback.
//!
//! Relay never writes API keys to SQLite. Keys live sealed with
//! ChaCha20-Poly1305 in the desktop keyring (Secret Service on Linux) and are
//! decrypted only when a request is sent or the settings screen asks for a
//! status badge. Plaintext entries written by older Relay versions are
//! re-sealed the first time they are read.

use keyring::Entry;
use relay_core::Provider;
use zeroize::Zeroize;

use crate::vault::Vault;

/// Keyring service name. All Relay keys share one namespace.
pub const SERVICE: &str = crate::vault::SERVICE;

/// Where a usable credential came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// Stored in the OS keyring.
    Keyring,
    /// Supplied by an environment variable.
    Environment,
    /// No credential is available.
    Missing,
}

impl KeySource {
    /// Short label for the settings screen.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Keyring => "Keyring",
            Self::Environment => "Environment",
            Self::Missing => "Not set",
        }
    }

    /// Whether a request can be authenticated.
    pub const fn is_set(self) -> bool {
        !matches!(self, Self::Missing)
    }
}

fn entry(provider: Provider) -> Result<Entry, String> {
    Entry::new(SERVICE, provider.id()).map_err(|error| error.to_string())
}

/// Decrypt a keyring value, transparently migrating legacy plaintext entries.
fn decode(provider: Provider, entry: &Entry, stored: &str) -> Result<Option<String>, String> {
    let stored = stored.trim();
    if stored.is_empty() {
        return Ok(None);
    }

    let vault = Vault::init()?;
    if Vault::is_sealed(stored) {
        let mut key = vault.open(stored)?;
        let trimmed = key.trim().to_owned();
        key.zeroize();
        return Ok((!trimmed.is_empty()).then_some(trimmed));
    }

    // Older Relay versions stored the key as plaintext in the keyring. Seal it
    // now, then continue as if it had always been sealed.
    let mut legacy = stored.to_owned();
    let sealed = vault.seal(&legacy);
    if let Err(error) = &sealed {
        tracing::warn!(provider = provider.id(), %error, "could not seal a legacy API key");
    } else if let Ok(sealed) = &sealed
        && let Err(error) = entry.set_password(sealed)
    {
        tracing::warn!(
            provider = provider.id(),
            %error,
            "could not migrate a legacy API key to encrypted storage"
        );
    }
    let key = legacy.trim().to_owned();
    legacy.zeroize();
    Ok((!key.is_empty()).then_some(key))
}

/// Read and decrypt a key from the keyring.
fn keyring_key(provider: Provider) -> Result<Option<String>, String> {
    let entry = entry(provider)?;
    let mut stored = match entry.get_password() {
        Ok(value) => value,
        Err(keyring::Error::NoEntry) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let decoded = decode(provider, &entry, &stored);
    stored.zeroize();
    decoded
}

/// Read the first non-empty environment variable configured for a provider.
fn environment_key(provider: Provider) -> Option<String> {
    for name in provider.env_keys() {
        if let Ok(value) = std::env::var(name) {
            let value = value.trim().to_owned();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

/// Resolve a credential, preferring the keyring over the environment.
///
/// Keyring failures fall back to the environment so a headless or locked
/// keyring does not disable providers that are configured by variable.
pub fn resolve(provider: Provider) -> Option<String> {
    match keyring_key(provider) {
        Ok(Some(key)) => Some(key),
        Ok(None) => environment_key(provider),
        Err(error) => {
            tracing::warn!(
                provider = provider.id(),
                %error,
                "could not read the API key from the keyring"
            );
            environment_key(provider)
        }
    }
}

/// Report where a credential would come from without exposing its value.
pub fn source(provider: Provider) -> KeySource {
    match keyring_key(provider) {
        Ok(Some(mut key)) => {
            key.zeroize();
            KeySource::Keyring
        }
        Ok(None) => match environment_key(provider) {
            Some(mut key) => {
                key.zeroize();
                KeySource::Environment
            }
            None => KeySource::Missing,
        },
        Err(_) => match environment_key(provider) {
            Some(mut key) => {
                key.zeroize();
                KeySource::Environment
            }
            None => KeySource::Missing,
        },
    }
}

/// Seal and store a key in the keyring. An empty value clears the stored key.
pub fn save(provider: Provider, value: &str) -> Result<(), String> {
    let entry = entry(provider)?;
    let mut value = value.trim().to_owned();
    let result = if value.is_empty() {
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            // Clearing a key that was never stored is not an error.
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    } else {
        match Vault::init().and_then(|vault| vault.seal(&value)) {
            Ok(sealed) => entry
                .set_password(&sealed)
                .map_err(|error| error.to_string()),
            Err(error) => Err(error),
        }
    };
    value.zeroize();
    result
}

/// Remove a stored key. Missing entries are ignored.
pub fn clear(provider: Provider) -> Result<(), String> {
    let entry = entry(provider)?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// Human-readable environment hint shown in settings.
pub fn environment_hint(provider: Provider) -> String {
    let names = provider.env_keys();
    if names.is_empty() {
        String::new()
    } else {
        names.join(" or ")
    }
}
