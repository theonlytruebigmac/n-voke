//! The N-central REST API's data shapes, limited to the fields migration uses.
//! Shapes come from the Phase 0 lab runs (docs/migration/10-phase0-findings.md).
//! Fields the API sometimes leaves out are optional, and IDs are read from
//! either a JSON string or number, since the spec isn't consistent about it.

use chrono::{DateTime, Duration, FixedOffset, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

fn lenient_id<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    match Value::deserialize(d)? {
        Value::String(s) => Ok(s),
        Value::Number(n) => Ok(n.to_string()),
        other => Err(serde::de::Error::custom(format!("expected an id, got {other}"))),
    }
}

pub(crate) fn lenient_opt_id<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    match Value::deserialize(d)? {
        Value::Null => Ok(None),
        Value::String(s) if s.trim().is_empty() => Ok(None),
        Value::String(s) => Ok(Some(s)),
        Value::Number(n) => Ok(Some(n.to_string())),
        other => Err(serde::de::Error::custom(format!("expected an id, got {other}"))),
    }
}

/// `GET /api/server-info`: needs a bearer token.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "ServerInfo.ts"))]
pub struct ServerInfo {
    /// N-central's own version, for example `2026.4.0.25`.
    pub ncentral: String,
    /// The REST API's version, for example `1.3.23`.
    pub version: String,
}

impl ServerInfo {
    /// Year and release, so `2026.4.0.25` is `(2026, 4)`.
    pub fn release(&self) -> Option<(u32, u32)> {
        let mut parts = self.ncentral.split('.');
        Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrgUnitType {
    #[serde(rename = "SO")]
    ServiceOrg,
    #[serde(rename = "CUSTOMER")]
    Customer,
    #[serde(rename = "SITE")]
    Site,
}

/// A service organization, customer or site.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OrgUnit {
    #[serde(deserialize_with = "lenient_id")]
    pub org_unit_id: String,
    pub org_unit_name: String,
    pub org_unit_type: OrgUnitType,
    #[serde(default, deserialize_with = "lenient_opt_id")]
    pub parent_id: Option<String>,
    /// Optional and often blank: only ever a confirming signal when matching.
    #[serde(default, deserialize_with = "lenient_opt_id")]
    pub external_id: Option<String>,
}

/// A device from `GET /api/org-units/{id}/devices` or `GET /api/devices/{id}`.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    #[serde(deserialize_with = "lenient_id")]
    pub device_id: String,
    #[serde(default)]
    pub long_name: Option<String>,
    #[serde(default)]
    pub discovered_name: Option<String>,
    #[serde(default)]
    pub device_class: Option<String>,
    #[serde(default)]
    pub supported_os: Option<String>,
    /// `winnt` for Windows.
    #[serde(default)]
    pub os_id: Option<String>,
    #[serde(default, deserialize_with = "lenient_opt_id")]
    pub customer_id: Option<String>,
    #[serde(default, deserialize_with = "lenient_opt_id")]
    pub site_id: Option<String>,
    #[serde(default, deserialize_with = "lenient_opt_id")]
    pub org_unit_id: Option<String>,
    /// UTC with a `Z` from `/api/devices/{id}`, but server-local time with **no
    /// zone** from the org-unit listing: see [`Device::last_checkin`].
    #[serde(default)]
    pub last_appliance_checkin_time: Option<String>,
    /// Present on `/api/devices/{id}`, with an offset.
    #[serde(default)]
    pub last_appliance_checkin_time_local: Option<String>,
}

impl Device {
    pub fn name(&self) -> &str {
        self.long_name.as_deref().filter(|n| !n.is_empty()).or(self.discovered_name.as_deref()).unwrap_or("(unnamed)")
    }

    pub fn is_windows(&self) -> bool {
        self.os_id.as_deref().is_some_and(|o| o.eq_ignore_ascii_case("winnt"))
    }

    /// The last check-in as a point in time, or None when it can't be read
    /// without guessing. The org-unit listing's value has no time zone, so it is
    /// never parsed: use `/api/devices/{id}` for check-in decisions.
    pub fn last_checkin(&self) -> Option<DateTime<Utc>> {
        [self.last_appliance_checkin_time_local.as_deref(), self.last_appliance_checkin_time.as_deref()]
            .into_iter()
            .flatten()
            .find_map(|t| DateTime::parse_from_rfc3339(t).ok())
            .map(|t| t.with_timezone(&Utc))
    }
}

/// `GET /api/org-units/{id}/registration-token`: a customer's or site's token.
#[derive(Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationToken {
    #[serde(default)]
    pub registration_token: Option<String>,
    #[serde(default)]
    pub registration_token_expiry_date: Option<String>,
}

// Never prints the token, so it can't reach a log through `{:?}`.
impl std::fmt::Debug for RegistrationToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistrationToken")
            .field("registration_token", &self.registration_token.as_ref().map(|_| "[redacted]"))
            .field("registration_token_expiry_date", &self.registration_token_expiry_date)
            .finish()
    }
}

