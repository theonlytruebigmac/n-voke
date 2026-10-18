//! Drawing the Run page: the form, the devices it will run on, and the picker popup.

use nvoke_core::{
    model::Asset,
    run::{Resolved, is_offline},
    targets::os_mismatch,
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap},
};

use crate::{
    app::{App, Load, NoticeKind, RunField, RunPane},
    picker::{PickItem, PickKind},
    theme::{self, ACCENT, BORDER, DIM, ERR, OK, SELECTED_BG, TEXT, WARN, panel},
    ui::{centered, clear, spinner, truncate},
};

const LABEL_WIDTH: usize = 14;

pub fn draw_run(frame: &mut Frame, area: Rect, app: &mut App, focused: bool) {
    let [form_area, devices_area] = if area.width >= 84 {
        Layout::horizontal([Constraint::Percentage(42), Constraint::Fill(1)]).areas(area)
    } else {
        Layout::vertical([Constraint::Percentage(55), Constraint::Fill(1)]).areas(area)
    };
    draw_form(frame, form_area, app, focused && app.run.pane == RunPane::Form);
    draw_devices(frame, devices_area, app, focused && app.run.pane == RunPane::Devices);
}

/// Humanizes an organization type: "ServiceOrganization" -> "Service organization".
fn org_kind(typename: &str) -> String {
    let mut out = String::new();
    for (i, c) in typename.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push(' ');
            out.extend(c.to_lowercase());
        } else if i == 0 {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
    }
    out
}

pub fn language_name(language: &str) -> String {
    match language {
        "POWERSHELL" => "PowerShell".into(),
        "BATCH" => "Batch".into(),
        "BASH" => "Bash".into(),
        "PYTHON" => "Python".into(),
        "" => String::new(),
        other => {
            let lower = other.to_lowercase().replace('_', " ");
            let mut chars = lower.chars();
            chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
        }
    }
}

pub fn agent_glyph(asset: &Asset) -> (&'static str, ratatui::style::Color) {
    match asset.agent_connection.as_ref().map(|c| c.status.as_str()) {
        Some("CONNECTED") => ("●", OK),
        Some(_) => ("○", WARN),
        None => ("·", DIM),
    }
}

// ---- the form ----------------------------------------------------------------

