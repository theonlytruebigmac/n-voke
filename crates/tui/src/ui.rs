//! Drawing. These functions only read the app state; they never change it
//! (apart from table scroll positions, which ratatui updates while rendering).

use chrono::{DateTime, Local, Utc};
use nvoke_core::audit::AuditEntry;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span},
    widgets::{
        Block, BorderType, Cell, Clear, LineGauge, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Sparkline, SparklineBar, Table, Wrap,
    },
};

use crate::{
    app::{App, Field, Focus, Load, Page, Screen, SettingsForm},
    input::TextInput,
    theme::{self, ACCENT, BORDER, DIM, ERR, OK, SCRIPT, SELECTED_BG, TEXT, WARN, panel},
};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn spinner(tick: u64) -> &'static str {
    SPINNER[(tick % SPINNER.len() as u64) as usize]
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    frame.render_widget(Block::new().style(theme::background()), area);
    match &app.screen {
        Screen::Loading => centered_message(frame, area, &format!("{} Loading…", spinner(app.tick)), ACCENT),
        Screen::Fatal(error) => {
            let text = format!("N-voke can't start:\n\n{error}\n\nPress q to quit.");
            centered_message(frame, area, &text, ERR);
        }
        Screen::Form(form) => draw_form(frame, area, form),
        Screen::Main => draw_main(frame, area, app),
    }
}

fn draw_main(frame: &mut Frame, area: Rect, app: &mut App) {
    let [header, body, footer] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1), Constraint::Length(1)]).areas(area);
    let [sidebar, content] = Layout::horizontal([Constraint::Length(22), Constraint::Fill(1)]).areas(body);

    draw_header(frame, header, app);
    draw_sidebar(frame, sidebar, app);

    let focused = app.focus == Focus::Content;
    match app.page {
        Page::Audit => draw_audit(frame, content, app, focused),
        Page::Settings => draw_settings(frame, content, app, focused),
        page => draw_placeholder(frame, content, page),
    }

    let hints: &[(&str, &str)] = match (app.focus, app.page) {
        (Focus::Sidebar, p) if p.interactive() => &[("↑↓", "page"), ("→", "open"), ("?", "help"), ("q", "quit")],
        (Focus::Sidebar, _) => &[("↑↓", "page"), ("?", "help"), ("q", "quit")],
        (Focus::Content, Page::Audit) => &[("↑↓", "move"), ("PgUp/PgDn", "jump"), ("r", "refresh"), ("←", "back"), ("?", "help")],
        (Focus::Content, _) => &[("enter", "edit"), ("←", "back"), ("?", "help"), ("q", "quit")],
    };
    frame.render_widget(Paragraph::new(theme::key_hints(hints)), footer);

    if app.show_help {
        draw_help(frame, area);
    }
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let host = app
        .settings
        .as_ref()
        .map(|s| s.api_url.trim_start_matches("https://").split('/').next().unwrap_or("").to_string())
        .unwrap_or_default();
    let clock = Local::now().format("%H:%M:%S").to_string();
    let right = format!("{host}  {clock} ");

    let [left_area, right_area] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(right.chars().count() as u16)]).areas(area);

    // The logo's colours drift along the letters as the ticks advance.
    let mut left = vec![Span::styled(" ◢◤ ", Style::new().fg(ACCENT))];
    left.extend("N-voke".chars().enumerate().map(|(i, c)| Span::styled(c.to_string(), Style::new().fg(theme::shimmer(i, app.tick)).add_modifier(Modifier::BOLD))));
    left.push(Span::styled(format!("  /  {}", app.page.title()), Style::new().fg(DIM)));
    frame.render_widget(Paragraph::new(Line::from(left)), left_area);
    frame.render_widget(Paragraph::new(right).style(Style::new().fg(DIM)).alignment(Alignment::Right), right_area);
}

