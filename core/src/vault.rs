//! Where the grants live: the system's keychain (macOS Keychain, Windows Credential Manager, the
//! Secret Service on Linux). [`Keychain`] is the `Storage` `ruchoir-native` files its refresh
//! tokens in, one entry per key it chooses.
//!
//! A Linux desktop may have no Secret Service at all. The app then keeps grants in memory only, for
//! the life of the process, and says so once: a token is never written to a plain file.

use std::collections::HashMap;
use std::sync::Mutex;

/// The keychain service every entry is filed under.
pub const SERVICE: &str = "com.ruchoir.desktop";

/// Why a token could not be stored.
#[derive(Debug)]
pub struct VaultError(pub String);

impl std::fmt::Display for VaultError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where the session token of each instance is kept.
pub trait TokenVault: Send + Sync {
    fn get(&self, id: &str) -> Option<String>;
    fn set(&self, id: &str, token: &str) -> Result<(), VaultError>;
    fn forget(&self, id: &str);
    /// Whether a token survives quitting the app.
    fn persistent(&self) -> bool;
}

/// The system's keychain.
pub struct KeyringVault;

impl KeyringVault {
    fn entry(id: &str) -> Option<keyring::Entry> {
        keyring::Entry::new(SERVICE, id).ok()
    }
}

impl TokenVault for KeyringVault {
    fn get(&self, id: &str) -> Option<String> {
        Self::entry(id)?.get_password().ok()
    }

    fn set(&self, id: &str, token: &str) -> Result<(), VaultError> {
        Self::entry(id)
            .ok_or_else(|| VaultError("the keychain is not available".into()))?
            .set_password(token)
            .map_err(|e| VaultError(e.to_string()))
    }

    fn forget(&self, id: &str) {
        if let Some(entry) = Self::entry(id) {
            let _ = entry.delete_credential();
        }
    }

    fn persistent(&self) -> bool {
        true
    }
}

/// Tokens in memory only, for a system without a keychain.
#[derive(Default)]
pub struct MemoryVault(Mutex<HashMap<String, String>>);

impl TokenVault for MemoryVault {
    fn get(&self, id: &str) -> Option<String> {
        self.0.lock().ok()?.get(id).cloned()
    }

    fn set(&self, id: &str, token: &str) -> Result<(), VaultError> {
        self.0
            .lock()
            .map_err(|_| VaultError("poisoned".into()))?
            .insert(id.to_string(), token.to_string());
        Ok(())
    }

    fn forget(&self, id: &str) {
        if let Ok(mut map) = self.0.lock() {
            map.remove(id);
        }
    }

    fn persistent(&self) -> bool {
        false
    }
}

/// Whether the keychain answers: store, read back and delete a throwaway entry.
pub fn keychain_works() -> bool {
    let probe = format!("probe-{}", crate::instances::random_hex(4));
    let Ok(entry) = keyring::Entry::new(SERVICE, &probe) else {
        return false;
    };
    let works = entry.set_password("probe").is_ok()
        && entry.get_password().ok().as_deref() == Some("probe");
    let _ = entry.delete_credential();
    works
}

/// The keychain if it works, else memory.
pub fn open_vault() -> Box<dyn TokenVault> {
    open_vault_with(keychain_works)
}

/// `open_vault` with the probe given, so the fallback is tested without a keychain.
pub fn open_vault_with(probe: impl FnOnce() -> bool) -> Box<dyn TokenVault> {
    if probe() {
        Box::new(KeyringVault)
    } else {
        Box::new(MemoryVault::default())
    }
}

/// The keychain (or memory, without one) as `ruchoir-native` stores in it.
pub struct Keychain(Box<dyn TokenVault>);

impl Keychain {
    /// The system's keychain if it works, else memory.
    pub fn open() -> Self {
        Self(open_vault())
    }

    /// Over a given vault (tests).
    pub fn with(vault: Box<dyn TokenVault>) -> Self {
        Self(vault)
    }

    /// Whether a grant survives quitting the app.
    pub fn persistent(&self) -> bool {
        self.0.persistent()
    }
}

impl ruchoir_native::Storage for Keychain {
    async fn get(&self, key: &str) -> Result<Option<String>, String> {
        Ok(self.0.get(key))
    }

    async fn set(&self, key: &str, value: &str) -> Result<(), String> {
        self.0.set(key, value).map_err(|e| e.0)
    }

    async fn delete(&self, key: &str) -> Result<(), String> {
        self.0.forget(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_keychain_stores_for_the_native_client() {
        use ruchoir_native::Storage;
        let keychain = Keychain::with(Box::new(MemoryVault::default()));
        assert!(!keychain.persistent());
        assert_eq!(keychain.get("ruchoir.k").await, Ok(None));
        keychain.set("ruchoir.k", "v").await.unwrap();
        assert_eq!(keychain.get("ruchoir.k").await, Ok(Some("v".into())));
        keychain.delete("ruchoir.k").await.unwrap();
        assert_eq!(keychain.get("ruchoir.k").await, Ok(None));
    }

    #[test]
    fn memory_vault_keeps_and_forgets() {
        let vault = MemoryVault::default();
        assert_eq!(vault.get("a"), None);
        vault.set("a", "t1").unwrap();
        assert_eq!(vault.get("a").as_deref(), Some("t1"));
        vault.forget("a");
        assert_eq!(vault.get("a"), None);
        assert!(!vault.persistent());
    }

    #[test]
    fn vault_falls_back_to_memory() {
        let vault = open_vault_with(|| false);
        assert!(!vault.persistent());
        vault.set("a", "t1").unwrap();
        assert_eq!(vault.get("a").as_deref(), Some("t1"));
        vault.forget("a");
        assert_eq!(vault.get("a"), None);
    }

    #[test]
    fn a_working_keychain_is_used() {
        assert!(open_vault_with(|| true).persistent());
    }
}