fn draw_form(frame: &mut Frame, area: Rect, app: &mut App, focused: bool) {
    // Whether a run can start, always visible in the panel's own border, because on a short window
    // the Run button itself can be scrolled out of sight.
    let status = {
        let run = &app.run;
        let devices = run.devices();
        let count = if devices.len() == 1 { devices[0].name.clone() } else { format!("{} devices", devices.len()) };
        if run.running {
            theme::footer(vec![Span::styled("starting…", Style::new().fg(ACCENT))])
        } else if run.confirming {
            theme::footer(vec![Span::styled(format!("enter again to run on {count}"), Style::new().fg(WARN).add_modifier(Modifier::BOLD))])
        } else if let Some(reason) = run.blocker() {
            theme::footer(vec![Span::styled(reason, Style::new().fg(DIM))])
        } else {
            theme::footer(vec![Span::styled(format!("ready · run on {count}"), Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))])
        }
    };
    let block = panel("run a script", focused).title_bottom(status);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = inner.width as usize;
    let value_width = width.saturating_sub(LABEL_WIDTH + 4);
    let tick = app.tick;
    let run = &app.run;
    let problems = if run.show_errors { run.problems() } else { Default::default() };

    let mut lines: Vec<Line> = vec![];
    let mut focus_line = 0usize;

    // One row: label, value, and the focus marker. Returns nothing; tracks where focus is.
    let mut row = |lines: &mut Vec<Line<'static>>, field: RunField, label: &str, value: Vec<Span<'static>>| {
        let is_focus = run.field == field;
        let marker = if is_focus && focused { "▌ " } else { "  " };
        let mut spans = vec![
            Span::styled(marker, Style::new().fg(TEXT)),
            // A row without a label (the Run button) starts flush left.
            Span::styled(format!("{:<w$}", truncate(label, LABEL_WIDTH - 1), w = if label.is_empty() { 0 } else { LABEL_WIDTH }), Style::new().fg(if is_focus { ACCENT } else { DIM })),
        ];
        spans.extend(value);
        let mut line = Line::from(spans);
        if is_focus && focused {
            // Pad to the full width so the highlight is a bar, not just under the text.
            let used: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            line.spans.push(Span::raw(" ".repeat(width.saturating_sub(used))));
            line = line.style(Style::new().bg(SELECTED_BG).add_modifier(Modifier::BOLD));
        }
        if is_focus {
            focus_line = lines.len();
        }
        lines.push(line);
    };
    let placeholder = |text: &str| vec![Span::styled(text.to_string(), Style::new().fg(DIM))];
    let cursor = |editing: bool| if editing && theme::cursor_visible(tick) { "▏" } else if editing { " " } else { "" };

    // Organization / script / targets.
    row(&mut lines, RunField::Org, "Organization", match &run.org {
        Some(o) => vec![Span::styled(truncate(&o.name, value_width), Style::new().fg(TEXT)), Span::styled(" ▾", Style::new().fg(DIM))],
        None => placeholder("choose an organization ▾"),
    });
    if run.org.is_some() {
        row(&mut lines, RunField::Script, "Script", match &run.script {
            Some(s) => vec![
                Span::styled(truncate(&s.name, value_width.saturating_sub(12)), Style::new().fg(TEXT)),
                Span::styled(format!("  {}", language_name(&s.language)), Style::new().fg(DIM)),
                Span::styled(" ▾", Style::new().fg(DIM)),
            ],
            None => placeholder("choose a script ▾"),
        });
        row(&mut lines, RunField::Targets, "Run on", if run.targets.is_empty() {
            placeholder("add devices or tags ▾")
        } else {
            let hint = if run.field == RunField::Targets && focused { "   c clears" } else { "" };
            vec![Span::styled(truncate(&run.label(), value_width.saturating_sub(12)), Style::new().fg(TEXT)), Span::styled(format!(" ▾{hint}"), Style::new().fg(DIM))]
        });
    }

    // The script's inputs.
    if let Some(script) = &run.script {
        let decls = script.inputs.as_deref().unwrap_or(&[]);
        if !decls.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::styled("  Inputs", Style::new().fg(DIM).add_modifier(Modifier::BOLD)));
        }
        for (i, decl) in decls.iter().enumerate() {
            let field = RunField::Input(i);
            let editing = run.editing && run.field == field;
            let error = problems.inputs.get(&decl.variable).cloned();
            let mut value: Vec<Span<'static>> = match run.values.get(&decl.variable) {
                Some(nvoke_core::run::FieldValue::Bool(on)) => vec![Span::styled(if *on { "[x] yes" } else { "[ ] no" }.to_string(), Style::new().fg(TEXT))],
                Some(nvoke_core::run::FieldValue::Text(t)) if !t.is_empty() => {
                    let shown = if decl.kind == "PASSWORD" { "•".repeat(t.chars().count()) } else { t.clone() };
                    let shown: String = shown.chars().rev().take(value_width.saturating_sub(2)).collect::<Vec<_>>().into_iter().rev().collect();
                    vec![Span::styled(shown, Style::new().fg(TEXT))]
                }
                _ if decl.kind == "BOOLEAN" => vec![Span::styled("[ ] no".to_string(), Style::new().fg(DIM))],
                _ => placeholder(if editing { "" } else { "optional" }),
            };
            if editing {
                value.push(Span::styled(cursor(true).to_string(), Style::new().fg(ACCENT)));
            }
            row(&mut lines, field, &decl.name, value);
            // The reason goes on its own line, so a narrow form never wraps it mid-sentence.
            if let Some(e) = error {
                push_problem(&mut lines, &format!("✗ {e}"), width);
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::styled("  Options", Style::new().fg(DIM).add_modifier(Modifier::BOLD)));
        let editing_name = run.editing && run.field == RunField::Name;
        row(&mut lines, RunField::Name, "Task name", if run.task_name.value.is_empty() && !editing_name {
            placeholder("auto-named")
        } else {
            vec![Span::styled(run.task_name.value.clone(), Style::new().fg(TEXT)), Span::styled(cursor(editing_name).to_string(), Style::new().fg(ACCENT))]
        });
        let editing_timeout = run.editing && run.field == RunField::Timeout;
        let default_timeout = script.timeout_seconds.filter(|t| *t > 0).map_or("the maximum, 3600s".to_string(), |t| format!("{t}s"));
        let timeout = if run.timeout.value.is_empty() && !editing_timeout {
            placeholder(&format!("{default_timeout} (the script's own)"))
        } else {
            vec![Span::styled(format!("{}s", run.timeout.value), Style::new().fg(TEXT)), Span::styled(cursor(editing_timeout).to_string(), Style::new().fg(ACCENT))]
        };
        row(&mut lines, RunField::Timeout, "Timeout", timeout);
        if let Some(e) = problems.timeout {
            push_problem(&mut lines, &format!("✗ use {e} seconds"), width);
        }
    }

    // The run button.
    lines.push(Line::from(""));
    let devices = run.devices();
    let count = if devices.len() == 1 { devices[0].name.clone() } else { format!("{} devices", devices.len()) };
    let (text, style) = if run.running {
        (format!("{} Starting…", spinner(tick)), Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))
    } else if run.confirming {
        // How many devices comes first, so it can never be the part that gets cut off.
        (format!("▶ Confirm: run on {count}"), Style::new().fg(WARN).add_modifier(Modifier::BOLD))
    } else if let Some(reason) = run.blocker() {
        (format!("▶ Run  ·  {reason}"), Style::new().fg(DIM))
    } else {
        (format!("▶ Run on {count}"), Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))
    };
    row(&mut lines, RunField::Run, "", vec![Span::styled(text, style)]);
    if run.confirming {
        lines.push(Line::styled(format!("  {}", run.script.as_ref().map_or("", |s| s.name.as_str())), Style::new().fg(TEXT)));
        // Anything in the list that looks risky is repeated here, where the decision is made.
        let language = run.script.as_ref().map(|s| s.language.clone());
        let risks = RunRisks::of(&devices, language.as_deref());
        for warning in risks.warnings() {
            push_wrapped(&mut lines, &format!("⚠ {warning}"), width, 2, Style::new().fg(WARN));
        }
        lines.push(Line::styled("  enter again to run · esc to cancel", Style::new().fg(DIM)));
    }
    if app.is_dry_run() {
        push_wrapped(&mut lines, "dry run: it will be held back, not sent", width, 2, Style::new().fg(WARN));
    }
    if let Some(notice) = &run.notice {
        lines.push(Line::from(""));
        let color = if notice.kind == NoticeKind::Error { ERR } else { ACCENT };
        push_wrapped(&mut lines, &notice.text, width, 2, Style::new().fg(color));
    }

    // Keep the focused row on screen when the form is taller than its panel.
    let height = inner.height as usize;
    let scroll = (focus_line + 3).saturating_sub(height) as u16;
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)).wrap(Wrap { trim: false }), inner);
}