fn draw_sidebar(frame: &mut Frame, area: Rect, app: &App) {
    let [nav, status] = Layout::vertical([Constraint::Length(Page::ALL.len() as u16 + 2), Constraint::Fill(1)]).areas(area);

    let focused = app.focus == Focus::Sidebar;
    let lines: Vec<Line> = Page::ALL
        .iter()
        .enumerate()
        .map(|(i, page)| {
            let label = format!(" {} {} {}", i + 1, page.icon(), page.title());
            if *page == app.page {
                // The selected page is a full-width bar; it is brighter while the sidebar has focus.
                let style = Style::new().fg(ACCENT).add_modifier(Modifier::BOLD);
                let style = if focused { style.bg(SELECTED_BG) } else { style };
                Line::styled(format!("{label:<width$}", width = nav.width.saturating_sub(2) as usize), style)
            } else {
                Line::styled(label, Style::new().fg(TEXT))
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).block(panel("menu", focused)), nav);

    let mark = |on: bool| if on { Span::styled("●", Style::new().fg(OK)) } else { Span::styled("○", Style::new().fg(ERR)) };
    let mut lines = vec![];
    if let Some(s) = &app.settings {
        lines.push(Line::from(vec![Span::raw(" "), mark(s.has_api_key), Span::styled(" API key", Style::new().fg(TEXT))]));
        lines.push(Line::from(vec![Span::raw(" "), mark(s.encryption_available), Span::styled(" encrypted", Style::new().fg(TEXT))]));
        let history = if s.history_days == 0 { "no history".to_string() } else { format!("history {}d", s.history_days) };
        lines.push(Line::styled(format!("   {history}"), Style::new().fg(DIM)));
    }
    lines.push(Line::from(""));
    lines.push(if app.busy() {
        Line::from(vec![Span::styled(format!(" {} ", spinner(app.tick)), Style::new().fg(ACCENT)), Span::styled("working", Style::new().fg(ACCENT))])
    } else {
        Line::styled(" ● idle", Style::new().fg(DIM))
    });
    frame.render_widget(Paragraph::new(lines).block(panel("status", false)), status);
}

fn draw_placeholder(frame: &mut Frame, area: Rect, page: Page) {
    let lines = vec![
        Line::from(""),
        Line::styled(format!("{} isn't built yet.", page.title()), Style::new().fg(WARN).add_modifier(Modifier::BOLD)),
        Line::from(""),
        Line::styled(page.blurb(), Style::new().fg(DIM)),
    ];
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center).block(panel(page.title(), false)), area);
}

// ---- audit log ---------------------------------------------------------------

/// Narrowest width at which the activity chart sits beside the totals.
const MIN_CHART_ROW: u16 = 70;

/// How far back the activity chart reaches.
const CHART_DAYS: i64 = 14;

/// Terminal columns per time slice: two filled and one gap, so neighbouring bars stay distinct.
const SLOT_COLS: usize = 3;

/// Spreads per-slice counts across `width` columns as bars. Empty slices (and the
/// gap column) are "absent", which the sparkline draws as a dim baseline.
fn bars(values: &[u64], width: usize, color: impl Fn(u64) -> Color) -> Vec<SparklineBar> {
    let mut out: Vec<SparklineBar> = (0..width.saturating_sub(values.len() * SLOT_COLS)).map(|_| SparklineBar::from(None::<u64>)).collect();
    for &n in values {
        if n == 0 {
            out.extend((0..SLOT_COLS).map(|_| SparklineBar::from(None::<u64>)));
        } else {
            let style = Some(Style::new().fg(color(n)));
            out.extend([SparklineBar::from(n).style(style), SparklineBar::from(n).style(style), SparklineBar::from(None::<u64>)]);
        }
    }
    out
}

/// Cuts text to `width` characters, ending in an ellipsis if anything was removed.
fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        text.to_string()
    } else {
        text.chars().take(width.saturating_sub(1)).chain(std::iter::once('…')).collect()
    }
}

fn parse_time(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text).ok().map(|t| t.with_timezone(&Utc))
}