/// Whether a registration token can be used for a launch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenStatus {
    /// None set (the usual state of a new destination).
    Missing,
    Valid { expires: DateTime<FixedOffset> },
    /// Valid, but ends within the warning window.
    Expiring { expires: DateTime<FixedOffset> },
    Expired { expires: DateTime<FixedOffset> },
    /// A token is set but its expiry can't be read, so it isn't trusted.
    UnknownExpiry,
}

impl RegistrationToken {
    /// Expiry looks like `2026-11-17T23:59:00-05:00[America/New_York]`: ISO 8601
    /// with a Java zone suffix, which is dropped (the offset is what counts).
    pub fn expires_at(&self) -> Option<DateTime<FixedOffset>> {
        let text = self.registration_token_expiry_date.as_deref()?;
        let without_zone = text.split('[').next().unwrap_or(text).trim();
        DateTime::parse_from_rfc3339(without_zone).ok()
    }

    pub fn status(&self, now: DateTime<Utc>, warn_within: Duration) -> TokenStatus {
        if self.registration_token.as_deref().is_none_or(|t| t.is_empty()) {
            return TokenStatus::Missing;
        }
        match self.expires_at() {
            None => TokenStatus::UnknownExpiry,
            Some(expires) if expires.with_timezone(&Utc) <= now => TokenStatus::Expired { expires },
            Some(expires) if expires.with_timezone(&Utc) - now <= warn_within => TokenStatus::Expiring { expires },
            Some(expires) => TokenStatus::Valid { expires },
        }
    }
}

/// What identifies a device across servers, from `GET /api/devices/{id}/assets`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceIdentity {
    pub serial: Option<String>,
    pub hostname: Option<String>,
    /// Lowercase hex without separators; empty and all-zero addresses are dropped.
    pub macs: Vec<String>,
    pub smbios_uuid: Option<String>,
}

/// `AA-BB-CC-DD-EE-FF`, `aa:bb:...` and `aabb...` all become `aabbccddeeff`.
/// Empty, all-zero and malformed addresses give None.
pub fn normalize_mac(mac: &str) -> Option<String> {
    let hex: String = mac.chars().filter(|c| !matches!(c, ':' | '-' | '.')).collect::<String>().to_ascii_lowercase();
    let valid = hex.len() == 12 && hex.chars().all(|c| c.is_ascii_hexdigit()) && hex.chars().any(|c| c != '0');
    valid.then_some(hex)
}

impl DeviceIdentity {
    /// Reads the asset document. Keys are matched case-insensitively and the
    /// document may or may not be wrapped in `data`, since Phase 0 only recorded
    /// the dotted paths.
    pub fn from_assets(document: &Value) -> Self {
        let root = document.get("data").filter(|d| d.is_object()).unwrap_or(document);
        let text = |v: Option<&Value>| v.and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(String::from);
        let computer = key(root, "computersystem");
        let mut macs: Vec<String> = key(root, "networkadapter")
            .and_then(|n| key(n, "list"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|adapter| key(adapter, "macaddress").and_then(Value::as_str).and_then(normalize_mac))
            .collect();
        macs.sort();
        macs.dedup();
        DeviceIdentity {
            serial: text(computer.and_then(|c| key(c, "serialnumber"))),
            hostname: text(computer.and_then(|c| key(c, "netbiosname"))),
            macs,
            smbios_uuid: text(key(root, "_extra").and_then(|e| key(e, "computersystem")).and_then(|c| key(c, "uuid"))),
        }
    }
}

fn key<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    value.as_object()?.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v)
}

