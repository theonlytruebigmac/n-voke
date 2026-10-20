//! Drawing the Migrate page. It only reads `App`: the project list, or the open project's
//! dashboard (summary, device table, the selected device's timeline).

use chrono::Utc;
use nvoke_core::migration::{project::Stage, report::stage_label};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Cell, Paragraph, Row, Table, Wrap},
};

use crate::{
    app::{
        App, Load,
        migrate::{NewProject, NpField, OpenProject, path_word, since},
    },
    theme::{ACCENT, BORDER, DIM, ERR, OK, SELECTED_BG, TEXT, WARN, panel},
    ui::{centered, clear, draw_field, spinner},
};

/// How a stage looks: needs-attention red, finished green, in flight cyan, waiting dim.
pub fn stage_color(stage: Stage) -> ratatui::style::Color {
    if stage.needs_attention() {
        ERR
    } else if matches!(stage, Stage::Healthy | Stage::CleanedUp | Stage::Retired | Stage::Resolved) {
        OK
    } else if matches!(stage, Stage::Planned | Stage::Excluded) {
        DIM
    } else {
        ACCENT
    }
}

pub fn draw_migrate(frame: &mut Frame, area: Rect, app: &mut App, focused: bool) {
    if app.migrate.open.is_some() {
        draw_project(frame, area, app, focused);
        if app.migrate.choosing.is_some() {
            draw_choose(frame, area, app);
        }
        if let Some(notes) = &app.migrate.notes {
            draw_notes(frame, area, notes);
        }
    } else {
        draw_list(frame, area, app, focused);
        if let Some(form) = &app.migrate.form {
            draw_new_project(frame, area, form);
        }
    }
    if app.migrate.servers.open {
        draw_servers(frame, area, app);
    }
    if let Some(form) = &app.migrate.servers.form {
        draw_server_form(frame, area, form);
    }
}

fn draw_list(frame: &mut Frame, area: Rect, app: &mut App, focused: bool) {
    let block = panel("migration projects", focused);
    let message = |frame: &mut Frame, text: String, color| frame.render_widget(Paragraph::new(text).style(Style::new().fg(color)).wrap(Wrap { trim: true }).block(block.clone()), area);
    match &app.migrate.projects {
        Load::NotLoaded | Load::Loading => message(frame, format!("{} Loading…", spinner(app.tick)), ACCENT),
        Load::Failed(error) => message(frame, error.clone(), ERR),
        Load::Ready(list) if list.is_empty() => message(
            frame,
            "No migration projects yet.\n\nCreate one with:  nvoke migrate-project create --name <name> --source <id> --dest <id>\nThen plan it:  nvoke migrate-project plan <project>\n\nAfter that it appears here, and you can watch and run it from this page.".into(),
            DIM,
        ),
        Load::Ready(list) => {
            let rows: Vec<Row> = list
                .iter()
                .map(|p| {
                    let attention = if p.attention > 0 { Cell::from(p.attention.to_string()).style(Style::new().fg(ERR).add_modifier(Modifier::BOLD)) } else { Cell::from("-").style(Style::new().fg(DIM)) };
                    let state = if p.closed { "finished" } else { "" };
                    Row::new(vec![Cell::from(p.name.clone()).style(Style::new().fg(TEXT).add_modifier(Modifier::BOLD)), Cell::from(p.devices.to_string()), attention, Cell::from(p.updated_at.clone()).style(Style::new().fg(DIM)), Cell::from(state).style(Style::new().fg(OK))])
                })
                .collect();
            let widths = [Constraint::Fill(1), Constraint::Length(8), Constraint::Length(10), Constraint::Length(22), Constraint::Length(10)];
            let header = Row::new(["Project", "Devices", "Attention", "Last change", ""]).style(Style::new().fg(DIM).add_modifier(Modifier::BOLD));
            let table = Table::new(rows, widths).header(header).block(block).row_highlight_style(Style::new().bg(SELECTED_BG).add_modifier(Modifier::BOLD));
            frame.render_stateful_widget(table, area, &mut app.migrate.list);
        }
    }
}

