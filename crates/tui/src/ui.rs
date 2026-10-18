//! Drawing. These functions only read the app state; they never change it
//! (apart from table scroll positions, which ratatui updates while rendering).

use chrono::{DateTime, Local, Utc};
use nvoke_core::{
    audit::AuditEntry,
    jobs::Job,
    task_state::{PhaseCounts, ResultGroup, TaskPhase, group_headline, outcome_label, task_outcome},
};
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
    app::{App, Field, Focus, Load, Page, Pane, Screen, SettingsForm},
    input::TextInput,
    theme::{self, ACCENT, BORDER, DIM, ERR, OK, SCRIPT, SELECTED_BG, TEXT, WARN, panel, panel_padded},
};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub(crate) fn spinner(tick: u64) -> &'static str {
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
        Page::Run => crate::run_ui::draw_run(frame, content, app, focused),
        Page::Tags if app.tag_form.is_some() => crate::tag_form_ui::draw_tag_form(frame, content, app),
        Page::Tags => crate::tags_ui::draw_tags(frame, content, app, focused),
        Page::Activity => draw_activity(frame, content, app, focused),
        Page::Audit => draw_audit(frame, content, app, focused),
        Page::Settings => draw_settings(frame, content, app, focused),
    }

    let hints: &[(&str, &str)] = match (app.focus, app.page) {
        (Focus::Sidebar, p) if p.interactive() => &[("↑↓", "page"), ("→", "open"), ("?", "help"), ("q", "quit")],
        (Focus::Sidebar, _) => &[("↑↓", "page"), ("?", "help"), ("q", "quit")],
        (Focus::Content, Page::Tags) if app.tag_form.as_ref().is_some_and(|f| f.editing) => &[("type", "edit"), ("enter", "done"), ("tab", "next field"), ("esc", "done")],
        (Focus::Content, Page::Tags) if app.tag_form.is_some() && app.picker.is_none() => &[("↑↓", "field"), ("enter", "choose/edit"), ("←→", "part/switch"), ("+", "add row"), ("x", "remove row"), ("esc", "leave")],
        (Focus::Content, Page::Tags) if app.picker.is_some() => &[("type", "search"), ("↑↓", "move"), ("enter", "add/remove"), ("esc", "done")],
        (Focus::Content, Page::Tags) if app.tags.searching => &[("type", "filter"), ("enter", "done"), ("esc", "clear")],
        (Focus::Content, Page::Tags) if app.tags.pane == crate::app::TagsPane::Devices => &[("↑↓", "device"), ("space", "tick"), ("a", "all"), ("x", "remove"), ("+", "add"), ("R", "run script"), ("←", "back")],
        (Focus::Content, Page::Tags) => &[("↑↓", "tag"), ("/", "search"), ("→", "devices"), ("n", "new"), ("e", "edit"), ("+", "add devices"), ("x", "delete"), ("R", "run script"), ("←", "back")],
        (Focus::Content, Page::Run) if app.picker.is_some() => &[("type", "search"), ("↑↓", "move"), ("enter", "choose"), ("esc", "close")],
        (Focus::Content, Page::Run) if app.run.editing => &[("type", "edit"), ("enter", "done"), ("esc", "done"), ("tab", "next field")],
        (Focus::Content, Page::Run) if app.run.pane == crate::app::RunPane::Devices => &[("↑↓", "device"), ("space", "include/leave out"), ("a", "all/none"), ("←", "back to form")],
        (Focus::Content, Page::Run) => &[("↑↓", "field"), ("enter", "choose/edit"), ("→", "devices"), ("←", "back"), ("?", "help")],
        (Focus::Content, Page::Activity) if app.pane == Pane::Results => &[("↑↓", "group"), ("enter", "read the output"), ("PgUp/PgDn", "scroll"), ("←", "back"), ("?", "help")],
        (Focus::Content, Page::Activity) => &[("↑↓", "run"), ("→", "results"), ("s", "stop"), ("w", "watch"), ("R", "run again"), ("F", "re-run failed"), ("T", "tag devices"), ("W", "watch task"), ("x", "remove"), ("←", "back")],
        (Focus::Content, Page::Audit) => &[("↑↓", "move"), ("PgUp/PgDn", "jump"), ("r", "refresh"), ("←", "back"), ("?", "help")],
        (Focus::Content, _) => &[("enter", "edit"), ("←", "back"), ("?", "help"), ("q", "quit")],
    };
    let viewer_hints: Vec<(&str, &str)> = match &app.viewer {
        _ if app.watch.is_some() => vec![("esc", "cancel"), ("enter", "watch"), ("paste", "the task ID")],
        Some(v) if v.searching => vec![("esc", "cancel"), ("enter", "find"), ("type", "search")],
        Some(v) => {
            // Most important first: a narrow window cuts the end of the line, and what must never go is the way out.
            let mut h = vec![("esc", "close"), ("↑↓", "scroll"), ("PgUp/PgDn", "page")];
            if v.task_ids.len() > 1 {
                h.push(("←→", "device"));
            }
            h.extend([("/", "search"), ("n/N", "next/prev"), ("w", "wrap"), ("y", "copy"), ("e", "save")]);
            h
        }
        None => vec![],
    };
    let hints: &[(&str, &str)] = if app.viewer.is_some() || app.watch.is_some() { &viewer_hints } else { hints };
    let dry_hint = [("d", "held-back changes")];
    let hints: Vec<(&str, &str)> = if app.is_dry_run() { hints.iter().chain(dry_hint.iter()).copied().collect() } else { hints.to_vec() };
    let hints = hints.as_slice();
    let flash = app.flash.as_ref().filter(|(_, at)| app.tick.saturating_sub(*at) < 50);
    if let (None, Some((notice, _))) = (&app.confirm, flash) {
        let color = if notice.kind == crate::app::NoticeKind::Error { ERR } else { ACCENT };
        frame.render_widget(Paragraph::new(Line::styled(format!(" {}", notice.text), Style::new().fg(color).add_modifier(Modifier::BOLD))), footer);
    } else if let Some(confirm) = &app.confirm {
        let prompt = Line::from(vec![
            Span::styled(format!(" {} ", confirm.prompt()), Style::new().fg(WARN).add_modifier(Modifier::BOLD)),
            Span::styled("y", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" to confirm, any other key cancels", Style::new().fg(DIM)),
        ]);
        frame.render_widget(Paragraph::new(prompt), footer);
    } else {
        frame.render_widget(Paragraph::new(theme::key_hints(hints)), footer);
    }

    if app.watch.is_some() {
        draw_watch(frame, area, app);
    } else if app.viewer.is_some() {
        crate::viewer_ui::draw_viewer(frame, area, app);
    } else if app.picker.is_some() {
        crate::run_ui::draw_picker(frame, area, app);
    } else if app.show_dry_log {
        draw_dry_log(frame, area, app);
    } else if app.show_help {
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

    // The bar is the logo's violet, edge to edge.
    let bar = Style::new().bg(theme::HEADER_BG).fg(TEXT);
    frame.render_widget(Block::new().style(bar), area);
    let [left_area, right_area] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(right.chars().count() as u16)]).areas(area);

    // The logo's mark: a white N, a cyan prompt and a blinking cursor. The wordmark's
    // colours drift between cyan and white as the ticks advance.
    let cursor = if theme::cursor_visible(app.tick) { "_" } else { " " };
    let mut left = vec![
        Span::raw(" "),
        Span::styled("N", bar.fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::styled(">", bar.fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(cursor, bar.fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::raw("  "),
    ];
    left.extend("N-voke".chars().enumerate().map(|(i, c)| Span::styled(c.to_string(), bar.fg(theme::shimmer(i, app.tick)).add_modifier(Modifier::BOLD))));
    left.push(Span::styled(format!("  /  {}", app.page.title()), bar.fg(DIM)));
    if app.is_dry_run() {
        // Loud on purpose: it must be obvious at a glance that nothing is being sent.
        left.push(Span::raw("   "));
        left.push(Span::styled(" DRY RUN · nothing is sent ", Style::new().fg(theme::BG).bg(WARN).add_modifier(Modifier::BOLD)));
    }
    if app.key_rejected {
        left.push(Span::raw("   "));
        left.push(Span::styled(" API KEY REJECTED · replace it in Settings ", Style::new().fg(theme::BG).bg(ERR).add_modifier(Modifier::BOLD)));
    }
    frame.render_widget(Paragraph::new(Line::from(left)).style(bar), left_area);
    frame.render_widget(Paragraph::new(right).style(bar.fg(DIM)).alignment(Alignment::Right), right_area);
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
        if app.key_rejected {
            lines.push(Line::from(vec![Span::raw(" "), Span::styled("✗", Style::new().fg(ERR).add_modifier(Modifier::BOLD)), Span::styled(" API key rejected", Style::new().fg(ERR))]));
        } else {
            lines.push(Line::from(vec![Span::raw(" "), mark(s.has_api_key), Span::styled(" API key", Style::new().fg(TEXT))]));
        }
        lines.push(Line::from(vec![Span::raw(" "), mark(s.encryption_available), Span::styled(" encrypted", Style::new().fg(TEXT))]));
        let history = if s.history_days == 0 { "no history".to_string() } else { format!("history {}d", s.history_days) };
        lines.push(Line::styled(format!("   {history}"), Style::new().fg(DIM)));
    }
    lines.push(Line::from(""));
    let watching = app.job_list.iter().filter(|j| j.is_active()).count();
    lines.push(if watching > 0 {
        Line::from(vec![Span::styled(format!(" {} ", spinner(app.tick)), Style::new().fg(ACCENT)), Span::styled(format!("watching {watching} run{}", if watching == 1 { "" } else { "s" }), Style::new().fg(ACCENT))])
    } else if app.busy() {
        Line::from(vec![Span::styled(format!(" {} ", spinner(app.tick)), Style::new().fg(ACCENT)), Span::styled("working", Style::new().fg(ACCENT))])
    } else {
        Line::styled(" ● idle", Style::new().fg(DIM))
    });
    frame.render_widget(Paragraph::new(lines).block(panel("status", false)), status);
}

// ---- activity ----------------------------------------------------------------

pub fn phase_color(phase: TaskPhase) -> Color {
    match phase {
        TaskPhase::Succeeded => OK,
        TaskPhase::NonZero => WARN,
        TaskPhase::Failed => ERR,
        TaskPhase::Running | TaskPhase::Queued => ACCENT,
        TaskPhase::Stopped => DIM,
    }
}

/// A symbol for each phase, so the state never rests on colour alone.
fn phase_glyph(phase: TaskPhase, tick: u64) -> &'static str {
    match phase {
        TaskPhase::Succeeded => "✓",
        TaskPhase::NonZero => "!",
        TaskPhase::Failed => "✗",
        TaskPhase::Running => spinner(tick),
        TaskPhase::Queued => "…",
        TaskPhase::Stopped => "○",
    }
}

/// How a whole job looks in the list: still going, or how it ended.
fn job_glyph(job: &Job, counts: &PhaseCounts, tick: u64) -> (&'static str, Color) {
    if job.is_active() {
        (spinner(tick), ACCENT)
    } else if counts.failed > 0 {
        ("✗", ERR)
    } else if counts.non_zero > 0 {
        ("!", WARN)
    } else if counts.succeeded > 0 && counts.succeeded == counts.total() {
        ("✓", OK)
    } else {
        ("○", DIM)
    }
}

/// A bar `width` cells wide split by outcome: heavy line for done, a light
/// dashed line for waiting, and a moving texture for what is running now.
pub fn progress_cells(counts: &PhaseCounts, width: usize, tick: u64) -> Vec<(char, Color)> {
    let total = counts.total();
    if total == 0 || width == 0 {
        return vec![('╌', BORDER); width];
    }
    let parts = [
        (counts.succeeded, TaskPhase::Succeeded),
        (counts.non_zero, TaskPhase::NonZero),
        (counts.failed, TaskPhase::Failed),
        (counts.running, TaskPhase::Running),
        (counts.queued, TaskPhase::Queued),
        (counts.stopped, TaskPhase::Stopped),
    ];
    // Largest-remainder rounding, so the cells always add up to `width` and any non-empty part shows.
    let mut cells: Vec<usize> = parts.iter().map(|(n, _)| n * width / total).collect();
    for (i, (n, _)) in parts.iter().enumerate() {
        if *n > 0 && cells[i] == 0 {
            cells[i] = 1;
        }
    }
    while cells.iter().sum::<usize>() > width {
        let i = (0..cells.len()).max_by_key(|&i| cells[i]).unwrap();
        cells[i] -= 1;
    }
    while cells.iter().sum::<usize>() < width {
        let i = (0..parts.len()).filter(|&i| parts[i].0 > 0).max_by_key(|&i| parts[i].0 * width % total).unwrap();
        cells[i] += 1;
    }
    let mut out = vec![];
    for ((_, phase), n) in parts.iter().zip(cells) {
        for k in 0..n {
            let glyph = match phase {
                TaskPhase::Succeeded | TaskPhase::NonZero | TaskPhase::Failed => '━',
                TaskPhase::Running => if (k as u64 + tick / 3).is_multiple_of(2) { '━' } else { '╸' },
                TaskPhase::Queued | TaskPhase::Stopped => '╌',
            };
            out.push((glyph, phase_color(*phase)));
        }
    }
    out
}

