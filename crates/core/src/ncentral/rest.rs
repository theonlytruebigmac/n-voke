//! The N-central REST client. It follows the same rules as `Core::graphql`:
//! every write is audited, dry-run holds every write back and records exactly
//! what would have been sent (secrets redacted), and 429s are retried.
//!
//! Sessions: the user's JWT (from the keychain) is exchanged for an access token
//! at `POST /api/auth/authenticate`; the token is kept in memory only and
//! renewed by authenticating again shortly before it expires, or at once after
//! a 401 (a server restart ends every session). The refresh token the server
//! also returns is not used: authenticating again is verified in the lab, cheap,
//! and needs no extra state.

use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use regex::Regex;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::{
    sync::{Mutex as AsyncMutex, OwnedSemaphorePermit, Semaphore},
    time::Instant,
};

use super::model::{Device, DeviceIdentity, OrgUnit, RegistrationToken, ServerInfo, TaskDetail};
use crate::migration::endpoint_result::RestOutput;
use crate::{
    Core, DRY_RUN_ERROR,
    audit::{AuditEntry, AuditMeta},
    retry_wait,
};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The largest page N-central serves.
pub const PAGE_SIZE: usize = 1000;
/// A safety stop when paging, so a misbehaving server can't loop forever.
const MAX_PAGES: usize = 200;
/// How early a session is renewed, so a request never goes out with a token about to expire.
const SESSION_MARGIN: Duration = Duration::from_secs(60);
const RATE_LIMIT_RETRIES: u32 = 3;
const GATEWAY_RETRIES: u32 = 2;
/// The Windows Agent's software ID, used when the installer list doesn't say.
const WINDOWS_AGENT_SOFTWARE_ID: &str = "101";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
        }
    }
}

#[derive(Clone, Debug)]
pub struct RestRequest {
    pub method: Method,
    pub url: String,
    pub bearer: Option<String>,
    pub body: Option<Value>,
}

#[derive(Clone, Debug)]
pub struct RestResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub retry_after: Option<String>,
}

/// How requests reach the network. The real one is `ReqwestTransport`; tests supply their own.
pub trait RestTransport: Send + Sync {
    fn send<'a>(&'a self, request: RestRequest) -> BoxFuture<'a, Result<RestResponse, String>>;
}

pub struct ReqwestTransport(pub reqwest::Client);

impl ReqwestTransport {
    /// Speaks only HTTPS and never follows a redirect, so a request (a launch carries a registration token) goes to
    /// the host it was addressed to and nowhere else.
    fn strict() -> Result<Self, String> {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(format!("n-voke/{}", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .https_only(true)
            .build()
            .map(Self)
            .map_err(|e| format!("Could not build the HTTP client: {e}"))
    }
}

impl RestTransport for ReqwestTransport {
    fn send<'a>(&'a self, request: RestRequest) -> BoxFuture<'a, Result<RestResponse, String>> {
        Box::pin(async move {
            let method = match request.method {
                Method::Get => reqwest::Method::GET,
                Method::Post => reqwest::Method::POST,
                Method::Put => reqwest::Method::PUT,
                Method::Delete => reqwest::Method::DELETE,
            };
            let mut builder = self.0.request(method, &request.url).header(reqwest::header::ACCEPT, "application/json");
            if let Some(token) = &request.bearer {
                builder = builder.bearer_auth(token);
            }
            if let Some(body) = &request.body {
                builder = builder.json(body);
            }
            let response = builder.send().await.map_err(|e| format!("Request to {} failed: {e}", request.url.split('?').next().unwrap_or("")))?;
            let status = response.status().as_u16();
            let retry_after = response.headers().get(reqwest::header::RETRY_AFTER).and_then(|v| v.to_str().ok()).map(String::from);
            let body = response.bytes().await.map_err(|e| format!("Could not read the response: {e}"))?.to_vec();
            Ok(RestResponse { status, body, retry_after })
        })
    }
}

/// A failed REST call: the HTTP status when the server answered, and a message fit to show.
#[derive(Clone, Debug, PartialEq)]
pub struct RestError {
    pub status: Option<u16>,
    pub message: String,
    /// For a change: the request may have been carried out even though this is an error (the answer was lost,
    /// the server failed after starting, or the reply made no sense). Asking again could do it twice.
    pub outcome_unknown: bool,
}

impl RestError {
    fn new(status: Option<u16>, message: impl Into<String>) -> Self {
        Self { status, message: message.into(), outcome_unknown: false }
    }

    fn unknown_outcome(status: Option<u16>, message: impl Into<String>) -> Self {
        Self { outcome_unknown: true, ..Self::new(status, message) }
    }

    pub fn is_dry_run(&self) -> bool {
        self.message == DRY_RUN_ERROR
    }

    /// A create call for a name that is taken. N-central answers 400 with `OrganizationUnitAlreadyExistException` (the
    /// spec says 409), so both count.
    pub fn is_already_there(&self) -> bool {
        self.status == Some(409) || self.message.contains("AlreadyExist")
    }
}

impl std::fmt::Display for RestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<RestError> for String {
    fn from(e: RestError) -> String {
        e.message
    }
}

impl From<String> for RestError {
    fn from(message: String) -> Self {
        RestError::new(None, message)
    }
}

// ---- limits (no network) ---------------------------------------------------------

/// How many calls to an endpoint may be in flight at once, from N-central's own
/// limits: 50 by default, fewer for heavy endpoints. Exceeding them gets a 429.
/// Returns the limit's name (one semaphore per name) and its size.
pub fn limit_for(method: Method, path: &str) -> (&'static str, usize) {
    match (method, path) {
        (Method::Post, "/api/device") => ("create-device", 1),
        (Method::Post, "/api/scheduled-tasks/direct") => ("direct-task", 30),
        (_, p) if p.ends_with("/activation-key") => ("activation-key", 1),
        (Method::Get, p) if p == "/api/devices" || (p.starts_with("/api/org-units/") && p.ends_with("/devices")) => ("device-list", 5),
        _ => ("default", 50),
    }
}

/// IDs go into URL paths, so only plain identifiers are allowed.
pub fn check_id(id: &str) -> Result<&str, RestError> {
    if !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        Ok(id)
    } else {
        Err(RestError::new(None, format!("\"{id}\" is not a valid N-central ID.")))
    }
}

static REGISTRATION_TOKEN_ARG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(-RegistrationToken\s+)\S+").unwrap());

/// Text made safe to store or show: a registration token that was passed on a command line is blanked.
pub fn scrub_secrets(text: &str) -> String {
    REGISTRATION_TOKEN_ARG.replace_all(text, "${1}[redacted]").into_owned()
}