/// Counts entries per time slice across the last `CHART_DAYS` days, oldest first:
/// every change, and the failed ones.
fn bucket(entries: &[AuditEntry], slots: usize, now: DateTime<Utc>) -> (Vec<u64>, Vec<u64>) {
    let (mut all, mut failed) = (vec![0u64; slots], vec![0u64; slots]);
    let span = CHART_DAYS * 86_400;
    for entry in entries {
        let Some(at) = parse_time(&entry.timestamp) else { continue };
        let age = (now - at).num_seconds().max(0);
        if age >= span {
            continue;
        }
        let slot = slots - 1 - (age as usize * slots / span as usize).min(slots - 1);
        all[slot] += 1;
        if entry.outcome != "ok" {
            failed[slot] += 1;
        }
    }
    (all, failed)
}

fn ago(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (now - at).num_seconds().max(0);
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}

fn action_color(action: &str) -> Color {
    match action.split('.').next().unwrap_or("") {
        "script" => SCRIPT,
        "tag" => ACCENT,
        "settings" => WARN,
        _ => DIM,
    }
}

fn draw_audit(frame: &mut Frame, area: Rect, app: &mut App, focused: bool) {
    let log = match &app.audit {
        Load::Ready(log) => log,
        Load::Failed(error) => {
            frame.render_widget(Paragraph::new(error.as_str()).style(Style::new().fg(ERR)).wrap(Wrap { trim: true }).block(panel("audit log", focused)), area);
            return;
        }
        Load::NotLoaded | Load::Loading => {
            let text = format!("{} Loading…", spinner(app.tick));
            frame.render_widget(Paragraph::new(text).style(Style::new().fg(ACCENT)).block(panel("audit log", focused)), area);
            return;
        }
    };
    if log.entries.is_empty() {
        frame.render_widget(Paragraph::new("Nothing has been recorded yet.").style(Style::new().fg(DIM)).block(panel("audit log", focused)), area);
        return;
    }

    let now = Utc::now();
    let [top, table_area, detail_area] = Layout::vertical([Constraint::Length(10), Constraint::Fill(1), Constraint::Length(7)]).areas(area);
    // Below this width the chart would be a sliver, so the totals get the whole row.
    let show_chart = top.width >= MIN_CHART_ROW;
    let [chart_area, stats_area] = if show_chart {
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(30)]).areas(top)
    } else {
        [Rect::default(), top]
    };

    if show_chart {
        // Activity over time: one bar per slice of the last two weeks, taller and
        // brighter where more happened. Failures get their own red strip underneath.
        let chart_block = panel(&format!("activity · last {CHART_DAYS} days"), false);
        let inner = chart_block.inner(chart_area);
        frame.render_widget(chart_block, chart_area);
        let [changes_area, failures_area, axis_area] = Layout::vertical([Constraint::Fill(1), Constraint::Length(2), Constraint::Length(1)]).areas(inner);
        let width = inner.width as usize;
        let (all, failed) = bucket(&log.entries, (width / SLOT_COLS).max(1), now);
        let max = all.iter().copied().max().unwrap_or(1).max(1);
        frame.render_widget(Sparkline::default().data(bars(&all, width, |n| theme::volume(n as f32 / max as f32))).max(max).absent_value_symbol(symbols::bar::ONE_EIGHTH).absent_value_style(Style::new().fg(BORDER)), changes_area);
        let fmax = failed.iter().copied().max().unwrap_or(1).max(1);
        frame.render_widget(Sparkline::default().data(bars(&failed, width, |_| ERR)).max(fmax).absent_value_symbol(symbols::bar::ONE_EIGHTH).absent_value_style(Style::new().fg(BORDER)), failures_area);

        // A legend and the time axis, so the bars can be read without guessing.
        let legend = Line::from(vec![
            Span::styled("■", Style::new().fg(theme::volume(0.7))),
            Span::styled(" changes   ", Style::new().fg(DIM)),
            Span::styled("■", Style::new().fg(ERR)),
            Span::styled(" failed", Style::new().fg(DIM)),
        ]);
        frame.render_widget(Paragraph::new(format!("{CHART_DAYS}d ago")).style(Style::new().fg(DIM)), axis_area);
        // The legend only gets the middle if it clears both end labels.
        if axis_area.width >= 36 {
            frame.render_widget(Paragraph::new(legend).alignment(Alignment::Center), axis_area);
        }
        frame.render_widget(Paragraph::new("now").style(Style::new().fg(DIM)).alignment(Alignment::Right), axis_area);
    }

    // Totals.
    let total = log.entries.len();
    let errors = log.entries.iter().filter(|e| e.outcome != "ok").count();
    let ratio = (total - errors) as f64 / total as f64;
    let last = log.entries.first().and_then(|e| parse_time(&e.timestamp)).map(|t| ago(t, now)).unwrap_or_default();
    let stats_block = panel("totals", false);
    let stats_inner = stats_block.inner(stats_area);
    frame.render_widget(stats_block, stats_area);
    let [numbers, gauge] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(stats_inner);
    let number = |label: &str, value: String, color: Color| Line::from(vec![Span::styled(format!(" {label:<10}"), Style::new().fg(DIM)), Span::styled(value, Style::new().fg(color).add_modifier(Modifier::BOLD))]);
    frame.render_widget(
        Paragraph::new(vec![
            number("changes", total.to_string(), TEXT),
            number("succeeded", (total - errors).to_string(), OK),
            number("failed", errors.to_string(), if errors > 0 { ERR } else { DIM }),
            number("latest", last, TEXT),
        ]),
        numbers,
    );
    frame.render_widget(
        LineGauge::default()
            .ratio(ratio)
            .label(format!(" {:>3.0}% ok ", ratio * 100.0))
            .filled_symbol(symbols::line::THICK_HORIZONTAL)
            .unfilled_symbol(symbols::line::THICK_HORIZONTAL)
            .filled_style(Style::new().fg(theme::health(ratio)))
            .unfilled_style(Style::new().fg(BORDER)),
        gauge,
    );

    // The entries, newest first. On a narrow window the action column goes, so the
    // summary (the part that matters) keeps its room.
    let inner_width = table_area.width.saturating_sub(2) as usize;
    let show_action = inner_width >= 90;
    let mut widths = vec![Constraint::Length(19), Constraint::Length(1)];
    let mut headers = vec!["When", ""];
    if show_action {
        widths.push(Constraint::Length(16));
        headers.push("Action");
    }
    widths.push(Constraint::Fill(1));
    headers.push("Summary");
    let spacing = 2usize;
    let fixed = 2 /* selection marker */ + 19 + 1 + if show_action { 16 } else { 0 } + spacing * (widths.len() - 1) + 1 /* scrollbar */;
    let summary_width = inner_width.saturating_sub(fixed);

    let rows = log.entries.iter().map(|e| {
        let (mark, color) = if e.outcome == "ok" { ("✓", OK) } else { ("✗", ERR) };
        let mut cells = vec![
            Cell::from(e.timestamp.replace('T', " ").trim_end_matches('Z').to_string()).style(Style::new().fg(DIM)),
            Cell::from(mark).style(Style::new().fg(color)),
        ];
        if show_action {
            cells.push(Cell::from(e.action.clone()).style(Style::new().fg(action_color(&e.action))));
        }
        cells.push(Cell::from(truncate(&e.summary, summary_width)).style(Style::new().fg(if e.outcome == "ok" { TEXT } else { ERR })));
        Row::new(cells)
    });
    let table = Table::new(rows, widths)
        .column_spacing(spacing as u16)
        .header(Row::new(headers).style(Style::new().fg(DIM).add_modifier(Modifier::BOLD)))
        .row_highlight_style(Style::new().bg(SELECTED_BG).add_modifier(Modifier::BOLD))
        .highlight_symbol("▌ ")
        .block(panel("audit log", focused).title_bottom(Line::styled(format!(" {total} entries "), Style::new().fg(DIM))));
    frame.render_stateful_widget(table, table_area, &mut app.audit_table);

    let mut scroll = ScrollbarState::new(total).position(app.audit_table.selected().unwrap_or(0));
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight).begin_symbol(None).end_symbol(None).track_symbol(None).style(Style::new().fg(BORDER)),
        table_area.inner(ratatui::layout::Margin { vertical: 1, horizontal: 0 }),
        &mut scroll,
    );

    // Details of the selected entry.
    let label = |text: &str| Span::styled(format!("{text:<9}"), Style::new().fg(DIM));
    let mut lines = vec![];
    if let Some(entry) = app.audit_table.selected().and_then(|i| log.entries.get(i)) {
        lines.push(Line::from(vec![label("When"), Span::raw(entry.timestamp.clone())]));
        lines.push(Line::from(vec![label("User"), Span::raw(entry.user.clone())]));
        lines.push(Line::from(vec![label("Action"), Span::styled(entry.action.clone(), Style::new().fg(action_color(&entry.action)))]));
        lines.push(Line::from(vec![label("Summary"), Span::raw(entry.summary.clone())]));
        if let Some(error) = &entry.error {
            lines.push(Line::from(vec![label("Error"), Span::styled(error.clone(), Style::new().fg(ERR))]));
        }
    }
    let detail = Paragraph::new(lines).style(Style::new().fg(TEXT)).wrap(Wrap { trim: true }).block(panel("details", false).title_bottom(Line::styled(format!(" {} ", log.path), Style::new().fg(DIM))));
    frame.render_widget(detail, detail_area);
}