/// A validation message under its field, wrapped so every line stays under the value column.
fn push_problem(lines: &mut Vec<Line<'static>>, text: &str, width: usize) {
    push_wrapped(lines, text, width, 2 + LABEL_WIDTH, Style::new().fg(ERR));
}

/// Prose in the form: wrapped at words, every line indented, so nothing spills over the border.
fn push_wrapped(lines: &mut Vec<Line<'static>>, text: &str, width: usize, indent: usize, style: Style) {
    for (i, piece) in crate::output_view::wrap_words(text, width.saturating_sub(indent + 2).max(8)).into_iter().enumerate() {
        // Continuation lines line up under the words, not under a leading mark.
        let lead = if i == 0 { indent } else { indent + 2 };
        lines.push(Line::styled(format!("{}{piece}", " ".repeat(lead)), style));
    }
}

/// What looks risky about the devices a run is about to hit.
#[derive(Debug, PartialEq, Eq)]
pub struct RunRisks {
    pub offline: usize,
    pub wrong_os: usize,
}

impl RunRisks {
    pub fn of(devices: &[Asset], script_language: Option<&str>) -> Self {
        Self {
            offline: devices.iter().filter(|d| is_offline(d)).count(),
            wrong_os: devices.iter().filter(|d| os_mismatch(script_language, d.os_name()).is_some()).count(),
        }
    }