/// A request body made safe to log or show: password-typed values are blanked, and so is the
/// registration token inside a launch's `CommandLine`, whatever the parameter's type. N-central
/// itself shows that command line in full, so N-voke can't do more than not repeat it.
pub fn redact_body(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let password = map.get("type").and_then(Value::as_str).is_some_and(|t| t.eq_ignore_ascii_case("password"));
            Value::Object(
                map.iter()
                    .map(|(k, v)| {
                        let shown = match v {
                            // A launch's command line stays readable with only the token blanked; any other password goes entirely.
                            Value::String(s) if password && k == "value" && !REGISTRATION_TOKEN_ARG.is_match(s) => json!("[redacted]"),
                            other => redact_body(other),
                        };
                        (k.clone(), shown)
                    })
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(redact_body).collect()),
        Value::String(s) => Value::String(REGISTRATION_TOKEN_ARG.replace_all(s, "${1}[redacted]").into_owned()),
        other => other.clone(),
    }
}

/// What N-central's error body says, or a snippet of whatever came back.
fn describe_error(status: u16, text: &str) -> String {
    let text = &scrub_secrets(text);
    let from_body = serde_json::from_str::<Value>(text).ok().and_then(|b| {
        let message = b.get("message").and_then(Value::as_str).filter(|m| !m.is_empty());
        let kind = b.get("exceptionName").or_else(|| b.get("error")).and_then(Value::as_str);
        message.map(|m| kind.map_or_else(|| m.to_string(), |k| format!("{k}: {m}")))
    });
    let detail = from_body.unwrap_or_else(|| text.trim().chars().take(200).collect());
    let hint = match status {
        401 => " The JWT was refused: it has expired or been revoked. Replace it in Settings.",
        403 => " The JWT's user isn't allowed to do this.",
        404 => " Not found.",
        300..=399 => " The server redirected the request, and N-voke doesn't follow redirects. Check the host name.",
        _ => "",
    };
    match (detail.is_empty(), status) {
        (true, 429) => "HTTP 429: N-central is rate limiting requests. Wait a moment and try again.".into(),
        (true, _) => format!("HTTP {status}.{hint}"),
        (false, _) => format!("HTTP {status}: {detail}{hint}"),
    }
}

// ---- state ---------------------------------------------------------------------

struct Session {
    access: String,
    valid_until: Instant,
}

#[derive(Default)]
struct HostState {
    /// Held while authenticating, so concurrent requests share one login.
    session: AsyncMutex<Option<Session>>,
    limits: Mutex<HashMap<&'static str, Arc<Semaphore>>>,
}

impl HostState {
    async fn permit(&self, method: Method, path: &str) -> OwnedSemaphorePermit {
        let (name, size) = limit_for(method, path);
        let semaphore = self.limits.lock().unwrap().entry(name).or_insert_with(|| Arc::new(Semaphore::new(size))).clone();
        semaphore.acquire_owned().await.expect("the semaphore is never closed")
    }
}

/// Everything REST that outlives one call: sessions and limits per host, and the transport.
#[derive(Default)]
pub struct RestState {
    hosts: Mutex<HashMap<String, Arc<HostState>>>,
    transport: Mutex<Option<Arc<dyn RestTransport>>>,
}

impl RestState {
    fn host(&self, host: &str) -> Arc<HostState> {
        self.hosts.lock().unwrap().entry(host.to_string()).or_default().clone()
    }

    /// Ends the host's session, so the next call logs in again (a JWT was replaced, or a server removed).
    pub(crate) fn forget(&self, host: &str) {
        self.hosts.lock().unwrap().remove(host);
    }

    fn transport(&self, http: &reqwest::Client) -> Arc<dyn RestTransport> {
        self.transport.lock().unwrap().get_or_insert_with(|| Arc::new(ReqwestTransport::strict().unwrap_or_else(|_| ReqwestTransport(http.clone())))).clone()
    }
}

/// One REST call.
pub struct Call<'a> {
    pub method: Method,
    pub path: &'a str,
    pub query: Vec<(&'a str, String)>,
    pub body: Option<Value>,
    /// For writes: what to call the change in the audit log and dry-run view.
    pub audit: Option<AuditMeta>,
}

impl<'a> Call<'a> {
    pub fn get(path: &'a str) -> Self {
        Self { method: Method::Get, path, query: vec![], body: None, audit: None }
    }

    pub fn write(method: Method, path: &'a str, body: Option<Value>, audit: AuditMeta) -> Self {
        Self { method, path, query: vec![], body, audit: Some(audit) }
    }

    pub fn with_query(mut self, key: &'a str, value: impl ToString) -> Self {
        self.query.push((key, value.to_string()));
        self
    }
}

/// A launch on one device through the REST path.
pub struct DirectTask<'a> {
    pub name: &'a str,
    /// The repository item ID of the uploaded endpoint script.
    pub item_id: &'a str,
    /// The device's own customer ID (not a site's).
    pub customer_id: &'a str,
    pub device_id: &'a str,
    /// For the audit log and dry-run view.
    pub device_label: &'a str,
    pub command_line: &'a str,
}

impl RestResponse {
    fn body_bytes(&self) -> &[u8] {
        &self.body
    }

    /// The body as text, for the JSON the API speaks.
    pub fn text(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }
}

impl Core {
    /// Replaces how REST requests are sent. For tests of code built on the client.
    #[doc(hidden)]
    pub fn set_rest_transport(&self, transport: Arc<dyn RestTransport>) {
        *self.rest.transport.lock().unwrap() = Some(transport);
    }

    /// Sends one request, retrying what is safe to retry: a 429 always (the request
    /// was refused unprocessed), a gateway error only for reads.
    async fn rest_send(&self, request: RestRequest) -> Result<RestResponse, RestError> {
        let transport = self.rest.transport(&self.http);
        let is_read = request.method == Method::Get;
        let (mut rate_limited, mut gateway) = (0, 0);
        loop {
            let response = transport.send(request.clone()).await.map_err(|m| RestError::unknown_outcome(None, m))?;
            match response.status {
                429 if rate_limited < RATE_LIMIT_RETRIES => {
                    tokio::time::sleep(retry_wait(response.retry_after.as_deref(), rate_limited)).await;
                    rate_limited += 1;
                }
                502..=504 if is_read && gateway < GATEWAY_RETRIES => {
                    tokio::time::sleep(Duration::from_secs(1 << gateway)).await;
                    gateway += 1;
                }
                _ => return Ok(response),
            }
        }
    }