fn bar_spans(counts: &PhaseCounts, width: usize, tick: u64) -> Line<'static> {
    Line::from(progress_cells(counts, width, tick).into_iter().map(|(c, color)| Span::styled(c.to_string(), Style::new().fg(color))).collect::<Vec<_>>())
}

fn draw_activity(frame: &mut Frame, area: Rect, app: &mut App, focused: bool) {
    if app.job_list.is_empty() {
        let text = if app.jobs_loaded() {
            vec![
                Line::from(""),
                Line::styled("No runs yet.", Style::new().fg(TEXT).add_modifier(Modifier::BOLD)),
                Line::from(""),
                Line::styled("Runs started in the desktop app show up here too: both share one history.", Style::new().fg(DIM)),
            ]
        } else {
            vec![Line::from(""), Line::styled(format!("{} Loading run history…", spinner(app.tick)), Style::new().fg(ACCENT))]
        };
        frame.render_widget(Paragraph::new(text).alignment(Alignment::Center).block(panel("activity", focused)), area);
        return;
    }

    // Side by side when there is room, stacked otherwise.
    let [jobs_area, detail_area] = if area.width >= 100 {
        Layout::horizontal([Constraint::Fill(3), Constraint::Fill(2)]).areas(area)
    } else {
        // The list only needs its rows (plus borders, header and the footer line), and the rest is for the
        // run's detail, where the output is. Never more than 40% however many runs there are.
        let wanted = (app.job_list.len() as u16 + 4).max(5);
        let cap = (area.height * 2 / 5).max(5);
        Layout::vertical([Constraint::Length(wanted.min(cap)), Constraint::Fill(1)]).areas(area)
    };
    draw_job_list(frame, jobs_area, app, focused && app.pane == Pane::Jobs);
    draw_job_detail(frame, detail_area, app, focused && app.pane == Pane::Results);
}

fn draw_job_list(frame: &mut Frame, area: Rect, app: &mut App, focused: bool) {
    let inner_width = area.width.saturating_sub(2) as usize;
    let show_targets = inner_width >= 80;
    let bar_width = if inner_width >= 60 { 16 } else { 10 };
    let now = Utc::now();
    let tick = app.tick;

    let mut widths = vec![Constraint::Length(1), Constraint::Fill(2)];
    let mut headers = vec!["", "Script"];
    if show_targets {
        widths.push(Constraint::Fill(2));
        headers.push("Targets");
    }
    widths.extend([Constraint::Length(bar_width as u16), Constraint::Length(7), Constraint::Length(8)]);
    headers.extend(["Progress", "Done", "Started"]);

    // The two flexible columns split what the fixed ones leave over (2 for the marker, 2 between columns).
    let columns = if show_targets { 6 } else { 5 };
    let fixed = 2 + 1 + bar_width + 7 + 8 + 2 * (columns - 1);
    let flexible = inner_width.saturating_sub(fixed);
    let (script_width, target_width) = if show_targets { (flexible / 2, flexible - flexible / 2) } else { (flexible, 0) };

    let rows: Vec<Row> = app
        .job_list
        .iter()
        .map(|job| {
            let counts = nvoke_core::task_state::count_phases(&job.tasks);
            let (glyph, color) = job_glyph(job, &counts, tick);
            let done = counts.succeeded + counts.non_zero + counts.failed;
            let mut cells = vec![Cell::from(glyph).style(Style::new().fg(color)), Cell::from(truncate(&job.script_name, script_width)).style(Style::new().fg(TEXT))];
            if show_targets {
                cells.push(Cell::from(truncate(&job.target_label, target_width)).style(Style::new().fg(DIM)));
            }
            cells.push(Cell::from(bar_spans(&counts, bar_width, tick)));
            cells.push(Cell::from(format!("{done}/{}", counts.total())).style(Style::new().fg(TEXT)));
            cells.push(Cell::from(parse_time(&job.created_at).map(|t| ago(t, now)).unwrap_or_default()).style(Style::new().fg(DIM)));
            Row::new(cells)
        })
        .collect();

    let running = app.job_list.iter().filter(|j| j.is_active()).count();
    let footer = format!(" {} run{} · {running} running ", app.job_list.len(), if app.job_list.len() == 1 { "" } else { "s" });
    let table = Table::new(rows, widths)
        .column_spacing(2)
        .header(Row::new(headers).style(Style::new().fg(DIM).add_modifier(Modifier::BOLD)))
        .row_highlight_style(Style::new().bg(SELECTED_BG).add_modifier(Modifier::BOLD))
        .highlight_symbol("▌ ")
        .block(panel("activity", focused).title_bottom(theme::footer_text(&footer)));
    frame.render_stateful_widget(table, area, &mut app.job_table);
}

fn count_spans(counts: &PhaseCounts) -> Vec<Span<'static>> {
    let parts = [
        (counts.succeeded, TaskPhase::Succeeded, "passed"),
        (counts.non_zero, TaskPhase::NonZero, "non-zero"),
        (counts.failed, TaskPhase::Failed, "didn't run"),
        (counts.running, TaskPhase::Running, "running"),
        (counts.queued, TaskPhase::Queued, "waiting"),
        (counts.stopped, TaskPhase::Stopped, "not watched"),
    ];
    parts
        .iter()
        .filter(|(n, _, _)| *n > 0)
        .enumerate()
        .flat_map(|(i, (n, phase, label))| {
            // A still dot for "running": the summary doesn't animate, the list does.
            let glyph = if *phase == TaskPhase::Running { "●" } else { phase_glyph(*phase, 0) };
            let counted = Span::styled(format!("{glyph} {n} {label}"), Style::new().fg(phase_color(*phase)));
            if i == 0 { vec![counted] } else { vec![Span::raw("  "), counted] }
        })
        .collect()
}

fn draw_job_detail(frame: &mut Frame, area: Rect, app: &mut App, focused: bool) {
    let Some(job) = app.current_job().cloned() else { return };
    let now = Utc::now();

    // The output is what people come here to read, so it gets priority: on a small window the summary
    // shrinks, then goes, and the results table gets shorter, but the output keeps at least MIN_OUTPUT rows.
    const MIN_OUTPUT: u16 = 7;
    let summary_h: u16 = if area.height >= 32 { 5 } else if area.height >= 24 { 4 } else { 0 };
    let results_max: u16 = if area.height >= 34 { 9 } else if area.height >= 24 { 6 } else { 5 };
    let group_rows = (app.groups.len() as u16 + if area.height >= 24 { 3 } else { 2 }).clamp(3, results_max);
    let group_rows = group_rows.min(area.height.saturating_sub(summary_h + MIN_OUTPUT)).max(3);
    let [summary_area, results_area, output_area] = Layout::vertical([Constraint::Length(summary_h), Constraint::Length(group_rows), Constraint::Fill(1)]).areas(area);

    // Which run this is, and how it stands.
    let started = parse_time(&job.created_at).map(|t| ago(t, now)).unwrap_or_default();
    let org = job.org_name.clone().map(|o| format!("{o} · ")).unwrap_or_default();
    let mut lines = vec![
        Line::from(vec![Span::styled(job.script_name.clone(), Style::new().fg(TEXT).add_modifier(Modifier::BOLD)), Span::styled(format!("  →  {}", job.target_label), Style::new().fg(DIM))]),
        Line::styled(format!("{org}started {started}"), Style::new().fg(DIM)),
        Line::from(count_spans(&app.counts)),
    ];
    if job.warnings.as_ref().is_some_and(|w| !w.is_empty()) {
        lines.truncate(2);
        lines.push(Line::styled(format!("{} warning(s) when it was created", job.warnings.as_ref().map_or(0, Vec::len)), Style::new().fg(WARN)));
    }
    if summary_h >= 5 {
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(theme::panel_padded("run", false)), summary_area);
    } else if summary_h > 0 {
        // Two lines: which run, and how it stands. (The organization and start time go first.)
        let head = Line::from(vec![Span::styled(job.script_name.clone(), Style::new().fg(TEXT).add_modifier(Modifier::BOLD)), Span::styled(format!("  →  {}", job.target_label), Style::new().fg(DIM))]);
        frame.render_widget(Paragraph::new(vec![head, Line::from(count_spans(&app.counts))]).wrap(Wrap { trim: false }).block(theme::panel_padded("run", false)), summary_area);
    }

    // The groups: devices with the same result read as one line.
    let tasks = job.tasks.clone();
    let tick = app.tick;
    // What the count and label columns (and the gaps and marker) leave for the headline.
    let headline_width = (results_area.width as usize).saturating_sub(2 + 2 + 5 + 16 + 4);
    let rows: Vec<Row> = app
        .groups
        .iter()
        .map(|g| {
            let color = phase_color(g.phase);
            let label = group_label(g, &tasks);
            Row::new(vec![
                Cell::from(format!("{:>3} ×", g.tasks.len())).style(Style::new().fg(TEXT)),
                Cell::from(format!("{} {label}", phase_glyph(g.phase, tick))).style(Style::new().fg(color)),
                Cell::from(if g.phase.is_finished() { truncate(&group_headline(g, &tasks), headline_width) } else { String::new() }).style(Style::new().fg(DIM)),
            ])
        })
        .collect();
    let table = Table::new(rows, [Constraint::Length(5), Constraint::Length(16), Constraint::Fill(1)])
        .column_spacing(2)
        .row_highlight_style(Style::new().bg(SELECTED_BG).add_modifier(Modifier::BOLD))
        .highlight_symbol(if focused { "▌ " } else { "  " })
        .block({
            let block = panel("results", focused);
            // With no room for the summary, the counts live here so they are never lost.
            if summary_h == 0 { block.title_bottom(theme::footer(count_spans(&app.counts).into_iter().filter(|s| !s.content.trim().is_empty()).collect())) } else { block }
        });
    frame.render_stateful_widget(table, results_area, &mut app.group_table);

    // Which devices, and what they printed.
    let mut out: Vec<Line> = vec![];
    if let Some(g) = app.current_group() {
        let names: Vec<&str> = g.tasks.iter().map(|&i| tasks[i].device_name.as_str()).collect();
        let shown = names.iter().take(30).copied().collect::<Vec<_>>().join(", ");
        let more = if names.len() > 30 { format!(" … +{} more", names.len() - 30) } else { String::new() };
        out.push(Line::from(vec![Span::styled(format!("{} ", plural_devices(names.len())), Style::new().fg(DIM)), Span::styled(format!("{shown}{more}"), Style::new().fg(TEXT))]));
        if g.phase.is_finished() {
            let section = |out: &mut Vec<Line<'static>>, title: &str, text: &str, color: Color| {
                if !text.is_empty() {
                    out.push(Line::from(""));
                    out.push(Line::styled(title.to_string(), Style::new().fg(DIM).add_modifier(Modifier::BOLD)));
                    out.extend(text.lines().map(|l| Line::styled(l.to_string(), Style::new().fg(color))));
                }
            };
            section(&mut out, "output", &g.stdout, TEXT);
            section(&mut out, "errors", &g.stderr, ERR);
            // The same plain-words meaning as the full viewer, so the two never disagree.
            let meaning = g.tasks.first().and_then(|&i| crate::output_view::meaning(&tasks[i]));
            match &meaning {
                Some(m) => {
                    section(&mut out, "what this means", &m.explanation, TEXT);
                    if let Some(words) = &m.generic_wording {
                        out.push(Line::styled(format!("N-able's own words: “{words}”"), Style::new().fg(DIM)));
                    }
                    if let Some(problem) = &m.problem {
                        section(&mut out, "N-able says", problem, WARN);
                    }
                }
                None => section(&mut out, "N-able says", &g.error_message, WARN),
            }
            if g.stdout.is_empty() && g.stderr.is_empty() && g.error_message.is_empty() && meaning.is_none() {
                out.push(Line::from(""));
                out.push(Line::styled("No output.", Style::new().fg(DIM)));
            }
        } else {
            out.push(Line::from(""));
            out.push(Line::styled(format!("{} Waiting for results…", spinner(tick)), Style::new().fg(ACCENT)));
        }
    }
    // Don't scroll past the end, and say when there is more than fits.
    let text_width = output_area.width.saturating_sub(4).max(1) as usize; // borders and the padding inside them
    let rows = output_area.height.saturating_sub(2) as usize;
    let total: usize = out.iter().map(|l| l.width().max(1).div_ceil(text_width)).sum();
    let max_scroll = total.saturating_sub(rows);
    app.output_scroll = app.output_scroll.min(max_scroll as u16);
    // Part of the results pane but not where focus is shown: one title chip at a time, so it is clear where you are.
    let mut block = theme::panel_padded("devices & output", false);
    if total > rows {
        let at = app.output_scroll as usize;
        let hint = format!(" lines {}–{} of {total} · enter opens the full view ", at + 1, (at + rows).min(total));
        block = block.title_bottom(theme::footer(vec![Span::styled(hint.trim().to_string(), Style::new().fg(WARN))]));
    }
    let output = Paragraph::new(out).wrap(Wrap { trim: false }).scroll((app.output_scroll, 0)).block(block);
    frame.render_widget(output, output_area);
}

