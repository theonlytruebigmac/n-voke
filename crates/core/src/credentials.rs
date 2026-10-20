//! Secrets beyond the Ecoverse API key: one N-central user JWT per server, and a
//! second Ecoverse key when a destination is linked to another tenant. They live
//! in the OS keychain under the app's service name, one account each. The
//! original Ecoverse key keeps its own account (`api-key`) and code path.

use std::{collections::HashMap, sync::Mutex};

use crate::KEYRING_SERVICE;

/// A place to keep secrets by account name. Calls can block (keychain round
/// trips, unlock prompts), so callers run them off the async runtime.
pub trait SecretStore: Send + Sync {
    fn get(&self, account: &str) -> Result<Option<String>, String>;
    fn set(&self, account: &str, secret: &str) -> Result<(), String>;
    fn delete(&self, account: &str) -> Result<(), String>;
}

/// Keychain account holding the JWT for an N-central server.
pub fn ncentral_account(host: &str) -> String {
    format!("ncentral:{host}")
}

/// Keychain account holding an Ecoverse key for a tenant.
pub fn ecoverse_account(tenant_id: &str) -> String {
    format!("ecoverse:{tenant_id}")
}

/// The OS keychain.
pub struct KeyringStore;

fn entry(account: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, account).map_err(|e| {
        format!("The system keychain is unavailable ({e}). On Linux, make sure a Secret Service provider such as GNOME Keyring or KWallet is running.")
    })
}

impl SecretStore for KeyringStore {
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        match entry(account)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(format!("Could not read a secret from the keychain: {e}")),
        }
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        entry(account)?.set_password(secret).map_err(|e| format!("Could not save the secret to the keychain: {e}"))
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        match entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("Could not remove the secret from the keychain: {e}")),
        }
    }
}

/// Secrets held in memory only, so tests never touch the system keychain.
#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);

impl SecretStore for MemoryStore {
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        Ok(self.0.lock().unwrap().get(account).cloned())
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        self.0.lock().unwrap().insert(account.to_string(), secret.to_string());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(account);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_are_named_per_server_and_tenant() {
        assert_eq!(ncentral_account("new.example.com"), "ncentral:new.example.com");
        assert_eq!(ecoverse_account("t-1"), "ecoverse:t-1");
    }

    #[test]
    fn the_memory_store_keeps_deletes_and_forgets() {
        let store = MemoryStore::default();
        assert_eq!(store.get("a").unwrap(), None);
        store.set("a", "one").unwrap();
        store.set("a", "two").unwrap();
        assert_eq!(store.get("a").unwrap().as_deref(), Some("two"));
        store.delete("a").unwrap();
        store.delete("a").unwrap();
        assert_eq!(store.get("a").unwrap(), None);
    }
}
