//! Reports for a project: a CSV with one row per device, and an HTML summary that can be
//! sent to a customer. Both are built from the project alone, which holds no registration
//! token, so no secret can reach a report. Device names come from the managed machines and
//! are untrusted: CSV cells that a spreadsheet would run as a formula are defused, and
//! everything in the HTML is escaped.

use std::fmt::Write;

use super::project::{Project, ProjectDevice, Stage};
use crate::{Core, audit::AuditEntry, vault};

/// Which devices a report covers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    pub wave: Option<u32>,
    /// A destination customer or site ID.
    pub dest_org_unit: Option<String>,
}

fn selected<'a>(project: &'a Project, filter: &'a Filter) -> impl Iterator<Item = &'a ProjectDevice> {
    project
        .devices
        .iter()
        .filter(move |d| filter.wave.is_none_or(|w| d.wave == Some(w)))
        .filter(move |d| filter.dest_org_unit.as_deref().is_none_or(|u| d.dest_org_unit_id.as_deref() == Some(u)))
}

/// A cell as a spreadsheet will read it: quoted when needed, and with a leading `'` when it would otherwise
/// start a formula (`=`, `+`, `-`, `@`, tab or carriage return), which Excel and others would run.
pub fn csv_cell(value: &str) -> String {
    let defused = if value.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{value}") } else { value.to_string() };
    if defused.contains([',', '"', '\n', '\r']) { format!("\"{}\"", defused.replace('"', "\"\"")) } else { defused }
}

pub fn stage_label(stage: Stage) -> &'static str {
    match stage {
        Stage::Planned => "Planned",
        Stage::Unreachable => "Unreachable",
        Stage::Launched => "Launched",
        Stage::NotPickedUp => "Not picked up",
        Stage::LaunchFailed => "Launch failed",
        Stage::Staged => "Staged",
        Stage::StuckStaged => "Stuck staged",
        Stage::AgentRemoved => "Agent removed",
        Stage::Lost => "Lost",
        Stage::Registered => "Registered",
        Stage::WrongPlace => "Wrong place",
        Stage::Healthy => "Healthy",
        Stage::CleanedUp => "Cleaned up",
        Stage::Retired => "Retired",
        Stage::Excluded => "Excluded",
        Stage::Resolved => "Resolved",
    }
}

const CSV_HEADER: &str = "Device,Source device ID,Destination device ID,Wave,Path,Stage,In stage since,Attempts,Destination customer or site ID,Last error,Task ID";

pub fn csv(project: &Project, filter: &Filter) -> String {
    let mut out = String::from(CSV_HEADER);
    out.push_str("\r\n");
    for d in selected(project, filter) {
        let cells = [
            d.name.clone(),
            d.source_device_id.clone(),
            d.dest_device_id.clone().unwrap_or_default(),
            d.wave.map(|w| w.to_string()).unwrap_or_default(),
            d.path.map(|p| format!("{p:?}")).unwrap_or_default(),
            stage_label(d.stage).to_string(),
            d.entered_stage_at.clone(),
            d.attempts.to_string(),
            d.dest_org_unit_id.clone().unwrap_or_default(),
            d.last_error.clone().unwrap_or_default(),
            d.task_id.clone().unwrap_or_default(),
        ];
        out.push_str(&cells.iter().map(|c| csv_cell(c)).collect::<Vec<_>>().join(","));
        out.push_str("\r\n");
    }
    out
}

pub fn html_escape(text: &str) -> String {
    text.chars().fold(String::with_capacity(text.len()), |mut out, c| {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
        out
    })
}

