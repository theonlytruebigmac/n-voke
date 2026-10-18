//! Drawing the Tags page: the list with its search, and the selected tag with its devices.

use nvoke_core::{model::Tag, rules::{describe_rule, filter_to_rule}};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Cell, Paragraph, Row, Table, Wrap},
};

use crate::{
    app::{App, Load, NoticeKind, TagsPane},
    run_ui::agent_glyph,
    theme::{self, ACCENT, DIM, ERR, SELECTED_BG, TEXT, WARN, panel, panel_padded},
    ui::{spinner, truncate},
};

pub fn draw_tags(frame: &mut Frame, area: Rect, app: &mut App, focused: bool) {
    let [list_area, detail_area] = if area.width >= 84 {
        let list_width = (area.width * 36 / 100).clamp(30, 46);
        Layout::horizontal([Constraint::Length(list_width), Constraint::Fill(1)]).areas(area)
    } else {
        Layout::vertical([Constraint::Percentage(40), Constraint::Fill(1)]).areas(area)
    };
    draw_list(frame, list_area, app, focused && app.tags.pane == TagsPane::List);
    draw_detail(frame, detail_area, app, focused && app.tags.pane == TagsPane::Devices);
}

/// "◆" for a static tag (devices you choose), "↻" for a dynamic one (fills itself from a rule).
pub fn kind_glyph(tag: &Tag) -> &'static str {
    if tag.is_dynamic() { "↻" } else { "◆" }
}

pub fn kind_word(tag: &Tag) -> &'static str {
    if tag.is_dynamic() { "Dynamic" } else { "Static" }
}

fn draw_list(frame: &mut Frame, area: Rect, app: &mut App, focused: bool) {
    let tick = app.tick;
    let state = &app.tags;
    let block = panel("tags", focused);
    let inner = block.inner(area);
    let count = state.tags().len();
    let footer = match &state.all {
        Load::Ready(_) => format!("{count} tag{}", if count == 1 { "" } else { "s" }),
        Load::Failed(_) => "could not load".into(),
        _ => format!("{} loading", spinner(tick)),
    };
    frame.render_widget(block.title_bottom(theme::footer_text(&footer)), area);

    let [search_area, rule_area, rows_area] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
    let blink = if theme::cursor_visible(tick) { "▏" } else { " " };
    let search = if state.searching {
        Line::from(vec![Span::styled(" / ", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)), Span::styled(state.term.value.clone(), Style::new().fg(TEXT)), Span::styled(blink, Style::new().fg(ACCENT))])
    } else if !state.term.value.is_empty() {
        Line::from(vec![Span::styled(" / ", Style::new().fg(ACCENT)), Span::styled(state.term.value.clone(), Style::new().fg(TEXT)), Span::styled("   esc clears", Style::new().fg(DIM))])
    } else {
        Line::styled(" / to search", Style::new().fg(DIM))
    };
    frame.render_widget(Paragraph::new(search), search_area);
    frame.render_widget(ratatui::widgets::Block::new().borders(ratatui::widgets::Borders::TOP).border_style(Style::new().fg(theme::BORDER)), rule_area);

    match &state.all {
        Load::Failed(e) => {
            let mut lines = vec![Line::styled(e.clone(), Style::new().fg(ERR))];
            if nvoke_core::api::is_access_denied(e) {
                lines.push(Line::from(""));
                lines.push(Line::styled("Replace your key in Settings (5).", Style::new().fg(WARN)));
            }
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), rows_area);
            return;
        }
        Load::Ready(list) if list.is_empty() => {
            let text = if state.term.value.is_empty() { "No tags yet. Press n to create the first." } else { "No matching tags." };
            frame.render_widget(Paragraph::new(text).style(Style::new().fg(DIM)).alignment(Alignment::Center).wrap(Wrap { trim: true }), rows_area);
            return;
        }
        Load::Ready(_) => {}
        _ => {
            frame.render_widget(Paragraph::new(format!("{} Loading tags…", spinner(tick))).style(Style::new().fg(ACCENT)).alignment(Alignment::Center), rows_area);
            return;
        }
    }

    let width = rows_area.width as usize;
    let name_width = width.saturating_sub(10);
    // The organization is only worth a second line when the list spans more than one: when every tag
    // is in the same place, it is noise, and one line per tag fits twice as many.
    let orgs: std::collections::HashSet<&str> = state.tags().iter().filter_map(|t| t.organization.as_ref().map(|o| o.id.as_str())).collect();
    let show_org = orgs.len() > 1;
    let rows: Vec<Row> = state
        .tags()
        .iter()
        .map(|t| {
            let count = t.asset_count.unwrap_or(0).to_string();
            let mut name = vec![Line::styled(truncate(&t.name, name_width), Style::new().fg(TEXT))];
            if show_org {
                name.push(Line::styled(truncate(t.organization.as_ref().map_or("", |o| o.name.as_str()), name_width), Style::new().fg(DIM)));
            }
            Row::new(vec![
                Cell::from(kind_glyph(t)).style(Style::new().fg(if t.is_dynamic() { WARN } else { DIM })),
                Cell::from(name),
                Cell::from(Line::styled(count, Style::new().fg(DIM)).alignment(Alignment::Right)),
            ])
            .height(if show_org { 2 } else { 1 })
        })
        .collect();
    let table = Table::new(rows, [Constraint::Length(2), Constraint::Fill(1), Constraint::Length(5)])
        .column_spacing(1)
        .row_highlight_style(Style::new().bg(SELECTED_BG).add_modifier(Modifier::BOLD))
        .highlight_symbol("▌");
    frame.render_stateful_widget(table, rows_area, &mut app.tags.table);
}