// ---- settings ----------------------------------------------------------------

fn draw_settings(frame: &mut Frame, area: Rect, app: &App, focused: bool) {
    let block = panel("settings", focused);
    let Some(s) = &app.settings else {
        frame.render_widget(Paragraph::new("Loading…").block(block), area);
        return;
    };
    let yes_no = |b: bool| if b { Span::styled("● yes", Style::new().fg(OK)) } else { Span::styled("○ no", Style::new().fg(ERR)) };
    let label = |text: &str| Span::styled(format!(" {text:<20}"), Style::new().fg(DIM));
    let history = if s.history_days == 0 { "not saved".to_string() } else { format!("{} days", s.history_days) };
    let mut lines = vec![
        Line::from(""),
        Line::from(vec![label("API endpoint"), Span::styled(s.api_url.clone(), Style::new().fg(TEXT))]),
        Line::from(vec![label("API key saved"), yes_no(s.has_api_key)]),
        Line::from(vec![label("Keep run history"), Span::styled(history, Style::new().fg(TEXT))]),
        Line::from(vec![label("Encrypted at rest"), yes_no(s.encryption_available)]),
    ];
    if let Some(error) = &s.encryption_error {
        lines.push(Line::from(""));
        lines.push(Line::styled(format!(" {error}"), Style::new().fg(WARN)));
    }
    if focused {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::raw(" Press "), Span::styled("enter", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)), Span::raw(" to edit.")]).style(Style::new().fg(DIM)));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }).block(block), area);
}