    pub fn warnings(&self) -> Vec<String> {
        let plural = |n: usize| if n == 1 { "device is" } else { "devices are" };
        let mut out = vec![];
        if self.offline > 0 {
            out.push(format!("{} {} offline and may not run it until they reconnect", self.offline, plural(self.offline)));
        }
        if self.wrong_os > 0 {
            out.push(format!("{} {} on an OS this script may not support (OS?)", self.wrong_os, plural(self.wrong_os)));
        }
        out
    }
}

/// The empty devices panel: the steps of a run, ticked off as the form is filled in, with the next one pointed at.
fn draw_steps(frame: &mut Frame, area: Rect, app: &App) {
    let run = &app.run;
    let has_inputs = run.script.as_ref().is_some_and(|s| s.inputs.as_ref().is_some_and(|i| !i.is_empty()));
    let mut steps: Vec<(bool, &str)> = vec![(run.org.is_some(), "Choose an organization"), (run.script.is_some(), "Choose a script"), (!run.targets.is_empty(), "Choose devices or tags to run on")];
    if has_inputs {
        steps.push((run.script.is_some() && !run.problems().any() && !run.values.is_empty(), "Fill in the script's inputs (optional)"));
    }
    steps.push((false, "Review the devices that will run, leaving any out"));
    steps.push((false, "Press enter twice to confirm and run"));
    let next = steps.iter().position(|(done, _)| !done);
    let mut lines = vec![Line::from(""), Line::styled("How a run works", Style::new().fg(TEXT).add_modifier(Modifier::BOLD)), Line::from("")];
    for (i, (done, text)) in steps.iter().enumerate() {
        let (mark, style) = if *done {
            ("✓", Style::new().fg(OK))
        } else if Some(i) == next {
            ("→", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))
        } else {
            ("○", Style::new().fg(DIM))
        };
        lines.push(Line::from(vec![Span::styled(format!("   {mark} "), style), Span::styled((*text).to_string(), if Some(i) == next { Style::new().fg(TEXT).add_modifier(Modifier::BOLD) } else { Style::new().fg(if *done { DIM } else { TEXT }) })]));
    }
    lines.push(Line::from(""));
    lines.push(Line::styled("   The devices appear here as soon as you choose who to run on.", Style::new().fg(DIM)));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(panel("devices", false)), area);
}

// ---- the devices -----------------------------------------------------------------