fn draw_detail(frame: &mut Frame, area: Rect, app: &mut App, focused: bool) {
    let tick = app.tick;
    let Some(tag) = app.tags.shown().cloned() else {
        let empty = matches!(app.tags.all, Load::Ready(ref l) if l.is_empty()) && app.tags.term.value.is_empty();
        let text = vec![
            Line::from(""),
            Line::styled(if empty { "Create your first tag with n" } else { "Select a tag" }, Style::new().fg(TEXT).add_modifier(Modifier::BOLD)),
            Line::from(""),
            Line::styled("Static tags hold the devices you choose.", Style::new().fg(DIM)),
            Line::styled("Dynamic tags fill themselves from rules.", Style::new().fg(DIM)),
        ];
        frame.render_widget(Paragraph::new(text).alignment(Alignment::Center).block(panel_padded("tag", false)), area);
        return;
    };
    let is_static = !tag.is_dynamic();

    let mut footer_parts: Vec<Span<'static>> = vec![];
    if !app.tags.checked.is_empty() {
        footer_parts.push(Span::styled(format!("{} selected", app.tags.checked.len()), Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)));
    } else if let Load::Ready(m) = &app.tags.members {
        footer_parts.push(Span::styled(format!("{} device{}", m.len(), if m.len() == 1 { "" } else { "s" }), Style::new().fg(DIM)));
    }
    let mut block = panel_padded(&tag.name, focused);
    if !footer_parts.is_empty() {
        block = block.title_bottom(theme::footer(footer_parts));
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Header: what it is, whose it is, how many devices; for a dynamic tag, its rule.
    let count = match &app.tags.members {
        Load::Ready(m) => m.len() as i64,
        _ => tag.asset_count.unwrap_or(0),
    };
    let org = tag.organization.as_ref().map_or(String::new(), |o| format!(" · {}", o.name));
    let mut head = vec![Line::from(vec![
        Span::styled(format!("{} ", kind_glyph(&tag)), Style::new().fg(if tag.is_dynamic() { WARN } else { DIM })),
        Span::styled(format!("{}{org} · {count} device{}", kind_word(&tag), if count == 1 { "" } else { "s" }), Style::new().fg(TEXT)),
    ])];
    if let Some(d) = tag.description.as_deref().filter(|d| !d.is_empty()) {
        head.push(Line::styled(d.to_string(), Style::new().fg(DIM)));
    }
    if tag.is_dynamic() {
        let rule = tag.filter.as_deref().map(|f| filter_to_rule(&serde_json::Value::String(f.to_string())).map(|r| describe_rule(&r)).unwrap_or_else(|| f.to_string())).unwrap_or_default();
        head.push(Line::from(vec![Span::styled("Rule  ", Style::new().fg(DIM)), Span::styled(rule, Style::new().fg(TEXT))]));
    }
    let head_height = (head.iter().map(|l| l.width().div_ceil(inner.width.max(1) as usize).max(1)).sum::<usize>() as u16 + 1).min(inner.height / 2);
    let [head_area, devices_area, notice_area] = Layout::vertical([Constraint::Length(head_height), Constraint::Fill(1), Constraint::Length(app.tags.notice.is_some() as u16 + (tag.is_dynamic() as u16))]).areas(inner);
    frame.render_widget(Paragraph::new(head).wrap(Wrap { trim: false }), head_area);

    // The devices.
    match &app.tags.members {
        Load::Failed(e) => frame.render_widget(Paragraph::new(e.clone()).style(Style::new().fg(ERR)).wrap(Wrap { trim: false }), devices_area),
        Load::Ready(members) if members.is_empty() => {
            let text = if is_static { "No devices yet. Press + to add some." } else { "No devices match right now." };
            frame.render_widget(Paragraph::new(text).style(Style::new().fg(DIM)).alignment(Alignment::Center), devices_area);
        }
        Load::Ready(members) => {
            let wide = devices_area.width >= 60;
            let mut widths = vec![];
            let mut headers = vec![];
            if is_static {
                widths.push(Constraint::Length(3));
                headers.push("");
            }
            widths.extend([Constraint::Fill(3), Constraint::Fill(2)]);
            headers.extend(["Device", "Location"]);
            if wide {
                widths.push(Constraint::Fill(2));
                headers.push("Operating system");
            }
            let unit = (devices_area.width as usize).saturating_sub(if is_static { 8 } else { 4 }) / if wide { 7 } else { 5 };
            let rows: Vec<Row> = members
                .iter()
                .map(|m| {
                    let (glyph, color) = agent_glyph(m);
                    let ticked = app.tags.checked.contains(&m.id);
                    let location = [m.customer.as_ref().map(|c| c.name.as_str()), m.site.as_ref().map(|s| s.name.as_str())].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                    let mut cells = vec![];
                    if is_static {
                        cells.push(Cell::from(if ticked { "[x]" } else { "[ ]" }).style(Style::new().fg(if ticked { ACCENT } else { DIM })));
                    }
                    cells.push(Cell::from(Line::from(vec![Span::styled(glyph.to_string(), Style::new().fg(color)), Span::styled(format!(" {}", truncate(&m.name, (unit * 3).saturating_sub(2))), Style::new().fg(TEXT))])));
                    cells.push(Cell::from(truncate(&location, unit * 2)).style(Style::new().fg(DIM)));
                    if wide {
                        cells.push(Cell::from(truncate(m.os_name().unwrap_or(""), unit * 2)).style(Style::new().fg(DIM)));
                    }
                    Row::new(cells)
                })
                .collect();
            let table = Table::new(rows, widths)
                .column_spacing(2)
                .header(Row::new(headers).style(Style::new().fg(DIM).add_modifier(Modifier::BOLD)))
                // The highlight marks where you are, so only the focused panel has one.
                .row_highlight_style(if focused { Style::new().bg(SELECTED_BG).add_modifier(Modifier::BOLD) } else { Style::new() })
                .highlight_symbol(if focused { "▌ " } else { "  " });
            frame.render_stateful_widget(table, devices_area, &mut app.tags.members_table);
        }
        _ => frame.render_widget(Paragraph::new(format!("{} Loading devices…", spinner(tick))).style(Style::new().fg(ACCENT)).alignment(Alignment::Center), devices_area),
    }

    // A note (what just happened), or for a dynamic tag how its membership behaves.
    let mut notes: Vec<Line> = vec![];
    if let Some(n) = &app.tags.notice {
        notes.push(Line::styled(n.text.clone(), Style::new().fg(if n.kind == NoticeKind::Error { ERR } else { ACCENT })));
    }
    if tag.is_dynamic() {
        notes.push(Line::styled("Membership updates by itself and can lag a minute after a change.", Style::new().fg(DIM)));
    }
    frame.render_widget(Paragraph::new(notes), notice_area);
}