// ---- first launch / edit form ------------------------------------------------

fn draw_form(frame: &mut Frame, area: Rect, form: &SettingsForm) {
    let title = if form.first_run { "Welcome to N-voke" } else { "Edit settings" };
    let popup = centered(area, 72, 14);
    clear(frame, popup);
    let outer = panel(title, true);
    let inner = outer.inner(popup);
    frame.render_widget(outer, popup);

    let [intro, endpoint, token, message, hints] =
        Layout::vertical([Constraint::Length(2), Constraint::Length(3), Constraint::Length(3), Constraint::Fill(1), Constraint::Length(1)]).areas(inner);

    let intro_text = if form.first_run {
        "Paste an API-Access token from the N-able dashboard. It is stored in your system keychain."
    } else {
        "Leave the token empty to keep the saved one."
    };
    frame.render_widget(Paragraph::new(intro_text).style(Style::new().fg(TEXT)).wrap(Wrap { trim: true }), intro);

    let cursor = draw_field(frame, endpoint, "API endpoint", &form.endpoint, form.focus == Field::Endpoint);
    let token_cursor = draw_field(frame, token, "API token", &form.token, form.focus == Field::Token);
    if !form.busy {
        frame.set_cursor_position(if form.focus == Field::Endpoint { cursor } else { token_cursor });
    }

    let (text, color) = match (&form.error, form.busy) {
        (_, true) => ("Saving…".to_string(), ACCENT),
        (Some(error), _) => (error.clone(), ERR),
        _ => (String::new(), DIM),
    };
    frame.render_widget(Paragraph::new(text).style(Style::new().fg(color)).wrap(Wrap { trim: true }), message);

    let esc = if form.first_run { "quit" } else { "cancel" };
    frame.render_widget(Paragraph::new(theme::key_hints(&[("↑↓/tab", "switch field"), ("enter", "save"), ("esc", esc)])), hints);
}