fn plural_devices(n: usize) -> String {
    format!("{n} device{}:", if n == 1 { "" } else { "s" })
}

/// "Passed", "Exit 1001", "Waiting for device"...
fn group_label(g: &ResultGroup, tasks: &[nvoke_core::task_state::JobTask]) -> String {
    match (g.phase, g.tasks.first().and_then(|&i| task_outcome(&tasks[i]))) {
        (TaskPhase::NonZero | TaskPhase::Failed, Some(o)) => outcome_label(&o),
        (TaskPhase::Succeeded, _) => "Passed".into(),
        (TaskPhase::Queued, _) => "Waiting".into(),
        (phase, _) => phase.label().into(),
    }
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
pub(crate) fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        text.to_string()
    } else {
        text.chars().take(width.saturating_sub(1)).chain(std::iter::once('…')).collect()
    }
}

pub(crate) fn parse_time(text: &str) -> Option<DateTime<Utc>> {
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

pub(crate) fn ago(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (now - at).num_seconds().max(0);
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}

pub(crate) fn action_color(action: &str) -> Color {
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
        .block(panel("audit log", focused).title_bottom(theme::footer_text(&format!("{total} entries"))));
    frame.render_stateful_widget(table, table_area, &mut app.audit_table);

    // Only when there is something to scroll: otherwise it is a column of blocks that means nothing.
    let visible_rows = table_area.height.saturating_sub(3) as usize; // borders and the header row
    if total > visible_rows {
        let mut scroll = ScrollbarState::new(total).position(app.audit_table.selected().unwrap_or(0));
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight).begin_symbol(None).end_symbol(None).track_symbol(None).style(Style::new().fg(BORDER)),
            table_area.inner(ratatui::layout::Margin { vertical: 1, horizontal: 0 }),
            &mut scroll,
        );
    }

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
    let detail = Paragraph::new(lines).style(Style::new().fg(TEXT)).wrap(Wrap { trim: true }).block(panel_padded("details", false).title_bottom(theme::footer_text(&log.path)));
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
    let mode = if app.is_dry_run() {
        Span::styled("DRY RUN: changes are held back, not sent", Style::new().fg(WARN).add_modifier(Modifier::BOLD))
    } else {
        Span::styled("live", Style::new().fg(TEXT))
    };
    let mut lines = vec![
        Line::from(""),
        Line::from(vec![label("Mode"), mode]),
        Line::from(vec![label("API endpoint"), Span::styled(s.api_url.clone(), Style::new().fg(TEXT))]),
        Line::from(vec![label("API key saved"), yes_no(s.has_api_key)]),
        Line::from(vec![
            label("API key status"),
            if app.key_rejected { Span::styled("✗ rejected by N-able: expired or revoked", Style::new().fg(ERR).add_modifier(Modifier::BOLD)) } else { Span::styled("no problems seen", Style::new().fg(DIM)) },
        ]),
        Line::from(vec![label("Keep run history"), Span::styled(history, Style::new().fg(TEXT))]),
        Line::from(vec![label("Encrypted at rest"), yes_no(s.encryption_available)]),
    ];
    if let Some(error) = &s.encryption_error {
        lines.push(Line::from(""));
        lines.push(Line::styled(format!(" {error}"), Style::new().fg(WARN)));
    }
    if app.key_rejected {
        lines.push(Line::from(""));
        lines.push(Line::styled(" Create a new API-Access token in the N-able dashboard, then press enter here and paste it in.", Style::new().fg(WARN)));
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

fn draw_dry_log(frame: &mut Frame, area: Rect, app: &mut App) {
    let popup = centered(area, area.width.saturating_sub(6).min(130), area.height.saturating_sub(4));
    clear(frame, popup);
    let entries = app.dry_run_entries();
    let outer = panel("held back · not sent", true).title_bottom(theme::footer_text("↑↓ choose · PgUp/PgDn scroll · esc close"));
    let inner = outer.inner(popup);
    frame.render_widget(outer, popup);

    if entries.is_empty() {
        let text = vec![
            Line::from(""),
            Line::styled("Nothing held back yet.", Style::new().fg(TEXT).add_modifier(Modifier::BOLD)),
            Line::from(""),
            Line::styled("Changes you try to make appear here, with the exact request, instead of being sent.", Style::new().fg(DIM)),
        ];
        frame.render_widget(Paragraph::new(text).alignment(Alignment::Center), inner);
        return;
    }

    let [list_area, detail_area] = Layout::horizontal([Constraint::Percentage(40), Constraint::Fill(1)]).areas(inner);
    let rows = entries.iter().map(|e| {
        let time = e.timestamp.split('T').nth(1).unwrap_or("").trim_end_matches('Z').to_string();
        Row::new(vec![Cell::from(time).style(Style::new().fg(DIM)), Cell::from(truncate(&e.summary, (list_area.width as usize).saturating_sub(14))).style(Style::new().fg(TEXT))])
    });
    let table = Table::new(rows, [Constraint::Length(8), Constraint::Fill(1)])
        .column_spacing(2)
        .row_highlight_style(Style::new().bg(SELECTED_BG).add_modifier(Modifier::BOLD))
        .highlight_symbol("▌ ");
    frame.render_stateful_widget(table, list_area, &mut app.dry_table);

    let selected = app.dry_table.selected().and_then(|i| entries.get(i));
    let mut lines = vec![];
    if let Some(e) = selected {
        let label = |text: &str| Span::styled(format!("{text:<10}"), Style::new().fg(DIM));
        lines.push(Line::from(vec![label("Would do"), Span::styled(e.summary.clone(), Style::new().fg(TEXT).add_modifier(Modifier::BOLD))]));
        lines.push(Line::from(vec![label("Action"), Span::styled(e.action.clone(), Style::new().fg(action_color(&e.action)))]));
        lines.push(Line::from(vec![label("Operation"), Span::styled(e.operation.clone().unwrap_or_default(), Style::new().fg(TEXT))]));
        lines.push(Line::from(vec![label("Sent"), Span::styled("no: held back by dry run", Style::new().fg(WARN))]));
        lines.push(Line::from(""));
        lines.push(Line::styled("variables (passwords redacted)", Style::new().fg(DIM).add_modifier(Modifier::BOLD)));
        lines.extend(serde_json::to_string_pretty(&e.variables).unwrap_or_default().lines().map(|l| Line::styled(l.to_string(), Style::new().fg(TEXT))));
    }
    frame.render_widget(Paragraph::new(lines).scroll((app.dry_scroll, 0)).block(Block::new().borders(ratatui::widgets::Borders::LEFT).border_style(Style::new().fg(BORDER))), detail_area);
}

fn draw_watch(frame: &mut Frame, area: Rect, app: &App) {
    let Some(w) = app.watch.as_ref() else { return };
    let popup = centered(area, area.width.saturating_sub(6).min(80), 11.min(area.height));
    clear(frame, popup);
    let outer = panel_padded("Watch a task", true).title_bottom(theme::footer_text(if w.busy { "looking it up…" } else { "enter watch · esc cancel" }));
    let inner = outer.inner(popup);
    frame.render_widget(outer, popup);
    let [intro, input, message] = Layout::vertical([Constraint::Length(3), Constraint::Length(3), Constraint::Fill(1)]).areas(inner);
    frame.render_widget(Paragraph::new("Follow a run started elsewhere, such as from the PowerShell script. Paste its task ID.").style(Style::new().fg(TEXT)).wrap(Wrap { trim: true }), intro);
    let shown: String = w.input.value.chars().rev().take(input.width.saturating_sub(5) as usize).collect::<Vec<_>>().into_iter().rev().collect();
    let blink = if !w.busy && theme::cursor_visible(app.tick) { "▏" } else { "" };
    let field = Paragraph::new(Line::from(vec![Span::styled(shown, Style::new().fg(TEXT)), Span::styled(blink, Style::new().fg(ACCENT))]))
        .block(ratatui::widgets::Block::bordered().border_type(ratatui::widgets::BorderType::Rounded).title(" Task ID ").border_style(Style::new().fg(ACCENT)));
    frame.render_widget(field, input);
    let status = if w.busy {
        Line::styled(format!("{} Looking it up…", spinner(app.tick)), Style::new().fg(ACCENT))
    } else if let Some(e) = &w.error {
        Line::styled(e.clone(), Style::new().fg(ERR))
    } else {
        Line::from("")
    };
    frame.render_widget(Paragraph::new(status).wrap(Wrap { trim: false }), message);
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let popup = centered(area, 72, 46);
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
        heading("Run"),
        key("↑ ↓", "field · enter chooses or edits"),
        key("→", "devices (space leaves one out, a all/none)"),
        key("enter ×2", "confirm and run (d lists a dry run)"),
        Line::from(""),
        heading("Reading output (enter on a result)"),
        key("↑ ↓ PgUp PgDn", "scroll · home/end for the ends"),
        key("← →", "previous / next device in the group"),
        key("/  n  N", "search · next · previous match"),
        key("w  < >", "wrap lines · scroll sideways when not wrapped"),
        key("y  e", "copy to clipboard · save to a file"),
        Line::from(""),
        heading("Tags"),
        key("↑ ↓  /", "choose a tag · search by name"),
        key("→ then space", "tick devices (static tags) · a all · x remove"),
        key("+  n  e  x", "add devices · new tag · edit · delete"),
        key("R", "run a script on the tag's devices"),
        Line::from(""),
        heading("Activity"),
        key("→ / ←", "runs ⇄ results ⇄ menu"),
        key("s w x", "stop watching / watch / remove"),
        key("R  F", "run again / re-run failed"),
        key("T  W", "tag these devices · watch a task by its ID"),
        Line::from(""),
        heading("Inside a page"),
        key("↑ ↓", "move (j/k work too)"),
        key("PgUp PgDn", "jump ten rows"),
        key("home end", "first / last (g/G)"),
        key("← esc", "back to the menu"),
        key("r", "refresh"),
    ];
    frame.render_widget(Paragraph::new(lines).block(panel("keys", true).title_bottom(theme::footer_text("any key closes"))), popup);
}

/// Blanks an area for a popup. `Clear` also resets the background, so it is repainted.
pub(crate) fn clear(frame: &mut Frame, area: Rect) {
    frame.render_widget(Clear, area);
    frame.render_widget(Block::new().style(theme::background()), area);
}

fn centered_message(frame: &mut Frame, area: Rect, text: &str, color: Color) {
    let popup = centered(area, 70, text.lines().count() as u16 + 2);
    frame.render_widget(Paragraph::new(text).style(Style::new().fg(color)).alignment(Alignment::Center).wrap(Wrap { trim: true }).block(panel("N-voke", false)), popup);
}

/// A `width` x `height` rectangle in the middle of `area` (clamped to fit).
pub(crate) fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let [_, row, _] = Layout::vertical([Constraint::Fill(1), Constraint::Length(height.min(area.height)), Constraint::Fill(1)]).areas(area);
    let [_, cell, _] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(width.min(area.width)), Constraint::Fill(1)]).areas(row);
    cell
}

#[cfg(test)]
mod tests {

    use nvoke_core::{AuditLogView, Core, Settings, audit::AuditEntry};
    use ratatui::{Terminal, backend::TestBackend};
    use serde_json::Value;

    use super::*;

