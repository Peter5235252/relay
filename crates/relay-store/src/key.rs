//! The raw 256-bit key that SQLCipher uses for the local database.

use std::fmt::Write as _;

use zeroize::{Zeroize, Zeroizing};

/// A 256-bit SQLCipher key.
///
/// The key is derived from Relay's master key and passed to the database only
/// while a connection is opened. It is zeroized when dropped.
pub struct DatabaseKey([u8; 32]);

impl DatabaseKey {
    /// Wrap raw key bytes.
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Format the key as the raw-key literal SQLCipher expects.
    pub(crate) fn sqlcipher_literal(&self) -> Zeroizing<String> {
        let mut hex = Zeroizing::new(String::with_capacity(64));
        for byte in self.0 {
            let _ = write!(hex, "{byte:02x}");
        }
        Zeroizing::new(format!("\"x'{}'\"", hex.as_str()))
    }
}

impl Drop for DatabaseKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl std::fmt::Debug for DatabaseKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DatabaseKey(<redacted>)")
    }
}