/// Draws a labelled text field and returns where the cursor goes if it has focus.
fn draw_field(frame: &mut Frame, area: Rect, label: &str, input: &TextInput, focused: bool) -> (u16, u16) {
    let border = Style::new().fg(if focused { ACCENT } else { BORDER });
    let block = Block::bordered().border_type(BorderType::Rounded).title(format!(" {label} ")).border_style(border);
    let inner = block.inner(area);
    let shown = input.shown();
    // Show the end of a value that is wider than the box.
    let width = inner.width.saturating_sub(1) as usize;
    let count = shown.chars().count();
    let visible: String = shown.chars().skip(count.saturating_sub(width)).collect();
    let cursor_x = inner.x + visible.chars().count() as u16;
    frame.render_widget(Paragraph::new(visible).style(Style::new().fg(TEXT)).block(block), area);
    (cursor_x, inner.y)
}

// ---- overlays and helpers ----------------------------------------------------

fn draw_help(frame: &mut Frame, area: Rect) {
    let popup = centered(area, 56, 17);
    clear(frame, popup);
    let key = |k: &str, what: &str| Line::from(vec![Span::styled(format!(" {k:<14}"), Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)), Span::styled(what.to_string(), Style::new().fg(TEXT))]);
    let heading = |text: &str| Line::styled(format!(" {text}"), Style::new().fg(DIM).add_modifier(Modifier::BOLD));
    let lines = vec![
        heading("Anywhere"),
        key("1-5, tab", "switch page"),
        key("?", "this help"),
        key("q, ctrl-c", "quit"),
        Line::from(""),
        heading("Menu"),
        key("↑ ↓", "choose a page"),
        key("→ enter", "step into the page"),
        Line::from(""),
        heading("Inside a page"),
        key("↑ ↓", "move (j/k work too)"),
        key("PgUp PgDn", "jump ten rows"),
        key("home end", "first / last (g/G)"),
        key("← esc", "back to the menu"),
        key("r", "refresh"),
    ];
    frame.render_widget(Paragraph::new(lines).block(panel("keys", true).title_bottom(Line::styled(" any key closes ", Style::new().fg(DIM)))), popup);
}

/// Blanks an area for a popup. `Clear` also resets the background, so it is repainted.
fn clear(frame: &mut Frame, area: Rect) {
    frame.render_widget(Clear, area);
    frame.render_widget(Block::new().style(theme::background()), area);
}

fn centered_message(frame: &mut Frame, area: Rect, text: &str, color: Color) {
    let popup = centered(area, 70, text.lines().count() as u16 + 2);
    frame.render_widget(Paragraph::new(text).style(Style::new().fg(color)).alignment(Alignment::Center).wrap(Wrap { trim: true }).block(panel("N-voke", false)), popup);
}