fn draw_devices(frame: &mut Frame, area: Rect, app: &mut App, focused: bool) {
    let block = panel("devices", focused);
    let message = |frame: &mut Frame, text: String, color| frame.render_widget(Paragraph::new(text).style(Style::new().fg(color)).alignment(Alignment::Center).wrap(Wrap { trim: true }).block(panel("devices", focused)), area);

    if app.run.targets.is_empty() {
        return draw_steps(frame, area, app);
    }
    let list: Vec<Resolved> = match &app.run.resolved {
        Load::Ready(list) => list.clone(),
        Load::Failed(e) => return message(frame, format!("\n{e}"), ERR),
        _ => return message(frame, format!("\n{} Finding devices…", spinner(app.tick)), ACCENT),
    };
    if list.is_empty() {
        return message(frame, "\nNo devices in these tags.".into(), DIM);
    }

    let script_language = app.run.script.as_ref().map(|s| s.language.clone());
    let show_source = app.run.targets.iter().any(|t| matches!(t, nvoke_core::targets::Target::Tag(_)));
    let included = list.iter().filter(|r| !app.run.excluded.contains(&r.asset.id)).count();
    let offline = list.iter().filter(|r| !app.run.excluded.contains(&r.asset.id) && is_offline(&r.asset)).count();

    // Each row's text first, so the columns can be sized to what is actually there.
    struct Shown {
        off: bool,
        glyph: (&'static str, ratatui::style::Color),
        name: String,
        flags: Vec<&'static str>,
        location: String,
        source: String,
    }
    let shown: Vec<Shown> = list
        .iter()
        .map(|r| {
            let mut flags = vec![];
            if is_offline(&r.asset) {
                flags.push("offline");
            }
            if os_mismatch(script_language.as_deref(), r.asset.os_name()).is_some() {
                flags.push("OS?");
            }
            Shown {
                off: app.run.excluded.contains(&r.asset.id),
                glyph: agent_glyph(&r.asset),
                name: r.asset.name.clone(),
                flags,
                location: [r.asset.customer.as_ref().map(|c| c.name.as_str()), r.asset.site.as_ref().map(|s| s.name.as_str())].into_iter().flatten().collect::<Vec<_>>().join(" · "),
                source: r.source(),
            }
        })
        .collect();
    let name_text = |s: &Shown| 2 + s.name.chars().count() + s.flags.iter().map(|f| 1 + f.chars().count()).sum::<usize>();
    let widest = |f: &dyn Fn(&Shown) -> usize| shown.iter().map(f).max().unwrap_or(0);

    // Columns in order of importance: the device, where it came from (when tags are involved), then its location.
    // Each gets what it needs if there is room; the last one included takes the rest.
    let inner_width = area.width.saturating_sub(2) as usize;
    let gap = 2;
    let mut budget = inner_width.saturating_sub(2 /* highlight marker */ + 3 /* tick box */ + gap);
    let name_w = widest(&name_text).clamp(8, 40).min(budget);
    budget = budget.saturating_sub(name_w);
    let mut want_source = show_source;
    let mut source_w = if want_source { widest(&|s| s.source.chars().count()).clamp(6, 36) } else { 0 };
    let mut want_location = true;
    let location_w = widest(&|s| s.location.chars().count()).clamp(6, 26);
    if want_source && budget < gap + 10 {
        want_source = false;
        source_w = 0;
    }
    if want_source {
        source_w = source_w.min(budget.saturating_sub(gap));
        budget = budget.saturating_sub(gap + source_w);
    }
    if budget < gap + 8 {
        want_location = false;
    }

    let mut widths = vec![Constraint::Length(3), Constraint::Length(name_w as u16)];
    let mut headers = vec!["", "Device"];
    if want_source {
        widths.push(Constraint::Length(source_w as u16));
        headers.push("From");
    }
    if want_location {
        widths.push(Constraint::Fill(1));
        headers.push("Location");
    }
    let location_room = budget.saturating_sub(gap);

    let rows: Vec<Row> = shown
        .iter()
        .map(|s| {
            let base = if s.off { Style::new().fg(DIM) } else { Style::new().fg(TEXT) };
            let mut name = vec![Span::styled(s.glyph.0.to_string(), Style::new().fg(s.glyph.1)), Span::styled(format!(" {}", truncate(&s.name, name_w.saturating_sub(2 + s.flags.iter().map(|f| 1 + f.len()).sum::<usize>()))), base)];
            for flag in &s.flags {
                name.push(Span::styled(format!(" {flag}"), Style::new().fg(WARN)));
            }
            let mut cells = vec![Cell::from(if s.off { "[ ]" } else { "[x]" }).style(if s.off { Style::new().fg(DIM) } else { Style::new().fg(OK) }), Cell::from(Line::from(name))];
            if want_source {
                cells.push(Cell::from(truncate(&s.source, source_w)).style(Style::new().fg(DIM)));
            }
            if want_location {
                cells.push(Cell::from(truncate(&s.location, location_room.min(location_w.max(location_room)))).style(Style::new().fg(DIM)));
            }
            Row::new(cells)
        })
        .collect();

    let included_devices: Vec<Asset> = list.iter().filter(|r| !app.run.excluded.contains(&r.asset.id)).map(|r| r.asset.clone()).collect();
    let mismatched = RunRisks::of(&included_devices, script_language.as_deref()).wrong_os;
    // What matters first: the things that look risky, then how many are selected.
    let mut parts: Vec<Span<'static>> = vec![];
    let mut add = |text: String, style: Style| {
        if !parts.is_empty() {
            parts.push(Span::styled(" · ", Style::new().fg(DIM)));
        }
        parts.push(Span::styled(text, style));
    };
    if offline > 0 {
        add(format!("{offline} offline"), Style::new().fg(WARN));
    }
    if mismatched > 0 {
        add(format!("{mismatched} OS?"), Style::new().fg(WARN));
    }
    add(format!("{included} of {} selected", list.len()), Style::new().fg(DIM));
    let title_bottom = theme::footer(parts);
    let table = Table::new(rows, widths)
        .column_spacing(2)
        .header(Row::new(headers).style(Style::new().fg(DIM).add_modifier(Modifier::BOLD)))
        // The highlight marks where you are, so only the focused panel has one.
        .row_highlight_style(if focused { Style::new().bg(SELECTED_BG).add_modifier(Modifier::BOLD) } else { Style::new() })
        .highlight_symbol(if focused { "▌ " } else { "  " })
        .block(block.title_bottom(title_bottom));
    let table_area = if app.run.truncated {
        let [warning, rest] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
        frame.render_widget(Paragraph::new(format!(" Only the first {} devices of a tag are included.", nvoke_core::api::MAX_RESOLVED_DEVICES)).style(Style::new().fg(WARN)), warning);
        rest
    } else {
        area
    };
    frame.render_stateful_widget(table, table_area, &mut app.run.devices_table);
}

// ---- the picker -----------------------------------------------------------------

pub fn draw_picker(frame: &mut Frame, area: Rect, app: &mut App) {
    let tick = app.tick;
    let chosen = app.picker_chosen();
    let Some(picker) = app.picker.as_mut() else { return };
    // Tall enough for what is listed (plus the search line, status line and borders), never more than fits.
    let wanted = (picker.items.len().max(3) + 6).clamp(10, 25) as u16;
    let popup = centered(area, area.width.saturating_sub(6).min(86), area.height.saturating_sub(2).min(wanted));
    clear(frame, popup);

    let hints = match picker.kind {
        PickKind::Org if picker.purpose == crate::picker::Purpose::TagAccess => " ↑↓ move · enter choose · tab organization/customer/site · esc cancel ",
        PickKind::Targets => " ↑↓ move · enter add or remove · tab devices ⇄ tags · esc done ",
        PickKind::Devices => " ↑↓ move · enter add or remove · esc done ",
        PickKind::StaticTags => " ↑↓ move · enter choose · esc cancel ",
        _ => " ↑↓ move · enter choose · esc cancel ",
    };
    let mut title = vec![Span::styled(picker.title.clone(), Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))];
    if let Some(current) = picker.category {
        let (on, off) = (Style::new().fg(ACCENT).add_modifier(Modifier::BOLD), Style::new().fg(DIM));
        title.push(Span::raw("  "));
        for (i, c) in nvoke_core::collections::AccessCategory::ALL.iter().enumerate() {
            if i > 0 {
                title.push(Span::styled(" · ", off));
            }
            title.push(Span::styled(c.label(), if *c == current { on } else { off }));
        }
    }
    if picker.kind == PickKind::Targets {
        let (on, off) = (Style::new().fg(ACCENT).add_modifier(Modifier::BOLD), Style::new().fg(DIM));
        title.push(Span::raw("  "));
        title.push(Span::styled("Devices", if picker.tags_mode { off } else { on }));
        title.push(Span::styled(" · ", off));
        title.push(Span::styled("Tags", if picker.tags_mode { on } else { off }));
    }
    let outer = Block::bordered()
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(Line::from([vec![Span::styled("┤ ", Style::new().fg(ACCENT))], title, vec![Span::styled(" ├", Style::new().fg(ACCENT))]].concat()))
        .title_bottom(theme::footer_text(hints));
    let inner = outer.inner(popup);
    frame.render_widget(outer, popup);

    // A failure gets room to be read in full, with what to do about it.
    let denied = picker.error.as_deref().is_some_and(nvoke_core::api::is_access_denied);
    let status_rows = if picker.error.is_some() { if denied { 5 } else { 3 } } else { 1 };
    let [search_area, status_area, rule_area, list_area] = Layout::vertical([Constraint::Length(1), Constraint::Length(status_rows), Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
    let blink = if theme::cursor_visible(tick) { "▏" } else { " " };
    frame.render_widget(Paragraph::new(Line::from(vec![Span::styled(" › ", Style::new().fg(ACCENT)), Span::styled(picker.term().to_string(), Style::new().fg(TEXT)), Span::styled(blink, Style::new().fg(ACCENT))])), search_area);

    if let Some(e) = &picker.error {
        let mut lines = vec![Line::styled(format!(" {e}"), Style::new().fg(ERR))];
        if denied {
            lines.push(Line::from(""));
            lines.push(Line::styled(" To fix it: press esc, then ← to the menu, 5 for Settings, enter, and paste a new token.", Style::new().fg(WARN)));
        }
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), status_area);
    } else {
        let status = if picker.loading {
            Span::styled(format!(" {} searching…", spinner(tick)), Style::new().fg(ACCENT))
        } else {
            let n = picker.items.len();
            Span::styled(format!(" {n} {}", if n == 1 { "match" } else { "matches" }), Style::new().fg(DIM))
        };
        frame.render_widget(Paragraph::new(Line::from(status)), status_area);
    }
    frame.render_widget(Block::new().borders(Borders::TOP).border_style(Style::new().fg(BORDER)), rule_area);

    if picker.items.is_empty() {
        let hint = if picker.loading || picker.error.is_some() {
            ""
        } else if picker.kind == PickKind::Targets && !picker.tags_mode && picker.term().is_empty() {
            "Type to search devices, or press tab to pick a tag."
        } else {
            "Nothing matches."
        };
        frame.render_widget(Paragraph::new(hint).style(Style::new().fg(DIM)).alignment(Alignment::Center), list_area);
        return;
    }

    let wide = list_area.width as usize;
    let rows: Vec<Row> = picker.items.iter().map(|item| pick_row(item, &chosen, wide)).collect();
    let table = Table::new(rows, [Constraint::Length(2), Constraint::Fill(3), Constraint::Fill(2)])
        .column_spacing(2)
        .row_highlight_style(Style::new().bg(SELECTED_BG).add_modifier(Modifier::BOLD))
        .highlight_symbol("▌");
    frame.render_stateful_widget(table, list_area, &mut picker.table);
}

