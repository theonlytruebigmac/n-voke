//! Reports for a project: a CSV with one row per device, and an HTML summary that can be
//! sent to a customer. Both are built from the project alone, which holds no registration
//! token, so no secret can reach a report. Device names come from the managed machines and
//! are untrusted: CSV cells that a spreadsheet would run as a formula are defused, and
//! everything in the HTML is escaped.

use std::fmt::Write;

use chrono::{DateTime, FixedOffset, Utc};

use super::{
    planner::DeliveryPath,
    project::{Delivery, DestEcoverse, Project, ProjectDevice, Stage},
};
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

/// What the web page needs besides the project: the servers' names and the time it is made, in the reader's time zone.
#[derive(Clone, Debug)]
pub struct ReportContext {
    /// "Label (host)", or the server's ID when it is no longer configured.
    pub source: String,
    pub dest: String,
    pub generated_at: DateTime<Utc>,
    /// Times on the page are shown in this offset (the computer's own when the app writes it).
    pub offset: FixedOffset,
}

/// How the web page sorts devices first: done, still moving, needing someone, not started, or taken out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Outcome {
    Attention,
    Moving,
    NotLaunched,
    Done,
    LeftOut,
}

/// Every stage has a place here, so a stage added later fails the build until the report knows where it goes.
fn outcome(stage: Stage) -> Outcome {
    match stage {
        Stage::Healthy | Stage::CleanedUp | Stage::Retired | Stage::Resolved => Outcome::Done,
        Stage::Launched | Stage::Staged | Stage::AgentRemoved | Stage::Registered => Outcome::Moving,
        Stage::Unreachable | Stage::NotPickedUp | Stage::LaunchFailed | Stage::StuckStaged | Stage::Lost | Stage::WrongPlace => Outcome::Attention,
        Stage::Planned => Outcome::NotLaunched,
        Stage::Excluded => Outcome::LeftOut,
    }
}

/// The outcomes in the order the summary shows them, with their words and the class that colours them.
const OUTCOMES: [(Outcome, &str, &str); 5] = [
    (Outcome::Done, "Moved and checking in", "done"),
    (Outcome::Moving, "Still moving", "moving"),
    (Outcome::Attention, "Need attention", "attention"),
    (Outcome::NotLaunched, "Not launched yet", "idle"),
    (Outcome::LeftOut, "Left out", "out"),
];

fn outcome_class(o: Outcome) -> &'static str {
    OUTCOMES.iter().find(|(x, ..)| *x == o).map(|(_, _, class)| *class).unwrap_or("idle")
}

const ALL_STAGES: [Stage; 16] = [
    Stage::Planned,
    Stage::Unreachable,
    Stage::Launched,
    Stage::NotPickedUp,
    Stage::LaunchFailed,
    Stage::Staged,
    Stage::StuckStaged,
    Stage::AgentRemoved,
    Stage::Lost,
    Stage::Registered,
    Stage::WrongPlace,
    Stage::Healthy,
    Stage::CleanedUp,
    Stage::Retired,
    Stage::Excluded,
    Stage::Resolved,
];

/// A stage in the app's own words (the CSV keeps `stage_label`, which spreadsheets built on it already use).
fn stage_word(stage: Stage) -> &'static str {
    match stage {
        Stage::Planned => "Planned",
        Stage::Unreachable => "Can't reach",
        Stage::Launched => "Launched",
        Stage::NotPickedUp => "Not picked up",
        Stage::LaunchFailed => "Launch failed",
        Stage::Staged => "Staged",
        Stage::StuckStaged => "Stuck",
        Stage::AgentRemoved => "Moving",
        Stage::Lost => "Lost",
        Stage::Registered => "On the new server",
        Stage::WrongPlace => "Wrong place",
        Stage::Healthy => "Healthy",
        Stage::CleanedUp => "Cleaned up",
        Stage::Retired => "Done",
        Stage::Excluded => "Left out",
        Stage::Resolved => "Resolved",
    }
}

/// What a stage that needs someone means, for the reader who has to act on it.
fn attention_meaning(stage: Stage) -> &'static str {
    match stage {
        Stage::Unreachable => "No agent could reach these devices. Each is picked up again if it comes back online; otherwise it needs a visit on site.",
        Stage::NotPickedUp => "The move was sent but the device never started it. It may be switched off or busy.",
        Stage::LaunchFailed => "The server refused to start the move on these devices. The reason is shown for each.",
        Stage::StuckStaged => "The install was scheduled, but the old agent is still checking in.",
        Stage::Lost => "The old agent was removed and the device hasn't appeared on the new server. Plan a follow-up on site.",
        Stage::WrongPlace => "These devices registered under a different customer or site than planned.",
        _ => "",
    }
}

fn path_word(path: Option<DeliveryPath>) -> &'static str {
    match path {
        Some(DeliveryPath::Ecoverse) => "Ecoverse",
        Some(DeliveryPath::Rest) => "N-central agent",
        None => "",
    }
}