/// A self-contained page: totals by outcome, progress by wave, what needs attention, and every device.
pub fn html(project: &Project, filter: &Filter, generated_at: &str) -> String {
    let devices: Vec<&ProjectDevice> = selected(project, filter).collect();
    let mut by_stage: std::collections::BTreeMap<Stage, usize> = std::collections::BTreeMap::new();
    for d in &devices {
        *by_stage.entry(d.stage).or_default() += 1;
    }
    let done = devices.iter().filter(|d| matches!(d.stage, Stage::Healthy | Stage::CleanedUp | Stage::Retired | Stage::Resolved)).count();
    let e = html_escape;
    let mut out = String::new();
    let _ = write!(
        out,
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>{title}</title><style>\
         body{{font:14px/1.4 system-ui,sans-serif;margin:2rem;color:#1b1f23}}h1{{font-size:1.4rem}}h2{{font-size:1.1rem;margin-top:2rem}}\
         table{{border-collapse:collapse;width:100%}}th,td{{border:1px solid #d0d7de;padding:.3rem .5rem;text-align:left}}th{{background:#f6f8fa}}\
         .bad{{color:#b42318}}.ok{{color:#1a7f37}}</style></head><body><h1>{title}</h1>\
         <p>{from} &rarr; {to}. Generated {when}. {done} of {total} devices moved and checking in.</p>",
        title = e(&format!("Agent migration: {}", project.name)),
        from = e(&project.source_server_id),
        to = e(&project.dest_server_id),
        when = e(generated_at),
        done = done,
        total = devices.len(),
    );

    out.push_str("<h2>Outcome</h2><table><tr><th>Stage</th><th>Devices</th></tr>");
    for (stage, count) in &by_stage {
        let class = if stage.needs_attention() { " class=\"bad\"" } else { "" };
        let _ = write!(out, "<tr><td{class}>{}</td><td>{count}</td></tr>", e(stage_label(*stage)));
    }
    out.push_str("</table>");

    out.push_str("<h2>Waves</h2><table><tr><th>Wave</th><th>Devices</th><th>Moved</th><th>Needs attention</th></tr>");
    for wave in &project.waves {
        let in_wave: Vec<&&ProjectDevice> = devices.iter().filter(|d| d.wave == Some(wave.number)).collect();
        let moved = in_wave.iter().filter(|d| matches!(d.stage, Stage::Healthy | Stage::CleanedUp | Stage::Retired | Stage::Resolved)).count();
        let attention = in_wave.iter().filter(|d| d.stage.needs_attention()).count();
        let _ = write!(out, "<tr><td>{}</td><td>{}</td><td>{moved}</td><td>{attention}</td></tr>", e(&wave.label), in_wave.len());
    }
    out.push_str("</table>");

    let mut needing: Vec<&&ProjectDevice> = devices.iter().filter(|d| d.stage.needs_attention()).collect();
    needing.sort_by(|a, b| a.entered_stage_at.cmp(&b.entered_stage_at));
    if !needing.is_empty() {
        out.push_str("<h2>Needs attention</h2><table><tr><th>Device</th><th>Stage</th><th>Since</th><th>Why</th></tr>");
        for d in needing {
            let _ = write!(out, "<tr><td>{}</td><td class=\"bad\">{}</td><td>{}</td><td>{}</td></tr>", e(&d.name), e(stage_label(d.stage)), e(&d.entered_stage_at), e(d.last_error.as_deref().unwrap_or("")));
        }
        out.push_str("</table>");
    }

    out.push_str("<h2>Devices</h2><table><tr><th>Device</th><th>Wave</th><th>Path</th><th>Stage</th><th>Since</th><th>Attempts</th></tr>");
    for d in &devices {
        let _ = write!(
            out,
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            e(&d.name),
            d.wave.map(|w| w.to_string()).unwrap_or_default(),
            d.path.map(|p| format!("{p:?}")).unwrap_or_default(),
            e(stage_label(d.stage)),
            e(&d.entered_stage_at),
            d.attempts
        );
    }
    out.push_str("</table></body></html>");
    out
}

/// A file name that is safe on every system, from the project's name.
fn safe_name(text: &str) -> String {
    let cleaned: String = text.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    let trimmed = cleaned.trim_matches('-');
    if trimmed.is_empty() { "project".into() } else { trimmed.chars().take(40).collect() }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Csv,
    Html,
}

impl Core {
    /// Writes a report into `dir` (the downloads folder in the apps), readable only by the current user, and returns its path.
    pub fn migration_write_report(&self, project: &Project, filter: &Filter, format: Format, dir: &std::path::Path) -> Result<std::path::PathBuf, String> {
        let now = chrono::Utc::now();
        let (extension, body) = match format {
            Format::Csv => ("csv", csv(project, filter)),
            Format::Html => ("html", html(project, filter, &now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))),
        };
        vault::create_private_dir(dir)?;
        let path = dir.join(format!("nvoke-migration-{}-{}.{extension}", safe_name(&project.name), now.format("%Y%m%d-%H%M%S")));
        let result = vault::write_private(&path, body.as_bytes());
        self.record(&AuditEntry::new("migration.report", &format!("Exported a {extension} report of {}", project.name), result.clone(), serde_json::json!({ "project": project.id })));
        result?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::*;
    use crate::migration::{
        planner::{DeliveryPath, PlannedDevice},
        project::DestEcoverse,
    };

    fn now() -> DateTime<Utc> {
        "2026-10-20T12:00:00Z".parse().unwrap()
    }

    fn planned(id: &str, name: &str, dest: &str) -> PlannedDevice {
        PlannedDevice {
            device_id: id.into(),
            name: name.into(),
            source_customer_id: Some("10".into()),
            source_site_id: None,
            dest_org_unit_id: Some(dest.into()),
            path: Some(DeliveryPath::Rest),
            asset_id: None,
            blockers: vec![],
            warnings: vec![],
            excluded: false,
        }
    }

    fn project() -> Project {
        let mut p = Project::new("mig-1".into(), "Acme & Sons <move>", "src", "dst", DestEcoverse::NotLinked, now());
        p.add_planned(&[planned("1", "PC-1", "110"), planned("2", "=HYPERLINK(\"http://evil\")", "110"), planned("3", "<script>alert(1)</script>", "120")], now());
        p.create_wave("Pilot", &["1".into(), "2".into()], now()).unwrap();
        p.create_wave("Rest", &["3".into()], now()).unwrap();
        p.devices[0].stage = Stage::Healthy;
        p.devices[0].dest_device_id = Some("5001".into());
        p.devices[1].stage = Stage::Lost;
        p.devices[1].last_error = Some("nothing on the destination, \"still\" waiting".into());
        p
    }

    #[test]
    fn cells_are_quoted_and_formulas_are_defused() {
        assert_eq!(csv_cell("plain"), "plain");
        assert_eq!(csv_cell("a,b"), "\"a,b\"");
        assert_eq!(csv_cell("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_cell("line\nbreak"), "\"line\nbreak\"");
        for formula in ["=1+1", "+1", "-1", "@SUM(A1)", "\tcmd"] {
            assert!(csv_cell(formula).starts_with('\''), "{formula:?}");
        }
        assert_eq!(csv_cell("PC-1"), "PC-1", "a hyphen inside a name is fine");
    }

    #[test]
    fn the_csv_has_a_header_and_one_row_per_device() {
        let text = csv(&project(), &Filter::default());
        let lines: Vec<&str> = text.split("\r\n").filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].starts_with("Device,Source device ID,Destination device ID,Wave,Path,Stage"));
        assert!(lines[1].starts_with("PC-1,1,5001,1,Rest,Healthy,"), "{}", lines[1]);
        assert!(lines[2].starts_with("'=HYPERLINK(\"\"http://evil\"\")") || lines[2].starts_with("\"'=HYPERLINK("), "{}", lines[2]);
        assert!(lines[2].contains("Lost") && lines[2].contains("\"nothing on the destination, \"\"still\"\" waiting\""), "{}", lines[2]);
    }

    #[test]
    fn a_report_can_be_limited_to_a_wave_or_a_destination() {
        let p = project();
        let rows = |f: &Filter| csv(&p, f).lines().count() - 1;
        assert_eq!(rows(&Filter { wave: Some(1), dest_org_unit: None }), 2);
        assert_eq!(rows(&Filter { wave: None, dest_org_unit: Some("120".into()) }), 1);
        assert_eq!(rows(&Filter { wave: Some(2), dest_org_unit: Some("110".into()) }), 0);
    }

    #[test]
    fn the_html_escapes_every_untrusted_value_and_summarises_the_outcome() {
        let page = html(&project(), &Filter::default(), "2026-10-20T12:00:00Z");
        assert!(!page.contains("<script>") && page.contains("&lt;script&gt;alert(1)&lt;/script&gt;"), "{page}");
        assert!(page.contains("Acme &amp; Sons &lt;move&gt;"));
        assert!(page.contains("1 of 3 devices moved and checking in"));
        assert!(page.contains("Needs attention") && page.contains("Lost") && page.contains("&quot;still&quot;"));
        assert!(page.contains("<td>Pilot</td><td>2</td><td>1</td><td>1</td>"), "wave table: {page}");
        assert!(page.starts_with("<!doctype html>") && page.ends_with("</html>"));
    }

    #[test]
    fn an_all_clear_project_has_no_attention_section() {
        let mut p = project();
        p.devices[1].stage = Stage::Resolved;
        let page = html(&p, &Filter::default(), "now");
        assert!(!page.contains("<h2>Needs attention</h2>"));
    }

    #[test]
    fn file_names_are_safe_and_reports_are_written_private_and_audited() {
        assert_eq!(safe_name("Acme & Sons <move>"), "Acme---Sons--move");
        assert_eq!(safe_name("../../etc"), "etc");
        assert_eq!(safe_name("   "), "project");
        let core = Core::for_tests("report");
        let dir = std::env::temp_dir().join(format!("nvoke-report-{}", std::process::id()));
        let path = core.migration_write_report(&project(), &Filter::default(), Format::Csv, &dir).unwrap();
        assert!(path.starts_with(&dir) && path.extension().is_some_and(|e| e == "csv"));
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("nvoke-migration-Acme---Sons--move-"), "{name}");
        assert!(std::fs::read_to_string(&path).unwrap().starts_with("Device,"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o077, 0, "readable only by the current user");
        }
        assert!(core.audit_log(None).unwrap().entries.iter().any(|e| e.action == "migration.report"));
        let html_path = core.migration_write_report(&project(), &Filter::default(), Format::Html, &dir).unwrap();
        assert!(std::fs::read_to_string(html_path).unwrap().contains("<h1>"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