/// A `width` x `height` rectangle in the middle of `area` (clamped to fit).
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let [_, row, _] = Layout::vertical([Constraint::Fill(1), Constraint::Length(height.min(area.height)), Constraint::Fill(1)]).areas(area);
    let [_, cell, _] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(width.min(area.width)), Constraint::Fill(1)]).areas(row);
    cell
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nvoke_core::{AuditLogView, Core, Settings, audit::AuditEntry};
    use ratatui::{Terminal, backend::TestBackend};
    use serde_json::Value;

    use super::*;

    fn app() -> (tokio::runtime::Runtime, App) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let app = App::new(Arc::new(Core::new("test").unwrap()), runtime.handle().clone());
        (runtime, app)
    }

    fn settings(has_api_key: bool) -> Settings {
        Settings {
            api_url: "https://api.n-able.com/graphql".into(),
            has_api_key,
            history_days: 30,
            encryption_available: true,
            encryption_error: None,
        }
    }

    /// Renders the app into a 100x24 in-memory terminal and returns the screen as text.
    fn render(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| (0..buffer.area.width).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .map(|line| line.trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn first_launch_asks_for_the_token_and_hides_it() {
        let (_rt, mut app) = app();
        let s = settings(false);
        let mut form = SettingsForm::new(&s);
        form.token = TextInput::new("secret-token", true);
        app.screen = Screen::Form(form);
        let screen = render(&mut app);
        assert!(screen.contains("Welcome to N-voke"), "{screen}");
        assert!(screen.contains("API token"), "{screen}");
        assert!(screen.contains("••••••••••••"), "{screen}");
        assert!(!screen.contains("secret-token"), "the token must never be drawn:\n{screen}");
    }

    #[test]
    fn audit_page_lists_entries_with_the_selected_one_in_detail() {
        let (_rt, mut app) = app();
        app.settings = Some(settings(true));
        app.screen = Screen::Main;
        app.page = Page::Audit;
        let entries = vec![
            AuditEntry::new("tag.create", "Created static tag Servers", Ok(()), Value::Null),
            AuditEntry::new("script.run", "Ran Cleanup on 3 devices", Err("HTTP 500".into()), Value::Null),
        ];
        app.audit = Load::Ready(AuditLogView { path: "/tmp/audit.jsonl".into(), entries });
        app.audit_table.select(Some(1));
        let screen = render(&mut app);
        assert!(screen.contains("Audit log"), "{screen}");
        assert!(screen.contains("Created static tag Servers"), "{screen}");
        assert!(screen.contains("Error    HTTP 500"), "{screen}");
        assert!(screen.contains("/tmp/audit.jsonl"), "{screen}");
    }

    #[test]
    fn buckets_count_recent_entries_newest_last_and_ignore_old_ones() {
        let now = Utc::now();
        let at = |ago: chrono::Duration, outcome: Result<(), String>| {
            let mut e = AuditEntry::new("tag.create", "x", outcome, Value::Null);
            e.timestamp = (now - ago).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            e
        };
        let entries = vec![
            at(chrono::Duration::minutes(5), Ok(())),
            at(chrono::Duration::minutes(10), Err("boom".into())),
            at(chrono::Duration::days(7), Ok(())),
            at(chrono::Duration::days(30), Ok(())), // beyond the chart: ignored
        ];
        let (all, failed) = bucket(&entries, 14, now);
        assert_eq!(all.iter().sum::<u64>(), 3);
        assert_eq!(all[13], 2, "recent entries land in the last slot");
        assert_eq!(failed[13], 1);
        assert_eq!(all[6], 1, "a week-old entry lands mid-chart");
    }

    #[test]
    fn unbuilt_pages_say_so() {
        let (_rt, mut app) = app();
        app.settings = Some(settings(true));
        app.screen = Screen::Main;
        app.page = Page::Tags;
        assert!(render(&mut app).contains("Tags isn't built yet."));
    }

    /// Draws at the given size and returns the buffer.
    fn draw_at(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn text_of(buffer: &ratatui::buffer::Buffer) -> String {
        (0..buffer.area.height).map(|y| (0..buffer.area.width).map(|x| buffer[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
    }

    fn audit_app() -> (tokio::runtime::Runtime, App) {
        let (rt, mut app) = app();
        app.settings = Some(settings(true));
        app.screen = Screen::Main;
        app.page = Page::Audit;
        app.focus = Focus::Content;
        let entries = ["Deleted tag ScriptRunner_Test_Static", "Deleted tag ScriptRunner_Test_Failing_Dynamic"]
            .iter()
            .map(|s| AuditEntry::new("tag.delete", s, Ok(()), Value::Null))
            .collect();
        app.audit = Load::Ready(AuditLogView { path: "/tmp/audit.jsonl".into(), entries });
        app.audit_table.select(Some(0));
        (rt, app)
    }

    /// Readability depends on every cell having our own background: a cell left as
    /// "reset" shows the terminal's (maybe transparent, maybe light) background, and
    /// the colours were only checked against ours. Popups clear their area, so they are the risk.
    #[test]
    fn every_screen_paints_a_solid_background_at_every_size() {
        for (width, height) in [(120, 36), (100, 31), (70, 26), (50, 20)] {
            for (name, setup) in [
                ("audit", (|_: &mut App| {}) as fn(&mut App)),
                ("audit + help", |a| a.show_help = true),
                ("audit, menu focused", |a| a.focus = Focus::Sidebar),
                ("settings", |a| a.page = Page::Settings),
                ("placeholder", |a| a.page = Page::Run),
                ("form", |a| a.screen = Screen::Form(SettingsForm::new(&settings(false)))),
                ("loading", |a| a.screen = Screen::Loading),
                ("fatal", |a| a.screen = Screen::Fatal("No keychain".into())),
            ] {
                let (_rt, mut app) = audit_app();
                setup(&mut app);
                let buffer = draw_at(&mut app, width, height);
                for y in 0..height {
                    for x in 0..width {
                        let bg = buffer[(x, y)].bg;
                        assert!(matches!(bg, Color::Rgb(..)), "{name} at {width}x{height}: cell ({x},{y}) {:?} has background {bg:?}", buffer[(x, y)].symbol());
                    }
                }
            }
        }
    }

    #[test]
    fn narrow_windows_drop_the_chart_instead_of_squeezing_it() {
        let (_rt, mut app) = audit_app();
        let wide = text_of(&draw_at(&mut app, 120, 36));
        assert!(wide.contains("activity · last 14 days") && wide.contains("■ changes"), "{wide}");
        let narrow = text_of(&draw_at(&mut app, 70, 26));
        assert!(!narrow.contains("activity"), "{narrow}");
        assert!(narrow.contains("totals"), "{narrow}");
        assert!(!narrow.contains("changesnow"), "labels must not run together:\n{narrow}");
    }

    #[test]
    fn long_summaries_end_in_an_ellipsis_instead_of_being_cut_mid_word() {
        let (_rt, mut app) = audit_app();
        let screen = text_of(&draw_at(&mut app, 70, 26));
        assert!(screen.contains("…"), "{screen}");
        // The full text is always one keypress away in the details panel.
        assert!(screen.contains("Summary  Deleted tag ScriptRunner_Test_Static"), "{screen}");
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("abcdefghij", 6), "abcde…");
        assert_eq!(truncate("abc", 0), "…");
    }

    #[test]
    fn meaning_never_depends_on_colour_alone() {
        let (_rt, mut app) = audit_app();
        if let Load::Ready(log) = &mut app.audit {
            log.entries.push(AuditEntry::new("script.run", "Ran it", Err("HTTP 500".into()), Value::Null));
        }
        let screen = text_of(&draw_at(&mut app, 120, 36));
        assert!(screen.contains('✓') && screen.contains('✗'), "success and failure need different glyphs:\n{screen}");
        assert!(screen.contains("% ok"), "the gauge needs a number:\n{screen}");
    }
}