    /// A valid access token for the host, logging in when there is none or it's about to expire.
    async fn rest_access_token(&self, host: &str, state: &HostState) -> Result<String, RestError> {
        let mut session = state.session.lock().await;
        if let Some(s) = session.as_ref().filter(|s| s.valid_until > Instant::now()) {
            return Ok(s.access.clone());
        }
        let jwt = self.ncentral_jwt(host).await?.ok_or_else(|| RestError::new(None, format!("No JWT is saved for {host}. Add one in Settings.")))?;
        let response = self.rest_send(RestRequest { method: Method::Post, url: format!("https://{host}/api/auth/authenticate"), bearer: Some(jwt), body: None }).await?;
        if response.status >= 300 {
            return Err(RestError::new(Some(response.status), format!("Could not sign in to {host}: {}", describe_error(response.status, &response.text()))));
        }
        let body: Value = serde_json::from_str(&response.text()).map_err(|_| RestError::new(Some(response.status), format!("{host} answered the sign-in with something other than JSON. Is that an N-central server?")))?;
        let access = body.pointer("/tokens/access/token").and_then(Value::as_str).ok_or_else(|| RestError::new(Some(response.status), format!("{host} signed us in but returned no access token.")))?;
        let lifetime = Duration::from_secs(body.pointer("/tokens/access/expirySeconds").and_then(Value::as_u64).unwrap_or(3600));
        *session = Some(Session { access: access.to_string(), valid_until: Instant::now() + lifetime.saturating_sub(SESSION_MARGIN) });
        Ok(access.to_string())
    }

    /// One call to a configured server. Returns the parsed JSON body (`null` when there is none).
    pub async fn ncentral_call(&self, server_id: &str, call: Call<'_>) -> Result<Value, RestError> {
        let server = self.ncentral_server(server_id)?;
        let host = server.host;
        let is_write = call.method != Method::Get;
        let operation = format!("{} {}", call.method.as_str(), call.path);

        let mut url = reqwest::Url::parse(&format!("https://{host}{}", call.path)).map_err(|e| RestError::new(None, format!("Bad request path: {e}")))?;
        if !call.query.is_empty() {
            url.query_pairs_mut().extend_pairs(call.query.iter().map(|(k, v)| (*k, v.as_str())));
        }

        let (action, summary) = match &call.audit {
            Some(meta) => (meta.action.clone(), meta.summary.clone()),
            None => ("ncentral.write".to_string(), format!("{operation} on {host}")),
        };
        let redacted_body = call.body.as_ref().map(redact_body).unwrap_or(Value::Null);

        if is_write && self.is_dry_run() {
            self.push_dry_run(action, summary, Some(operation), json!({ "host": host, "body": redacted_body }));
            return Err(RestError::new(None, DRY_RUN_ERROR));
        }

        let state = self.rest.host(&host);
        let result = {
            let _permit = state.permit(call.method, call.path).await;
            self.rest_call_with_session(&host, &state, call.method, url.as_str(), call.body.clone()).await
        };

        if is_write {
            let outcome = result.as_ref().map(|_| ()).map_err(|e| e.message.clone());
            self.record(&AuditEntry::new(&action, &summary, outcome, json!({ "host": host, "operation": operation, "body": redacted_body })));
        }
        result
    }