fn pick_row(item: &PickItem, chosen: &std::collections::HashSet<String>, width: usize) -> Row<'static> {
    let mark = if chosen.contains(&item.key()) { Span::styled("✓", Style::new().fg(OK).add_modifier(Modifier::BOLD)) } else { Span::raw(" ") };
    let name_width = width.saturating_sub(8) * 3 / 5;
    let detail_width = width.saturating_sub(8) * 2 / 5;
    match item {
        PickItem::Org(o) => Row::new(vec![Cell::from(Line::from(mark)), Cell::from(truncate(&o.name, name_width)).style(Style::new().fg(TEXT)), Cell::from(org_kind(&o.typename)).style(Style::new().fg(DIM))]),
        PickItem::Script(s) => {
            let detail = match s.description.as_deref().filter(|d| !d.is_empty() && *d != s.name) {
                Some(d) => format!("{} · {d}", language_name(&s.language)),
                None => language_name(&s.language),
            };
            Row::new(vec![Cell::from(""), Cell::from(truncate(&s.name, name_width)).style(Style::new().fg(TEXT)), Cell::from(truncate(&detail, detail_width)).style(Style::new().fg(DIM))])
        }
        PickItem::Asset(a) => {
            let (glyph, color) = agent_glyph(a);
            Row::new(vec![
                Cell::from(Line::from(mark)),
                Cell::from(Line::from(vec![Span::styled(glyph.to_string(), Style::new().fg(color)), Span::styled(format!(" {}", truncate(&a.name, name_width.saturating_sub(2))), Style::new().fg(TEXT))])),
                Cell::from(truncate(a.customer.as_ref().map_or("", |c| c.name.as_str()), detail_width)).style(Style::new().fg(DIM)),
            ])
        }
        PickItem::NewTag(name) => Row::new(vec![
            Cell::from(Line::styled("+", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))),
            Cell::from(Line::styled(truncate(&format!("Create static tag “{name}”"), name_width + detail_width), Style::new().fg(ACCENT))),
            Cell::from(""),
        ]),
        PickItem::Tag(t) => {
            let kind = if t.is_dynamic() { "Dynamic" } else { "Static" };
            Row::new(vec![
                Cell::from(Line::from(mark)),
                Cell::from(truncate(&t.name, name_width)).style(Style::new().fg(TEXT)),
                Cell::from(format!("{kind} · {}", t.asset_count.unwrap_or(0))).style(Style::new().fg(DIM)),
            ])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_form_text_keeps_its_indent_on_every_line() {
        let mut lines = vec![];
        push_wrapped(&mut lines, "✗ expected a whole number, for example 42", 30, 14, Style::new());
        assert!(lines.len() > 1);
        for line in &lines {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(text.starts_with("              "), "{text:?} lost its indent");
            assert!(text.chars().count() <= 30, "{text:?} is wider than the panel");
        }
    }

    fn dev(id: &str, status: &str, os: &str) -> Asset {
        crate::app::run::tests::device(id, id, status, os)
    }

    #[test]
    fn risks_count_offline_and_wrong_os_devices() {
        let devices = [dev("a", "CONNECTED", "Windows 11"), dev("b", "DISCONNECTED", "Windows 10"), dev("c", "CONNECTED", "Ubuntu 22.04"), dev("d", "DISCONNECTED", "macOS")];
        let risks = RunRisks::of(&devices, Some("POWERSHELL"));
        assert_eq!(risks, RunRisks { offline: 2, wrong_os: 2 });
        let warnings = risks.warnings();
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].starts_with("2 devices are offline"), "{warnings:?}");
        assert!(warnings[1].starts_with("2 devices are on an OS"), "{warnings:?}");
    }

    #[test]
    fn a_clean_list_has_no_warnings_and_one_device_reads_naturally() {
        assert!(RunRisks::of(&[dev("a", "CONNECTED", "Windows 11")], Some("POWERSHELL")).warnings().is_empty());
        let one = RunRisks::of(&[dev("a", "DISCONNECTED", "Windows 11")], Some("POWERSHELL")).warnings();
        assert!(one[0].starts_with("1 device is offline"), "{one:?}");
        assert!(RunRisks::of(&[dev("a", "DISCONNECTED", "Linux")], None).wrong_os == 0, "no script language: no OS judgement");
    }

    #[test]
    fn organization_types_read_as_words() {
        assert_eq!(org_kind("Customer"), "Customer");
        assert_eq!(org_kind("ServiceOrganization"), "Service organization");
        assert_eq!(org_kind(""), "");
    }

    #[test]
    fn script_languages_read_as_names() {
        assert_eq!(language_name("POWERSHELL"), "PowerShell");
        assert_eq!(language_name("VBSCRIPT"), "Vbscript");
        assert_eq!(language_name("SHELL_SCRIPT"), "Shell script");
        assert_eq!(language_name(""), "");
    }
}