    fn app() -> (tokio::runtime::Runtime, App) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let app = App::new(Core::for_tests("tui"), runtime.handle().clone());
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
                ("activity", |a| a.page = Page::Activity),
                ("activity results", |a| {
                    a.page = Page::Activity;
                    a.pane = Pane::Results;
                }),
                ("fatal", |a| a.screen = Screen::Fatal("No keychain".into())),
            ] {
                let (_rt, mut app) = if name.starts_with("activity") { activity_app() } else { audit_app() };
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
        assert!(screen.contains("ScriptRunner_Test_Static"), "the whole name is in the details even though the table cut it:\n{screen}");
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

    // ---- activity -----------------------------------------------------------

    use nvoke_core::{
        jobs::Job,
        model::{Execution, ExecutionOutput},
        task_state::{JobTask, PhaseCounts},
    };

    fn finished_task(name: &str, status: &str, out: &str, err: Option<&str>) -> JobTask {
        let mut t = JobTask::new(&format!("t-{name}"), None, name, 0);
        t.task_status = Some(status.into());
        t.watching = false;
        t.execution = Some(Execution {
            id: "e".into(),
            status: status.into(),
            exit_code: None,
            error_message: err.map(String::from),
            started_at: None,
            updated_at: None,
            duration_milliseconds: Some(900.0),
            output: Some(ExecutionOutput { run_as: None, std_output: Some(out.into()), std_error: None, outputs: None }),
        });
        t
    }

    fn job(id: &str, script: &str, minutes_ago: i64, tasks: Vec<JobTask>) -> Job {
        Job {
            id: id.into(),
            name: script.into(),
            script_id: None,
            script: None,
            script_name: script.into(),
            org_id: None,
            org_name: Some("Acme Corp".into()),
            target_label: "PatchGroupTwo".into(),
            created_at: (Utc::now() - chrono::Duration::minutes(minutes_ago)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            timeout_seconds: 300,
            warnings: None,
            tasks,
        }
    }

    pub(super) fn activity_app() -> (tokio::runtime::Runtime, App) {
        let (rt, mut app) = app();
        app.settings = Some(settings(true));
        app.screen = Screen::Main;
        app.page = Page::Activity;
        app.focus = Focus::Content;
        let mut tasks: Vec<JobTask> = (0..31).map(|i| finished_task(&format!("PC-{i:02}"), "SUCCEEDED", &format!("PC-{i:02}: TPM present"), None)).collect();
        tasks.extend((31..38).map(|i| finished_task(&format!("PC-{i:02}"), "FAILED", "Fail - TPM not present", Some("something went wrong while parsing the PowerShell output"))));
        tasks.extend((38..40).map(|i| JobTask::new(&format!("t{i}"), None, &format!("PC-{i}"), 0)));
        app.jobs.seed(vec![
            job("1", "BitlockerPreChecker", 3, tasks),
            job("2", "Cleanup temp files", 95, vec![finished_task("SLS-2188", "SUCCEEDED", "Freed 2.1 GB", None)]),
            job("3", "App Usage Tracker", 60 * 26, vec![finished_task("A", "FAILED", "", Some("timed out")), finished_task("B", "SUCCEEDED", "ok", None)]),
        ]);
        app.sync_jobs();
        (rt, app)
    }

    #[test]
    fn activity_lists_runs_with_progress_and_groups_results_by_outcome() {
        let (_rt, mut app) = activity_app();
        let screen = text_of(&draw_at(&mut app, 130, 34));
        for expected in ["BitlockerPreChecker", "Cleanup temp files", "38/40", "3 runs · 1 running", "31 passed", "7 non-zero", "2 waiting", "Non-zero exit", "watching 1 run"] {
            assert!(screen.contains(expected), "missing {expected:?}:\n{screen}");
        }
        // The headline doesn't fit beside the counts, so it must end in an ellipsis, not stop mid-word.
        assert!(screen.contains("Fail - TPM…") || screen.contains("Fail - TPM n…") || screen.contains("Fail - TPM no…"), "{screen}");
    }

    #[test]
    fn the_selected_result_group_shows_its_devices_and_output() {
        let (_rt, mut app) = activity_app();
        app.pane = Pane::Results;
        // Groups are ordered: waiting, passed (31), non-zero (7).
        app.group_table.select(Some(1));
        let screen = text_of(&draw_at(&mut app, 130, 34));
        assert!(screen.contains("31 devices:") && screen.contains("PC-00, PC-01"), "{screen}");
        assert!(screen.contains("{device}: TPM present"), "{screen}");
        assert!(screen.contains("… +1 more"), "long device lists are cut: {screen}");
    }

    #[test]
    fn an_empty_history_explains_itself_and_loading_says_so() {
        let (_rt, mut app) = app();
        app.settings = Some(settings(true));
        app.screen = Screen::Main;
        app.page = Page::Activity;
        let loading = text_of(&draw_at(&mut app, 100, 24));
        assert!(loading.contains("Loading run history"), "{loading}");
        app.jobs.seed(vec![]);
        let empty = text_of(&draw_at(&mut app, 100, 24));
        assert!(empty.contains("No runs yet.") && empty.contains("desktop app"), "{empty}");
    }

    #[test]
    fn the_remove_prompt_replaces_the_key_hints() {
        let (_rt, mut app) = activity_app();
        app.confirm = Some(crate::app::Confirm::RemoveJob("1".into()));
        let screen = text_of(&draw_at(&mut app, 130, 34));
        assert!(screen.contains("Remove this run from your history?"), "{screen}");
    }

    #[test]
    fn progress_bars_always_fill_their_width_and_show_every_non_empty_outcome() {
        let counts = PhaseCounts { succeeded: 31, non_zero: 0, failed: 7, running: 0, queued: 2, stopped: 0 };
        for width in [1, 3, 10, 16, 40] {
            assert_eq!(progress_cells(&counts, width, 0).len(), width);
        }
        // One failure among thousands still gets a visible cell.
        let lopsided = PhaseCounts { succeeded: 5000, failed: 1, ..Default::default() };
        let cells = progress_cells(&lopsided, 16, 0);
        assert_eq!(cells.len(), 16);
        assert!(cells.iter().any(|(_, c)| *c == ERR), "the lone failure must be visible");
        // Nothing yet: a dim placeholder of the right width.
        assert_eq!(progress_cells(&PhaseCounts::default(), 8, 0), vec![('╌', BORDER); 8]);
        // Running cells animate; finished ones do not.
        let running = PhaseCounts { running: 4, ..Default::default() };
        assert_ne!(progress_cells(&running, 8, 0), progress_cells(&running, 8, 3));
        assert_eq!(progress_cells(&counts, 16, 0), progress_cells(&counts, 16, 99));
    }

    #[test]
    fn each_outcome_has_its_own_symbol_and_colour() {
        let phases = [TaskPhase::Succeeded, TaskPhase::NonZero, TaskPhase::Failed, TaskPhase::Queued, TaskPhase::Stopped];
        let glyphs: std::collections::HashSet<_> = phases.iter().map(|p| phase_glyph(*p, 0)).collect();
        assert_eq!(glyphs.len(), phases.len(), "outcomes must be tellable apart without colour");
        assert_ne!(phase_color(TaskPhase::Succeeded), phase_color(TaskPhase::Failed));
        assert_ne!(phase_color(TaskPhase::Failed), phase_color(TaskPhase::NonZero));
    }

    // ---- dry run ------------------------------------------------------------

    use nvoke_core::{api::{RunInput, RunScriptInput}, inputs::InputType, model::InputValue};

    fn dry_app() -> (tokio::runtime::Runtime, App) {
        let (rt, mut app) = app();
        app.core().set_dry_run(true);
        app.settings = Some(settings(true));
        app.screen = Screen::Main;
        (rt, app)
    }

    #[test]
    fn the_banner_shows_in_dry_run_and_only_then() {
        let (_rt, mut app) = dry_app();
        let dry = text_of(&draw_at(&mut app, 120, 30));
        assert!(dry.contains("DRY RUN · nothing is sent") && dry.contains("held-back changes"), "{dry}");
        app.core().set_dry_run(false);
        let live = text_of(&draw_at(&mut app, 120, 30));
        assert!(!live.contains("DRY RUN") && !live.contains("held-back"), "{live}");
    }

    #[test]
    fn the_banner_survives_a_narrow_window_and_settings_names_the_mode() {
        let (_rt, mut app) = dry_app();
        assert!(text_of(&draw_at(&mut app, 60, 20)).contains("DRY RUN"));
        app.page = Page::Settings;
        assert!(text_of(&draw_at(&mut app, 120, 30)).contains("DRY RUN: changes are held back, not sent"));
    }

    #[test]
    fn a_held_back_run_is_listed_with_its_request_and_never_its_password() {
        let (rt, mut app) = dry_app();
        let input = RunScriptInput {
            script_id: "s1".into(),
            script_name: "Cleanup".into(),
            target_label: "PatchGroupTwo".into(),
            asset_ids: vec!["a1".into(), "a2".into()],
            name: "Run Cleanup".into(),
            timeout_seconds: 120,
            inputs: vec![RunInput { name: "Pass".into(), variable: "pw".into(), kind: InputType::Password, value: InputValue { string_value: Some("hunter2-secret".into()), ..Default::default() } }],
        };
        let result = rt.block_on(app.core().run_script(&input));
        assert_eq!(result.err().as_deref(), Some(nvoke_core::DRY_RUN_ERROR));

        app.show_dry_log = true;
        app.dry_table.select(Some(0));
        let screen = text_of(&draw_at(&mut app, 130, 40));
        for expected in ["held back · not sent", "Ran Cleanup on 2 devices (PatchGroupTwo)", "script.run", "no: held back by dry run", "\"assetIds\"", "[redacted]"] {
            assert!(screen.contains(expected), "missing {expected:?}:\n{screen}");
        }
        assert!(!screen.contains("hunter2-secret"), "a password must never be drawn:\n{screen}");
    }

    #[test]
    fn the_held_back_list_explains_itself_when_empty() {
        let (_rt, mut app) = dry_app();
        app.show_dry_log = true;
        let screen = text_of(&draw_at(&mut app, 120, 30));
        assert!(screen.contains("Nothing held back yet.") && screen.contains("instead of being sent"), "{screen}");
    }

    #[test]
    fn the_held_back_list_paints_a_solid_background_at_every_size() {
        for (width, height) in [(130, 40), (80, 24), (50, 16)] {
            for entries in [0, 2] {
                let (rt, mut app) = dry_app();
                rt.block_on(async {
                    for i in 0..entries {
                        let _ = app.core().create_static_tag("o", &format!("Tag{i}"), "", "").await;
                    }
                });
                app.show_dry_log = true;
                app.dry_table.select((entries > 0).then_some(0));
                let buffer = draw_at(&mut app, width, height);
                for y in 0..height {
                    for x in 0..width {
                        assert!(matches!(buffer[(x, y)].bg, Color::Rgb(..)), "{width}x{height} with {entries} entries: cell ({x},{y}) is not painted");
                    }
                }
            }
        }
    }

    #[test]
    fn the_audit_scrollbar_appears_only_when_there_is_something_to_scroll() {
        let edge_has_blocks = |app: &mut App| {
            let buffer = draw_at(app, 110, 30);
            let x = buffer.area.width - 1;
            (0..buffer.area.height).any(|y| buffer[(x, y)].symbol() == "█")
        };
        let (_rt, mut few) = audit_app(); // two entries, plenty of room
        assert!(!edge_has_blocks(&mut few), "a scrollbar for two rows is noise");
        let (_rt2, mut many) = audit_app();
        if let Load::Ready(log) = &mut many.audit {
            for i in 0..60 {
                log.entries.push(AuditEntry::new("tag.create", &format!("Entry {i}"), Ok(()), Value::Null));
            }
        }
        assert!(edge_has_blocks(&mut many), "a long list needs its scrollbar");
    }

    // ---- the run page ---------------------------------------------------------------

    use crate::app::{RunField, RunPane, run::tests as run_tests};
    use nvoke_core::run::{FieldValue, Resolved};
    use ratatui::crossterm::event::KeyCode;

    /// The Run page with an organization, script (with inputs) and tag chosen, as a user would have it.
    fn filled_run_app() -> (tokio::runtime::Runtime, App) {
        let (rt, mut app) = run_tests::run_app();
        run_tests::press(&mut app, KeyCode::Enter);
        run_tests::pump(&mut app);
        run_tests::press(&mut app, KeyCode::Enter);
        run_tests::pump(&mut app);
        run_tests::press(&mut app, KeyCode::Enter);
        run_tests::pump(&mut app);
        run_tests::press(&mut app, KeyCode::Tab);
        run_tests::pump(&mut app);
        run_tests::press(&mut app, KeyCode::Enter);
        run_tests::press(&mut app, KeyCode::Esc);
        run_tests::pump(&mut app);
        app.run.field = RunField::Run;
        (rt, app)
    }

    #[test]
    fn an_empty_run_form_says_what_to_do_first() {
        let (_rt, mut app) = run_tests::run_app();
        let screen = text_of(&draw_at(&mut app, 130, 30));
        for expected in ["run a script", "choose an organization", "Run  ·  Choose an organization", "Choose who to run on and the devices appear here"] {
            assert!(screen.contains(expected), "missing {expected:?}:\n{screen}");
        }
    }

    #[test]
    fn a_filled_form_shows_the_choices_the_inputs_and_the_options() {
        let (_rt, mut app) = filled_run_app();
        app.run.values.insert("pw".into(), FieldValue::Text("hunter2-secret".into()));
        let screen = text_of(&draw_at(&mut app, 130, 32));
        for expected in ["Acme Corp", "BitlockerPre", "PowerShell", "PatchGroupTwo", "Inputs", "Count", "Fast", "[ ] no", "Pass", "Options", "Task name", "auto-named", "Timeout", "120s (the script's own)", "Run on 2 devices"] {
            assert!(screen.contains(expected), "missing {expected:?}:\n{screen}");
        }
        assert!(screen.contains("••••••••••••••") && !screen.contains("hunter2-secret"), "a password must be masked:\n{screen}");
    }

    #[test]
    fn problems_with_inputs_show_only_after_a_first_attempt() {
        let (_rt, mut app) = filled_run_app();
        app.run.values.insert("n".into(), FieldValue::Text("abc".into()));
        assert!(!text_of(&draw_at(&mut app, 130, 32)).contains("expected a whole number"), "don't nag before they've tried");
        app.run.show_errors = true;
        assert!(text_of(&draw_at(&mut app, 130, 32)).contains("✗ expected a whole number"));
        app.run.timeout = crate::input::TextInput::new("9999", false);
        assert!(text_of(&draw_at(&mut app, 130, 32)).contains("✗ use 0–3600 seconds"));
    }

    #[test]
    fn confirming_names_exactly_what_will_run_and_where() {
        let (_rt, mut app) = filled_run_app();
        app.run.confirming = true;
        let screen = text_of(&draw_at(&mut app, 130, 32));
        // It may wrap on a narrow form, but the count always leads, so it can never be the part that is cut off.
        assert!(screen.contains("Confirm: run on 2 devices") && screen.contains("BitlockerPreChecker") && screen.contains("enter again to run"), "{screen}");
        assert!(!screen.contains('⚠'), "no warnings when nothing looks risky:\n{screen}");
    }

    #[test]
    fn confirming_repeats_what_looks_risky_about_the_devices() {
        let (_rt, mut app) = filled_run_app();
        app.run.resolved = Load::Ready(vec![
            resolved(run_tests::device("a1", "PC-1", "CONNECTED", "Windows 11"), &[], true),
            resolved(run_tests::device("a2", "PC-2", "DISCONNECTED", "Windows 10"), &[], true),
            resolved(run_tests::device("a9", "LNX-1", "CONNECTED", "Ubuntu 22.04"), &[], true),
        ]);
        app.run.confirming = true;
        let screen = text_of(&draw_at(&mut app, 130, 34));
        assert!(screen.contains("Confirm: run on 3 devices"), "{screen}");
        assert!(screen.contains("⚠ 1 device is offline") && screen.contains("⚠ 1 device is on an OS"), "{screen}");
        // Leaving the risky ones out clears the warnings.
        app.run.excluded.insert("a2".into());
        app.run.excluded.insert("a9".into());
        assert!(!text_of(&draw_at(&mut app, 130, 34)).contains('⚠'));
    }

    #[test]
    fn a_dry_run_session_says_the_run_will_be_held_back_and_a_live_one_does_not() {
        let (_rt, mut app) = filled_run_app();
        assert!(!text_of(&draw_at(&mut app, 130, 32)).contains("held back"));
        app.core().set_dry_run(true);
        assert!(text_of(&draw_at(&mut app, 130, 32)).contains("dry run: it will be held back, not sent"));
    }

    #[test]
    fn notices_are_shown_under_the_button() {
        let (_rt, mut app) = filled_run_app();
        app.run.notice = Some(crate::app::run::Notice { kind: crate::app::NoticeKind::Error, text: "The script run was not created. Device offline".into() });
        assert!(text_of(&draw_at(&mut app, 130, 32)).contains("The script run was not created."));
    }

    #[test]
    fn typing_in_a_field_shows_what_is_typed() {
        let (_rt, mut app) = filled_run_app();
        app.run.field = RunField::Name;
        run_tests::press(&mut app, KeyCode::Enter);
        run_tests::type_text(&mut app, "Weekly cleanup");
        let screen = text_of(&draw_at(&mut app, 130, 32));
        assert!(screen.contains("Weekly cleanup") && !screen.contains("auto-named"), "{screen}");
        assert!(text_of(&draw_at(&mut app, 130, 32)).contains("type edit"), "the footer explains typing mode");
    }

    fn resolved(asset: nvoke_core::model::Asset, tags: &[&str], picked: bool) -> Resolved {
        Resolved { asset, via_tags: tags.iter().map(|t| t.to_string()).collect(), picked }
    }

    #[test]
    fn the_device_table_flags_offline_devices_wrong_os_and_ones_left_out() {
        let (_rt, mut app) = filled_run_app();
        app.run.resolved = Load::Ready(vec![
            resolved(run_tests::device("a9", "LNX-1", "CONNECTED", "Ubuntu 22.04"), &[], true),
            resolved(run_tests::device("a1", "PC-1", "CONNECTED", "Windows 11"), &["PatchGroupTwo"], true),
            resolved(run_tests::device("a2", "PC-2", "DISCONNECTED", "Windows 10"), &["PatchGroupTwo", "Servers"], false),
        ]);
        app.run.excluded.insert("a1".into());
        let screen = text_of(&draw_at(&mut app, 130, 32));
        assert!(screen.contains("LNX-1 OS?"), "a PowerShell script on Ubuntu is flagged:\n{screen}");
        assert!(screen.contains("PC-2 offline"), "{screen}");
        assert!(screen.contains("[ ]") && screen.contains("[x]"), "left-out and included are marked in text, not just colour:\n{screen}");
        assert!(screen.contains("2 of 3 selected") && screen.contains("1 offline") && screen.contains("1 OS?"), "{screen}");
        assert!(screen.contains("Added directly, PatchGroupTwo") && screen.contains("PatchGroupTwo, Servers"), "where each device came from:\n{screen}");
        assert!(screen.contains("From"), "{screen}");
    }

    #[test]
    fn the_device_table_says_what_is_going_on_while_loading_failing_or_empty() {
        let (_rt, mut app) = filled_run_app();
        app.run.resolved = Load::Loading;
        assert!(text_of(&draw_at(&mut app, 130, 32)).contains("Finding devices…"));
        app.run.resolved = Load::Failed("Access denied (HTTP 403)".into());
        assert!(text_of(&draw_at(&mut app, 130, 32)).contains("Access denied (HTTP 403)"));
        app.run.resolved = Load::Ready(vec![]);
        assert!(text_of(&draw_at(&mut app, 130, 32)).contains("No devices in these tags."));
    }

    #[test]
    fn a_tag_too_big_to_list_warns_that_only_the_first_devices_are_included() {
        let (_rt, mut app) = filled_run_app();
        app.run.truncated = true;
        assert!(text_of(&draw_at(&mut app, 130, 32)).contains("Only the first 1000 devices of a tag are included."));
    }

    #[test]
    fn the_organization_picker_lists_names_and_kinds() {
        let (_rt, mut app) = run_tests::run_app();
        run_tests::press(&mut app, KeyCode::Enter);
        run_tests::pump(&mut app);
        let screen = text_of(&draw_at(&mut app, 130, 32));
        for expected in ["Choose an organization", "Acme Corp", "Beta Ltd", "Customer", "2 matches", "enter choose", "esc cancel"] {
            assert!(screen.contains(expected), "missing {expected:?}:\n{screen}");
        }
    }

    #[test]
    fn the_target_picker_marks_chosen_items_and_shows_which_list_you_are_in() {
        let (_rt, mut app) = filled_run_app();
        app.run.field = RunField::Targets;
        run_tests::press(&mut app, KeyCode::Enter);
        run_tests::pump(&mut app);
        let devices = text_of(&draw_at(&mut app, 130, 32));
        assert!(devices.contains("Devices") && devices.contains("Tags") && devices.contains("PC-2") && devices.contains("LNX-1"), "{devices}");
        assert!(devices.contains("enter add or remove"), "{devices}");
        run_tests::press(&mut app, KeyCode::Tab);
        run_tests::pump(&mut app);
        let tags = text_of(&draw_at(&mut app, 130, 32));
        assert!(tags.contains("✓") && tags.contains("PatchGroupTwo") && tags.contains("Static · 2") && tags.contains("Dynamic · 40"), "{tags}");
    }

    #[test]
    fn a_picker_explains_an_empty_search_a_failure_and_the_wait() {
        let (_rt, mut app) = run_tests::run_app();
        run_tests::press(&mut app, KeyCode::Enter);
        run_tests::pump(&mut app);
        run_tests::type_text(&mut app, "zzz");
        run_tests::pump(&mut app);
        assert!(text_of(&draw_at(&mut app, 130, 32)).contains("Nothing matches."));
        let picker = app.picker.as_mut().unwrap();
        picker.error = Some("Access denied (HTTP 401)".into());
        assert!(text_of(&draw_at(&mut app, 130, 32)).contains("Access denied (HTTP 401)"));
        let picker = app.picker.as_mut().unwrap();
        picker.error = None;
        picker.loading = true;
        assert!(text_of(&draw_at(&mut app, 130, 32)).contains("searching…"));
    }

    #[test]
    fn the_device_picker_invites_you_to_type_when_nothing_is_listed() {
        let (_rt, mut app) = filled_run_app();
        app.run.field = RunField::Targets;
        run_tests::press(&mut app, KeyCode::Enter);
        run_tests::pump(&mut app);
        let picker = app.picker.as_mut().unwrap();
        picker.items.clear();
        picker.loading = false;
        assert!(text_of(&draw_at(&mut app, 130, 32)).contains("Type to search devices, or press tab to pick a tag."));
    }

    #[test]
    fn the_run_page_paints_a_solid_background_at_every_size_and_never_panics() {
        for (name, setup) in [
            ("empty", (|_: &mut App| {}) as fn(&mut App)),
            ("filled", |a| {
                a.run.field = RunField::Input(0);
                a.run.show_errors = true;
                a.run.values.insert("n".into(), FieldValue::Text("abc".into()));
            }),
            ("devices focused", |a| a.run.pane = RunPane::Devices),
            ("confirming", |a| a.run.confirming = true),
            ("org picker", |a| {
                a.run.field = RunField::Org;
                run_tests::press(a, KeyCode::Enter);
                run_tests::pump(a);
            }),
            ("target picker", |a| {
                a.run.field = RunField::Targets;
                run_tests::press(a, KeyCode::Enter);
                run_tests::pump(a);
                run_tests::press(a, KeyCode::Tab);
                run_tests::pump(a);
            }),
        ] {
            // Building the form takes real (if short) waits, so build once per state and draw it at every size.
            let (_rt, mut app) = filled_run_app();
            setup(&mut app);
            for (width, height) in [(130, 34), (100, 28), (80, 24), (60, 20), (40, 12), (24, 8)] {
                let buffer = draw_at(&mut app, width, height);
                for y in 0..height {
                    for x in 0..width {
                        assert!(matches!(buffer[(x, y)].bg, Color::Rgb(..)), "run page {name} at {width}x{height}: cell ({x},{y}) is not painted");
                    }
                }
            }
        }
    }

    #[test]
    fn long_names_end_in_an_ellipsis_in_the_form_and_the_table() {
        let (_rt, mut app) = filled_run_app();
        app.run.script = Some(run_tests::script("s9", "A script with a really long descriptive name that goes on and on", vec![]));
        app.run.resolved = Load::Ready(vec![resolved(run_tests::device("a1", "AN-EXTREMELY-LONG-DEVICE-HOSTNAME-THAT-WILL-NOT-FIT", "CONNECTED", "Windows 11"), &[], true)]);
        let screen = text_of(&draw_at(&mut app, 100, 28));
        assert!(screen.matches('…').count() >= 2, "{screen}");
    }

    // ---- an expired key -------------------------------------------------------------

    const DENIED: &str = "Access denied (HTTP 403): your API key is no longer active. It has expired or been revoked: create a new one in the N-able dashboard and replace it in Settings.";

    #[test]
    fn a_rejected_key_is_flagged_in_the_header_the_sidebar_and_settings() {
        let (_rt, mut app) = run_tests::run_app();
        app.run.field = RunField::Org;
        let calm = text_of(&draw_at(&mut app, 130, 30));
        assert!(!calm.contains("REJECTED") && !calm.contains("rejected"), "no alarm without a refusal:\n{calm}");

        app.key_rejected = true;
        let screen = text_of(&draw_at(&mut app, 130, 30));
        assert!(screen.contains("API KEY REJECTED · replace it in Settings"), "{screen}");
        assert!(screen.contains("✗ API key rejected"), "the sidebar must not keep saying the key is fine:\n{screen}");

        run_tests::go_to(&mut app, Page::Settings);
        let settings = text_of(&draw_at(&mut app, 130, 30));
        assert!(settings.contains("rejected by N-able: expired or revoked") && settings.contains("Create a new API-Access token"), "{settings}");
    }

    #[test]
    fn a_refused_search_shows_the_whole_reason_and_how_to_fix_it() {
        let (_rt, mut app) = run_tests::run_app();
        run_tests::press(&mut app, KeyCode::Enter);
        let seq = app.picker.as_ref().unwrap().seq;
        run_tests::deliver(&mut app, crate::app::Msg::PickResults { seq, result: Err(DENIED.into()) });
        let screen = text_of(&draw_at(&mut app, 130, 30));
        // The message wraps across lines, so check pieces that each sit on one.
        for expected in ["Access denied (HTTP 403)", "no longer active", "revoked: create a new one in the N-able dashboard", "replace it in Settings.", "To fix it: press esc", "5 for Settings"] {
            assert!(screen.contains(expected), "missing {expected:?} (it must not be cut off):\n{screen}");
        }
        // Another kind of failure shows its message but not instructions about the key.
        run_tests::deliver(&mut app, crate::app::Msg::PickResults { seq, result: Err("Request to https://x failed: timed out".into()) });
        let other = text_of(&draw_at(&mut app, 130, 30));
        assert!(other.contains("timed out") && !other.contains("To fix it"), "{other}");
    }

    #[test]
    fn the_rejected_key_states_paint_a_solid_background_at_every_size() {
        for (width, height) in [(130, 34), (80, 24), (50, 16)] {
            let (_rt, mut app) = run_tests::run_app();
            app.key_rejected = true;
            run_tests::press(&mut app, KeyCode::Enter);
            let seq = app.picker.as_ref().unwrap().seq;
            run_tests::deliver(&mut app, crate::app::Msg::PickResults { seq, result: Err(DENIED.into()) });
            let buffer = draw_at(&mut app, width, height);
            for y in 0..height {
                for x in 0..width {
                    assert!(matches!(buffer[(x, y)].bg, Color::Rgb(..)), "{width}x{height}: cell ({x},{y}) is not painted");
                }
            }
        }
    }

    // ---- reading output ---------------------------------------------------------------

    /// The run from the bug report: one device, a non-zero exit, an eight-line report.
    fn bitlocker_app() -> (tokio::runtime::Runtime, App) {
        let (rt, mut app) = app();
        app.settings = Some(settings(true));
        app.screen = Screen::Main;
        app.page = Page::Activity;
        app.focus = Focus::Content;
        app.pane = Pane::Results;
        let report = "Fail - Bitlocker Key Dependancys Not Met\n----------------------------------\nUEFI - False\nTPM Ready - True\nTPM Present - True\nTPM Autoprovisioning - Enabled\nSecure Boot - Off\nBitlocker - Not Enabled";
        let task = finished_task("HP Laptop - Vienna", "FAILED", report, Some("something went wrong while parsing the PowerShell output"));
        app.jobs.seed(vec![job("1", "BitlockerPreChecker", 1, vec![task])]);
        app.sync_jobs();
        (rt, app)
    }

    #[test]
    fn a_small_window_still_shows_a_whole_short_report() {
        // The reported case: the window was about 100x30 and the output panel was squeezed to a few rows.
        for (width, height) in [(104, 33), (100, 30), (96, 28), (90, 26), (80, 24)] {
            let (_rt, mut app) = bitlocker_app();
            let screen = text_of(&draw_at(&mut app, width, height));
            for line in ["Fail - Bitlocker Key Dependancys Not Met", "UEFI - False", "TPM Ready - True", "TPM Present - True", "TPM Autoprovisioning - Enabled", "Secure Boot - Off", "Bitlocker - Not Enabled"] {
                assert!(screen.contains(line), "{width}x{height}: {line:?} is missing:\n{screen}");
            }
        }
    }

    #[test]
    fn one_run_does_not_take_two_fifths_of_a_narrow_window() {
        let (_rt, mut app) = bitlocker_app();
        let buffer = draw_at(&mut app, 100, 36);
        // The jobs panel's bottom border is the line carrying its "1 run · 0 running" footer.
        let screen = text_of(&buffer);
        let footer_row = screen.lines().position(|l| l.contains("1 run · 0 running")).expect("footer");
        assert!(footer_row <= 9, "the run list should be a few rows tall, not 40% of the screen (footer on row {footer_row}):\n{screen}");
    }

    #[test]
    fn a_single_run_reads_as_one_run() {
        let (_rt, mut app) = bitlocker_app();
        let screen = text_of(&draw_at(&mut app, 120, 30));
        assert!(screen.contains("1 run · 0 running") && !screen.contains("1 runs"), "{screen}");
    }

    // ---- boundaries and structure -----------------------------------------------------

    use crate::app::viewer::tests as viewer_tests;

    /// The text of every title "chip" on screen: runs of cells drawn on the accent colour.
    fn chips(buffer: &ratatui::buffer::Buffer) -> Vec<String> {
        let mut found = vec![];
        for y in 0..buffer.area.height {
            let mut current = String::new();
            for x in 0..buffer.area.width {
                let cell = &buffer[(x, y)];
                if cell.bg == ACCENT {
                    current.push_str(cell.symbol());
                } else if !current.is_empty() {
                    found.push(std::mem::take(&mut current).trim().to_string());
                }
            }
            if !current.is_empty() {
                found.push(current.trim().to_string());
            }
        }
        found.into_iter().filter(|c| !c.is_empty()).collect()
    }

    #[test]
    fn exactly_one_panel_shows_where_focus_is() {
        // Two chips at once would make "where am I" a guess; this is the bug seen in the first screenshot.
        type Build = Box<dyn Fn() -> (tokio::runtime::Runtime, App)>;
        let cases: Vec<(&str, Build, &str)> = vec![
            ("activity runs", Box::new(|| { let (rt, mut a) = bitlocker_app(); a.pane = Pane::Jobs; (rt, a) }), "activity"),
            ("activity results", Box::new(bitlocker_app), "results"),
            ("run form", Box::new(run_tests::run_app), "run a script"),
            ("run devices", Box::new(|| { let (rt, mut a) = filled_run_app(); a.run.pane = RunPane::Devices; (rt, a) }), "devices"),
            ("audit", Box::new(|| { let (rt, a) = audit_app(); (rt, a) }), "audit log"),
            ("menu", Box::new(|| { let (rt, mut a) = bitlocker_app(); a.focus = Focus::Sidebar; (rt, a) }), "menu"),
        ];
        for (name, build, expected) in cases {
            let (_rt, mut app) = build();
            let found = chips(&draw_at(&mut app, 130, 34));
            assert_eq!(found, [expected], "{name}: the focus chip(s) were {found:?}");
        }
    }

    #[test]
    fn text_set_into_a_border_is_always_bracketed_like_a_title() {
        let (_rt, mut app) = bitlocker_app();
        let screen = text_of(&draw_at(&mut app, 130, 34));
        assert!(screen.contains("┤ 1 run · 0 running ├"), "the runs footer:\n{screen}");
        let (_rt, mut audit) = audit_app();
        let screen = text_of(&draw_at(&mut audit, 110, 30));
        assert!(screen.contains("┤ 2 entries ├") && screen.contains("┤ /tmp/audit.jsonl ├"), "{screen}");
        let (_rt, mut run) = filled_run_app();
        let screen = text_of(&draw_at(&mut run, 130, 30));
        assert!(screen.contains("┤ 2 of 2 selected ├"), "the devices footer:\n{screen}");
    }

    #[test]
    fn panels_are_a_lighter_surface_than_the_page_and_selection_stands_out_from_both() {
        let (_rt, mut app) = bitlocker_app();
        let buffer = draw_at(&mut app, 130, 34);
        // The menu panel's blank interior, the page's key-hint row, and the selected result row.
        assert_eq!(buffer[(3, 15)].bg, theme::PANEL_BG, "inside a panel");
        assert_eq!(buffer[(60, 33)].bg, theme::BG, "the page, outside any panel");
        let selected = (0..buffer.area.height).any(|y| (0..buffer.area.width).any(|x| buffer[(x, y)].bg == SELECTED_BG));
        assert!(selected, "the selected row has its own colour");
    }

    #[test]
    fn text_never_touches_a_panels_border() {
        let (_rt, mut app) = bitlocker_app();
        let screen = text_of(&draw_at(&mut app, 100, 30));
        let line = screen.lines().find(|l| l.contains("UEFI - False")).expect("the output is shown");
        assert!(line.contains("│ UEFI - False"), "there is a column of space between the border and the text: {line:?}");
        let (_rt, mut audit) = audit_app();
        let screen = text_of(&draw_at(&mut audit, 110, 30));
        let details = screen.lines().find(|l| l.contains("Action")).expect("details");
        assert!(details.contains("│ Action"), "{details:?}");
    }

    // ---- the output viewer ---------------------------------------------------------------

    #[test]
    fn the_viewer_has_a_title_numbered_lines_section_rules_and_a_position_readout() {
        let (_rt, mut app) = viewer_tests::viewer_app();
        viewer_tests::open(&mut app);
        let screen = viewer_tests::draw(&mut app, 100, 30);
        for expected in ["BitlockerPreChecker · PC-1 · device 1 of 3", "── Output ", "  1 line 1: ok", "┤ lines ", " of ", "% · wrap on ├", "Task ID    t-PC-1"] {
            assert!(screen.contains(expected), "missing {expected:?}:\n{screen}");
        }
        // The essentials always show, even where the line is cut: how to leave, scroll, and search.
        for hint in ["esc close", "scroll", "page", "device", "search"] {
            assert!(screen.contains(hint), "hint {hint:?} missing:\n{screen}");
        }
        // With room, everything shows, including switching devices because there are three.
        let wide = viewer_tests::draw(&mut app, 130, 30);
        for hint in ["esc close", "scroll", "page", "device", "search", "next/prev", "wrap", "copy", "save"] {
            assert!(wide.contains(hint), "hint {hint:?} missing at 130 columns:\n{wide}");
        }
        // The way out is visible even in a narrow window, where the end of the hints is cut off.
        assert!(viewer_tests::draw(&mut app, 70, 24).contains("esc close"), "a narrow window must still show how to leave");
        // The last section is at the end: its own rule, then what N-able said.
        run_tests::press(&mut app, KeyCode::End);
        let end = viewer_tests::draw(&mut app, 100, 30);
        assert!(end.contains("── N-able says ") && end.contains("exit status 1") && end.contains("100 line 100: TPM not ready"), "{end}");
    }

    #[test]
    fn search_hits_are_marked_only_while_searching_and_the_current_one_stands_out() {
        let (_rt, mut app) = viewer_tests::viewer_app();
        viewer_tests::open(&mut app);
        // The body is rows 2..=h-4 (below the title border, above the status line).
        let (w, h) = (100u16, 30u16);
        let body = 2..=(h - 4);
        let marked = |b: &ratatui::buffer::Buffer, color: Color| body.clone().flat_map(|y| (0..w).map(move |x| (x, y))).filter(|&(x, y)| b[(x, y)].bg == color && b[(x, y)].symbol() != " ").count();
        let hits_on_screen = |b: &ratatui::buffer::Buffer| body.clone().map(|y| (0..w).map(|x| b[(x, y)].symbol()).collect::<String>().to_lowercase().matches("tpm").count()).sum::<usize>();

        let before = draw_at(&mut app, w, h);
        assert_eq!(marked(&before, WARN) + marked(&before, ACCENT), 0, "nothing is marked until you search");

        run_tests::press(&mut app, KeyCode::Char('/'));
        run_tests::type_text(&mut app, "tpm");
        run_tests::press(&mut app, KeyCode::Enter);
        run_tests::press(&mut app, KeyCode::Char('n')); // move to a later hit, which is below the first screen's worth
        let after = draw_at(&mut app, w, h);
        let visible = hits_on_screen(&after);
        assert!(visible >= 1, "the current match is on screen");
        assert_eq!(marked(&after, WARN) + marked(&after, ACCENT), visible * 3, "every visible hit is marked, three letters each");
        assert_eq!(marked(&after, ACCENT), 3, "exactly one (the current) hit stands out from the rest");

        run_tests::press(&mut app, KeyCode::Esc);
        let cleared = draw_at(&mut app, w, h);
        assert_eq!(marked(&cleared, WARN) + marked(&cleared, ACCENT), 0, "esc clears the marks");
    }

    #[test]
    fn the_viewer_paints_a_solid_background_at_every_size() {
        let (_rt, mut app) = viewer_tests::viewer_app();
        viewer_tests::open(&mut app);
        for (w, h) in [(130, 40), (100, 30), (60, 16), (30, 8), (12, 4)] {
            let buffer = draw_at(&mut app, w, h);
            for y in 0..h {
                for x in 0..w {
                    assert!(matches!(buffer[(x, y)].bg, Color::Rgb(..)), "viewer at {w}x{h}: cell ({x},{y}) is not painted");
                }
            }
        }
    }

    #[test]
    fn the_run_status_is_always_in_the_forms_border_even_when_the_button_is_off_screen() {
        let (_rt, mut app) = run_tests::run_app();
        let screen = text_of(&draw_at(&mut app, 120, 28));
        assert!(screen.contains("┤ Choose an organization ├"), "a blocker is named:\n{screen}");
        let (_rt, mut app) = filled_run_app();
        app.run.field = RunField::Org; // the button is far below the focused row
        // A short window: the form is scrolled to the focus and the Run row is out of sight.
        let screen = text_of(&draw_at(&mut app, 100, 18));
        assert!(screen.contains("┤ ready · run on 2 devices ├"), "{screen}");
        app.run.confirming = true;
        assert!(text_of(&draw_at(&mut app, 100, 18)).contains("enter again to run on 2 devices"));
    }

    // ---- explaining a non-zero exit -----------------------------------------------------------

    #[test]
    fn a_non_zero_exit_is_explained_inline_instead_of_shown_as_an_error() {
        let (_rt, mut app) = bitlocker_app();
        let screen = text_of(&draw_at(&mut app, 130, 40));
        assert!(screen.contains("what this means") && screen.contains("The script ran and finished"), "{screen}");
        // The panel is narrow, so the quoted sentence wraps: check the pieces that sit on one line.
        assert!(screen.contains("N-able's own words:") && screen.contains("wrong while"), "still there, quietly:\n{screen}");
        assert!(!screen.contains("N-able says"), "the generic sentence must not be presented as N-able's complaint:\n{screen}");
    }

    #[test]
    fn the_viewer_explains_it_too_and_only_says_what_is_true_about_wrapping() {
        let (_rt, mut app) = bitlocker_app();
        run_tests::press(&mut app, KeyCode::Enter);
        // Wide enough that nothing wraps, so the whole sentence is on one line and no wrap hint is shown.
        let screen = text_of(&draw_at(&mut app, 170, 34));
        for expected in ["── What this means ", "The script ran and finished", "N-able's own words: “something went wrong while parsing the PowerShell output”"] {
            assert!(screen.contains(expected), "missing {expected:?}:\n{screen}");
        }
        assert!(!screen.contains("N-able says"), "{screen}");
        assert!(!screen.contains("wrapped"), "nothing here wraps, so no hint about wrapping:\n{screen}");
        // With a line that really is wrapped, the hint appears.
        let narrow = text_of(&draw_at(&mut app, 50, 24));
        assert!(narrow.contains("Some long lines are wrapped"), "{narrow}");
    }

    #[test]
    fn a_real_complaint_from_nable_is_still_shown_as_one() {
        let (_rt, mut app) = bitlocker_app();
        let task = finished_task("PC-1", "FAILED", "partial output", Some("error trying to run script: exit status 1001"));
        app.jobs.seed(vec![job("1", "Cleanup", 1, vec![task])]);
        app.sync_jobs();
        let screen = text_of(&draw_at(&mut app, 130, 40));
        assert!(screen.contains("N-able says") && screen.contains("error trying to run script") && screen.contains("what this means") && screen.contains("code 1001"), "{screen}");
    }

    #[test]
    fn a_long_task_id_wraps_under_its_label_not_back_to_the_edge() {
        let (_rt, mut app) = bitlocker_app();
        let mut task = finished_task("HP", "FAILED", "x", Some("y"));
        task.task_id = "MDFhMTUwMzgtM2VhMC03YWUxLWExYjktNjVlZWUwYzEyNWNmOmE3YWY0YjJhLWVjY2MtNDY0MC04MWI0LWVhZDE5NjBkZTc0MA==".into();
        app.jobs.seed(vec![job("1", "S", 1, vec![task])]);
        app.sync_jobs();
        run_tests::press(&mut app, KeyCode::Enter);
        let screen = text_of(&draw_at(&mut app, 60, 24));
        let lines: Vec<&str> = screen.lines().collect();
        let at = lines.iter().position(|l| l.contains("Task ID")).expect("task id row");
        // Columns, not bytes: the border characters are several bytes wide.
        let col = |line: &str, needle: &str| line.find(needle).map(|b| line[..b].chars().count());
        let label_col = col(lines[at], "Task ID").unwrap();
        let value_col = col(lines[at], "MDFh").unwrap();
        assert!(value_col > label_col + 5, "the value starts to the right of its label");
        // The next row is the value's continuation: blank up to the value column, then text.
        let next: Vec<char> = lines[at + 1].chars().collect();
        assert!(next[..value_col].iter().all(|c| matches!(c, ' ' | '│')), "continuation must not start at the label's edge: {:?}", lines[at + 1]);
        assert!(next[value_col] != ' ', "continuation text starts under the value: {:?}", lines[at + 1]);
    }

    // ---- the tags page --------------------------------------------------------------------

    use crate::app::tags::tests as tags_tests;

    #[test]
    fn the_tag_list_shows_names_organizations_counts_and_which_kind_each_is() {
        let (_rt, mut app) = tags_tests::tags_app();
        tags_tests::settle(&mut app);
        let screen = text_of(&draw_at(&mut app, 130, 30));
        for expected in ["Servers", "Windows10", "Zed", "Acme", "◆", "↻", "┤ 3 tags ├", "/ to search"] {
            assert!(screen.contains(expected), "missing {expected:?}:\n{screen}");
        }
    }

    #[test]
    fn a_static_tag_shows_its_description_and_devices_with_tick_boxes() {
        let (_rt, mut app) = tags_tests::tags_app();
        tags_tests::settle(&mut app);
        let screen = text_of(&draw_at(&mut app, 130, 30));
        for expected in ["Static · Acme · 2 devices", "Production servers", "PC-1", "PC-3", "[ ]", "Windows 11", "┤ 2 devices ├"] {
            assert!(screen.contains(expected), "missing {expected:?}:\n{screen}");
        }
        run_tests::press(&mut app, KeyCode::Right);
        run_tests::press(&mut app, KeyCode::Char(' '));
        let ticked = text_of(&draw_at(&mut app, 130, 30));
        assert!(ticked.contains("[x]") && ticked.contains("┤ 1 selected ├"), "{ticked}");
    }

    #[test]
    fn a_dynamic_tag_shows_its_rule_in_words_and_no_tick_boxes() {
        let (_rt, mut app) = tags_tests::tags_app();
        tags_tests::settle(&mut app);
        run_tests::press(&mut app, KeyCode::Down);
        tags_tests::settle(&mut app);
        let screen = text_of(&draw_at(&mut app, 130, 30));
        for expected in ["Dynamic · Acme · 3 devices", "Rule  Operating system contains “Windows 10”", "Membership updates by itself", "PC-4", "PC-5"] {
            assert!(screen.contains(expected), "missing {expected:?}:\n{screen}");
        }
        assert!(!screen.contains("[ ]") && !screen.contains("[x]"), "devices of a dynamic tag are not ticked by hand:\n{screen}");
    }

    #[test]
    fn the_search_box_shows_what_is_typed_and_how_to_clear_it() {
        let (_rt, mut app) = tags_tests::tags_app();
        tags_tests::settle(&mut app);
        run_tests::press(&mut app, KeyCode::Char('/'));
        run_tests::type_text(&mut app, "win");
        tags_tests::settle(&mut app);
        let typing = text_of(&draw_at(&mut app, 130, 30));
        assert!(typing.contains("/ win") && typing.contains("┤ 1 tag ├") && !typing.contains("Zed"), "{typing}");
        assert!(typing.contains("filter") && typing.contains("clear"), "the hints change while typing:\n{typing}");
        run_tests::press(&mut app, KeyCode::Enter);
        assert!(text_of(&draw_at(&mut app, 130, 30)).contains("esc clears"));
    }

    #[test]
    fn the_list_explains_an_empty_search_a_wait_and_a_refused_key() {
        let (_rt, mut app) = tags_tests::tags_app();
        tags_tests::settle(&mut app);
        run_tests::press(&mut app, KeyCode::Char('/'));
        run_tests::type_text(&mut app, "zzzz");
        tags_tests::settle(&mut app);
        assert!(text_of(&draw_at(&mut app, 130, 30)).contains("No matching tags."));
        assert!(text_of(&draw_at(&mut app, 130, 30)).contains("Select a tag"), "and nothing is shown on the right");

        let (_rt, mut loading) = tags_tests::tags_app();
        assert!(text_of(&draw_at(&mut loading, 130, 30)).contains("Loading tags…"), "before anything has arrived");

        let (_rt, mut denied) = tags_tests::tags_app();
        tags_tests::settle(&mut denied);
        tags_tests::fail_list(&mut denied, "Access denied (HTTP 403): your API key is no longer active.");
        let screen = text_of(&draw_at(&mut denied, 130, 30));
        assert!(screen.contains("Access denied") && screen.contains("Replace your key in Settings"), "{screen}");
    }

    #[test]
    fn a_tag_with_no_devices_says_so_and_how_to_add_them() {
        let (_rt, mut app) = tags_tests::tags_app();
        tags_tests::settle(&mut app);
        run_tests::press(&mut app, KeyCode::End);
        tags_tests::settle(&mut app);
        let screen = text_of(&draw_at(&mut app, 130, 30));
        assert!(screen.contains("No devices yet. Press + to add some."), "{screen}");
    }

    #[test]
    fn the_footer_asks_what_a_delete_or_removal_will_do_before_it_happens() {
        let (_rt, mut app) = tags_tests::tags_app();
        tags_tests::settle(&mut app);
        run_tests::press(&mut app, KeyCode::Char('x'));
        let screen = text_of(&draw_at(&mut app, 140, 30));
        assert!(screen.contains("Delete tag Servers? It is removed from 2 devices; the devices are not affected.") && screen.contains("y to confirm"), "{screen}");
    }

    #[test]
    fn the_add_devices_picker_marks_chosen_devices_and_says_enter_adds_or_removes() {
        let (_rt, mut app) = tags_tests::tags_app();
        tags_tests::seed_assets(&app, "o1", "", vec![run_tests::device("a2", "PC-2", "CONNECTED", "Windows 11"), run_tests::device("a9", "LNX-1", "CONNECTED", "Ubuntu 22.04")]);
        tags_tests::settle(&mut app);
        run_tests::press(&mut app, KeyCode::Char('+'));
        run_tests::pump(&mut app);
        run_tests::press(&mut app, KeyCode::Enter);
        let screen = text_of(&draw_at(&mut app, 130, 30));
        for expected in ["Add devices to Servers", "PC-2", "LNX-1", "✓", "enter add or remove", "esc done"] {
            assert!(screen.contains(expected), "missing {expected:?}:\n{screen}");
        }
    }

    #[test]
    fn the_tags_page_has_one_focus_chip_and_it_follows_the_pane() {
        let (_rt, mut app) = tags_tests::tags_app();
        tags_tests::settle(&mut app);
        assert_eq!(chips(&draw_at(&mut app, 130, 30)), ["tags"]);
        run_tests::press(&mut app, KeyCode::Right);
        assert_eq!(chips(&draw_at(&mut app, 130, 30)), ["Servers"], "the devices panel is titled with the tag");
    }

    #[test]
    fn the_tags_page_paints_a_solid_background_at_every_size_and_never_panics() {
        let (_rt, mut app) = tags_tests::tags_app();
        tags_tests::settle(&mut app);
        for step in 0..3 {
            for (w, h) in [(130, 34), (100, 28), (80, 24), (60, 18), (40, 10), (24, 6)] {
                let buffer = draw_at(&mut app, w, h);
                for y in 0..h {
                    for x in 0..w {
                        assert!(matches!(buffer[(x, y)].bg, Color::Rgb(..)), "tags page (step {step}) at {w}x{h}: cell ({x},{y}) is not painted");
                    }
                }
            }
            match step {
                0 => run_tests::press(&mut app, KeyCode::Right),
                _ => {
                    run_tests::press(&mut app, KeyCode::Left);
                    run_tests::press(&mut app, KeyCode::Char('x'));
                }
            }
        }
    }

    #[test]
    fn only_the_focused_tag_panel_shows_a_row_highlight() {
        let (_rt, mut app) = tags_tests::tags_app();
        tags_tests::settle(&mut app);
        let count = |b: &ratatui::buffer::Buffer| (0..b.area.height).map(|y| (0..b.area.width).filter(|&x| b[(x, y)].bg == SELECTED_BG).count()).filter(|&n| n > 0).count();
        // List focused: the selected tag (two rows) is highlighted, the device table is not.
        let list = draw_at(&mut app, 130, 30);
        let devices_side = (60..130).any(|x| (0..30).any(|y| list[(x, y)].bg == SELECTED_BG));
        assert!(!devices_side, "an unfocused table must not look focused");
        assert!(count(&list) >= 1);
        run_tests::press(&mut app, KeyCode::Right);
        let devices = draw_at(&mut app, 130, 30);
        assert!((60..130).any(|x| (0..30).any(|y| devices[(x, y)].bg == SELECTED_BG)), "the focused device row is highlighted");
    }

    // ---- the tag form ----------------------------------------------------------------------------

    use crate::app::tag_form::tests as form_tests;
    use crate::app::{FormMode, TagField};

    #[test]
    fn a_new_static_tag_form_lists_its_fields_and_says_it_is_not_ready() {
        let (_rt, mut app) = form_tests::form_app();
        let screen = text_of(&draw_at(&mut app, 130, 30));
        for expected in ["┤ new tag ├", "Type", "● Static", "○ Dynamic", "Holds the devices you choose", "Name", "letters, numbers", "Description", "Category", "Usable in", "choose where it can be used", "Devices", "add devices (optional)", "▶ Create tag", "┤ not ready yet ├", "No devices yet."] {
            assert!(screen.contains(expected), "missing {expected:?}:\n{screen}");
        }
        assert_eq!(chips(&draw_at(&mut app, 130, 30)), ["new tag"], "one focus chip");
    }

    #[test]
    fn problems_show_only_after_a_first_attempt_to_save() {
        let (_rt, mut app) = form_tests::form_app();
        assert!(!text_of(&draw_at(&mut app, 130, 30)).contains("✗"));
        form_tests::form_mut(&mut app).field = TagField::Submit;
        run_tests::press(&mut app, KeyCode::Enter);
        let screen = text_of(&draw_at(&mut app, 130, 30));
        assert!(screen.contains("✗ Enter a name.") && screen.contains("✗ Choose where the tag can be used") && screen.contains("Fix the highlighted fields first."), "{screen}");
    }

    #[test]
    fn typing_shows_in_the_field_and_the_ready_footer_says_what_the_button_will_do() {
        let (_rt, mut app) = form_tests::form_app();
        form_tests::type_into(&mut app, TagField::Name, "Servers");
        form_tests::choose_org(&mut app, "Acme Corp");
        let screen = text_of(&draw_at(&mut app, 130, 30));
        assert!(screen.contains("Servers") && screen.contains("Acme Corp") && screen.contains("┤ ready ├"), "{screen}");
        form_tests::form_mut(&mut app).field = TagField::Submit;
        run_tests::press(&mut app, KeyCode::Enter);
        let confirming = text_of(&draw_at(&mut app, 130, 30));
        // The footer is short; the full sentence, with where it is made, is on the button (and may wrap).
        assert!(confirming.contains("┤ enter again to confirm ├") && confirming.contains("▶ Confirm: create static tag Servers in Acme"), "{confirming}");
    }

    #[test]
    fn a_dynamic_form_shows_the_rule_rows_and_a_preview_that_waits_for_what_it_needs() {
        let (_rt, mut app) = form_tests::form_app();
        form_tests::form_mut(&mut app).field = TagField::Kind;
        run_tests::press(&mut app, KeyCode::Char(' '));
        let screen = text_of(&draw_at(&mut app, 130, 34));
        for expected in ["● Dynamic", "Fills itself from a rule.", "Match", "of these conditions", "Device name", "contains", "…", "┤ matches now ├", "Add a condition to see which devices match."] {
            assert!(screen.contains(expected), "missing {expected:?}:\n{screen}");
        }
        assert!(!screen.contains("add devices (optional)"), "a dynamic tag fills itself, so there is no device picker");
        // With a value but no place yet, it says what is missing.
        form_tests::form_mut(&mut app).field = TagField::Condition(0);
        form_tests::form_mut(&mut app).part = crate::app::CondPart::Value;
        run_tests::press(&mut app, KeyCode::Enter);
        run_tests::type_text(&mut app, "PC");
        run_tests::press(&mut app, KeyCode::Enter);
        assert!(text_of(&draw_at(&mut app, 130, 34)).contains("Choose where the tag can be used to preview"));
    }

    #[test]
    fn the_preview_shows_matching_devices_the_total_and_the_empty_and_error_cases() {
        let (_rt, mut app) = form_tests::form_app();
        form_tests::form_mut(&mut app).field = TagField::Kind;
        run_tests::press(&mut app, KeyCode::Char(' '));
        form_tests::choose_org(&mut app, "Acme Corp");
        form_tests::form_mut(&mut app).field = TagField::Condition(0);
        form_tests::form_mut(&mut app).part = crate::app::CondPart::Value;
        run_tests::press(&mut app, KeyCode::Enter);
        run_tests::type_text(&mut app, "PC");
        run_tests::press(&mut app, KeyCode::Enter);
        assert!(text_of(&draw_at(&mut app, 130, 34)).contains("Finding matching devices…"));
        let (seq, key) = form_tests::preview_ids(&app);
        form_tests::deliver_preview(&mut app, seq, &key, Ok((vec![run_tests::device("a1", "PC-1", "CONNECTED", "Windows 11"), run_tests::device("a2", "PC-2", "DISCONNECTED", "Windows 10")], 40)));
        let screen = text_of(&draw_at(&mut app, 130, 34));
        assert!(screen.contains("PC-1") && screen.contains("PC-2") && screen.contains("┤ 2 of 40 ├"), "{screen}");
        form_tests::deliver_preview(&mut app, seq, &key, Ok((vec![], 0)));
        assert!(text_of(&draw_at(&mut app, 130, 34)).contains("No devices match yet."));
        form_tests::deliver_preview(&mut app, seq, &key, Err("Request failed: timed out".into()));
        assert!(text_of(&draw_at(&mut app, 130, 34)).contains("Request failed: timed out"));
    }

    #[test]
    fn the_condition_row_marks_which_part_is_selected() {
        let (_rt, mut app) = form_tests::form_app();
        form_tests::form_mut(&mut app).field = TagField::Kind;
        run_tests::press(&mut app, KeyCode::Char(' '));
        form_tests::form_mut(&mut app).field = TagField::Condition(0);
        let hint = text_of(&draw_at(&mut app, 150, 34));
        assert!(hint.contains("←→ part · enter changes · + adds a row · x removes"), "{hint}");
        // A narrower form gets a shorter hint rather than one that wraps under the highlight.
        let narrow = text_of(&draw_at(&mut app, 80, 34));
        assert!(narrow.contains("←→ part · + add · x remove") && !narrow.contains("enter changes"), "{narrow}");
        // The selected part is drawn as a cyan chip, one at a time.
        let on_accent = |b: &ratatui::buffer::Buffer| (0..b.area.height).flat_map(|y| (0..b.area.width).map(move |x| (x, y))).filter(|&(x, y)| b[(x, y)].bg == ACCENT && b[(x, y)].symbol() != " ").collect::<Vec<_>>();
        let first = draw_at(&mut app, 130, 34);
        run_tests::press(&mut app, KeyCode::Right);
        let second = draw_at(&mut app, 130, 34);
        assert_ne!(on_accent(&first), on_accent(&second), "moving to the next part moves the chip");
    }

    #[test]
    fn the_access_picker_shows_the_three_categories_with_the_current_one_marked() {
        let (_rt, mut app) = form_tests::form_app();
        form_tests::form_mut(&mut app).field = TagField::Access;
        run_tests::press(&mut app, KeyCode::Enter);
        run_tests::pump(&mut app);
        let screen = text_of(&draw_at(&mut app, 130, 30));
        for expected in ["Where can this tag be used?", "Organization", "Customer", "Site", "MSP Inc", "tab organization/customer/site"] {
            assert!(screen.contains(expected), "missing {expected:?}:\n{screen}");
        }
    }

    #[test]
    fn an_edit_form_names_the_tag_and_says_where_static_devices_are_managed() {
        let (_rt, mut app) = tags_tests::tags_app();
        tags_tests::settle(&mut app);
        run_tests::press(&mut app, KeyCode::Char('e'));
        let screen = text_of(&draw_at(&mut app, 130, 30));
        assert!(screen.contains("┤ edit Servers ├") && screen.contains("▶ Save changes") && screen.contains("use + and x on the tag's"), "{screen}");
        assert!(!screen.contains("Usable in") && !screen.contains("Category"), "place and category are fixed once made:\n{screen}");
        assert!(app.tag_form.as_ref().is_some_and(|f| f.mode == FormMode::Edit));
    }

    #[test]
    fn a_rule_the_editor_cannot_show_is_displayed_as_stored_and_kept() {
        let (_rt, mut app) = tags_tests::tags_app();
        let mut odd = run_tests::tag("t2", "Windows10", true, 3);
        odd.filter = Some(r#"{"and":[{"cpu":{"cores":{"gte":4}}}]}"#.into());
        tags_tests::cache_tag(&app, odd);
        tags_tests::settle(&mut app);
        run_tests::press(&mut app, KeyCode::Down);
        tags_tests::settle(&mut app);
        run_tests::press(&mut app, KeyCode::Char('e'));
        let screen = text_of(&draw_at(&mut app, 130, 30));
        assert!(screen.contains("kept as it is") && screen.contains("Stored rule") && screen.contains("cores"), "{screen}");
    }

    #[test]
    fn the_form_paints_a_solid_background_at_every_size_and_never_panics() {
        for kind_step in 0..2 {
            let (_rt, mut app) = form_tests::form_app();
            if kind_step == 1 {
                form_tests::form_mut(&mut app).field = TagField::Kind;
                run_tests::press(&mut app, KeyCode::Char(' '));
                form_tests::choose_org(&mut app, "Acme Corp");
            }
            form_tests::form_mut(&mut app).show_errors = true;
            for (w, h) in [(130, 34), (100, 28), (80, 24), (60, 18), (40, 10), (24, 6)] {
                let buffer = draw_at(&mut app, w, h);
                for y in 0..h {
                    for x in 0..w {
                        assert!(matches!(buffer[(x, y)].bg, Color::Rgb(..)), "tag form (dynamic: {}) at {w}x{h}: cell ({x},{y}) is not painted", kind_step == 1);
                    }
                }
            }
        }
    }

    // ---- watch a task, tag a run's devices, the footer message -----------------------------------------


    fn activity_with_org() -> (tokio::runtime::Runtime, App) {
        let (rt, mut app) = viewer_tests::viewer_app();
        let mut job = app.job_list[0].clone();
        job.org_id = Some("o1".into());
        job.org_name = Some("Acme".into());
        app.jobs.seed(vec![job]);
        app.sync_jobs();
        app.collections_seed_tags(vec![run_tests::tag("t1", "Servers", false, 2), run_tests::tag("t2", "Windows10", true, 5)]);
        (rt, app)
    }

    #[test]
    fn the_watch_box_explains_itself_shows_what_is_pasted_and_the_reason_for_a_failure() {
        let (_rt, mut app) = viewer_tests::viewer_app();
        run_tests::press(&mut app, KeyCode::Char('W'));
        let screen = text_of(&draw_at(&mut app, 120, 30));
        for expected in ["Watch a task", "Follow a run started elsewhere", "Paste", "its task ID", "Task ID", "enter watch · esc cancel", "paste"] {
            assert!(screen.contains(expected), "missing {expected:?}:\n{screen}");
        }
        run_tests::type_text(&mut app, "MDFhMTUwMzgtM2VhMC03YWUx==");
        assert!(text_of(&draw_at(&mut app, 120, 30)).contains("MDFhMTUwMzgtM2VhMC03YWUx=="));
        run_tests::deliver(&mut app, crate::app::Msg::Watched(Err("No task found with that ID.".into())));
        assert!(text_of(&draw_at(&mut app, 120, 30)).contains("No task found with that ID."));
    }

    #[test]
    fn while_looking_a_task_up_the_box_says_so() {
        let (_rt, mut app) = viewer_tests::viewer_app();
        run_tests::press(&mut app, KeyCode::Char('W'));
        run_tests::type_text(&mut app, "abc");
        app.watch.as_mut().unwrap().busy = true;
        let screen = text_of(&draw_at(&mut app, 120, 30));
        assert!(screen.contains("Looking it up…") && screen.contains("looking it up…"), "{screen}");
    }

    #[test]
    fn the_footer_shows_a_recent_message_in_place_of_the_hints_and_a_question_wins_over_it() {
        let (_rt, mut app) = viewer_tests::viewer_app();
        app.flash(crate::app::NoticeKind::Info, "Added 3 devices.");
        let screen = text_of(&draw_at(&mut app, 120, 30));
        assert!(screen.lines().last().unwrap().contains("Added 3 devices."), "{screen}");
        app.confirm = Some(crate::app::Confirm::RemoveJob("j".into()));
        let asking = text_of(&draw_at(&mut app, 120, 30));
        assert!(asking.lines().last().unwrap().contains("Remove this run from your history?") && !asking.lines().last().unwrap().contains("Added 3"), "{asking}");
        app.confirm = None;
        app.tick += 60;
        assert!(!text_of(&draw_at(&mut app, 120, 30)).contains("Added 3 devices."), "it goes away on its own");
    }

    #[test]
    fn the_tag_picker_for_a_runs_devices_lists_static_tags_and_offers_to_create_one() {
        let (_rt, mut app) = activity_with_org();
        run_tests::press(&mut app, KeyCode::Char('T'));
        run_tests::pump(&mut app);
        let screen = text_of(&draw_at(&mut app, 120, 30));
        assert!(screen.contains("Tag 3 devices") && screen.contains("Servers") && !screen.contains("Windows10"), "{screen}");
        run_tests::type_text(&mut app, "patching");
        run_tests::pump(&mut app);
        let creating = text_of(&draw_at(&mut app, 120, 30));
        assert!(creating.contains("Create static tag “patching”"), "{creating}");
    }

    #[test]
    fn the_watch_box_paints_a_solid_background_at_every_size() {
        let (_rt, mut app) = viewer_tests::viewer_app();
        run_tests::press(&mut app, KeyCode::Char('W'));
        run_tests::type_text(&mut app, "x");
        run_tests::deliver(&mut app, crate::app::Msg::Watched(Err("No task found with that ID.".into())));
        for (w, h) in [(120, 30), (80, 20), (50, 12), (30, 6), (12, 3)] {
            let buffer = draw_at(&mut app, w, h);
            for y in 0..h {
                for x in 0..w {
                    assert!(matches!(buffer[(x, y)].bg, Color::Rgb(..)), "watch box at {w}x{h}: cell ({x},{y}) is not painted");
                }
            }
        }
    }
}