    async fn rest_call_with_session(&self, host: &str, state: &HostState, method: Method, url: &str, body: Option<Value>) -> Result<Value, RestError> {
        let mut renewed = false;
        let response = loop {
            let access = self.rest_access_token(host, state).await?;
            let response = self.rest_send(RestRequest { method, url: url.to_string(), bearer: Some(access), body: body.clone() }).await?;
            // A 401 on a session that was fine a moment ago means the server forgot it (a restart).
            if response.status == 401 && !renewed {
                *state.session.lock().await = None;
                renewed = true;
                continue;
            }
            break response;
        };
        if response.status >= 300 {
            let message = describe_error(response.status, &response.text());
            return Err(if response.status >= 500 { RestError::unknown_outcome(Some(response.status), message) } else { RestError::new(Some(response.status), message) });
        }
        if response.text().trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&response.text()).map_err(|_| RestError::unknown_outcome(Some(response.status), format!("{host} returned something other than JSON for {}.", url.split('?').next().unwrap_or(url))))
    }

    /// Every item of a paged list, 1000 at a time. Lists come as `{ data: [...], totalPages, ... }`.
    pub async fn ncentral_get_all<T: DeserializeOwned>(&self, server_id: &str, path: &str, extra: &[(&str, String)]) -> Result<Vec<T>, RestError> {
        let mut items = Vec::new();
        for page in 1..=MAX_PAGES {
            let mut call = Call::get(path).with_query("pageNumber", page).with_query("pageSize", PAGE_SIZE);
            call.query.extend(extra.iter().map(|(k, v)| (*k, v.clone())));
            let body = self.ncentral_call(server_id, call).await?;
            let data = body.get("data").and_then(Value::as_array).ok_or_else(|| RestError::new(None, format!("Unexpected response from {path}: no list of items.")))?;
            for item in data {
                items.push(serde_json::from_value(item.clone()).map_err(|e| RestError::new(None, format!("Unexpected response from {path}: {e}")))?);
            }
            let total_pages = body.get("totalPages").and_then(Value::as_u64);
            let last = data.is_empty() || total_pages.map_or(data.len() < PAGE_SIZE, |t| page as u64 >= t);
            if last {
                return Ok(items);
            }
        }
        Err(RestError::new(None, format!("{path} has more than {MAX_PAGES} pages; stopping.")))
    }

    async fn ncentral_get_data<T: DeserializeOwned>(&self, server_id: &str, path: &str) -> Result<T, RestError> {
        let body = self.ncentral_call(server_id, Call::get(path)).await?;
        let data = body.get("data").unwrap_or(&body);
        serde_json::from_value(data.clone()).map_err(|e| RestError::new(None, format!("Unexpected response from {path}: {e}")))
    }

    // ---- reads ----------------------------------------------------------------

    /// Signs in and reads the server's version: the Connect step's first check.
    pub async fn ncentral_server_info(&self, server_id: &str) -> Result<ServerInfo, RestError> {
        let body = self.ncentral_call(server_id, Call::get("/api/server-info")).await?;
        serde_json::from_value(body.get("data").filter(|d| d.is_object()).unwrap_or(&body).clone()).map_err(|e| RestError::new(None, format!("Unexpected response from /api/server-info: {e}")))
    }

    /// Every service organization, customer and site on the server, flat; `parent_id` links them into a tree.
    pub async fn ncentral_org_units(&self, server_id: &str) -> Result<Vec<OrgUnit>, RestError> {
        self.ncentral_get_all(server_id, "/api/org-units", &[]).await
    }

    /// The devices in an org unit and everything under it.
    pub async fn ncentral_org_unit_devices(&self, server_id: &str, org_unit_id: &str) -> Result<Vec<Device>, RestError> {
        let id = check_id(org_unit_id)?;
        self.ncentral_get_all(server_id, &format!("/api/org-units/{id}/devices"), &[]).await
    }

    /// One device, with zoned check-in times (the org-unit listing's have no time zone).
    pub async fn ncentral_device(&self, server_id: &str, device_id: &str) -> Result<Device, RestError> {
        self.ncentral_get_data(server_id, &format!("/api/devices/{}", check_id(device_id)?)).await
    }

    /// Serial, host name, MACs and SMBIOS UUID, for matching a device across servers.
    pub async fn ncentral_device_identity(&self, server_id: &str, device_id: &str) -> Result<DeviceIdentity, RestError> {
        let path = format!("/api/devices/{}/assets", check_id(device_id)?);
        let body = self.ncentral_call(server_id, Call::get(&path)).await?;
        Ok(DeviceIdentity::from_assets(&body))
    }

    /// A customer's or site's registration token. A service organization answers 403.
    pub async fn ncentral_registration_token(&self, server_id: &str, org_unit_id: &str) -> Result<RegistrationToken, RestError> {
        self.ncentral_get_data(server_id, &format!("/api/org-units/{}/registration-token", check_id(org_unit_id)?)).await
    }

    /// Per-device results of a direct task. Empty when the task hasn't reported yet, or when its history was
    /// deleted along with the repository item (the two look the same).
    pub async fn ncentral_task_details(&self, server_id: &str, task_id: &str) -> Result<Vec<TaskDetail>, RestError> {
        let path = format!("/api/scheduled-tasks/{}/status/details", check_id(task_id)?);
        let body = self.ncentral_call(server_id, Call::get(&path)).await?;
        let data = body.get("data").cloned().unwrap_or(Value::Null);
        let items = match data {
            Value::Array(items) => items,
            Value::Null => vec![],
            single => vec![single],
        };
        items.into_iter().map(|i| serde_json::from_value(i).map_err(|e| RestError::new(None, format!("Unexpected response from {path}: {e}")))).collect()
    }

    /// Downloads and reads a direct task's output zip. N-central serves it without authentication, so no
    /// credential is sent, and only the path N-central documents for these files on the configured host is fetched.
    pub async fn ncentral_task_output(&self, server_id: &str, output_file_name: &str) -> Result<RestOutput, RestError> {
        const PREFIX: &str = "/download/scheduled_tasks/output_files/";
        let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | ' ');
        if !output_file_name.starts_with(PREFIX) || output_file_name.contains("..") || !output_file_name.chars().all(plain) {
            return Err(RestError::new(None, "The task's output file isn't where N-central keeps them."));
        }
        let host = self.ncentral_server(server_id)?.host;
        let mut url = reqwest::Url::parse(&format!("https://{host}/")).map_err(|e| RestError::new(None, e.to_string()))?;
        url.set_path(output_file_name);
        if !url.path().starts_with(PREFIX) {
            return Err(RestError::new(None, "The task's output file isn't where N-central keeps them."));
        }
        let response = self.rest_send(RestRequest { method: Method::Get, url: url.to_string(), bearer: None, body: None }).await?;
        if response.status >= 300 {
            return Err(RestError::new(Some(response.status), describe_error(response.status, "")));
        }
        RestOutput::from_zip(response.body_bytes()).map_err(|m| RestError::new(Some(response.status), m))
    }

    // ---- writes ---------------------------------------------------------------

    /// Runs the uploaded endpoint script on one device as Local System. The inputs travel as one
    /// `CommandLine` parameter, which N-central shows in full in the task's details, token included.
    /// Returns the direct task's ID. Never retried on a gateway error: the task may have been created.
    pub async fn ncentral_run_direct_task(&self, server_id: &str, task: &DirectTask<'_>) -> Result<String, RestError> {
        let host = self.ncentral_server(server_id)?.host;
        let numeric = |what: &str, v: &str| v.parse::<u64>().map_err(|_| RestError::new(None, format!("The {what} \"{v}\" isn't numeric.")));
        let body = json!({
            "name": task.name,
            "itemId": numeric("script item ID", task.item_id)?,
            "taskType": "Script",
            "customerId": numeric("customer ID", task.customer_id)?,
            "deviceId": numeric("device ID", task.device_id)?,
            "credential": { "type": "LocalSystem" },
            "parameters": [{ "name": "CommandLine", "value": task.command_line, "type": "password", "description": "N-voke migration inputs" }],
        });
        let meta = AuditMeta { action: "migration.launch.rest".into(), summary: format!("{} on {} via {host}", task.name, task.device_label) };
        let result = self.ncentral_call(server_id, Call::write(Method::Post, "/api/scheduled-tasks/direct", Some(body), meta)).await;
        let body = match result {
            Err(e) if e.status == Some(500) && e.message.contains("REST API accessible flag") => {
                return Err(RestError::new(Some(500), format!("The endpoint script on {host} is missing, or Enable API is off for it. Upload it again in Prepare.")));
            }
            other => other?,
        };
        body.pointer("/data/taskId")
            .and_then(|v| v.as_str().map(String::from).or_else(|| v.as_u64().map(|n| n.to_string())))
            .ok_or_else(|| RestError::unknown_outcome(None, format!("{host} accepted the task but returned no task ID.")))
    }

    /// Creates a customer under a service organization and returns its ID. A name that is already taken is a 409.
    pub async fn ncentral_create_customer(&self, server_id: &str, service_org_id: &str, name: &str) -> Result<String, RestError> {
        let so = check_id(service_org_id)?;
        let host = self.ncentral_server(server_id)?.host;
        let meta = AuditMeta { action: "migration.structure.customer".into(), summary: format!("Created the customer {name} on {host}") };
        let path = format!("/api/service-orgs/{so}/customers");
        let body = self.ncentral_call(server_id, Call::write(Method::Post, &path, Some(json!({ "customerName": name })), meta)).await?;
        created_id(&body, "customerId", &host)
    }

    /// Creates a site under a customer and returns its ID. The site endpoint is marked as a preview in N-central's spec.
    pub async fn ncentral_create_site(&self, server_id: &str, customer_id: &str, name: &str) -> Result<String, RestError> {
        let customer = check_id(customer_id)?;
        let host = self.ncentral_server(server_id)?.host;
        let meta = AuditMeta { action: "migration.structure.site".into(), summary: format!("Created the site {name} on {host}") };
        let path = format!("/api/customers/{customer}/sites");
        let body = self.ncentral_call(server_id, Call::write(Method::Post, &path, Some(json!({ "siteName": name })), meta)).await?;
        created_id(&body, "siteId", &host)
    }

    /// Removes a device's record from the server, leaving the device itself alone (`removeAgents=false`: nothing is sent to
    /// it). The record then reads 404. Returns whether there was a record to remove.
    pub async fn ncentral_delete_device_record(&self, server_id: &str, device_id: &str, label: &str) -> Result<bool, RestError> {
        let id = check_id(device_id)?;
        let host = self.ncentral_server(server_id)?.host;
        let meta = AuditMeta { action: "migration.retire".into(), summary: format!("Removed the old record of {label} from {host}") };
        let path = format!("/api/devices/{id}");
        match self.ncentral_call(server_id, Call::write(Method::Delete, &path, None, meta).with_query("removeAgents", false)).await {
            Ok(_) => Ok(true),
            Err(e) if e.status == Some(404) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Creates a registration token for a customer or site that has none, and returns it.
    /// A valid token is left unchanged (confirmed in the lab), so this is safe to call
    /// for one that may exist; an expired one's behaviour is not yet known. The token
    /// itself is never written to the audit log.
    pub async fn ncentral_generate_registration_token(&self, server_id: &str, org_unit_id: &str, label: &str) -> Result<RegistrationToken, RestError> {
        let id = check_id(org_unit_id)?;
        let host = self.ncentral_server(server_id)?.host;
        let installers = self
            .ncentral_call(server_id, Call::get(&format!("/api/customers/{id}/software/installers")).with_query("softwareType", "Agent").with_query("installerType", "Generic"))
            .await?;
        let software_id = windows_agent_software_id(&installers);
        let meta = AuditMeta { action: "ncentral.registrationToken.generate".into(), summary: format!("Generated a registration token for {label} on {host}") };
        let path = format!("/api/customers/{id}/software/installers");
        let call = Call::write(Method::Post, &path, Some(json!({ "softwareId": software_id })), meta);
        self.ncentral_call(server_id, call).await?;
        self.ncentral_registration_token(server_id, id).await
    }
}

/// The ID a create call answered with, in the body or under `data`. A 2xx without one is an unknown outcome: it may exist.
fn created_id(body: &Value, key: &str, host: &str) -> Result<String, RestError> {
    let found = body.get(key).or_else(|| body.pointer(&format!("/data/{key}")));
    found
        .and_then(|v| v.as_str().map(String::from).or_else(|| v.as_u64().map(|n| n.to_string())))
        .ok_or_else(|| RestError::unknown_outcome(None, format!("{host} accepted the request but returned no {key}. Check whether it was created before trying again.")))
}

/// The Windows Agent's `softwareId` from the installer list, or the known default.
fn windows_agent_software_id(installers: &Value) -> String {
    let data = installers.get("data").unwrap_or(installers);
    data.as_array()
        .into_iter()
        .flatten()
        .find(|i| i.get("operatingSystem").and_then(Value::as_str).is_some_and(|o| o.eq_ignore_ascii_case("winnt")))
        .and_then(|i| match i.get("softwareId")? {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .unwrap_or_else(|| WINDOWS_AGENT_SOFTWARE_ID.to_string())
}

/// Test doubles for code built on the REST client.
#[cfg(test)]
pub(crate) mod testing {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    pub type Handler = dyn Fn(&RestRequest, usize) -> RestResponse + Send + Sync;

    /// Answers requests from a function and records them, so tests see exactly what was sent.
    pub struct Fake {
        pub handler: Box<Handler>,
        pub seen: Mutex<Vec<RestRequest>>,
        pub calls: AtomicUsize,
    }

    impl Fake {
        pub fn new(handler: impl Fn(&RestRequest, usize) -> RestResponse + Send + Sync + 'static) -> Arc<Self> {
            Arc::new(Self { handler: Box::new(handler), seen: Mutex::new(vec![]), calls: AtomicUsize::new(0) })
        }

        pub fn seen(&self) -> Vec<RestRequest> {
            self.seen.lock().unwrap().clone()
        }

        pub fn paths(&self) -> Vec<String> {
            self.seen().iter().map(|r| format!("{} {}", r.method.as_str(), r.url.trim_start_matches("https://").split_once('/').map_or("", |(_, p)| p))).collect()
        }
    }

    impl RestTransport for Fake {
        fn send<'a>(&'a self, request: RestRequest) -> BoxFuture<'a, Result<RestResponse, String>> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            let response = (self.handler)(&request, n);
            self.seen.lock().unwrap().push(request);
            Box::pin(async move { Ok(response) })
        }
    }

    pub fn ok(body: Value) -> RestResponse {
        RestResponse { status: 200, body: body.to_string().into_bytes(), retry_after: None }
    }

    pub fn status(status: u16, body: &str) -> RestResponse {
        RestResponse { status, body: body.as_bytes().to_vec(), retry_after: None }
    }

    pub fn auth_ok(expiry: u64) -> RestResponse {
        ok(json!({ "tokens": { "access": { "token": "access-1", "expirySeconds": expiry }, "refresh": { "token": "refresh-1", "expirySeconds": 90000 } } }))
    }
}

#[cfg(test)]
mod tests {
    use super::{testing::*, *};

    /// A core with one server, "src", already holding a JWT, talking to `fake`.
    async fn core_with(fake: &Arc<Fake>) -> (Arc<Core>, String) {
        let core = Core::for_tests("rest");
        let server = core.save_ncentral_server(None, "Source", "src.example.com", Some("the-jwt".into()), None).await.unwrap();
        core.set_rest_transport(fake.clone());
        (core, server.id)
    }

    #[test]
    fn heavy_endpoints_get_small_limits() {
        assert_eq!(limit_for(Method::Post, "/api/scheduled-tasks/direct"), ("direct-task", 30));
        assert_eq!(limit_for(Method::Post, "/api/device"), ("create-device", 1));
        assert_eq!(limit_for(Method::Get, "/api/devices/5/activation-key"), ("activation-key", 1));
        assert_eq!(limit_for(Method::Get, "/api/devices"), ("device-list", 5));
        assert_eq!(limit_for(Method::Get, "/api/org-units/50/devices"), ("device-list", 5));
        assert_eq!(limit_for(Method::Get, "/api/devices/9001"), ("default", 50));
    }

    #[test]
    fn ids_in_paths_are_plain_identifiers() {
        assert!(check_id("1234").is_ok() && check_id("abc-1_2").is_ok());
        for bad in ["", "1/../2", "1?x=y", "a b", "1%2f2"] {
            assert!(check_id(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_launch_command_line_never_shows_its_token() {
        let body = json!({ "name": "Migrate", "parameters": [
            { "name": "CommandLine", "type": "string", "value": "-Action Launch -DestServer new.example.com -RegistrationToken ab12-secret -MigrationId m1" },
            { "name": "Other", "type": "password", "value": "hunter2" }
        ] });
        let text = redact_body(&body).to_string();
        assert!(!text.contains("ab12-secret") && !text.contains("hunter2"), "{text}");
        assert!(text.contains("-Action Launch -DestServer new.example.com -RegistrationToken [redacted] -MigrationId m1"), "{text}");
    }

    #[test]
    fn errors_use_the_servers_message_and_say_what_to_do() {
        let text = describe_error(403, r#"{"message":"Customer not accessible","exceptionName":"RegistrationTokenNotAccessibleException"}"#);
        assert!(text.starts_with("HTTP 403: RegistrationTokenNotAccessibleException: Customer not accessible"), "{text}");
        assert!(describe_error(401, "").contains("Replace it in Settings"));
        assert!(crate::is_rate_limited(&describe_error(429, "")));
        assert!(describe_error(500, "<html>oops</html>").contains("<html>oops</html>"));
    }

    #[tokio::test]
    async fn it_signs_in_once_and_reuses_the_session() {
        let fake = Fake::new(|req, _| if req.url.ends_with("/api/auth/authenticate") { auth_ok(3600) } else { ok(json!({ "ncentral": "2026.4.0.25", "version": "1.3.23" })) });
        let (core, id) = core_with(&fake).await;
        let info = core.ncentral_server_info(&id).await.unwrap();
        core.ncentral_server_info(&id).await.unwrap();
        assert_eq!(info.release(), Some((2026, 4)));
        assert_eq!(fake.paths(), ["POST api/auth/authenticate", "GET api/server-info", "GET api/server-info"]);
        let seen = fake.seen();
        assert_eq!(seen[0].bearer.as_deref(), Some("the-jwt"), "sign-in uses the JWT");
        assert_eq!(seen[1].bearer.as_deref(), Some("access-1"), "everything else uses the access token");
    }

    #[tokio::test(start_paused = true)]
    async fn a_session_about_to_expire_is_renewed() {
        let fake = Fake::new(|req, _| if req.url.ends_with("/authenticate") { auth_ok(3600) } else { ok(json!({ "ncentral": "x", "version": "1" })) });
        let (core, id) = core_with(&fake).await;
        core.ncentral_server_info(&id).await.unwrap();
        tokio::time::advance(Duration::from_secs(3600 - 61)).await;
        core.ncentral_server_info(&id).await.unwrap();
        assert_eq!(fake.paths().iter().filter(|p| p.contains("authenticate")).count(), 1);
        tokio::time::advance(Duration::from_secs(2)).await;
        core.ncentral_server_info(&id).await.unwrap();
        assert_eq!(fake.paths().iter().filter(|p| p.contains("authenticate")).count(), 2, "renewed inside the last minute");
    }

    #[tokio::test]
    async fn a_401_after_a_server_restart_signs_in_again_and_retries() {
        let fake = Fake::new(|req, n| match (req.url.ends_with("/authenticate"), n) {
            (true, _) => auth_ok(3600),
            (false, 1) => status(401, ""),
            _ => ok(json!({ "ncentral": "x", "version": "1" })),
        });
        let (core, id) = core_with(&fake).await;
        core.ncentral_server_info(&id).await.unwrap();
        assert_eq!(fake.paths(), ["POST api/auth/authenticate", "GET api/server-info", "POST api/auth/authenticate", "GET api/server-info"]);
    }

    #[tokio::test]
    async fn a_refused_jwt_says_so_and_a_missing_one_says_where_to_add_it() {
        let fake = Fake::new(|_, _| status(401, r#"{"message":"Invalid token"}"#));
        let (core, id) = core_with(&fake).await;
        let error = core.ncentral_server_info(&id).await.unwrap_err();
        assert!(error.message.contains("Could not sign in to src.example.com") && error.message.contains("Replace it in Settings"), "{error}");
        assert_eq!(error.status, Some(401));

        let other = core.save_ncentral_server(None, "No key", "nokey.example.com", None, None).await.unwrap();
        let error = core.ncentral_server_info(&other.id).await.unwrap_err();
        assert_eq!(error.message, "No JWT is saved for nokey.example.com. Add one in Settings.");
    }

    #[tokio::test]
    async fn lists_are_fetched_page_by_page_until_the_last() {
        let fake = Fake::new(|req, _| {
            if req.url.ends_with("/authenticate") {
                return auth_ok(3600);
            }
            let page2 = req.url.contains("pageNumber=2");
            let unit = |id: &str| json!({ "orgUnitId": id, "orgUnitName": format!("Unit {id}"), "orgUnitType": "CUSTOMER", "parentId": "1" });
            ok(json!({ "data": if page2 { vec![unit("3")] } else { vec![unit("1"), unit("2")] }, "totalPages": 2, "pageNumber": if page2 { 2 } else { 1 } }))
        });
        let (core, id) = core_with(&fake).await;
        let units = core.ncentral_org_units(&id).await.unwrap();
        assert_eq!(units.iter().map(|u| u.org_unit_id.as_str()).collect::<Vec<_>>(), ["1", "2", "3"]);
        let paths = fake.paths();
        assert!(paths[1].contains("api/org-units?pageNumber=1&pageSize=1000"), "{paths:?}");
        assert!(paths[2].contains("pageNumber=2"), "{paths:?}");
        assert_eq!(paths.len(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn a_rate_limited_call_waits_as_told_and_is_retried_even_for_a_write() {
        let fake = Fake::new(|req, n| match (req.url.ends_with("/authenticate"), n) {
            (true, _) => auth_ok(3600),
            (false, 1) => RestResponse { status: 429, body: vec![], retry_after: Some("3".into()) },
            _ => ok(json!({})),
        });
        let (core, id) = core_with(&fake).await;
        let started = Instant::now();
        let meta = AuditMeta { action: "test.write".into(), summary: "A write".into() };
        core.ncentral_call(&id, Call::write(Method::Put, "/api/x", Some(json!({})), meta)).await.unwrap();
        assert_eq!(started.elapsed(), Duration::from_secs(3));
        assert_eq!(fake.seen().len(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn a_gateway_error_is_retried_for_reads_but_never_for_writes() {
        let reads = Fake::new(|req, n| if req.url.ends_with("/authenticate") { auth_ok(3600) } else if n < 3 { status(503, "") } else { ok(json!({ "data": { "deviceId": 1 } })) });
        let (core, id) = core_with(&reads).await;
        assert_eq!(core.ncentral_device(&id, "1").await.unwrap().device_id, "1");

        let writes = Fake::new(|req, _| if req.url.ends_with("/authenticate") { auth_ok(3600) } else { status(503, "") });
        let (core, id) = core_with(&writes).await;
        let meta = AuditMeta { action: "test.write".into(), summary: "A write".into() };
        let error = core.ncentral_call(&id, Call::write(Method::Post, "/api/x", None, meta)).await.unwrap_err();
        assert_eq!(error.status, Some(503));
        assert_eq!(writes.seen().len(), 2, "one sign-in, one attempt: a write may have gone through");
    }

    #[tokio::test]
    async fn dry_run_sends_nothing_for_a_write_and_lists_it_redacted() {
        let fake = Fake::new(|_, _| ok(json!({})));
        let (core, id) = core_with(&fake).await;
        core.set_dry_run(true);
        let body = json!({ "parameters": [{ "name": "CommandLine", "type": "password", "value": "-RegistrationToken ab12-secret" }] });
        let meta = AuditMeta { action: "migration.launch".into(), summary: "Launched DESKTOP-42".into() };
        let error = core.ncentral_call(&id, Call::write(Method::Post, "/api/scheduled-tasks/direct", Some(body), meta)).await.unwrap_err();
        assert!(error.is_dry_run());
        assert!(fake.seen().is_empty(), "not even a sign-in");
        let log = core.dry_run_log();
        assert_eq!((log[0].action.as_str(), log[0].summary.as_str(), log[0].operation.as_deref()), ("migration.launch", "Launched DESKTOP-42", Some("POST /api/scheduled-tasks/direct")));
        assert!(!log[0].variables.to_string().contains("ab12-secret"));
        assert!(core.audit_log(None).unwrap().entries.iter().all(|e| e.action != "migration.launch"), "a held-back change isn't a change");
    }

    #[tokio::test]
    async fn reads_still_work_in_dry_run() {
        let fake = Fake::new(|req, _| if req.url.ends_with("/authenticate") { auth_ok(3600) } else { ok(json!({ "ncentral": "2026.4.1.11", "version": "1.3.23" })) });
        let (core, id) = core_with(&fake).await;
        core.set_dry_run(true);
        assert_eq!(core.ncentral_server_info(&id).await.unwrap().ncentral, "2026.4.1.11");
    }

    #[tokio::test]
    async fn generating_a_token_posts_the_windows_agent_audits_without_the_token_and_returns_it() {
        let fake = Fake::new(|req, _| {
            let path = req.url.trim_start_matches("https://src.example.com");
            match (req.method, path) {
                (_, "/api/auth/authenticate") => auth_ok(3600),
                (Method::Get, p) if p.starts_with("/api/customers/1234/software/installers") => ok(json!({ "data": [
                    { "softwareId": "202", "operatingSystem": "macos" }, { "softwareId": 101, "operatingSystem": "winnt" }] })),
                (Method::Post, "/api/customers/1234/software/installers") => status(201, r#"{"downloadLink":"https://x/a.exe","expirationDate":"2026-10-23"}"#),
                (Method::Get, "/api/org-units/1234/registration-token") => ok(json!({ "data": { "registrationToken": "ab12-secret-token", "registrationTokenExpiryDate": "2026-11-17T23:59:00-05:00[America/New_York]" } })),
                other => panic!("unexpected request {other:?}"),
            }
        });
        let (core, id) = core_with(&fake).await;
        let token = core.ncentral_generate_registration_token(&id, "1234", "Acme (Destination)").await.unwrap();
        assert_eq!(token.registration_token.as_deref(), Some("ab12-secret-token"));
        assert_eq!(token.expires_at().unwrap().to_rfc3339(), "2026-11-17T23:59:00-05:00");

        let post = fake.seen().into_iter().find(|r| r.method == Method::Post && r.url.ends_with("/software/installers")).unwrap();
        assert_eq!(post.body, Some(json!({ "softwareId": "101" })));
        let log = core.audit_log(None).unwrap().entries;
        let entry = log.iter().find(|e| e.action == "ncentral.registrationToken.generate").expect("audited");
        assert_eq!(entry.summary, "Generated a registration token for Acme (Destination) on src.example.com");
        assert!(!serde_json::to_string(&log).unwrap().contains("ab12-secret-token"));
    }

    #[tokio::test]
    async fn a_service_org_has_no_token_and_says_so_with_its_status() {
        let fake = Fake::new(|req, _| if req.url.ends_with("/authenticate") { auth_ok(3600) } else { status(403, r#"{"exceptionName":"RegistrationTokenNotAccessibleException","message":"Not a customer"}"#) });
        let (core, id) = core_with(&fake).await;
        let error = core.ncentral_registration_token(&id, "50").await.unwrap_err();
        assert_eq!(error.status, Some(403));
        assert!(error.message.contains("RegistrationTokenNotAccessibleException"));
    }

    #[tokio::test]
    async fn a_token_that_isnt_set_comes_back_as_null_fields_not_an_error() {
        let fake = Fake::new(|req, _| if req.url.ends_with("/authenticate") { auth_ok(3600) } else { ok(json!({ "data": { "registrationToken": null, "registrationTokenExpiryDate": null } })) });
        let (core, id) = core_with(&fake).await;
        assert_eq!(core.ncentral_registration_token(&id, "1234").await.unwrap().registration_token, None);
    }

    #[tokio::test]
    async fn device_identity_is_read_from_the_assets_document() {
        let fake = Fake::new(|req, _| {
            if req.url.ends_with("/authenticate") {
                return auth_ok(3600);
            }
            ok(json!({ "data": { "computersystem": { "serialnumber": "VMW-1", "netbiosname": "PC1" }, "networkadapter": { "list": [{ "macaddress": "AA:BB:CC:DD:EE:01" }] } } }))
        });
        let (core, id) = core_with(&fake).await;
        let identity = core.ncentral_device_identity(&id, "9001").await.unwrap();
        assert_eq!((identity.serial.as_deref(), identity.macs.as_slice()), (Some("VMW-1"), ["aabbccddee01".to_string()].as_slice()));
    }

    #[tokio::test]
    async fn only_one_login_happens_when_calls_run_together() {
        let fake = Fake::new(|req, _| if req.url.ends_with("/authenticate") { auth_ok(3600) } else { ok(json!({ "ncentral": "x", "version": "1" })) });
        let (core, id) = core_with(&fake).await;
        let (a, b, c) = tokio::join!(core.ncentral_server_info(&id), core.ncentral_server_info(&id), core.ncentral_server_info(&id));
        assert!(a.is_ok() && b.is_ok() && c.is_ok());
        assert_eq!(fake.paths().iter().filter(|p| p.contains("authenticate")).count(), 1);
    }

    fn direct<'a>(command_line: &'a str) -> DirectTask<'a> {
        DirectTask { name: "N-voke migration Launch", item_id: "77", customer_id: "1234", device_id: "9001", device_label: "DESKTOP-42", command_line }
    }

    #[tokio::test]
    async fn a_direct_task_sends_the_lab_proven_body_audits_without_the_token_and_returns_the_task_id() {
        let fake = Fake::new(|req, _| match req.url.trim_start_matches("https://src.example.com") {
            "/api/auth/authenticate" => auth_ok(3600),
            "/api/scheduled-tasks/direct" => status(201, r#"{"data":{"taskId":"5550"}}"#),
            other => panic!("unexpected {other}"),
        });
        let (core, id) = core_with(&fake).await;
        let line = "-Action Launch -RegistrationToken ab12-secret-token -MigrationId m1";
        assert_eq!(core.ncentral_run_direct_task(&id, &direct(line)).await.unwrap(), "5550");
        let sent = fake.seen().into_iter().find(|r| r.url.ends_with("/direct")).unwrap();
        let body = sent.body.unwrap();
        assert_eq!(body["itemId"], 77);
        assert_eq!((body["customerId"].as_u64(), body["deviceId"].as_u64(), body["taskType"].as_str()), (Some(1234), Some(9001), Some("Script")));
        assert_eq!(body["credential"], json!({ "type": "LocalSystem" }));
        assert_eq!(body["parameters"][0]["name"], "CommandLine");
        assert_eq!(body["parameters"][0]["value"], line, "the device gets the real value");
        let log = serde_json::to_string(&core.audit_log(None).unwrap().entries).unwrap();
        assert!(log.contains("migration.launch.rest") && log.contains("-RegistrationToken [redacted] -MigrationId m1"), "{log}");
        assert!(!log.contains("ab12-secret-token"));
    }

    #[tokio::test]
    async fn a_missing_script_item_is_reported_as_a_setup_problem_and_non_numeric_ids_are_refused() {
        let fake = Fake::new(|req, _| if req.url.ends_with("/authenticate") { auth_ok(3600) } else { status(500, r#"{"message":"TaskCreateException: Failed to verify REST API accessible flag for provided script. taskItemID=77"}"#) });
        let (core, id) = core_with(&fake).await;
        let error = core.ncentral_run_direct_task(&id, &direct("-Action Status")).await.unwrap_err();
        assert!(error.message.contains("missing, or Enable API is off") && error.message.contains("src.example.com"), "{error}");
        let mut bad = direct("-Action Status");
        bad.item_id = "7/../7";
        assert!(core.ncentral_run_direct_task(&id, &bad).await.unwrap_err().message.contains("isn't numeric"));
    }

    #[tokio::test]
    async fn a_direct_task_in_dry_run_is_listed_with_the_token_blanked_and_not_sent() {
        let fake = Fake::new(|_, _| ok(json!({})));
        let (core, id) = core_with(&fake).await;
        core.set_dry_run(true);
        assert!(core.ncentral_run_direct_task(&id, &direct("-Action Launch -RegistrationToken ab12-secret-token -X y")).await.unwrap_err().is_dry_run());
        assert!(fake.seen().is_empty());
        let shown = core.dry_run_log()[0].variables.to_string();
        assert!(shown.contains("-RegistrationToken [redacted] -X y") && !shown.contains("ab12-secret-token"), "{shown}");
    }

    #[tokio::test]
    async fn task_details_accept_a_list_one_item_or_nothing() {
        let bodies = [
            json!({ "data": [{ "status": "Success", "message": "ok", "taskId": 8, "outputFileName": "/download/scheduled_tasks/output_files/a.zip" }], "totalItems": 1 }),
            json!({ "data": { "status": "Pending", "message": "" } }),
            json!({ "data": [], "totalItems": 0 }),
        ];
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let bodies = Arc::new((bodies, calls));
        let shared = bodies.clone();
        let fake = Fake::new(move |req, _| {
            if req.url.ends_with("/authenticate") {
                return auth_ok(3600);
            }
            let n = shared.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            ok(shared.0[n].clone())
        });
        let (core, id) = core_with(&fake).await;
        let first = core.ncentral_task_details(&id, "5550").await.unwrap();
        assert_eq!((first[0].status.as_str(), first[0].task_id.as_deref()), ("Success", Some("8")));
        assert_eq!(core.ncentral_task_details(&id, "5550").await.unwrap()[0].status, "Pending");
        assert!(core.ncentral_task_details(&id, "5550").await.unwrap().is_empty());
        assert!(fake.paths().iter().any(|p| p == "GET api/scheduled-tasks/5550/status/details"));
        assert!(core.ncentral_task_details(&id, "5/../x").await.is_err());
    }

    #[tokio::test]
    async fn the_output_zip_is_fetched_without_a_credential_and_only_from_where_n_central_keeps_it() {
        let text = "Exit Code: 0\r\nNVOKE-RESULT {\"result\":\"staged\",\"errorCode\":0,\"taskCreated\":true}\r\n";
        let zip = crate::migration::endpoint_result::tests::zip_of("Output-Formatted.txt", text.as_bytes(), true);
        let fake = Fake::new(move |req, _| if req.url.ends_with("/authenticate") { auth_ok(3600) } else { RestResponse { status: 200, body: zip.clone(), retry_after: None } });
        let (core, id) = core_with(&fake).await;
        let out = core.ncentral_task_output(&id, "/download/scheduled_tasks/output_files/Acme/PC/5550/Run_PC_20261020.zip").await.unwrap();
        assert_eq!((out.exit_code, out.result.unwrap().result.as_str()), (Some(0), "staged"));
        let download = fake.seen().into_iter().find(|r| r.url.contains("/download/")).unwrap();
        assert_eq!(download.bearer, None, "nothing secret goes with an unauthenticated download");
        assert!(download.url.starts_with("https://src.example.com/download/scheduled_tasks/output_files/"));
        let before = fake.seen().len();
        for bad in ["/api/devices/1", "/download/scheduled_tasks/output_files/../../etc/passwd", "https://evil.example.com/download/scheduled_tasks/output_files/x.zip", "/download/scheduled_tasks/output_files/x.zip?a=b", "/download/scheduled_tasks/output_files/a\\b.zip", "/download/scheduled_tasks/output_files/%2e%2e/%2e%2e/api/x", "/download/scheduled_tasks/output_files/.%2e/x.zip", "/download/scheduled_tasks/output_files/a%2fb.zip"] {
            assert!(core.ncentral_task_output(&id, bad).await.is_err(), "{bad}");
        }
        assert_eq!(fake.seen().len(), before, "refused before any request");
    }
}
