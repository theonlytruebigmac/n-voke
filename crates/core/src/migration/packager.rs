//! The endpoint script as N-voke ships it: turning the built `nvoke-migration.ps1`
//! into an Ecoverse script, deciding whether to create, replace or leave it, and
//! building the exact inputs of every launch (Ecoverse's typed inputs, and the REST
//! path's `CommandLine`). The rules come from docs/migration/05-endpoint-scripts.md
//! and are the same ones the script checks, so a bad value is refused here before
//! anything is sent to a device.

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::ecoverse::{ScriptDefinition, ScriptOutput};
use crate::{
    Core,
    api::RunInput,
    inputs::InputType,
    model::{InputValue, Script},
};

/// Every published script is named with this prefix, so N-voke can find its own.
pub const SCRIPT_NAME_PREFIX: &str = "N-voke migration ";
/// A launch can take minutes just to start PowerShell on a starved machine.
pub const SCRIPT_TIMEOUT_SECONDS: u32 = 900;

/// Every input the script reads, in the order they are declared. Ecoverse refuses a run that
/// omits any of them, so each launch sends all of them.
pub const INPUT_NAMES: [&str; 11] = [
    "Action",
    "DestServer",
    "DestCustomerId",
    "RegistrationToken",
    "MigrationId",
    "LauncherVersion",
    "TaskRepeatMinutes",
    "TaskExpiryDays",
    "CleanMode",
    "ExpectUfa",
    "ReviewDays",
];

pub const OUTPUT_NAMES: [&str; 9] = ["Result", "ErrorCode", "Message", "ScriptVersion", "TaskCreated", "StagedAt", "StatusJson", "TaskState", "Cleaned"];

/// The sources the endpoint script is built from, embedded so the app needs no build step and no file to find.
const TEMPLATE_SOURCE: &str = include_str!("../../../../scripts/migration/endpoint/nvoke-migration.template.ps1");
const INSTALLER_SOURCE: &str = include_str!("../../../../scripts/migration/endpoint/installAgent.ps1");
const BASE_VERSION: &str = "0.1.0";

/// The built script, ready to publish or export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    /// For example `0.1.0+3f9a2c1`: what the script reports as `ScriptVersion`.
    pub version: String,
    pub content: String,
}

impl Package {
    /// The script this build of N-voke ships: the same output `scripts/migration/endpoint/build.mjs` produces.
    pub fn embedded() -> Result<Self, String> {
        Self::build(TEMPLATE_SOURCE, INSTALLER_SOURCE)
    }

    /// Embeds the installer (base64, with its SHA-256 for the launcher to check) in the template and stamps a
    /// version from the content hash, exactly as `build.mjs` does: change either source and the version changes.
    pub fn build(template: &str, installer: &str) -> Result<Self, String> {
        for (name, text) in [("installAgent.ps1", installer), ("the launcher template", template)] {
            if text.bytes().any(|b| !(matches!(b, 9 | 10 | 13) || (32..=126).contains(&b))) {
                return Err(format!("{name} must be plain ASCII: Windows PowerShell 5.1 reads a file without a byte-order mark as ANSI."));
            }
        }
        let installer_sha: String = Sha256::digest(installer.as_bytes()).iter().map(|b| format!("{b:02X}")).collect();
        let mut both = Sha256::new();
        both.update(template.as_bytes());
        both.update(installer.as_bytes());
        let hash: String = both.finalize().iter().map(|b| format!("{b:02x}")).collect();
        let version = format!("{BASE_VERSION}+{}", &hash[..7]);
        let built = template
            .replace("__SCRIPT_VERSION__", &version)
            .replacen("__INSTALLER_B64__", &B64.encode(installer.as_bytes()), 1)
            .replacen("__INSTALLER_SHA256__", &installer_sha, 1);
        // Windows PowerShell expects CRLF.
        Self::from_built(&built.replace("\r\n", "\n").replace('\n', "\r\n"))
    }

