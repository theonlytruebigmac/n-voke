//! N-central servers: which ones N-voke knows about, their JWTs, and the REST
//! client that talks to them. Server addresses and labels are in `config.json`
//! (not secret); each server's JWT is in the keychain, account `ncentral:<host>`.

pub mod model;
pub mod rest;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{Core, ECO_KEY, audit::AuditEntry, blocking, credentials::{ecoverse_account, ncentral_account}};

/// Which Ecoverse key a server's devices are reached with. Ecoverse is the preferred way to launch and track,
/// with N-central REST (the JWT) as the fallback.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "EcoCredential.ts"))]
#[serde(rename_all = "camelCase")]
pub enum EcoCredential {
    /// The app's own Ecoverse key (Settings): the usual case when the server is in the tenant that key belongs to.
    #[default]
    AppKey,
    /// A token saved for this server (keychain account `ecoverse:<server id>`): another Ecoverse tenant.
    Own,
    /// The server isn't used through Ecoverse; its devices are reached over REST only.
    None,
}

/// One N-central server, as saved in `config.json`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub id: String,
    pub label: String,
    /// Plain host name: no scheme, port or path.
    pub host: String,
    /// The Ecoverse tenant this server is linked to, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ecoverse_tenant: Option<String>,
    /// Which Ecoverse key reaches its devices.
    #[serde(default)]
    pub ecoverse: EcoCredential,
}

/// A server as shown to the user: never includes the JWT, only whether one is saved.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "ServerView.ts"))]
#[serde(rename_all = "camelCase")]
pub struct ServerView {
    pub id: String,
    pub label: String,
    pub host: String,
    pub ecoverse_tenant: Option<String>,
    pub has_jwt: bool,
    pub ecoverse: EcoCredential,
    /// A token of its own is saved for this server (only meaningful when `ecoverse` is `own`).
    pub has_ecoverse_token: bool,
}

/// Turns what a person typed into the plain host name used everywhere else.
/// `https://New.Example.com/` becomes `new.example.com`. Only HTTPS is accepted,
/// and a port, path or credentials are refused, because the same value is later
/// handed to the endpoint installer, which takes a bare host name.
pub fn normalize_host(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    let rest = match trimmed.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("https") => rest,
        Some(_) => return Err("The server address must use https://".into()),
        None => trimmed,
    };
    let host = rest.trim_end_matches('/').to_ascii_lowercase();
    if host.is_empty() {
        return Err("Enter the server's host name, for example n-central.example.com.".into());
    }
    if host.contains(':') {
        return Err("Enter the host name without a port: N-central is reached on 443.".into());
    }
    if host.contains(['/', '?', '#', '@']) || host.contains(char::is_whitespace) {
        return Err("Enter only the host name, with no path or credentials.".into());
    }
    let label_ok = |l: &str| !l.is_empty() && !l.starts_with('-') && !l.ends_with('-') && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if host.len() > 253 || !host.split('.').all(label_ok) {
        return Err(format!("\"{host}\" is not a valid host name."));
    }
    Ok(host)
}

impl Core {
    /// The saved servers, each with whether its JWT is in the keychain.
    pub async fn ncentral_servers(&self) -> Result<Vec<ServerView>, String> {
        let servers = self.load_config().ncentral_servers;
        let mut views = Vec::with_capacity(servers.len());
        for server in servers {
            views.push(self.server_view(server).await?);
        }
        Ok(views)
    }

    pub(crate) fn ncentral_server(&self, id: &str) -> Result<ServerConfig, String> {
        self.load_config().ncentral_servers.into_iter().find(|s| s.id == id).ok_or_else(|| format!("No N-central server with id {id} is configured."))
    }

    pub(crate) async fn ncentral_jwt(&self, host: &str) -> Result<Option<String>, String> {
        let secrets = self.secrets.clone();
        let account = ncentral_account(host);
        blocking(move || secrets.get(&account)).await
    }

