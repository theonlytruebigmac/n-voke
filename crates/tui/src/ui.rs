//! Drawing. These functions only read the app state; they never change it
//! (apart from list scroll positions, which ratatui updates while rendering).

use nvoke_core::audit::AuditEntry;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, List, ListItem, Paragraph, Wrap},
};

use crate::{
    app::{App, Field, Load, Page, Screen, SettingsForm},
    input::TextInput,
};

const ACCENT: Color = Color::Cyan;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    match &app.screen {
        Screen::Loading => centered_message(frame, area, "Loading…", Color::Gray),
        Screen::Fatal(error) => {
            let text = format!("N-voke can't start:\n\n{error}\n\nPress q to quit.");
            centered_message(frame, area, &text, Color::Red);
        }
        Screen::Form(form) => draw_form(frame, area, form),
        Screen::Main => draw_main(frame, area, app),
    }
}

fn draw_main(frame: &mut Frame, area: Rect, app: &mut App) {
    let [body, footer] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
    let [sidebar, content] = Layout::horizontal([Constraint::Length(18), Constraint::Fill(1)]).areas(body);

    let items = Page::ALL.iter().enumerate().map(|(i, page)| {
        let style = if *page == app.page { Style::new().fg(ACCENT).bold() } else { Style::new() };
        ListItem::new(format!(" {} {}", i + 1, page.title())).style(style)
    });
    frame.render_widget(List::new(items).block(Block::bordered().title(" N-voke ")), sidebar);

    match app.page {
        Page::Audit => draw_audit(frame, content, app),
        Page::Settings => draw_settings(frame, content, app),
        page => {
            let text = format!("{} isn't built yet.", page.title());
            frame.render_widget(Paragraph::new(text).block(Block::bordered().title(format!(" {} ", page.title()))), content);
        }
    }

    let hints = match app.page {
        Page::Audit => "j/k move · g/G first/last · r refresh · tab next page · q quit",
        Page::Settings => "e edit · tab next page · q quit",
        _ => "1-5 or tab switch page · q quit",
    };
    frame.render_widget(Paragraph::new(hints).style(Style::new().fg(Color::DarkGray)), footer);
}

fn draw_audit(frame: &mut Frame, area: Rect, app: &mut App) {
    let block = Block::bordered().title(" Audit log ");
    let log = match &app.audit {
        Load::Ready(log) => log,
        Load::Failed(error) => {
            frame.render_widget(Paragraph::new(error.as_str()).style(Style::new().fg(Color::Red)).wrap(Wrap { trim: true }).block(block), area);
            return;
        }
        Load::NotLoaded | Load::Loading => {
            frame.render_widget(Paragraph::new("Loading…").block(block), area);
            return;
        }
    };
    if log.entries.is_empty() {
        frame.render_widget(Paragraph::new("Nothing has been recorded yet.").block(block), area);
        return;
    }

    let [list_area, detail_area] = Layout::vertical([Constraint::Fill(1), Constraint::Length(7)]).areas(area);

    let items: Vec<ListItem> = log.entries.iter().map(entry_row).collect();
    let list = List::new(items)
        .block(block.title_bottom(format!(" {} entries ", log.entries.len())))
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    frame.render_stateful_widget(list, list_area, &mut app.audit_list);

    let selected = app.audit_list.selected().and_then(|i| log.entries.get(i));
    let mut lines = vec![];
    if let Some(entry) = selected {
        lines.push(Line::from(vec![Span::styled("When     ", Style::new().dim()), Span::raw(entry.timestamp.clone())]));
        lines.push(Line::from(vec![Span::styled("User     ", Style::new().dim()), Span::raw(entry.user.clone())]));
        lines.push(Line::from(vec![Span::styled("Action   ", Style::new().dim()), Span::raw(entry.action.clone())]));
        lines.push(Line::from(vec![Span::styled("Summary  ", Style::new().dim()), Span::raw(entry.summary.clone())]));
        if let Some(error) = &entry.error {
            lines.push(Line::from(vec![Span::styled("Error    ", Style::new().dim()), Span::styled(error.clone(), Style::new().fg(Color::Red))]));
        }
    }
    let detail = Paragraph::new(lines).wrap(Wrap { trim: true }).block(Block::bordered().title(" Details ").title_bottom(format!(" {} ", log.path)));
    frame.render_widget(detail, detail_area);
}

fn entry_row(entry: &AuditEntry) -> ListItem<'static> {
    let (mark, color) = if entry.outcome == "ok" { ("✓", Color::Green) } else { ("✗", Color::Red) };
    // "2026-10-18T17:02:23Z" -> "2026-10-18 17:02:23"
    let when = entry.timestamp.replace('T', " ").trim_end_matches('Z').to_string();
    ListItem::new(Line::from(vec![
        Span::styled(format!(" {mark} "), Style::new().fg(color)),
        Span::styled(format!("{when}  "), Style::new().dim()),
        Span::raw(entry.summary.clone()),
    ]))
}