/// A stage as an event records it (`AgentRemoved: why`), in the report's words.
fn stage_event(detail: &str) -> String {
    let (name, why) = detail.split_once(':').map_or((detail, ""), |(n, w)| (n, w.trim()));
    match ALL_STAGES.iter().find(|s| format!("{s:?}") == name.trim()) {
        Some(stage) if why.is_empty() => stage_word(*stage).to_string(),
        Some(stage) => format!("{}: {why}", stage_word(*stage)),
        None => detail.to_string(),
    }
}

fn event_kind(kind: &str) -> &str {
    match kind {
        "stage" => "Stage",
        "path" => "Delivery",
        "launch" => "Launch",
        "match" => "Match",
        "note" => "Note",
        "status" => "Status",
        other => other,
    }
}

/// The events a reader wants in a device's history: every change, and only the latest status read. Repeated
/// checks and collected output are left out, so the page stays a readable size with thousands of devices.
fn history(device: &ProjectDevice) -> (Vec<&super::project::Event>, usize) {
    let last_status = device.events.iter().rposition(|e| e.kind == "status");
    let kept: Vec<_> = device.events.iter().enumerate().filter(|(i, e)| !matches!(e.kind.as_str(), "poll" | "output" | "status") || Some(*i) == last_status).map(|(_, e)| e).collect();
    let skipped = kept.len().saturating_sub(HISTORY_SHOWN);
    (kept.into_iter().skip(skipped).collect(), skipped)
}

/// The most events listed per device, the latest kept.
const HISTORY_SHOWN: usize = 30;
/// The longest event text shown; longer text is cut with an ellipsis.
const DETAIL_SHOWN: usize = 400;