    /// Checks the built script and reads its version. Placeholders left in it mean `build.mjs` wasn't run.
    pub fn from_built(content: &str) -> Result<Self, String> {
        if let Some(bad) = content.bytes().find(|b| !(matches!(b, 9 | 10 | 13) || (32..=126).contains(b))) {
            return Err(format!("The endpoint script must be plain ASCII (found byte {bad}): Windows PowerShell 5.1 reads a file without a byte-order mark as ANSI."));
        }
        if content.contains("__SCRIPT_VERSION__") || content.contains("__INSTALLER_B64__") || content.contains("__INSTALLER_SHA256__") {
            return Err("The endpoint script still has build placeholders. Run `node scripts/migration/endpoint/build.mjs` first.".into());
        }
        let version = content
            .lines()
            .find_map(|l| l.trim().strip_prefix("$ScriptVersion = '")?.strip_suffix('\''))
            .filter(|v| v.split_once('+').is_some_and(|(base, hash)| !base.is_empty() && hash.len() >= 7 && hash.chars().all(|c| c.is_ascii_hexdigit())))
            .ok_or("The endpoint script doesn't carry a version like 0.1.0+3f9a2c1.")?;
        Ok(Self { version: version.into(), content: content.into() })
    }

    /// `N-voke migration 0.1.0 (3f9a2c1)`: the content hash is in the name, so a changed script is a different name.
    pub fn script_name(&self) -> String {
        let (base, hash) = self.version.split_once('+').unwrap_or((&self.version, ""));
        format!("{SCRIPT_NAME_PREFIX}{base} ({hash})")
    }

    /// The Ecoverse script, with the same inputs (and their defaults) and outputs as the script declares.
    pub fn definition(&self, organization_id: &str) -> ScriptDefinition {
        let text = |value: &str| InputValue { string_value: Some(value.into()), ..Default::default() };
        let number = |value: i64| InputValue { int_value: Some(value), ..Default::default() };
        let input = |name: &str, kind: InputType, value: InputValue| RunInput { name: name.into(), variable: name.into(), kind, value };
        let inputs = INPUT_NAMES
            .iter()
            .map(|&name| match name {
                "Action" => input(name, InputType::String, text("Launch")),
                "RegistrationToken" => input(name, InputType::Password, text("-")),
                "TaskRepeatMinutes" => input(name, InputType::Int, number(15)),
                "TaskExpiryDays" => input(name, InputType::Int, number(14)),
                "CleanMode" => input(name, InputType::String, text("full")),
                "ExpectUfa" => input(name, InputType::String, text("false")),
                "ReviewDays" => input(name, InputType::Int, number(7)),
                other => input(other, InputType::String, text("-")),
            })
            .collect();
        ScriptDefinition {
            organization_id: organization_id.into(),
            name: self.script_name(),
            description: format!("N-voke agent migration endpoint script {}. Moves a Windows device to another N-central server. Published by N-voke: replaced automatically when the script changes.", self.version),
            content: self.content.clone(),
            timeout_seconds: SCRIPT_TIMEOUT_SECONDS,
            inputs,
            outputs: OUTPUT_NAMES.iter().map(|&n| ScriptOutput { name: n.into(), variable: n.into(), kind: InputType::String }).collect(),
        }
    }
}

/// What publishing will do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "action")]
pub enum PublishAction {
    Create,
    /// A script of ours with another version: replaced in place, keeping its ID.
    Update { id: String },
    UpToDate { id: String },
}