fn stage_chips(open: &OpenProject) -> Line<'static> {
    let mut spans = vec![];
    for (stage, count) in open.project.stage_counts() {
        spans.push(Span::styled(format!("{count} "), Style::new().fg(stage_color(stage)).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(format!("{}   ", stage_label(stage)), Style::new().fg(stage_color(stage))));
    }
    Line::from(spans)
}

fn draw_project(frame: &mut Frame, area: Rect, app: &mut App, focused: bool) {
    let tick = app.tick;
    let busy = app.migrate.busy;
    let Some(open) = app.migrate.open.as_mut() else { return };
    let title = if open.read_only { format!("{} · read-only", open.project.name) } else { open.project.name.clone() };
    let outer = panel(&title, focused);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let [summary, body] = Layout::vertical([Constraint::Length(4), Constraint::Fill(1)]).areas(inner);

    let project = &open.project;
    let (source, dest) = (&open.server_labels.0, &open.server_labels.1);
    let moved = if project.is_reinstall() { format!("{source} (reinstall)") } else { format!("{source}  →  {dest}") };
    let mut status = vec![Span::styled(format!(" {} devices", project.devices.len()), Style::new().fg(TEXT))];
    if project.attention_count() > 0 {
        status.push(Span::styled(format!("  ·  {} need attention", project.attention_count()), Style::new().fg(ERR).add_modifier(Modifier::BOLD)));
    }
    match (busy, open.polled_at_tick) {
        (Some(what), _) => status.push(Span::styled(format!("  ·  {} {what}", spinner(tick)), Style::new().fg(WARN))),
        (None, Some(at)) => status.push(Span::styled(format!("  ·  polled {} s ago", tick.saturating_sub(at) / 10), Style::new().fg(DIM))),
        (None, None) if !open.read_only => status.push(Span::styled("  ·  not polled yet", Style::new().fg(DIM))),
        _ => {}
    }
    let waves: Vec<String> = project.waves.iter().map(|w| format!("{}{}", w.label, if w.launched_at.is_some() { " ✓" } else { "" })).collect();
    let lines = vec![
        Line::from(vec![Span::styled(format!(" {moved}"), Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))]),
        Line::from(status),
        Line::from(vec![Span::raw(" "), Span::styled(if waves.is_empty() { "no waves yet".to_string() } else { format!("waves: {}", waves.join(", ")) }, Style::new().fg(DIM))]),
        Line::from(vec![Span::raw(" ")].into_iter().chain(stage_chips(open).spans).collect::<Vec<_>>()),
    ];
    frame.render_widget(Paragraph::new(lines), summary);

    let [table_area, detail_area] = Layout::horizontal([Constraint::Percentage(62), Constraint::Fill(1)]).areas(body);
    draw_devices(frame, table_area, open, focused);
    draw_timeline(frame, detail_area, open);
}

fn draw_devices(frame: &mut Frame, area: Rect, open: &mut OpenProject, focused: bool) {
    let now = Utc::now();
    let order = open.order();
    let rows: Vec<Row> = order
        .iter()
        .map(|&i| {
            let d = &open.project.devices[i];
            let mark = if d.stage.needs_attention() || !d.match_candidates.is_empty() { "!" } else { " " };
            Row::new(vec![
                Cell::from(mark).style(Style::new().fg(ERR).add_modifier(Modifier::BOLD)),
                Cell::from(d.name.clone()).style(Style::new().fg(TEXT)),
                Cell::from(d.wave.map_or("-".to_string(), |w| w.to_string())).style(Style::new().fg(DIM)),
                Cell::from(path_word(d.path)).style(Style::new().fg(DIM)),
                Cell::from(stage_label(d.stage)).style(Style::new().fg(stage_color(d.stage))),
                Cell::from(since(&d.entered_stage_at, now)).style(Style::new().fg(DIM)),
            ])
        })
        .collect();
    let widths = [Constraint::Length(1), Constraint::Fill(1), Constraint::Length(4), Constraint::Length(9), Constraint::Length(14), Constraint::Length(10)];
    let header = Row::new(["", "Device", "Wave", "Path", "Stage", "In stage"]).style(Style::new().fg(DIM).add_modifier(Modifier::BOLD));
    let table = Table::new(rows, widths).header(header).block(panel("devices", focused)).row_highlight_style(Style::new().bg(SELECTED_BG).add_modifier(Modifier::BOLD));
    frame.render_stateful_widget(table, area, &mut open.table);
}