fn clip(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() { format!("{}…", head.trim_end()) } else { head }
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

fn percent(part: usize, whole: usize) -> f64 {
    if whole == 0 { 0.0 } else { part as f64 * 100.0 / whole as f64 }
}

const STYLE: &str = r#"
:root{color-scheme:dark;--ink:#e9eaee;--soft:#a4a8b4;--faint:#7e8392;--rule:#22242d;--rule-strong:#363947;--paper:#101116;--tint:#1c1e26;
--done:#4fae78;--moving:#7194dc;--attention:#e2754f;--idle:#787b86;--out:#3b3e4a}
*{box-sizing:border-box}
body{margin:0;background:var(--paper);color:var(--ink);font:14px/1.55 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,"Helvetica Neue",Arial,sans-serif;font-variant-numeric:tabular-nums;-webkit-text-size-adjust:100%}
main{max-width:1060px;margin:0 auto;padding:56px 40px 72px}
header{border-bottom:2px solid var(--ink);padding-bottom:26px}
.doc{margin:0 0 8px;color:var(--soft)}
h1{margin:0;font-size:30px;line-height:1.2;font-weight:650;letter-spacing:-.01em}
.facts{display:grid;grid-template-columns:repeat(auto-fit,minmax(210px,1fr));gap:16px 36px;margin:26px 0 0}
.facts dt{color:var(--soft);font-size:12.5px}
.facts dd{margin:2px 0 0;font-weight:500;overflow-wrap:anywhere}
section{margin-top:48px}
h2{font-size:18px;line-height:1.3;font-weight:650;margin:0 0 6px}
h3{font-size:15px;font-weight:650;margin:28px 0 4px}
.note{color:var(--soft);margin:0 0 16px;max-width:52em}
.lede{font-size:19px;line-height:1.5;margin:10px 0 22px;max-width:44em}
.bar{display:flex;height:12px;background:var(--tint)}
.bar i{display:block;height:100%}
.done{background:var(--done)}.moving{background:var(--moving)}.attention{background:var(--attention)}.idle{background:var(--idle)}.out{background:var(--out)}
.key{display:flex;flex-wrap:wrap;gap:12px 40px;margin:18px 0 0;padding:0;list-style:none}
.key li{min-width:120px}
.key b{display:block;font-size:24px;line-height:1.2;font-weight:600}
.key span{color:var(--soft);font-size:13px}
.key i,.st i{display:inline-block;width:9px;height:9px;margin-right:7px}
.st{white-space:nowrap}
.st i{border-radius:50%}
table{width:100%;border-collapse:collapse;font-size:13px}
thead{display:table-header-group}
table.fixed{table-layout:fixed}
table.fixed td.n{padding-right:14px}
th{text-align:left;font-weight:600;color:var(--soft);font-size:12.5px;border-bottom:1px solid var(--rule-strong);padding:8px 14px 8px 0;white-space:nowrap}
td{border-bottom:1px solid var(--rule);padding:8px 14px 8px 0;vertical-align:top;overflow-wrap:anywhere}
th:last-child,td:last-child{padding-right:0}
.n{text-align:right;white-space:nowrap}
.dim{color:var(--faint)}
.why{color:var(--soft)}
.prog{display:flex;align-items:center;gap:10px;white-space:nowrap}
.prog .bar{width:110px;height:6px}
a{color:inherit;text-decoration-color:var(--rule-strong);text-underline-offset:2px}
a:hover{text-decoration-color:currentColor}
.tools{display:flex;flex-wrap:wrap;align-items:center;gap:10px;margin:0 0 12px}
.tools input,.tools select{font:inherit;color:inherit;background:var(--paper);border:1px solid var(--rule-strong);border-radius:4px;padding:6px 10px}
.tools input{flex:1 1 260px;max-width:360px}
.tools span{color:var(--soft)}
details{border-bottom:1px solid var(--rule)}
details:target summary,details[open] summary{font-weight:600}
summary{display:flex;align-items:baseline;gap:12px;padding:8px 0;cursor:pointer;list-style:none}
summary::-webkit-details-marker{display:none}
summary::before{content:"";flex:none;width:0;height:0;border:4.5px solid transparent;border-left:6px solid var(--faint);transform:translateY(-1px);transition:transform .1s}
details[open] summary::before{transform:translate(-2px,1px) rotate(90deg)}
summary b{flex:1;font-weight:inherit;overflow-wrap:anywhere}
ol.events{list-style:none;margin:0 0 12px;padding:0 0 0 18px;border-left:2px solid var(--rule)}
ol.events li{display:grid;grid-template-columns:150px 80px 1fr;gap:12px;padding:3px 0 3px 12px;font-size:13px}
ol.events time{color:var(--soft);white-space:nowrap}
ol.events .k{color:var(--faint)}
ol.events .x{white-space:pre-wrap;overflow-wrap:anywhere}
footer{margin-top:56px;padding-top:14px;border-top:1px solid var(--rule);color:var(--faint);font-size:12.5px}
@media (max-width:700px){main{padding:32px 18px 48px}ol.events li{grid-template-columns:1fr}.hide-narrow{display:none}}
@media print{:root{color-scheme:light;--ink:#1c1d1f;--soft:#56595f;--faint:#878a90;--rule:#e4e2dd;--rule-strong:#c8c5be;--paper:#fff;--tint:#f4f3ef;--done:#2f7a4c;--moving:#3c62a3;--attention:#b5431f;--idle:#aaa69c;--out:#d9d6cf}
.bar i,.key i,.st i{-webkit-print-color-adjust:exact;print-color-adjust:exact}
main{padding:0;max-width:none}.tools{display:none}section{break-inside:auto}tr,details{break-inside:avoid}h2,h3{break-after:avoid}a{text-decoration:none}}
"#;

/// Filters the device table, and opens a device's history when a link to it is followed.
const SCRIPT: &str = r#"
(function(){
var q=document.getElementById('q'),f=document.getElementById('f'),out=document.getElementById('shown');
var rows=[].slice.call(document.querySelectorAll('#devices tbody tr'));
function run(){var t=q.value.trim().toLowerCase(),s=f.value,n=0;
rows.forEach(function(r){var ok=(!s||r.getAttribute('data-s')===s)&&(!t||r.getAttribute('data-t').indexOf(t)>=0);r.hidden=!ok;if(ok)n++;});
out.textContent=n===rows.length?'':n+' of '+rows.length+' shown';}
if(q&&f){q.addEventListener('input',run);f.addEventListener('change',run);}
function openTarget(){var id=decodeURIComponent(location.hash.slice(1)),el=id&&document.getElementById(id);if(el&&el.tagName==='DETAILS')el.open=true;}
window.addEventListener('hashchange',openTarget);openTarget();
})();
"#;

/// A self-contained page to read or send to a customer: where the move stands, by wave and by destination, what needs
/// someone and why, every device, and each device's history. It works offline, is dark like the app on screen, and
/// prints light.
pub fn html(project: &Project, filter: &Filter, ctx: &ReportContext) -> String {
    let e = html_escape;
    let local = |at: &str| DateTime::parse_from_rfc3339(at).map(|t| t.with_timezone(&ctx.offset));
    let when = |at: &str| local(at).map(|t| t.format("%-d %b %Y, %H:%M").to_string()).unwrap_or_else(|_| at.to_string());
    let day = |at: &str| local(at).map(|t| t.format("%-d %b %Y").to_string()).unwrap_or_else(|_| at.to_string());

    let mut devices: Vec<(usize, &ProjectDevice)> = selected(project, filter).enumerate().collect();
    devices.sort_by(|(_, a), (_, b)| (a.wave.is_none(), a.wave, a.name.to_lowercase()).cmp(&(b.wave.is_none(), b.wave, b.name.to_lowercase())));
    let count = |o: Outcome| devices.iter().filter(|(_, d)| outcome(d.stage) == o).count();
    let total = devices.len();
    let in_scope = total - count(Outcome::LeftOut);
    let (done, moving, attention, not_launched) = (count(Outcome::Done), count(Outcome::Moving), count(Outcome::Attention), count(Outcome::NotLaunched));
    let same = project.source_server_id == project.dest_server_id;
    let linked = project.dest_ecoverse != DestEcoverse::NotLinked;
    let waiting_eco = devices.iter().filter(|(_, d)| project.waiting_for_ecoverse(d)).count();

    let mut out = String::with_capacity(4096 + total * 1500);
    let title = format!("{}: agent migration report", project.name);
    let _ = write!(
        out,
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{}</title><style>{STYLE}</style></head><body><main>",
        e(&title)
    );

    // The heading and the facts a reader asks first: between which servers, how, and since when.
    let _ = write!(out, "<header><p class=\"doc\">Agent migration report</p><h1>{}</h1><dl class=\"facts\">", e(&project.name));
    let fact = |out: &mut String, label: &str, value: &str| {
        let _ = write!(out, "<div><dt>{}</dt><dd>{}</dd></div>", e(label), e(value));
    };
    if same {
        fact(&mut out, "Server", &ctx.source);
        fact(&mut out, "Kind", "Agents reinstalled on the same server");
    } else {
        fact(&mut out, "From", &ctx.source);
        fact(&mut out, "To", &ctx.dest);
    }
    fact(&mut out, "Delivery", if project.options.delivery == Delivery::RestOnly { "N-central agent only" } else { "Ecoverse where it reaches the device, N-central agent otherwise" });
    fact(&mut out, "Project started", &day(&project.created_at));
    let launches: Vec<&str> = project.waves.iter().filter_map(|w| w.launched_at.as_deref()).collect();
    if let (Some(first), Some(last)) = (launches.iter().min(), launches.iter().max()) {
        fact(&mut out, if first == last { "Launched" } else { "Launched between" }, &if first == last { day(first) } else { format!("{} and {}", day(first), day(last)) });
    }
    let generated = ctx.generated_at.with_timezone(&ctx.offset);
    fact(&mut out, "Report made", &format!("{} (UTC{})", generated.format("%-d %b %Y, %H:%M"), generated.format("%:z")));
    if let Some(w) = filter.wave {
        fact(&mut out, "Covers", &project.waves.iter().find(|x| x.number == w).map_or(format!("Wave {w}"), |x| x.label.clone()));
    }
    out.push_str("</dl></header>");

    // Summary: one sentence, one bar, the numbers under it.
    out.push_str("<section><h2>Where it stands</h2>");
    let moved_word = if same { "reinstalled and checking in" } else { "on the new server and checking in" };
    let lede = if in_scope == 0 {
        "No devices are in this project yet.".to_string()
    } else {
        let mut s = format!("{done} of {in_scope} devices ({:.0}%) are {moved_word}.", percent(done, in_scope).floor());
        if linked && waiting_eco > 0 {
            let _ = write!(s, " {} still waiting for Ecoverse to receive {} data.", if waiting_eco == 1 { "1 of them is".to_string() } else { format!("{waiting_eco} of them are") }, if waiting_eco == 1 { "its" } else { "their" });
        }
        if attention > 0 {
            let _ = write!(s, " {} {} attention.", attention, if attention == 1 { "needs" } else { "need" });
        }
        if moving > 0 {
            let _ = write!(s, " {} {} still moving.", moving, if moving == 1 { "is" } else { "are" });
        }
        if not_launched > 0 {
            let _ = write!(s, " {} {} launched yet.", not_launched, if not_launched == 1 { "hasn't been" } else { "haven't been" });
        }
        s
    };
    let _ = write!(out, "<p class=\"lede\">{}</p>", e(&lede));
    if total > 0 {
        out.push_str("<div class=\"bar\" role=\"img\" aria-label=\"Devices by outcome\">");
        for (o, label, class) in OUTCOMES {
            let n = count(o);
            if n > 0 {
                let _ = write!(out, "<i class=\"{class}\" style=\"width:{:.2}%\" title=\"{}: {n}\"></i>", percent(n, total), e(label));
            }
        }
        out.push_str("</div><ul class=\"key\">");
        for (o, label, class) in OUTCOMES {
            let n = count(o);
            if n > 0 || o != Outcome::LeftOut {
                let _ = write!(out, "<li><b>{n}</b><span><i class=\"{class}\"></i>{}</span></li>", e(label));
            }
        }
        out.push_str("</ul>");
    }
    out.push_str("</section>");

    // Waves.
    if !project.waves.is_empty() {
        out.push_str("<section><h2>Waves</h2><p class=\"note\">Devices are moved in waves, a few at a time.</p><table><thead><tr><th>Wave</th><th>Launched</th><th class=\"n\">Devices</th><th class=\"n\">Done</th><th class=\"n\">Moving</th><th class=\"n\">Need attention</th><th class=\"hide-narrow\">Progress</th></tr></thead><tbody>");
        for wave in &project.waves {
            let in_wave: Vec<&ProjectDevice> = devices.iter().map(|(_, d)| *d).filter(|d| d.wave == Some(wave.number)).collect();
            let n = |o: Outcome| in_wave.iter().filter(|d| outcome(d.stage) == o).count();
            let done = n(Outcome::Done);
            let _ = write!(
                out,
                "<tr><td>{}</td><td>{}</td><td class=\"n\">{}</td><td class=\"n\">{done}</td><td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"hide-narrow\"><span class=\"prog\"><span class=\"bar\"><i class=\"done\" style=\"width:{:.2}%\"></i></span>{:.0}%</span></td></tr>",
                e(&wave.label),
                wave.launched_at.as_deref().map_or("<span class=\"dim\">Not yet</span>".to_string(), |at| e(&day(at))),
                in_wave.len(),
                n(Outcome::Moving),
                n(Outcome::Attention),
                percent(done, in_wave.len()),
                percent(done, in_wave.len()).floor(),
            );
        }
        out.push_str("</tbody></table>");
        let unassigned = devices.iter().filter(|(_, d)| d.wave.is_none() && d.stage == Stage::Planned).count();
        if unassigned > 0 {
            let _ = write!(out, "<p class=\"note\" style=\"margin-top:10px\">{} not in a wave yet.</p>", e(&plural(unassigned, "planned device")));
        }
        out.push_str("</section>");
    }

    // Destinations: where the devices went, by customer or site.
    let mut by_dest: std::collections::BTreeMap<String, [usize; 4]> = std::collections::BTreeMap::new();
    for (_, d) in devices.iter().filter(|(_, d)| d.stage != Stage::Excluded) {
        let name = d.dest_name.clone().or_else(|| d.dest_org_unit_id.as_ref().map(|id| format!("ID {id}"))).unwrap_or_else(|| "No destination".into());
        let row = by_dest.entry(name).or_default();
        row[0] += 1;
        match outcome(d.stage) {
            Outcome::Done => row[1] += 1,
            Outcome::Moving => row[2] += 1,
            Outcome::Attention => row[3] += 1,
            _ => {}
        }
    }
    if !by_dest.is_empty() {
        let mut rows: Vec<_> = by_dest.into_iter().collect();
        rows.sort_by(|a, b| b.1[0].cmp(&a.1[0]).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
        let _ = write!(
            out,
            "<section><h2>By destination</h2><p class=\"note\">{} on the {} server.</p><table><thead><tr><th>Customer or site</th><th class=\"n\">Devices</th><th class=\"n\">Done</th><th class=\"n\">Moving</th><th class=\"n\">Need attention</th><th class=\"hide-narrow\">Progress</th></tr></thead><tbody>",
            if rows.len() == 1 { "1 customer or site".to_string() } else { format!("{} customers and sites", rows.len()) },
            if same { "same" } else { "new" }
        );
        for (name, [n, done, moving, attention]) in rows {
            let _ = write!(
                out,
                "<tr><td>{}</td><td class=\"n\">{n}</td><td class=\"n\">{done}</td><td class=\"n\">{moving}</td><td class=\"n\">{attention}</td><td class=\"hide-narrow\"><span class=\"prog\"><span class=\"bar\"><i class=\"done\" style=\"width:{:.2}%\"></i></span>{:.0}%</span></td></tr>",
                e(&name),
                percent(done, n),
                percent(done, n).floor()
            );
        }
        out.push_str("</tbody></table></section>");
    }

    // What needs someone, grouped by reason, oldest first in each.
    if attention > 0 {
        let _ = write!(out, "<section><h2>Needs attention</h2><p class=\"note\">{}, grouped by reason. The longest-waiting come first.</p>", e(&plural(attention, "device")));
        for stage in ALL_STAGES.into_iter().filter(|s| outcome(*s) == Outcome::Attention) {
            let mut group: Vec<&(usize, &ProjectDevice)> = devices.iter().filter(|(_, d)| d.stage == stage).collect();
            if group.is_empty() {
                continue;
            }
            group.sort_by(|a, b| a.1.entered_stage_at.cmp(&b.1.entered_stage_at));
            let _ = write!(
                out,
                "<h3>{} <span class=\"dim\">· {}</span></h3><p class=\"note\">{}</p><table class=\"fixed\"><colgroup><col style=\"width:19%\"><col style=\"width:19%\"><col style=\"width:10%\"><col style=\"width:16%\"><col style=\"width:8%\"><col></colgroup><thead><tr><th>Device</th><th>Customer</th><th>Wave</th><th>Since</th><th class=\"n\">Attempts</th><th>Reason</th></tr></thead><tbody>",
                e(stage_word(stage)),
                group.len(),
                e(attention_meaning(stage))
            );
            for (i, d) in group {
                let _ = write!(
                    out,
                    "<tr><td><a href=\"#d{i}\">{}</a></td><td>{}</td><td>{}</td><td class=\"st\">{}</td><td class=\"n\">{}</td><td class=\"why\">{}</td></tr>",
                    e(&d.name),
                    e(d.customer_name.as_deref().unwrap_or("")),
                    e(&wave_label(project, d.wave)),
                    e(&when(&d.entered_stage_at)),
                    d.attempts,
                    e(&clip(d.last_error.as_deref().unwrap_or(""), DETAIL_SHOWN)),
                );
            }
            out.push_str("</tbody></table>");
        }
        out.push_str("</section>");
    }

    // Every device, with a search box and a stage filter that work when the page is opened.
    if total > 0 {
        let _ = write!(
            out,
            "<section><h2>Every device</h2><p class=\"note\">{}. Select a name for its history.</p><div class=\"tools\"><input id=\"q\" type=\"search\" placeholder=\"Find a device, customer or site\" aria-label=\"Find a device, customer or site\"><select id=\"f\" aria-label=\"Stage\"><option value=\"\">Every stage</option>",
            e(&plural(total, "device"))
        );
        for stage in ALL_STAGES.into_iter().filter(|s| devices.iter().any(|(_, d)| d.stage == *s)) {
            let _ = write!(out, "<option value=\"{0}\">{0}</option>", e(stage_word(stage)));
        }
        out.push_str("</select><span id=\"shown\"></span></div><table id=\"devices\"><thead><tr><th>Device</th><th>Customer</th><th>Moved to</th><th>Wave</th><th class=\"hide-narrow\">Delivery</th><th>Stage</th><th class=\"hide-narrow\">Since</th><th class=\"n hide-narrow\">Attempts</th></tr></thead><tbody>");
        for (i, d) in &devices {
            let customer = d.customer_name.as_deref().unwrap_or("");
            let dest = d.dest_name.as_deref().unwrap_or("");
            let search = format!("{} {} {}", d.name, customer, dest).to_lowercase();
            let _ = write!(
                out,
                "<tr data-s=\"{}\" data-t=\"{}\"><td><a href=\"#d{i}\">{}</a></td><td>{}</td><td>{}</td><td>{}</td><td class=\"hide-narrow\">{}</td><td class=\"st\"><i class=\"{}\"></i>{}</td><td class=\"hide-narrow st\">{}</td><td class=\"n hide-narrow\">{}</td></tr>",
                e(stage_word(d.stage)),
                e(&search),
                e(&d.name),
                e(customer),
                e(dest),
                e(&wave_label(project, d.wave)),
                path_word(d.path),
                outcome_class(outcome(d.stage)),
                e(stage_word(d.stage)),
                e(&day(&d.entered_stage_at)),
                d.attempts,
            );
        }
        out.push_str("</tbody></table></section>");

        // Each device's history, folded until it is opened or linked to.
        out.push_str("<section><h2>Device history</h2><p class=\"note\">What happened to each device, latest last. Repeated checks are left out.</p>");
        for (i, d) in &devices {
            let (events, skipped) = history(d);
            let _ = write!(out, "<details id=\"d{i}\"><summary><b>{}</b><span class=\"st\"><i class=\"{}\"></i>{}</span></summary>", e(&d.name), outcome_class(outcome(d.stage)), e(stage_word(d.stage)));
            let mut ids = vec![format!("Source device ID {}", d.source_device_id)];
            if let Some(id) = &d.dest_device_id {
                ids.push(format!("new device ID {id}"));
            }
            if let Some(task) = &d.task_id {
                ids.push(format!("task {task}"));
            }
            let _ = write!(out, "<p class=\"note\" style=\"margin:4px 0 8px 20px\">{}</p><ol class=\"events\">", e(&ids.join(" · ")));
            if skipped > 0 {
                let _ = write!(out, "<li><span></span><span></span><span class=\"dim\">{} earlier not shown</span></li>", e(&plural(skipped, "event")));
            }
            for ev in events {
                let text = if ev.kind == "stage" { stage_event(&ev.detail) } else { ev.detail.clone() };
                let _ = write!(out, "<li><time>{}</time><span class=\"k\">{}</span><span class=\"x\">{}</span></li>", e(&when(&ev.at)), e(event_kind(&ev.kind)), e(&clip(&text, DETAIL_SHOWN)));
            }
            out.push_str("</ol></details>");
        }
        out.push_str("</section>");
    }

    let _ = write!(out, "<footer>Made by N-voke {} on {}. Device names and reasons are as the devices and servers reported them.</footer></main><script>{SCRIPT}</script></body></html>", env!("CARGO_PKG_VERSION"), e(&generated.format("%-d %b %Y at %H:%M").to_string()));
    out
}

fn wave_label(project: &Project, wave: Option<u32>) -> String {
    wave.map(|w| project.waves.iter().find(|x| x.number == w).map_or(format!("Wave {w}"), |x| x.label.clone())).unwrap_or_default()
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
            Format::Html => {
                let server = |id: &str| self.ncentral_server(id).map_or(id.to_string(), |s| if s.label == s.host || s.label.is_empty() { s.host } else { format!("{} ({})", s.label, s.host) });
                let ctx = ReportContext { source: server(&project.source_server_id), dest: server(&project.dest_server_id), generated_at: now, offset: chrono::Offset::fix(chrono::Local::now().offset()) };
                ("html", html(project, filter, &ctx))
            }
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

    fn ctx() -> ReportContext {
        ReportContext { source: "Old <one>".into(), dest: "New".into(), generated_at: now(), offset: FixedOffset::east_opt(0).unwrap() }
    }

    #[test]
    fn the_html_escapes_every_untrusted_value_and_summarises_the_outcome() {
        let mut p = project();
        p.devices[2].customer_name = Some("Bob's <b>shop</b>".into());
        p.devices[1].record(now(), "note", "<img src=x onerror=alert(1)>");
        p.waves[0].launched_at = Some("2026-10-20T12:00:00Z".into());
        let page = html(&p, &Filter::default(), &ctx());
        assert!(!page.contains("<script>alert") && page.contains("&lt;script&gt;alert(1)&lt;/script&gt;"), "{page}");
        assert!(!page.contains("<img") && !page.contains("<b>shop"), "device events and customer names are escaped");
        assert!(page.contains("Acme &amp; Sons &lt;move&gt;") && page.contains("Old &lt;one&gt;"));
        assert!(page.contains("1 of 3 devices (33%) are on the new server and checking in. 1 needs attention. 1 hasn&#39;t been launched yet."), "{page}");
        assert!(page.contains("<h2>Needs attention</h2>") && page.contains("<h3>Lost") && page.contains("&quot;still&quot;"));
        assert!(page.contains("<td>Pilot</td><td>20 Oct 2026</td><td class=\"n\">2</td><td class=\"n\">1</td><td class=\"n\">0</td><td class=\"n\">1</td>"), "wave table: {page}");
        assert!(page.contains("Report made</dt><dd>20 Oct 2026, 12:00 (UTC+00:00)"));
        assert!(page.starts_with("<!doctype html>") && page.ends_with("</html>"));
    }

    #[test]
    fn times_are_shown_in_the_readers_time_zone() {
        let page = html(&project(), &Filter::default(), &ReportContext { offset: FixedOffset::west_opt(5 * 3600).unwrap(), ..ctx() });
        assert!(page.contains("20 Oct 2026, 07:00 (UTC-05:00)"), "{page}");
    }

    #[test]
    fn a_device_history_reads_in_plain_words_and_leaves_out_repeated_checks() {
        let mut p = project();
        let d = &mut p.devices[0];
        d.record(now(), "stage", "AgentRemoved: the old agent stopped checking in");
        for i in 0..5 {
            d.record(now(), "poll", &format!("seen {i}"));
            d.record(now(), "status", &format!("status read {i}"));
        }
        d.record(now(), "output", "a long script output");
        let page = html(&p, &Filter::default(), &ctx());
        let history = &page[page.find("<details id=\"d0\">").unwrap()..];
        let history = &history[..history.find("</details>").unwrap()];
        assert!(history.contains("Moving: the old agent stopped checking in"), "{history}");
        assert!(history.contains("status read 4") && !history.contains("status read 3"), "only the latest status: {history}");
        assert!(!history.contains("seen 0") && !history.contains("script output"), "{history}");
        assert!(history.contains("new device ID 5001"));
    }

    #[test]
    fn a_long_history_keeps_the_latest_events() {
        let mut p = project();
        for i in 0..(HISTORY_SHOWN + 5) {
            p.devices[0].record(now(), "note", &format!("note {i}"));
        }
        let page = html(&p, &Filter::default(), &ctx());
        assert!(page.contains(&format!("note {}", HISTORY_SHOWN + 4)) && !page.contains("note 0<"));
        assert!(page.contains("6 events earlier not shown"), "the planned stage and 5 notes are left out");
    }

    #[test]
    fn devices_are_counted_by_destination() {
        let mut p = project();
        p.devices[0].dest_name = Some("Contoso HQ".into());
        p.devices[1].dest_name = Some("Contoso HQ".into());
        let page = html(&p, &Filter::default(), &ctx());
        assert!(page.contains("2 customers and sites on the new server"), "{page}");
        assert!(page.contains("<tr><td>Contoso HQ</td><td class=\"n\">2</td><td class=\"n\">1</td><td class=\"n\">0</td><td class=\"n\">1</td>"), "{page}");
        assert!(page.contains("<tr><td>ID 120</td><td class=\"n\">1</td>"), "a device planned before names were kept shows its ID");
    }

    #[test]
    fn an_all_clear_project_has_no_attention_section() {
        let mut p = project();
        p.devices[1].stage = Stage::Resolved;
        let page = html(&p, &Filter::default(), &ctx());
        assert!(!page.contains("<h2>Needs attention</h2>"));
    }

    #[test]
    fn a_reinstall_names_one_server() {
        let mut p = project();
        p.dest_server_id = p.source_server_id.clone();
        let page = html(&p, &Filter::default(), &ctx());
        assert!(page.contains("<dt>Server</dt>") && !page.contains("<dt>To</dt>"));
        assert!(page.contains("are reinstalled and checking in"));
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
        assert!(std::fs::read_to_string(html_path).unwrap().contains("<h1>Acme &amp; Sons &lt;move&gt;</h1>"));
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod sample_page {
    use super::*;
    use crate::migration::{planner::PlannedDevice, project::DestEcoverse};

    /// Writes a realistic page to look at: `NVOKE_SAMPLE_REPORT=/path.html cargo test sample_page -- --ignored`.
    #[test]
    #[ignore]
    fn write_a_sample() {
        let t = |h: i64| chrono::DateTime::parse_from_rfc3339("2026-10-14T09:00:00Z").unwrap().with_timezone(&Utc) + chrono::Duration::hours(h);
        let mut p = Project::new("m".into(), "Northwind Traders to new N-central", "src", "dst", DestEcoverse::SameTenant, t(0));
        let customers = ["Northwind HQ", "Northwind Warehouse", "Fabrikam Dental", "Contoso Legal", "Tailspin Toys"];
        let planned: Vec<PlannedDevice> = (0..60)
            .map(|i| PlannedDevice {
                device_id: (1000 + i).to_string(),
                name: format!("{}-{:03}", ["NW-DESK", "NW-LAP", "FAB-PC", "CON-WS", "TT-POS"][i % 5], i),
                source_customer_id: Some((i % 5).to_string()),
                source_site_id: None,
                dest_org_unit_id: Some((500 + i % 5).to_string()),
                path: Some(if i % 3 == 0 { DeliveryPath::Rest } else { DeliveryPath::Ecoverse }),
                asset_id: None,
                blockers: vec![],
                warnings: vec![],
                excluded: i == 59,
            })
            .collect();
        p.add_planned(&planned, t(1));
        for (i, d) in p.devices.iter_mut().enumerate() {
            d.customer_name = Some(customers[i % 5].into());
            d.dest_name = Some(customers[i % 5].into());
        }
        let ids = |r: std::ops::Range<usize>| r.map(|i| (1000 + i).to_string()).collect::<Vec<_>>();
        p.create_wave("Pilot", &ids(0..5), t(2)).unwrap();
        p.create_wave("Wave 2", &ids(5..30), t(20)).unwrap();
        p.create_wave("Wave 3", &ids(30..50), t(40)).unwrap();
        p.waves[0].launched_at = Some(t(3).to_rfc3339());
        p.waves[1].launched_at = Some(t(26).to_rfc3339());
        let stages = |i: usize| match i {
            0..=4 => Stage::CleanedUp,
            5..=24 => Stage::Healthy,
            25 => Stage::Lost,
            26 => Stage::NotPickedUp,
            27 => Stage::LaunchFailed,
            28 => Stage::AgentRemoved,
            29 => Stage::Registered,
            59 => Stage::Excluded,
            _ => Stage::Planned,
        };
        for (i, d) in p.devices.iter_mut().enumerate() {
            let s = stages(i);
            if s == Stage::Planned || s == Stage::Excluded {
                continue;
            }
            d.record(t(26), "path", "Ecoverse: launched through Ecoverse");
            d.record(t(26), "stage", "Launched");
            d.record(t(26), "launch", "task accepted");
            for k in 0..4 {
                d.record(t(27), "poll", &format!("check {k}"));
            }
            if matches!(s, Stage::Healthy | Stage::CleanedUp | Stage::AgentRemoved | Stage::Registered | Stage::Lost) {
                d.record(t(27), "stage", "Staged");
                d.record(t(27), "stage", "AgentRemoved: the old agent stopped checking in");
            }
            if matches!(s, Stage::Healthy | Stage::CleanedUp) {
                d.record(t(28), "stage", "Registered");
                d.record(t(28), "stage", "Healthy: seen on the new server");
                d.dest_device_id = Some((88000 + i).to_string());
                d.ecoverse = Some(if i % 4 == 0 { crate::migration::project::EcoState::WaitingForData } else { crate::migration::project::EcoState::Reporting });
            }
            if s == Stage::CleanedUp {
                d.record(t(60), "stage", "CleanedUp");
            }
            d.record(t(29), "status", "Status via N-central: Last log: done\nstatus.json: {\"step\":\"healthy\"}");
            d.stage = s;
            d.entered_stage_at = t(28).to_rfc3339();
            d.attempts = if s == Stage::NotPickedUp { 2 } else { 1 };
            d.task_id = Some(format!("{}", 7700 + i));
        }
        p.devices[25].last_error = Some("The old agent was removed 3 hours ago and nothing appeared on the new server.".into());
        p.devices[27].last_error = Some("N-central refused the task: the device has no Windows Agent.".into());
        let ctx = ReportContext { source: "Northwind old (n1.example.com)".into(), dest: "Northwind new (n2.example.com)".into(), generated_at: t(80), offset: FixedOffset::west_opt(4 * 3600).unwrap() };
        std::fs::write(std::env::var("NVOKE_SAMPLE_REPORT").unwrap(), html(&p, &Filter::default(), &ctx)).unwrap();
    }
}