fn draw_settings(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::bordered().title(" Settings ");
    let Some(s) = &app.settings else {
        frame.render_widget(Paragraph::new("Loading…").block(block), area);
        return;
    };
    let yes_no = |b: bool| if b { Span::styled("yes", Style::new().fg(Color::Green)) } else { Span::styled("no", Style::new().fg(Color::Red)) };
    let history = if s.history_days == 0 { "not saved".to_string() } else { format!("{} days", s.history_days) };
    let mut lines = vec![
        Line::from(vec![Span::styled("API endpoint      ", Style::new().dim()), Span::raw(s.api_url.clone())]),
        Line::from(vec![Span::styled("API key saved     ", Style::new().dim()), yes_no(s.has_api_key)]),
        Line::from(vec![Span::styled("Keep run history  ", Style::new().dim()), Span::raw(history)]),
        Line::from(vec![Span::styled("Encrypted at rest ", Style::new().dim()), yes_no(s.encryption_available)]),
    ];
    if let Some(error) = &s.encryption_error {
        lines.push(Line::from(""));
        lines.push(Line::styled(error.clone(), Style::new().fg(Color::Yellow)));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }).block(block), area);
}

fn draw_form(frame: &mut Frame, area: Rect, form: &SettingsForm) {
    let title = if form.first_run { " Welcome to N-voke " } else { " Edit settings " };
    let popup = centered(area, 72, 14);
    frame.render_widget(Clear, popup);
    let outer = Block::bordered().border_type(BorderType::Rounded).title(title);
    let inner = outer.inner(popup);
    frame.render_widget(outer, popup);

    let [intro, endpoint, token, message, hints] =
        Layout::vertical([Constraint::Length(2), Constraint::Length(3), Constraint::Length(3), Constraint::Fill(1), Constraint::Length(1)]).areas(inner);

    let intro_text = if form.first_run {
        "Paste an API-Access token from the N-able dashboard. It is stored in your system keychain."
    } else {
        "Leave the token empty to keep the saved one."
    };
    frame.render_widget(Paragraph::new(intro_text).wrap(Wrap { trim: true }), intro);

    let cursor = draw_field(frame, endpoint, "API endpoint", &form.endpoint, form.focus == Field::Endpoint);
    let token_cursor = draw_field(frame, token, "API token", &form.token, form.focus == Field::Token);
    if !form.busy {
        frame.set_cursor_position(if form.focus == Field::Endpoint { cursor } else { token_cursor });
    }

    let (text, color) = match (&form.error, form.busy) {
        (_, true) => ("Saving…".to_string(), Color::Gray),
        (Some(error), _) => (error.clone(), Color::Red),
        _ => (String::new(), Color::Gray),
    };
    frame.render_widget(Paragraph::new(text).style(Style::new().fg(color)).wrap(Wrap { trim: true }), message);

    let esc = if form.first_run { "esc quit" } else { "esc cancel" };
    frame.render_widget(Paragraph::new(format!("tab switch field · enter save · {esc}")).style(Style::new().fg(Color::DarkGray)), hints);
}

/// Draws a labelled text field and returns where the cursor goes if it has focus.
fn draw_field(frame: &mut Frame, area: Rect, label: &str, input: &TextInput, focused: bool) -> (u16, u16) {
    let border = if focused { Style::new().fg(ACCENT) } else { Style::new().fg(Color::DarkGray) };
    let block = Block::bordered().title(format!(" {label} ")).border_style(border);
    let inner = block.inner(area);
    let shown = input.shown();
    // Show the end of a value that is wider than the box.
    let width = inner.width.saturating_sub(1) as usize;
    let count = shown.chars().count();
    let visible: String = shown.chars().skip(count.saturating_sub(width)).collect();
    let cursor_x = inner.x + visible.chars().count() as u16;
    frame.render_widget(Paragraph::new(visible).block(block), area);
    (cursor_x, inner.y)
}

fn centered_message(frame: &mut Frame, area: Rect, text: &str, color: Color) {
    let popup = centered(area, 70, text.lines().count() as u16 + 2);
    frame.render_widget(Paragraph::new(text).style(Style::new().fg(color)).wrap(Wrap { trim: true }).block(Block::bordered()), popup);
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
        app.audit_list.select(Some(1));
        let screen = render(&mut app);
        assert!(screen.contains("Audit log"), "{screen}");
        assert!(screen.contains("Created static tag Servers"), "{screen}");
        assert!(screen.contains("Error    HTTP 500"), "{screen}");
        assert!(screen.contains("/tmp/audit.jsonl"), "{screen}");
    }

    #[test]
    fn unbuilt_pages_say_so() {
        let (_rt, mut app) = app();
        app.settings = Some(settings(true));
        app.screen = Screen::Main;
        app.page = Page::Tags;
        assert!(render(&mut app).contains("Tags isn't built yet."));
    }
}