fn draw_timeline(frame: &mut Frame, area: Rect, open: &OpenProject) {
    let block = panel("timeline", false);
    let Some(index) = open.selected_device() else {
        frame.render_widget(Paragraph::new("No devices yet.").style(Style::new().fg(DIM)).block(block), area);
        return;
    };
    let device = &open.project.devices[index];
    let mut lines = vec![Line::from(vec![Span::styled(device.name.clone(), Style::new().fg(TEXT).add_modifier(Modifier::BOLD)), Span::styled(format!("  {}", stage_label(device.stage)), Style::new().fg(stage_color(device.stage)))])];
    if let Some(error) = &device.last_error {
        lines.push(Line::styled(error.clone(), Style::new().fg(ERR)));
    }
    if !device.match_candidates.is_empty() {
        lines.push(Line::styled(format!("{} devices fit. Press m to choose.", device.match_candidates.len()), Style::new().fg(WARN).add_modifier(Modifier::BOLD)));
    }
    lines.push(Line::raw(""));
    // The newest events that fit, oldest first, each on its own lines.
    let room = area.height.saturating_sub(lines.len() as u16 + 2) as usize;
    let mut events: Vec<Line> = vec![];
    for event in device.events.iter().rev().take(room.max(1)) {
        let time = event.at.get(11..19).unwrap_or(&event.at);
        let first = event.detail.lines().next().unwrap_or("");
        events.push(Line::from(vec![Span::styled(format!("{time} "), Style::new().fg(DIM)), Span::styled(format!("{} ", event.kind), Style::new().fg(ACCENT)), Span::styled(first.to_string(), Style::new().fg(TEXT))]));
    }
    events.reverse();
    lines.extend(events);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }).block(block), area);
}

/// The destination devices that fit the selected one, to pick from.
fn draw_choose(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some(choosing) = app.migrate.choosing.as_mut() else { return };
    let Some(open) = app.migrate.open.as_ref() else { return };
    let Some(device) = open.project.devices.get(choosing.device) else { return };
    let rows: Vec<Row> = device.match_candidates.iter().map(|c| Row::new(vec![Cell::from(c.name.clone()).style(Style::new().fg(TEXT)), Cell::from(format!("device {}", c.device_id)).style(Style::new().fg(DIM))])).collect();
    let height = (rows.len() as u16 + 4).min(area.height);
    let width = 60.min(area.width);
    let popup = Rect::new(area.x + (area.width - width) / 2, area.y + (area.height.saturating_sub(height)) / 3, width, height);
    frame.render_widget(ratatui::widgets::Clear, popup);
    let table = Table::new(rows, [Constraint::Fill(1), Constraint::Length(18)]).block(panel(&format!("which one is {}?  enter chooses · esc cancels", device.name), true)).row_highlight_style(Style::new().bg(SELECTED_BG).add_modifier(Modifier::BOLD));
    frame.render_stateful_widget(table, popup, &mut choosing.table);
}