/// One device's result from `GET /api/scheduled-tasks/{id}/status/details`. Read this, never `/status`,
/// which reports failed runs as completed.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskDetail {
    /// `Pending` (the device is offline), `In Progress`, `Success` or `Failed`.
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub message: Option<String>,
    /// Only a preview of about 500 characters, and only stderr if the script wrote any: don't parse it.
    #[serde(default)]
    pub output: Option<String>,
    /// Where the full output zip is, relative to the server.
    #[serde(default)]
    pub output_file_name: Option<String>,
    /// The per-device task ID, which differs from the direct task's.
    #[serde(default, deserialize_with = "lenient_opt_id")]
    pub task_id: Option<String>,
    #[serde(default, deserialize_with = "lenient_opt_id")]
    pub device_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn org_units_accept_string_or_number_ids_and_blank_external_ids() {
        let unit: OrgUnit = serde_json::from_value(json!({
            "orgUnitId": "1234", "orgUnitName": "Acme", "orgUnitType": "CUSTOMER", "parentId": 50, "externalId": "", "descendantCount": 3
        }))
        .unwrap();
        assert_eq!((unit.org_unit_id.as_str(), unit.parent_id.as_deref(), unit.external_id), ("1234", Some("50"), None));
        assert_eq!(unit.org_unit_type, OrgUnitType::Customer);
        let so: OrgUnit = serde_json::from_value(json!({ "orgUnitId": 50, "orgUnitName": "Root", "orgUnitType": "SO", "parentId": null })).unwrap();
        assert_eq!((so.org_unit_id.as_str(), so.parent_id), ("50", None));
    }

    #[test]
    fn devices_expose_name_os_and_a_zoned_checkin_only() {
        let device: Device = serde_json::from_value(json!({
            "deviceId": 9001, "longName": "DESKTOP-42", "osId": "winnt", "customerId": 1234, "siteId": "77",
            "lastApplianceCheckinTime": "2026-10-19T20:37:02.100Z", "lastApplianceCheckinTimeLocal": "2026-10-19T16:37:02.100-04:00"
        }))
        .unwrap();
        assert_eq!((device.device_id.as_str(), device.name(), device.is_windows()), ("9001", "DESKTOP-42", true));
        assert_eq!(device.last_checkin().unwrap().to_rfc3339(), "2026-10-19T20:37:02.100+00:00");

        // The org-unit listing has no zone, so it must not be read as UTC.
        let listed: Device = serde_json::from_value(json!({ "deviceId": "9001", "discoveredName": "pc", "lastApplianceCheckinTime": "2026-10-19T20:37:02.1" })).unwrap();
        assert_eq!(listed.name(), "pc");
        assert!(listed.last_checkin().is_none());
    }

    #[test]
    fn a_token_is_missing_valid_expiring_or_expired() {
        let now = DateTime::parse_from_rfc3339("2026-11-12T12:00:00Z").unwrap().with_timezone(&Utc);
        let token = |expiry: Option<&str>| RegistrationToken { registration_token: Some("abc-123".into()), registration_token_expiry_date: expiry.map(String::from) };
        let week = Duration::days(7);

        assert_eq!(RegistrationToken { registration_token: None, registration_token_expiry_date: None }.status(now, week), TokenStatus::Missing);
        assert_eq!(RegistrationToken { registration_token: Some(String::new()), registration_token_expiry_date: None }.status(now, week), TokenStatus::Missing);
        assert!(matches!(token(Some("2026-12-17T23:59:00-05:00[America/New_York]")).status(now, week), TokenStatus::Valid { .. }));
        assert!(matches!(token(Some("2026-11-17T23:59:00-05:00[America/New_York]")).status(now, week), TokenStatus::Expiring { .. }));
        assert!(matches!(token(Some("2026-11-01T23:59:00-05:00")).status(now, week), TokenStatus::Expired { .. }));
        assert_eq!(token(None).status(now, week), TokenStatus::UnknownExpiry);
        assert_eq!(token(Some("soon")).status(now, week), TokenStatus::UnknownExpiry);
    }

    #[test]
    fn a_token_never_appears_in_debug_output() {
        let token = RegistrationToken { registration_token: Some("super-secret-token".into()), registration_token_expiry_date: None };
        assert!(!format!("{token:?}").contains("super-secret"));
    }

    #[test]
    fn macs_are_normalized_and_junk_is_dropped() {
        assert_eq!(normalize_mac("AA-BB-CC-DD-EE-01").as_deref(), Some("aabbccddee01"));
        assert_eq!(normalize_mac("aa:bb:cc:dd:ee:01").as_deref(), Some("aabbccddee01"));
        assert_eq!(normalize_mac(""), None);
        assert_eq!(normalize_mac("00:00:00:00:00:00"), None);
        assert_eq!(normalize_mac("not a mac"), None);
    }

    #[test]
    fn identity_is_read_from_the_asset_document_wrapped_or_not() {
        let body = json!({
            "computersystem": { "serialnumber": " VMW-123 ", "netbiosname": "DESKTOP-42" },
            "networkadapter": { "list": [ { "macaddress": "AA-BB-CC-DD-EE-01" }, { "macaddress": "" }, { "macaddress": "aa:bb:cc:dd:ee:01" } ] },
            "_extra": { "computersystem": { "uuid": "4C4C4544" } }
        });
        let expected = DeviceIdentity { serial: Some("VMW-123".into()), hostname: Some("DESKTOP-42".into()), macs: vec!["aabbccddee01".into()], smbios_uuid: Some("4C4C4544".into()) };
        assert_eq!(DeviceIdentity::from_assets(&body), expected);
        assert_eq!(DeviceIdentity::from_assets(&json!({ "data": body })), expected);
        assert_eq!(DeviceIdentity::from_assets(&json!({})), DeviceIdentity::default());
    }

    #[test]
    fn the_release_is_read_from_the_version() {
        assert_eq!(ServerInfo { ncentral: "2026.4.0.25".into(), version: "1.3.23".into() }.release(), Some((2026, 4)));
        assert_eq!(ServerInfo { ncentral: "unknown".into(), version: "1".into() }.release(), None);
    }
}