    /// Adds a server, or updates the one with `id`. A JWT, when given, replaces
    /// the saved one and ends any session using the old one. Changing the host
    /// carries the saved JWT over to the new host.
    pub async fn save_ncentral_server(
        &self,
        id: Option<String>,
        label: &str,
        host: &str,
        jwt: Option<String>,
        ecoverse_tenant: Option<String>,
    ) -> Result<ServerView, String> {
        let host = normalize_host(host)?;
        let label = if label.trim().is_empty() { host.clone() } else { label.trim().to_string() };
        let ecoverse_tenant = ecoverse_tenant.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        let jwt = jwt.map(|j| j.trim().to_string()).filter(|j| !j.is_empty());

        let mut config = self.load_config();
        if config.ncentral_servers.iter().any(|s| s.host == host && Some(&s.id) != id.as_ref()) {
            return Err(format!("{host} is already in the server list."));
        }
        let previous = id.as_ref().and_then(|id| config.ncentral_servers.iter().find(|s| &s.id == id).cloned());
        if id.is_some() && previous.is_none() {
            return Err("That server no longer exists.".into());
        }
        let server = ServerConfig {
            id: id.unwrap_or_else(|| format!("srv-{}", &uuid::Uuid::new_v4().simple().to_string()[..8])),
            label,
            host: host.clone(),
            ecoverse_tenant,
            ecoverse: previous.as_ref().map(|p| p.ecoverse).unwrap_or_default(),
        };

        let secrets = self.secrets.clone();
        let account = ncentral_account(&host);
        let result = match (&jwt, &previous) {
            (Some(jwt), previous) => {
                let jwt = jwt.clone();
                let old_account = previous.as_ref().filter(|old| old.host != host).map(|old| ncentral_account(&old.host));
                blocking(move || {
                    secrets.set(&account, &jwt)?;
                    // The old host's copy would otherwise stay in the keychain with nothing pointing at it.
                    old_account.map_or(Ok(()), |old| secrets.delete(&old))
                })
                .await
            }
            (None, Some(old)) if old.host != host => {
                let old_account = ncentral_account(&old.host);
                blocking(move || {
                    if let Some(secret) = secrets.get(&old_account)? {
                        secrets.set(&account, &secret)?;
                    }
                    secrets.delete(&old_account)
                })
                .await
            }
            _ => Ok(()),
        };
        let summary = match (&previous, &jwt) {
            (None, _) => format!("Added N-central server {host}"),
            (Some(_), Some(_)) => format!("Updated N-central server {host} and replaced its JWT"),
            (Some(_), None) => format!("Updated N-central server {host}"),
        };
        self.record(&AuditEntry::new("settings.ncentral_server", &summary, result.clone(), json!({ "host": host })));
        result?;

        match config.ncentral_servers.iter_mut().find(|s| s.id == server.id) {
            Some(existing) => *existing = server.clone(),
            None => config.ncentral_servers.push(server.clone()),
        }
        self.save_config(&config)?;
        if jwt.is_some() || previous.as_ref().is_some_and(|p| p.host != host) {
            self.rest.forget(&host);
            if let Some(old) = previous.filter(|p| p.host != host) {
                self.rest.forget(&old.host);
            }
        }
        self.server_view(server).await
    }

    async fn server_view(&self, server: ServerConfig) -> Result<ServerView, String> {
        let has_jwt = self.ncentral_jwt(&server.host).await?.is_some();
        let has_ecoverse_token = if server.ecoverse == EcoCredential::Own {
            let secrets = self.secrets.clone();
            let account = ecoverse_account(&server.id);
            blocking(move || secrets.get(&account)).await?.is_some()
        } else {
            false
        };
        Ok(ServerView {
            id: server.id,
            label: server.label,
            host: server.host,
            ecoverse_tenant: server.ecoverse_tenant,
            has_jwt,
            ecoverse: server.ecoverse,
            has_ecoverse_token,
        })
    }

    /// Chooses which Ecoverse key reaches a server's devices. `Own` needs a token: the one given now, or one saved
    /// before. Leaving `Own` deletes the saved token, so none is left behind in the keychain.
    pub async fn set_server_ecoverse(&self, id: &str, mode: EcoCredential, token: Option<String>) -> Result<ServerView, String> {
        let mut config = self.load_config();
        let index = config.ncentral_servers.iter().position(|s| s.id == id).ok_or("That server no longer exists.")?;
        let host = config.ncentral_servers[index].host.clone();
        let token = token.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());

