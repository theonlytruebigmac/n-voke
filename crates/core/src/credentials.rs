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

/// Keeps every secret of this store in ONE keychain item (a JSON object), read once per run. The OS asks to unlock
/// each keychain item separately (in an unsigned dev build, again after each rebuild), so one item per server and
/// per token meant a prompt for each. A secret still in its own old item is moved into the bundle the first time it
/// is asked for, and the old item is deleted once the bundle holds it.
pub struct BundleStore<S> {
    inner: S,
    state: Mutex<Bundle>,
}

#[derive(Default)]
struct Bundle {
    /// `None` until the item has been read.
    secrets: Option<HashMap<String, String>>,
    /// Accounts already looked for in their old, separate item.
    legacy_checked: std::collections::HashSet<String>,
}

const BUNDLE_ACCOUNT: &str = "secrets";

impl<S: SecretStore> BundleStore<S> {
    pub fn new(inner: S) -> Self {
        Self { inner, state: Mutex::new(Bundle::default()) }
    }

    fn load<'a>(&self, state: &'a mut Bundle) -> Result<&'a mut HashMap<String, String>, String> {
        if state.secrets.is_none() {
            let map = match self.inner.get(BUNDLE_ACCOUNT)? {
                Some(json) => serde_json::from_str(&json).map_err(|e| format!("The saved secrets in the keychain are unreadable ({e})."))?,
                None => HashMap::new(),
            };
            state.secrets = Some(map);
        }
        Ok(state.secrets.as_mut().expect("loaded above"))
    }

    fn save(&self, map: &HashMap<String, String>) -> Result<(), String> {
        self.inner.set(BUNDLE_ACCOUNT, &serde_json::to_string(map).map_err(|e| e.to_string())?)
    }
}

impl<S: SecretStore> SecretStore for BundleStore<S> {
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        let mut guard = self.state.lock().unwrap();
        let state = &mut *guard;
        if let Some(found) = self.load(state)?.get(account) {
            return Ok(Some(found.clone()));
        }
        if !state.legacy_checked.insert(account.to_string()) {
            return Ok(None);
        }
        let Some(old) = self.inner.get(account)? else { return Ok(None) };
        let map = state.secrets.as_mut().expect("loaded above");
        map.insert(account.to_string(), old.clone());
        // The old item goes only after the bundle holds the secret, so a failed save loses nothing.
        self.save(map)?;
        self.inner.delete(account)?;
        Ok(Some(old))
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        let mut guard = self.state.lock().unwrap();
        let map = self.load(&mut guard)?;
        map.insert(account.to_string(), secret.to_string());
        self.save(map)?;
        self.inner.delete(account)
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        let mut guard = self.state.lock().unwrap();
        let map = self.load(&mut guard)?;
        if map.remove(account).is_some() {
            self.save(map)?;
        }
        self.inner.delete(account)
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

    #[test]
    fn the_bundle_keeps_every_secret_in_one_item_and_moves_old_items_in() {
        let inner = std::sync::Arc::new(MemoryStore::default());
        struct Shared(std::sync::Arc<MemoryStore>);
        impl SecretStore for Shared {
            fn get(&self, a: &str) -> Result<Option<String>, String> {
                self.0.get(a)
            }
            fn set(&self, a: &str, s: &str) -> Result<(), String> {
                self.0.set(a, s)
            }
            fn delete(&self, a: &str) -> Result<(), String> {
                self.0.delete(a)
            }
        }
        inner.set("ncentral:old.example.com", "legacy-jwt").unwrap();

        let store = BundleStore::new(Shared(inner.clone()));
        assert_eq!(store.get("nothing").unwrap(), None);
        assert_eq!(store.get("ncentral:old.example.com").unwrap().as_deref(), Some("legacy-jwt"), "an old item is still found");
        assert_eq!(inner.get("ncentral:old.example.com").unwrap(), None, "and is gone from its own item once moved");
        store.set("ecoverse:srv-1", "tok").unwrap();
        assert_eq!(store.get("ecoverse:srv-1").unwrap().as_deref(), Some("tok"));

        // A new run reads everything from the single item.
        let again = BundleStore::new(Shared(inner.clone()));
        assert_eq!(again.get("ncentral:old.example.com").unwrap().as_deref(), Some("legacy-jwt"));
        assert_eq!(again.get("ecoverse:srv-1").unwrap().as_deref(), Some("tok"));
        again.delete("ecoverse:srv-1").unwrap();
        assert_eq!(BundleStore::new(Shared(inner.clone())).get("ecoverse:srv-1").unwrap(), None);
        assert!(inner.get(BUNDLE_ACCOUNT).unwrap().is_some());
        assert_eq!(inner.get("ecoverse:srv-1").unwrap(), None, "no separate item is ever left behind");
    }
}