/// Decides from the scripts found in the organization. The version is in the name, so the same name
/// is the same script. If several of ours exist (someone copied one), the exact name wins, else the first.
pub fn plan_publish(package: &Package, found: &[Script]) -> PublishAction {
    let name = package.script_name();
    let ours: Vec<&Script> = found.iter().filter(|s| s.name.starts_with(SCRIPT_NAME_PREFIX)).collect();
    if let Some(same) = ours.iter().find(|s| s.name == name) {
        return PublishAction::UpToDate { id: same.id.clone() };
    }
    match ours.first() {
        Some(other) => PublishAction::Update { id: other.id.clone() },
        None => PublishAction::Create,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Published {
    pub script_id: String,
    pub action: PublishAction,
}

impl Package {
    /// The file name for the manual upload: the version is in it, so an old copy is never mistaken for the current one.
    pub fn file_name(&self) -> String {
        format!("nvoke-migration-{}.ps1", self.version.replace('+', "-"))
    }
}

impl Core {
    /// Writes the script to `dir` for the operator to upload to an N-central server's repository (with **Enable API**
    /// on), which is how the REST path gets it. Returns the file's path.
    pub fn migration_export_script(&self, package: &Package, dir: &std::path::Path) -> Result<std::path::PathBuf, String> {
        crate::vault::create_private_dir(dir)?;
        let path = dir.join(package.file_name());
        let result = crate::vault::write_private(&path, package.content.as_bytes());
        self.record(&crate::audit::AuditEntry::new("migration.script.export", &format!("Exported the endpoint script {}", package.version), result.clone(), serde_json::json!({ "version": package.version })));
        result?;
        Ok(path)
    }

    /// Publishes the endpoint script to an Ecoverse organization, only if it isn't already there at this version.
    pub async fn publish_migration_script(&self, package: &Package, organization_id: &str) -> Result<Published, String> {
        let found = self.search_scripts(organization_id, SCRIPT_NAME_PREFIX.trim()).await?;
        let action = plan_publish(package, &found);
        let script_id = match &action {
            PublishAction::UpToDate { id } => id.clone(),
            PublishAction::Create => self.publish_script(&package.definition(organization_id), None).await?,
            PublishAction::Update { id } => self.publish_script(&package.definition(organization_id), Some(id)).await?,
        };
        Ok(Published { script_id, action })
    }
}

// ---- launch inputs --------------------------------------------------------------------

/// What a launch tells the script. The action decides which fields matter; the rest are sent as `-`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operation {
    Launch { dest_server: String, dest_customer_id: String, registration_token: String },
    /// Read the device's record through whichever agent it has now.
    Status,
    /// Collect the record and remove everything the migration left.
    Cleanup,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchParams {
    pub operation: Operation,
    pub migration_id: String,
    /// The script version N-voke published; the script refuses to run if it is a different build.
    pub launcher_version: String,
    pub task_repeat_minutes: u32,
    pub task_expiry_days: u32,
    pub clean_full: bool,
    /// True only when the destination is linked to Ecoverse.
    pub expect_ufa: bool,
    pub review_days: u32,
}

fn is_host_name(host: &str) -> bool {
    let label = |l: &str| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    host.contains('.') && host.len() <= 253 && host.split('.').all(label)
}

fn is_registration_token(token: &str) -> bool {
    (8..=128).contains(&token.len()) && token.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

impl LaunchParams {
    /// Refuses what the script would refuse, and anything `cmd.exe` would alter or act on in the REST command line.
    pub fn validate(&self) -> Result<(), String> {
        if let Operation::Launch { dest_server, dest_customer_id, registration_token } = &self.operation {
            if !is_host_name(dest_server) {
                return Err(format!("The destination \"{dest_server}\" isn't a plain host name."));
            }
            if dest_customer_id.is_empty() || !dest_customer_id.chars().all(|c| c.is_ascii_digit()) {
                return Err(format!("The destination customer ID \"{dest_customer_id}\" isn't numeric."));
            }
            if !is_registration_token(registration_token) {
                return Err("The destination's registration token is missing or not in the expected form.".into());
            }
        }
        let plain = |v: &str| !v.is_empty() && v.len() <= 200 && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '/' | '-'));
        if !plain(&self.migration_id) || !plain(&self.launcher_version) {
            return Err("The migration ID and script version can only use letters, digits and . _ + / -".into());
        }
        if self.review_days > 999 {
            return Err("The review period can be at most 999 days.".into());
        }
        Ok(())
    }

    /// The value of each input by name, validated. `-` where an action doesn't use it.
    fn values(&self) -> Result<Vec<(&'static str, String)>, String> {
        self.validate()?;
        let (action, server, customer, token) = match &self.operation {
            Operation::Launch { dest_server, dest_customer_id, registration_token } => ("Launch", dest_server.as_str(), dest_customer_id.as_str(), registration_token.as_str()),
            Operation::Status => ("Status", "-", "-", "-"),
            Operation::Cleanup => ("Cleanup", "-", "-", "-"),
        };
        Ok(vec![
            ("Action", action.into()),
            ("DestServer", server.into()),
            ("DestCustomerId", customer.into()),
            ("RegistrationToken", token.into()),
            ("MigrationId", self.migration_id.clone()),
            ("LauncherVersion", self.launcher_version.clone()),
            ("TaskRepeatMinutes", self.task_repeat_minutes.to_string()),
            ("TaskExpiryDays", self.task_expiry_days.to_string()),
            ("CleanMode", if self.clean_full { "full" } else { "none" }.into()),
            ("ExpectUfa", self.expect_ufa.to_string()),
            ("ReviewDays", self.review_days.to_string()),
        ])
    }

    /// The Ecoverse run inputs: all eleven, typed as the script declares them.
    pub fn ecoverse_inputs(&self) -> Result<Vec<RunInput>, String> {
        self.values()?
            .into_iter()
            .map(|(name, value)| {
                let (kind, value) = match name {
                    "RegistrationToken" => (InputType::Password, InputValue { string_value: Some(value), ..Default::default() }),
                    "TaskRepeatMinutes" | "TaskExpiryDays" | "ReviewDays" => (InputType::Int, InputValue { int_value: Some(value.parse().map_err(|_| "not a number")?), ..Default::default() }),
                    _ => (InputType::String, InputValue { string_value: Some(value), ..Default::default() }),
                };
                Ok(RunInput { name: name.into(), variable: name.into(), kind, value })
            })
            .collect()
    }

    /// The REST path's single `CommandLine` value: `-Name value` pairs. N-central shows this in full in
    /// the task's details, so the token is visible there (see docs/migration/08-security.md).
    pub fn command_line(&self) -> Result<String, String> {
        Ok(self.values()?.into_iter().map(|(name, value)| format!("-{name} {value}")).collect::<Vec<_>>().join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEMPLATE: &str = include_str!("../../../../scripts/migration/endpoint/nvoke-migration.template.ps1");

    fn built() -> String {
        TEMPLATE.replace("__SCRIPT_VERSION__", "0.1.0+3f9a2c1").replace("__INSTALLER_B64__", "QUJD").replace("__INSTALLER_SHA256__", &"A".repeat(64))
    }

    fn script(id: &str, name: &str) -> Script {
        Script { id: id.into(), name: name.into(), updated_at: None, language: "POWERSHELL".into(), description: None, timeout_seconds: None, inputs: None }
    }

    fn launch() -> LaunchParams {
        LaunchParams {
            operation: Operation::Launch { dest_server: "new.example.com".into(), dest_customer_id: "1234".into(), registration_token: "abcd-1234-efgh".into() },
            migration_id: "mig-1/wave-2".into(),
            launcher_version: "0.1.0+3f9a2c1".into(),
            task_repeat_minutes: 15,
            task_expiry_days: 14,
            clean_full: true,
            expect_ufa: false,
            review_days: 7,
        }
    }

    #[test]
    fn a_built_script_gives_its_version_and_a_hash_versioned_name() {
        let package = Package::from_built(&built()).unwrap();
        assert_eq!(package.version, "0.1.0+3f9a2c1");
        assert_eq!(package.script_name(), "N-voke migration 0.1.0 (3f9a2c1)");
        assert!(package.script_name().len() <= 64);
    }

    #[test]
    fn an_unbuilt_or_non_ascii_script_is_refused() {
        assert!(Package::from_built(TEMPLATE).unwrap_err().contains("build placeholders"));
        assert!(Package::from_built(&format!("{}\n# caf\u{e9}", built())).unwrap_err().contains("ASCII"));
        assert!(Package::from_built("Write-Output 'no version'").unwrap_err().contains("version"));
        assert!(Package::from_built("$ScriptVersion = 'dev'").is_err());
    }

    #[test]
    fn the_declared_inputs_and_outputs_are_exactly_what_the_script_uses() {
        // The names the script reads as inputs are the variables it uses before assigning them; every one we
        // declare must appear in the script, and every output we declare must be assigned by it.
        for name in INPUT_NAMES {
            assert!(TEMPLATE.contains(&format!("${name}")), "the script never mentions input {name}");
        }
        for name in OUTPUT_NAMES {
            assert!(TEMPLATE.contains(&format!("${name} = ")), "the script never sets output {name}");
        }
        let definition = Package::from_built(&built()).unwrap().definition("org-1");
        assert_eq!(definition.inputs.iter().map(|i| i.variable.as_str()).collect::<Vec<_>>(), INPUT_NAMES);
        assert_eq!(definition.outputs.len(), OUTPUT_NAMES.len());
        assert!(definition.validate().is_ok(), "{:?}", definition.validate());
    }

    #[test]
    fn the_token_input_is_a_password_and_numbers_are_numbers() {
        let definition = Package::from_built(&built()).unwrap().definition("org-1");
        let kind = |n: &str| definition.inputs.iter().find(|i| i.name == n).unwrap().kind;
        assert_eq!(kind("RegistrationToken"), InputType::Password);
        assert_eq!((kind("ReviewDays"), kind("TaskRepeatMinutes"), kind("TaskExpiryDays")), (InputType::Int, InputType::Int, InputType::Int));
        assert_eq!(kind("Action"), InputType::String);
        let published = definition.to_input(None);
        assert_eq!(published["inputs"][3]["value"]["stringValue"], "-", "no secret in the published defaults");
    }

    #[test]
    fn publishing_creates_updates_or_leaves_alone() {
        let package = Package::from_built(&built()).unwrap();
        assert_eq!(plan_publish(&package, &[]), PublishAction::Create);
        assert_eq!(plan_publish(&package, &[script("x", "Cleanup disks")]), PublishAction::Create, "someone else's script is never touched");
        let old = script("s-old", "N-voke migration 0.1.0 (aaaaaaa)");
        assert_eq!(plan_publish(&package, std::slice::from_ref(&old)), PublishAction::Update { id: "s-old".into() });
        let same = script("s-same", "N-voke migration 0.1.0 (3f9a2c1)");
        assert_eq!(plan_publish(&package, &[old, same]), PublishAction::UpToDate { id: "s-same".into() });
    }

    #[tokio::test]
    async fn publishing_looks_in_the_organization_first_and_changes_nothing_if_it_cant() {
        let core = Core::for_tests("package");
        core.set_dry_run(true);
        // The search is a read and no API is reachable, so nothing is created or held back.
        let result = core.publish_migration_script(&Package::from_built(&built()).unwrap(), "org-1").await;
        assert!(result.is_err());
        assert!(core.dry_run_log().is_empty());
    }

    #[test]
    fn an_ecoverse_launch_sends_all_eleven_inputs_typed_and_the_token_as_a_password() {
        let inputs = launch().ecoverse_inputs().unwrap();
        assert_eq!(inputs.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), INPUT_NAMES);
        let by = |n: &str| inputs.iter().find(|i| i.name == n).unwrap();
        assert_eq!(by("RegistrationToken").kind, InputType::Password);
        assert_eq!(by("RegistrationToken").value.string_value.as_deref(), Some("abcd-1234-efgh"));
        assert_eq!((by("ReviewDays").kind, by("ReviewDays").value.int_value), (InputType::Int, Some(7)));
        assert_eq!(by("CleanMode").value.string_value.as_deref(), Some("full"));
        assert_eq!(by("ExpectUfa").value.string_value.as_deref(), Some("false"));
    }

    #[test]
    fn status_and_cleanup_send_dashes_for_what_they_dont_use() {
        let mut params = launch();
        params.operation = Operation::Status;
        let inputs = params.ecoverse_inputs().unwrap();
        let text = |n: &str| inputs.iter().find(|i| i.name == n).unwrap().value.string_value.clone().unwrap();
        assert_eq!((text("Action").as_str(), text("DestServer").as_str(), text("RegistrationToken").as_str()), ("Status", "-", "-"));
        params.operation = Operation::Cleanup;
        assert!(params.command_line().unwrap().starts_with("-Action Cleanup -DestServer - -DestCustomerId - -RegistrationToken - "));
    }

    #[test]
    fn the_rest_command_line_has_every_input_as_a_name_value_pair() {
        assert_eq!(
            launch().command_line().unwrap(),
            "-Action Launch -DestServer new.example.com -DestCustomerId 1234 -RegistrationToken abcd-1234-efgh -MigrationId mig-1/wave-2 -LauncherVersion 0.1.0+3f9a2c1 \
             -TaskRepeatMinutes 15 -TaskExpiryDays 14 -CleanMode full -ExpectUfa false -ReviewDays 7"
        );
    }

    #[test]
    fn what_the_script_or_cmd_would_refuse_is_refused_before_anything_is_sent() {
        let with = |f: &dyn Fn(&mut LaunchParams)| {
            let mut p = launch();
            f(&mut p);
            p.validate()
        };
        let set = |p: &mut LaunchParams, server: &str, customer: &str, token: &str| p.operation = Operation::Launch { dest_server: server.into(), dest_customer_id: customer.into(), registration_token: token.into() };
        assert!(with(&|p| set(p, "https://new.example.com", "1", "abcd-1234")).unwrap_err().contains("host name"));
        assert!(with(&|p| set(p, "new.example.com:8443", "1", "abcd-1234")).is_err());
        assert!(with(&|p| set(p, "localhost", "1", "abcd-1234")).is_err(), "a bare name isn't a fully qualified host");
        assert!(with(&|p| set(p, "new.example.com", "12a", "abcd-1234")).unwrap_err().contains("numeric"));
        assert!(with(&|p| set(p, "new.example.com", "1", "short")).unwrap_err().contains("token"));
        assert!(with(&|p| set(p, "new.example.com", "1", "abcd1234&calc")).is_err());
        assert!(with(&|p| p.migration_id = "50%PATH%".into()).is_err(), "cmd.exe would expand it");
        assert!(with(&|p| p.migration_id = "a\"b".into()).is_err());
        for hostile in ["a b", "a&calc", "a|b", "a<b", "a^b", "a(b)", "a!b", "a;b", "a\nb"] {
            assert!(with(&|p| p.migration_id = hostile.into()).is_err(), "{hostile:?}");
            assert!(with(&|p| p.launcher_version = hostile.into()).is_err(), "{hostile:?}");
        }
        assert!(with(&|p| p.launcher_version = "0.1.0+3f9a2c1".into()).is_ok());
        assert!(with(&|p| p.migration_id = String::new()).is_err());
        assert!(with(&|p| p.review_days = 1000).is_err());
        assert!(launch().validate().is_ok());
        assert!(launch().command_line().is_ok());
        let mut bad = launch();
        bad.review_days = 1000;
        assert!(bad.ecoverse_inputs().is_err() && bad.command_line().is_err(), "both paths validate");
    }

    #[test]
    fn the_embedded_script_is_built_stamped_and_ready_to_publish() {
        let package = Package::embedded().unwrap();
        assert!(package.version.starts_with("0.1.0+") && package.version.len() == "0.1.0+".len() + 7, "{}", package.version);
        assert!(package.content.contains(&format!("$ScriptVersion = '{}'", package.version)));
        assert!(package.content.contains("\r\n") && !package.content.contains("\n\n\n\n"), "CRLF line endings");
        assert!(!package.content.contains("__INSTALLER_B64__"));
        assert!(package.definition("org-1").validate().is_ok());
    }

    #[test]
    fn the_version_follows_the_content_and_the_installer_is_embedded_with_its_hash() {
        let a = Package::build("# t __SCRIPT_VERSION__\n$B = '__INSTALLER_B64__'\n$S = '__INSTALLER_SHA256__'\n$ScriptVersion = '__SCRIPT_VERSION__'\n", "write-output 'hi'\n").unwrap();
        let b = Package::build("# t __SCRIPT_VERSION__\n$B = '__INSTALLER_B64__'\n$S = '__INSTALLER_SHA256__'\n$ScriptVersion = '__SCRIPT_VERSION__'\n", "write-output 'hi!'\n").unwrap();
        assert_ne!(a.version, b.version, "changing the installer changes the version");
        assert!(a.content.contains("$B = 'd3JpdGUtb3V0cHV0ICdoaScK'"), "{}", a.content);
        let sha = Sha256::digest(b"write-output 'hi'\n").iter().map(|x| format!("{x:02X}")).collect::<String>();
        assert!(a.content.contains(&format!("$S = '{sha}'")));
        assert!(Package::build("# caf\u{e9}", "x").unwrap_err().contains("ASCII"));
        assert!(Package::build("no version marker here", "x").is_err());
    }

    /// The Rust build and `build.mjs` must give the same bytes, or a script published by one wouldn't match the other.
    /// Skipped where Node isn't installed (CI has it).
    #[test]
    fn it_matches_what_the_node_build_produces() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/migration/endpoint");
        let Ok(output) = std::process::Command::new("node").arg(dir.join("build.mjs")).output() else { return };
        if !output.status.success() {
            return;
        }
        let node = std::fs::read(dir.join("dist/nvoke-migration.ps1")).unwrap();
        assert_eq!(Package::embedded().unwrap().content.as_bytes(), node.as_slice());
    }

    #[test]
    fn the_export_is_a_versioned_private_file_with_the_exact_script() {
        let package = Package::embedded().unwrap();
        assert_eq!(package.file_name(), format!("nvoke-migration-{}.ps1", package.version.replace('+', "-")));
        let core = Core::for_tests("export-script");
        let dir = std::env::temp_dir().join(format!("nvoke-export-{}", std::process::id()));
        let path = core.migration_export_script(&package, &dir).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), package.content.as_bytes());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o077, 0);
        }
        assert!(core.audit_log(None).unwrap().entries.iter().any(|e| e.action == "migration.script.export"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