/// What an action found, to read: scrolls with the arrow keys, any other key closes it.
fn draw_notes(frame: &mut Frame, area: Rect, notes: &crate::app::migrate::Notes) {
    let popup = centered(area, area.width.saturating_sub(8).min(110), area.height.saturating_sub(4));
    clear(frame, popup);
    let outer = panel(&notes.title, true).title_bottom(crate::theme::footer_text("↑↓ scroll · any other key closes"));
    let inner = outer.inner(popup);
    frame.render_widget(outer, popup);
    let lines: Vec<Line> = notes
        .lines
        .iter()
        .map(|l| {
            let style = if l.starts_with('!') { Style::new().fg(WARN) } else if l.starts_with("      ") { Style::new().fg(DIM) } else { Style::new().fg(TEXT) };
            Line::styled(l.clone(), style)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((notes.scroll, 0)), inner);
}

fn draw_select(frame: &mut Frame, area: Rect, label: &str, value: &str, focused: bool) {
    let border = Style::new().fg(if focused { ACCENT } else { BORDER });
    let block = ratatui::widgets::Block::bordered().border_type(ratatui::widgets::BorderType::Rounded).title(format!(" {label} ")).border_style(border);
    let arrows = Style::new().fg(if focused { ACCENT } else { DIM });
    let line = Line::from(vec![Span::styled("‹ ", arrows), Span::styled(value.to_string(), Style::new().fg(TEXT).add_modifier(Modifier::BOLD)), Span::styled(" ›", arrows), Span::styled(if focused { "   ← → to change" } else { "" }, Style::new().fg(DIM))]);
    frame.render_widget(Paragraph::new(line).block(block), area);
}

fn draw_new_project(frame: &mut Frame, area: Rect, form: &NewProject) {
    let fields = form.fields();
    let height = fields.len() as u16 * 3 + 5;
    let popup = centered(area, 78, height);
    clear(frame, popup);
    let outer = crate::theme::panel_padded("new migration project", true);
    let inner = outer.inner(popup);
    frame.render_widget(outer, popup);

    let mut constraints: Vec<Constraint> = fields.iter().map(|_| Constraint::Length(3)).collect();
    constraints.push(Constraint::Fill(1));
    constraints.push(Constraint::Length(1));
    let rows = Layout::vertical(constraints).split(inner);
    let server = |i: usize| form.servers.get(i).map_or(String::new(), |s| format!("{} ({})", s.label, s.host));
    let mut cursor = None;
    for (i, field) in fields.iter().enumerate() {
        let focused = form.field == *field;
        let row = rows[i];
        match field {
            NpField::Name => cursor = Some(draw_field(frame, row, "Name", &form.name, focused)).filter(|_| focused).or(cursor),
            NpField::Source => draw_select(frame, row, "Moving devices from", &format!("{}{}", server(form.source), if focused { "   a: add a server" } else { "" }), focused),
            NpField::Dest => {
                let same = if form.dest == form.source { "  (the same server: a reinstall)" } else { "" };
                draw_select(frame, row, "to", &format!("{}{same}{}", server(form.dest), if focused { "   a: add a server" } else { "" }), focused);
            }
            NpField::Eco => draw_select(frame, row, "Is the destination linked to Ecoverse?", NewProject::ECO[form.eco], focused),
            NpField::Tenant => cursor = Some(draw_field(frame, row, "The other tenant's ID", &form.tenant, focused)).filter(|_| focused).or(cursor),
            NpField::SourceOrg => cursor = Some(draw_field(frame, row, "Source Ecoverse organization ID(s) (empty: N-central only)", &form.source_org, focused)).filter(|_| focused).or(cursor),
            NpField::DestOrg => cursor = Some(draw_field(frame, row, "Destination Ecoverse organization ID(s)", &form.dest_org, focused)).filter(|_| focused).or(cursor),
        }
    }
    let message = rows[fields.len()];
    let (text, color) = match (&form.error, form.busy) {
        (_, true) => ("Creating…".to_string(), ACCENT),
        (Some(e), _) => (e.clone(), ERR),
        _ => (String::new(), DIM),
    };
    frame.render_widget(Paragraph::new(text).style(Style::new().fg(color)).wrap(Wrap { trim: true }), message);
    frame.render_widget(Paragraph::new(crate::theme::fit_hints(&[("tab", "next field"), ("← →", "choose"), ("enter", "create"), ("esc", "cancel")], rows[fields.len() + 1].width as usize, false)), rows[fields.len() + 1]);
    if let (Some(position), false) = (cursor, form.busy) {
        frame.set_cursor_position(position);
    }
}

/// The N-central servers: where they are, whether a JWT is saved, and which Ecoverse tenant each belongs to.
fn draw_servers(frame: &mut Frame, area: Rect, app: &mut App) {
    let popup = centered(area, area.width.saturating_sub(10).min(100), 16.min(area.height));
    clear(frame, popup);
    let hints = crate::theme::footer_text("↑↓ choose · a add · e edit · t test the connection · x remove · esc close");
    let block = panel("N-central servers", true).title_bottom(hints);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    match &app.migrate.servers.list {
        Load::NotLoaded | Load::Loading => frame.render_widget(Paragraph::new(format!("{} Loading…", spinner(app.tick))).style(Style::new().fg(ACCENT)), inner),
        Load::Failed(error) => frame.render_widget(Paragraph::new(error.clone()).style(Style::new().fg(ERR)).wrap(Wrap { trim: true }), inner),
        Load::Ready(list) if list.is_empty() => frame.render_widget(Paragraph::new("No servers yet. Press a to add the first one: its host name and an API JWT (N-central: Administration, User Management, API Access).").style(Style::new().fg(DIM)).wrap(Wrap { trim: true }), inner),
        Load::Ready(list) => {
            let rows: Vec<Row> = list
                .iter()
                .map(|s| {
                    let jwt = if s.has_jwt { Cell::from("JWT saved").style(Style::new().fg(OK)) } else { Cell::from("NO JWT").style(Style::new().fg(ERR).add_modifier(Modifier::BOLD)) };
                    Row::new(vec![Cell::from(s.label.clone()).style(Style::new().fg(TEXT).add_modifier(Modifier::BOLD)), Cell::from(s.host.clone()).style(Style::new().fg(TEXT)), jwt, Cell::from(s.ecoverse_tenant.clone().unwrap_or_default()).style(Style::new().fg(DIM))])
                })
                .collect();
            let widths = [Constraint::Length(22), Constraint::Fill(1), Constraint::Length(10), Constraint::Length(20)];
            let header = Row::new(["Name", "Host", "JWT", "Ecoverse tenant"]).style(Style::new().fg(DIM).add_modifier(Modifier::BOLD));
            let table = Table::new(rows, widths).header(header).row_highlight_style(Style::new().bg(SELECTED_BG).add_modifier(Modifier::BOLD));
            frame.render_stateful_widget(table, inner, &mut app.migrate.servers.table);
        }
    }
}

fn draw_server_form(frame: &mut Frame, area: Rect, form: &crate::app::servers::ServerForm) {
    use crate::app::servers::{ServerForm, ServerFormMode, SfField};
    let popup = centered(area, 78, 19);
    clear(frame, popup);
    let title = if form.mode == ServerFormMode::Add { "add an N-central server" } else { "edit server" };
    let outer = crate::theme::panel_padded(title, true);
    let inner = outer.inner(popup);
    frame.render_widget(outer, popup);
    let rows = Layout::vertical([Constraint::Length(3), Constraint::Length(3), Constraint::Length(3), Constraint::Length(3), Constraint::Fill(1), Constraint::Length(1)]).split(inner);
    let jwt_label = if form.mode == ServerFormMode::Add { "API JWT (stored in your keychain, never shown again)" } else { "API JWT (leave empty to keep the saved one)" };
    let mut cursor = None;
    for (i, field) in ServerForm::FIELDS.iter().enumerate() {
        let focused = form.field == *field;
        let (label, input) = match field {
            SfField::Host => ("Host name, e.g. n-central.example.com", &form.host),
            SfField::Label => ("Name shown in N-voke (optional)", &form.label),
            SfField::Jwt => (jwt_label, &form.jwt),
            SfField::Tenant => ("Ecoverse tenant ID this server is linked to (optional)", &form.tenant),
        };
        let at = draw_field(frame, rows[i], label, input, focused);
        if focused {
            cursor = Some(at);
        }
    }
    let (text, color) = match (&form.error, form.busy) {
        (_, true) => ("Saving, then signing in to check it…".to_string(), ACCENT),
        (Some(e), _) => (e.clone(), ERR),
        _ => (String::new(), DIM),
    };
    frame.render_widget(Paragraph::new(text).style(Style::new().fg(color)).wrap(Wrap { trim: true }), rows[4]);
    frame.render_widget(Paragraph::new(crate::theme::fit_hints(&[("tab", "next field"), ("enter", "save and test"), ("esc", "cancel")], rows[5].width as usize, false)), rows[5]);
    if let (Some(position), false) = (cursor, form.busy) {
        frame.set_cursor_position(position);
    }
}
