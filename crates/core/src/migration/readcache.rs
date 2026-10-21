//! A short memory of what the servers last said, so opening a step, choosing a row or paging a table doesn't read the
//! same customers, devices and tokens again. Entries are kept for minutes, not longer: they are labels and counts for
//! screens, never what the tracker decides on (the tracker reads fresh). Anything that changes a server drops what it
//! may have changed, and *Read servers again* drops everything for the project's two servers.

use std::{
    any::Any,
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use crate::{
    Core,
    ncentral::{
        model::{Device, DeviceIdentity, OrgUnit, RegistrationToken},
        rest::RestError,
    },
};

/// Customers and sites change rarely and are read by every screen that names them.
const UNITS_FOR: Duration = Duration::from_secs(300);
/// A customer's devices: counts on Map and the plan's input.
const DEVICES_FOR: Duration = Duration::from_secs(300);
/// Tokens are what Map says is missing, so a short memory: generating one drops it at once.
const TOKENS_FOR: Duration = Duration::from_secs(120);
/// A device's serial number, host name and addresses don't change while a migration is under way.
const IDENTITY_FOR: Duration = Duration::from_secs(1800);

struct Entry {
    at: Instant,
    value: Arc<dyn Any + Send + Sync>,
}

fn store() -> &'static Mutex<HashMap<String, Entry>> {
    static STORE: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

fn key(server_id: &str, what: &str, arg: &str) -> String {
    format!("{server_id}|{what}|{arg}")
}

/// The remembered value if it is younger than `ttl`, else whatever `fetch` returns (which is then remembered). A
/// failed read is not remembered.
async fn cached<T, E>(key: String, ttl: Duration, fetch: impl Future<Output = Result<T, E>>) -> Result<T, E>
where
    T: Clone + Send + Sync + 'static,
{
    let hit = store().lock().unwrap().get(&key).filter(|e| e.at.elapsed() < ttl).and_then(|e| e.value.downcast_ref::<T>().cloned());
    if let Some(value) = hit {
        return Ok(value);
    }
    let value = fetch.await?;
    store().lock().unwrap().insert(key, Entry { at: Instant::now(), value: Arc::new(value.clone()) });
    Ok(value)
}

pub fn forget_everything() {
    store().lock().unwrap().clear();
}

/// Forgets everything remembered about one server.
pub fn forget_server(server_id: &str) {
    let prefix = format!("{server_id}|");
    store().lock().unwrap().retain(|k, _| !k.starts_with(&prefix));
}

impl Core {
    pub(crate) async fn org_units_cached(&self, server_id: &str) -> Result<Vec<OrgUnit>, RestError> {
        cached(key(server_id, "units", ""), UNITS_FOR, self.ncentral_org_units(server_id)).await
    }

    pub(crate) async fn org_unit_devices_cached(&self, server_id: &str, unit_id: &str) -> Result<Vec<Device>, RestError> {
        cached(key(server_id, "devices", unit_id), DEVICES_FOR, self.ncentral_org_unit_devices(server_id, unit_id)).await
    }

    pub(crate) async fn registration_token_cached(&self, server_id: &str, unit_id: &str) -> Result<RegistrationToken, RestError> {
        cached(key(server_id, "token", unit_id), TOKENS_FOR, self.ncentral_registration_token(server_id, unit_id)).await
    }

    /// A device's identity, remembered across polls: matching a moved device reads the identity of every new device
    /// under its customer, once per source device otherwise.
    pub(crate) async fn device_identity_cached(&self, server_id: &str, device_id: &str) -> Result<DeviceIdentity, RestError> {
        cached(key(server_id, "identity", device_id), IDENTITY_FOR, self.ncentral_device_identity(server_id, device_id)).await
    }

    /// Drops what is remembered about a project's two servers and its plan, so the next read goes to the servers.
    pub async fn migration_forget_reads(&self, id: &str) -> Result<(), String> {
        let project = self.migration_load(id).await?.ok_or_else(|| format!("There is no migration project {id}."))?;
        forget_server(&project.source_server_id);
        forget_server(&project.dest_server_id);
        super::planview::forget_plan(id);
        super::trackview::forget_names(&project.source_server_id);
        super::trackview::forget_names(&project.dest_server_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn a_value_is_read_once_until_it_is_forgotten_and_a_failure_is_not_kept() {
        let calls = AtomicUsize::new(0);
        let read = |fail: bool| {
            let calls = &calls;
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                if fail { Err("down") } else { Ok(vec![1, 2, 3]) }
            }
        };
        let k = || key("srv-readcache-test", "units", "");
        assert!(cached(k(), Duration::from_secs(60), read(true)).await.is_err());
        assert_eq!(cached(k(), Duration::from_secs(60), read(false)).await.unwrap(), [1, 2, 3], "a failure left nothing behind");
        assert_eq!(cached(k(), Duration::from_secs(60), read(false)).await.unwrap(), [1, 2, 3]);
        assert_eq!(calls.load(Ordering::SeqCst), 2, "the third read came from memory");
        forget_server("srv-readcache-test");
        cached(k(), Duration::from_secs(60), read(false)).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(cached(k(), Duration::ZERO, read(false)).await.unwrap(), [1, 2, 3]);
        assert_eq!(calls.load(Ordering::SeqCst), 4, "a zero lifetime always reads");
    }

    #[tokio::test]
    async fn forgetting_one_server_leaves_another_alone() {
        let one = key("srv-a-readcache-test", "units", "");
        let two = key("srv-b-readcache-test", "units", "");
        cached::<_, ()>(one.clone(), Duration::from_secs(60), async { Ok(1u8) }).await.unwrap();
        cached::<_, ()>(two.clone(), Duration::from_secs(60), async { Ok(2u8) }).await.unwrap();
        forget_server("srv-a-readcache-test");
        assert!(store().lock().unwrap().get(&one).is_none());
        assert!(store().lock().unwrap().get(&two).is_some());
    }
}