        let secrets = self.secrets.clone();
        let account = ecoverse_account(id);
        let result = match (mode, token) {
            (EcoCredential::Own, Some(token)) => blocking(move || secrets.set(&account, &token)).await,
            (EcoCredential::Own, None) => match blocking({
                let (secrets, account) = (secrets.clone(), account.clone());
                move || secrets.get(&account)
            })
            .await?
            {
                Some(_) => Ok(()),
                None => Err("Paste the Ecoverse token for this server.".to_string()),
            },
            (_, _) => blocking(move || secrets.delete(&account)).await,
        };
        let summary = match mode {
            EcoCredential::AppKey => format!("Set {host} to use the app's Ecoverse key"),
            EcoCredential::Own => format!("Set {host} to use its own Ecoverse token"),
            EcoCredential::None => format!("Set {host} to not use Ecoverse (REST only)"),
        };
        self.record(&AuditEntry::new("settings.ncentral_server", &summary, result.clone(), json!({ "host": host, "ecoverse": format!("{mode:?}") })));
        result?;

        config.ncentral_servers[index].ecoverse = mode;
        let server = config.ncentral_servers[index].clone();
        self.save_config(&config)?;
        self.server_view(server).await
    }

    /// Whether this server's devices can be reached through Ecoverse at all.
    pub(crate) fn server_uses_ecoverse(&self, server_id: &str) -> bool {
        self.ncentral_server(server_id).is_ok_and(|s| s.ecoverse != EcoCredential::None)
    }

    /// Runs an Ecoverse call with the key that belongs to `server_id`: the app's key, a token saved for the
    /// server, or an error when the server isn't used through Ecoverse. Only the one call sees the saved token.
    pub(crate) async fn with_ecoverse<T>(&self, server_id: &str, call: impl std::future::Future<Output = Result<T, String>>) -> Result<T, String> {
        let server = self.ncentral_server(server_id)?;
        match server.ecoverse {
            EcoCredential::AppKey => call.await,
            EcoCredential::None => Err(format!("Ecoverse isn't used for {}. Choose a key for it under Manage servers to use Ecoverse.", server.label)),
            EcoCredential::Own => {
                let secrets = self.secrets.clone();
                let account = ecoverse_account(&server.id);
                let token = blocking(move || secrets.get(&account)).await?.ok_or_else(|| format!("No Ecoverse token is saved for {}. Add one under Manage servers.", server.label))?;
                ECO_KEY.scope(token, call).await
            }
        }
    }

    /// Forgets a server and its JWT. Projects that used it keep their history.
    pub async fn remove_ncentral_server(&self, id: &str) -> Result<(), String> {
        let server = self.ncentral_server(id)?;
        let secrets = self.secrets.clone();
        let account = ncentral_account(&server.host);
        let eco_account = ecoverse_account(&server.id);
        let result = blocking(move || secrets.delete(&account).and_then(|()| secrets.delete(&eco_account))).await;
        self.record(&AuditEntry::new("settings.ncentral_server", &format!("Removed N-central server {}", server.host), result.clone(), json!({ "host": server.host })));
        result?;
        let mut config = self.load_config();
        config.ncentral_servers.retain(|s| s.id != id);
        self.save_config(&config)?;
        self.rest.forget(&server.host);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_reduced_to_a_plain_lowercase_name() {
        assert_eq!(normalize_host("new.example.com").unwrap(), "new.example.com");
        assert_eq!(normalize_host("  HTTPS://New.Example.com/ ").unwrap(), "new.example.com");
        assert_eq!(normalize_host("192.168.1.10").unwrap(), "192.168.1.10");
    }

    #[test]
    fn anything_but_a_bare_https_host_is_refused() {
        for bad in ["", "http://new.example.com", "ftp://x.com", "new.example.com:8443", "new.example.com/api", "user@new.example.com", "new example.com", "-bad.example.com", "a..b.com", "https://"] {
            assert!(normalize_host(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[tokio::test]
    async fn servers_are_saved_listed_updated_and_removed_without_exposing_the_jwt() {
        let core = Core::for_tests("servers");
        assert!(core.ncentral_servers().await.unwrap().is_empty());

        let added = core.save_ncentral_server(None, "Old server", "https://Old.example.com", Some(" jwt-1 ".into()), None).await.unwrap();
        assert_eq!(added.host, "old.example.com");
        assert!(added.has_jwt);
        assert_eq!(core.ncentral_jwt("old.example.com").await.unwrap().as_deref(), Some("jwt-1"), "whitespace is trimmed");
        assert!(!serde_json::to_string(&added).unwrap().contains("jwt-1"));

        let duplicate = core.save_ncentral_server(None, "Again", "old.example.com", None, None).await;
        assert!(duplicate.unwrap_err().contains("already in the server list"));

        // Renaming the host keeps the saved JWT.
        let moved = core.save_ncentral_server(Some(added.id.clone()), "", "older.example.com", None, Some("tenant-1".into())).await.unwrap();
        assert_eq!((moved.label.as_str(), moved.has_jwt, moved.ecoverse_tenant.as_deref()), ("older.example.com", true, Some("tenant-1")));
        assert_eq!(core.ncentral_jwt("old.example.com").await.unwrap(), None);
        assert_eq!(core.ncentral_jwt("older.example.com").await.unwrap().as_deref(), Some("jwt-1"));

        core.remove_ncentral_server(&added.id).await.unwrap();
        assert!(core.ncentral_servers().await.unwrap().is_empty());
        assert_eq!(core.ncentral_jwt("older.example.com").await.unwrap(), None);
        let log = core.audit_log(None).unwrap().entries;
        assert_eq!(log.len(), 3);
        assert!(!serde_json::to_string(&log).unwrap().contains("jwt-1"), "the audit log never holds the JWT");
    }

    #[tokio::test]
    async fn changing_the_host_and_the_jwt_together_leaves_no_copy_under_the_old_host() {
        let core = Core::for_tests("servers-rehost");
        let added = core.save_ncentral_server(None, "S", "old.example.com", Some("jwt-1".into()), None).await.unwrap();
        core.save_ncentral_server(Some(added.id), "S", "new.example.com", Some("jwt-2".into()), None).await.unwrap();
        assert_eq!(core.ncentral_jwt("old.example.com").await.unwrap(), None);
        assert_eq!(core.ncentral_jwt("new.example.com").await.unwrap().as_deref(), Some("jwt-2"));
    }

    async fn server(core: &Core, host: &str) -> ServerView {
        core.save_ncentral_server(None, "", host, Some("jwt".into()), None).await.unwrap()
    }

    /// Makes one Ecoverse request and returns the key it was sent with.
    async fn key_used(core: &Core) -> String {
        core.set_graphql_fake(|_, _| (200, r#"{"data":{}}"#.to_string()));
        core.graphql("query Q { x }", &serde_json::json!({}), None).await.unwrap();
        core.last_key_used.lock().unwrap().clone().unwrap()
    }

    #[tokio::test]
    async fn a_server_uses_the_apps_ecoverse_key_until_told_otherwise() {
        let core = Core::for_tests("eco-default");
        let s = server(&core, "old.example.com").await;
        assert_eq!((s.ecoverse, s.has_ecoverse_token), (EcoCredential::AppKey, false));
        assert!(core.server_uses_ecoverse(&s.id));
        assert_eq!(core.with_ecoverse(&s.id, async { Ok(key_used(&core).await) }).await.unwrap(), "test-key-not-real");
    }

    #[tokio::test]
    async fn a_server_with_its_own_token_makes_its_calls_with_that_token_and_nobody_elses_do() {
        let core = Core::for_tests("eco-own");
        let a = server(&core, "a.example.com").await;
        let b = server(&core, "b.example.com").await;
        let saved = core.set_server_ecoverse(&b.id, EcoCredential::Own, Some("  tenant-b-token \n".into())).await.unwrap();
        assert_eq!((saved.ecoverse, saved.has_ecoverse_token), (EcoCredential::Own, true));
        assert!(!serde_json::to_string(&saved).unwrap().contains("tenant-b-token"), "the token never goes out in a view");

        assert_eq!(core.with_ecoverse(&b.id, async { Ok(key_used(&core).await) }).await.unwrap(), "tenant-b-token");
        assert_eq!(core.with_ecoverse(&a.id, async { Ok(key_used(&core).await) }).await.unwrap(), "test-key-not-real");
        assert_eq!(key_used(&core).await, "test-key-not-real", "outside the scope the app's key is used again");
    }

    #[tokio::test]
    async fn scopes_nest_so_a_call_for_another_server_inside_one_uses_that_servers_key() {
        let core = Core::for_tests("eco-nest");
        let a = server(&core, "a.example.com").await;
        let b = server(&core, "b.example.com").await;
        core.set_server_ecoverse(&a.id, EcoCredential::Own, Some("token-a".into())).await.unwrap();
        core.set_server_ecoverse(&b.id, EcoCredential::Own, Some("token-b".into())).await.unwrap();
        let both = core
            .with_ecoverse(&a.id, async {
                let outer = key_used(&core).await;
                let inner = core.with_ecoverse(&b.id, async { Ok(key_used(&core).await) }).await?;
                Ok((outer, inner, key_used(&core).await))
            })
            .await
            .unwrap();
        assert_eq!(both, ("token-a".to_string(), "token-b".to_string(), "token-a".to_string()));
    }

    #[tokio::test]
    async fn a_server_not_used_through_ecoverse_refuses_the_call_and_says_how_to_change_it() {
        let core = Core::for_tests("eco-none");
        let s = server(&core, "rest-only.example.com").await;
        core.set_server_ecoverse(&s.id, EcoCredential::None, None).await.unwrap();
        assert!(!core.server_uses_ecoverse(&s.id));
        let err = core.with_ecoverse(&s.id, async { Ok(key_used(&core).await) }).await.unwrap_err();
        assert!(err.contains("Ecoverse isn't used for rest-only.example.com") && err.contains("Manage servers"), "{err}");
    }

    #[tokio::test]
    async fn its_own_token_is_needed_before_it_can_be_chosen_and_is_deleted_when_no_longer_used() {
        let core = Core::for_tests("eco-lifecycle");
        let s = server(&core, "a.example.com").await;
        let err = core.set_server_ecoverse(&s.id, EcoCredential::Own, None).await.unwrap_err();
        assert!(err.contains("Paste the Ecoverse token"), "{err}");
        assert_eq!(core.ncentral_servers().await.unwrap()[0].ecoverse, EcoCredential::AppKey, "a refused choice changes nothing");

        core.set_server_ecoverse(&s.id, EcoCredential::Own, Some("tok".into())).await.unwrap();
        // Choosing Own again without a token keeps the saved one.
        assert!(core.set_server_ecoverse(&s.id, EcoCredential::Own, None).await.unwrap().has_ecoverse_token);
        // Editing the server doesn't lose the choice.
        let edited = core.save_ncentral_server(Some(s.id.clone()), "Renamed", "a.example.com", None, None).await.unwrap();
        assert_eq!((edited.ecoverse, edited.has_ecoverse_token), (EcoCredential::Own, true));

        let back = core.set_server_ecoverse(&s.id, EcoCredential::AppKey, None).await.unwrap();
        assert_eq!((back.ecoverse, back.has_ecoverse_token), (EcoCredential::AppKey, false), "no token left behind");
    }

    #[tokio::test]
    async fn removing_a_server_removes_its_ecoverse_token_and_the_audit_log_never_holds_it() {
        let core = Core::for_tests("eco-remove");
        let s = server(&core, "a.example.com").await;
        core.set_server_ecoverse(&s.id, EcoCredential::Own, Some("secret-eco-token".into())).await.unwrap();
        core.remove_ncentral_server(&s.id).await.unwrap();
        let secrets = core.secrets.clone();
        let account = ecoverse_account(&s.id);
        assert_eq!(blocking(move || secrets.get(&account)).await.unwrap(), None);
        assert!(!serde_json::to_string(&core.audit_log(None).unwrap().entries).unwrap().contains("secret-eco-token"));
    }
}
