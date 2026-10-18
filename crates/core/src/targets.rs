//! What a run is aimed at: devices and tags, and how to describe them.
//! A port of `targets.ts`.

use crate::model::{Asset, Tag};

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Device(Asset),
    Tag(Tag),
}

impl Target {
    pub fn key(&self) -> String {
        match self {
            Target::Device(a) => format!("d:{}", a.id),
            Target::Tag(t) => format!("t:{}", t.id),
        }
    }
}

/// "PatchGroupTwo + 2 devices", "SLS-2188", "3 tags + 1 device".
pub fn target_label(targets: &[Target]) -> String {
    let tags: Vec<&Tag> = targets.iter().filter_map(|t| if let Target::Tag(t) = t { Some(t) } else { None }).collect();
    let devices: Vec<&Asset> = targets.iter().filter_map(|t| if let Target::Device(a) = t { Some(a) } else { None }).collect();
    let mut parts = vec![];
    match tags.len() {
        0 => {}
        1 => parts.push(tags[0].name.clone()),
        n => parts.push(format!("{n} tags")),
    }
    if devices.len() == 1 && tags.is_empty() {
        parts.push(devices[0].name.clone());
    } else if !devices.is_empty() {
        parts.push(format!("{} device{}", devices.len(), if devices.len() == 1 { "" } else { "s" }));
    }
    if parts.is_empty() { "No targets".into() } else { parts.join(" + ") }
}

/// Explains why a script probably can't run on a device's OS, or None.
pub fn os_mismatch(language: Option<&str>, os_name: Option<&str>) -> Option<&'static str> {
    let (language, os_name) = (language.filter(|l| !l.is_empty())?, os_name.filter(|o| !o.is_empty())?);
    let is_windows = os_name.to_lowercase().contains("windows");
    // Scripts in these languages only run on Windows.
    if matches!(language, "POWERSHELL" | "BATCH" | "CMD" | "VBSCRIPT") && !is_windows {
        return Some("not Windows");
    }
    if matches!(language, "BASH" | "SHELL" | "SH" | "ZSH") && is_windows {
        return Some("Windows");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(name: &str) -> Target {
        Target::Tag(Tag { id: name.into(), name: name.into(), typename: "StaticTag".into(), asset_count: None, updated_at: None, description: None, organization: None, filter: None })
    }
    fn device(name: &str) -> Target {
        Target::Device(Asset::stub(name, name))
    }

    #[test]
    fn labels_targets_compactly() {
        assert_eq!(target_label(&[tag("PatchGroupTwo"), device("a"), device("b")]), "PatchGroupTwo + 2 devices");
        assert_eq!(target_label(&[device("SLS-2188")]), "SLS-2188");
        assert_eq!(target_label(&[tag("a"), tag("b"), tag("c"), device("x")]), "3 tags + 1 device");
        assert_eq!(target_label(&[]), "No targets");
    }

    #[test]
    fn flags_scripts_that_cant_run_on_a_devices_os() {
        assert_eq!(os_mismatch(Some("POWERSHELL"), Some("Ubuntu 22.04")), Some("not Windows"));
        assert_eq!(os_mismatch(Some("BASH"), Some("Windows 11 Pro")), Some("Windows"));
        assert_eq!(os_mismatch(Some("POWERSHELL"), Some("Windows 10")), None);
        assert_eq!(os_mismatch(Some("PYTHON"), Some("macOS")), None);
        assert_eq!(os_mismatch(None, Some("Windows 10")), None);
        assert_eq!(os_mismatch(Some("BASH"), None), None);
    }

    #[test]
    fn keys_tell_devices_and_tags_apart() {
        assert_ne!(tag("x").key(), device("x").key());
    }
}
