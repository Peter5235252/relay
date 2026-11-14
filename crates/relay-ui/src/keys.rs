//! OS keyring access with environment-variable fallback.
//!
//! Relay never writes API keys to SQLite. Keys live in the desktop keyring
//! (Secret Service on Linux) and are read only when a request is sent or when
//! the settings screen asks for a status badge.

use keyring::Entry;
use relay_core::Provider;
use zeroize::Zeroize;

/// Keyring service name. All Relay keys share one namespace.
pub const SERVICE: &str = "relay";

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

/// Read a key from the keyring, ignoring missing entries and backend errors.
fn keyring_key(provider: Provider) -> Option<String> {
    let entry = entry(provider).ok()?;
    let key = entry.get_password().ok()?;
    let key = key.trim().to_owned();
    (!key.is_empty()).then_some(key)
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
pub fn resolve(provider: Provider) -> Option<String> {
    keyring_key(provider).or_else(|| environment_key(provider))
}

/// Report where a credential would come from without exposing its value.
pub fn source(provider: Provider) -> KeySource {
    if keyring_key(provider).is_some() {
        KeySource::Keyring
    } else if environment_key(provider).is_some() {
        KeySource::Environment
    } else {
        KeySource::Missing
    }
}

/// Store a key in the keyring. An empty value clears the stored key.
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
        entry
            .set_password(&value)
            .map_err(|error| error.to_string())
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
