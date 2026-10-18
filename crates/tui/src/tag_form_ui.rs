//! Drawing the new-tag / edit-tag form.

use nvoke_core::rules::describe_rule;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Cell, Paragraph, Row, Table, Wrap},
};

use crate::{
    app::{App, CondPart, FormMode, NoticeKind, TagField, TagForm},
    app::tag_form::summary,
    run_ui::agent_glyph,
    theme::{self, ACCENT, DIM, ERR, OK, TEXT, WARN, panel},
    ui::{spinner, truncate},
};

const LABEL: usize = 13;

pub fn draw_tag_form(frame: &mut Frame, area: Rect, app: &mut App) {
    let tick = app.tick;
    let Some(form) = app.tag_form.as_ref() else { return };
    let [form_area, side_area] = if area.width >= 100 {
        Layout::horizontal([Constraint::Percentage(56), Constraint::Fill(1)]).areas(area)
    } else {
        Layout::vertical([Constraint::Percentage(60), Constraint::Fill(1)]).areas(area)
    };
    draw_form(frame, form_area, form, tick);
    draw_side(frame, side_area, form, tick);
}

fn draw_form(frame: &mut Frame, area: Rect, form: &TagForm, tick: u64) {
    let title = match (&form.mode, &form.tag) {
        (FormMode::Edit, Some(t)) => format!("edit {}", t.name),
        _ => "new tag".to_string(),
    };
    // Whether it can be saved, always visible in the panel's border (the button may be scrolled out of sight).
    let status = if form.busy {
        theme::footer(vec![Span::styled("saving…", Style::new().fg(ACCENT))])
    } else if form.confirming {
        theme::footer(vec![Span::styled("enter again to confirm", Style::new().fg(WARN).add_modifier(Modifier::BOLD))])
    } else if form.problems().any() {
        theme::footer(vec![Span::styled("not ready yet", Style::new().fg(DIM))])
    } else {
        // Short on purpose: the border clips long text. The full sentence is on the button when you confirm.
        theme::footer(vec![Span::styled("ready", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))])
    };
    let block = panel(&title, true).title_bottom(status);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let width = inner.width as usize;
    let value_width = width.saturating_sub(LABEL + 4);
    let problems = if form.show_errors { form.problems() } else { Default::default() };
    let cursor = |on: bool| if on && theme::cursor_visible(tick) { "▏" } else if on { " " } else { "" };

    let mut lines: Vec<Line> = vec![];
    let mut focus_line = 0usize;
    let mut row = |lines: &mut Vec<Line<'static>>, field: TagField, label: &str, value: Vec<Span<'static>>| {
        let is_focus = form.field == field;
        let mut spans = vec![
            Span::styled(if is_focus { "▌ " } else { "  " }, Style::new().fg(TEXT)),
            Span::styled(format!("{label:<w$}", w = if label.is_empty() { 0 } else { LABEL }), Style::new().fg(if is_focus { ACCENT } else { DIM })),
        ];
        spans.extend(value);
        let mut line = Line::from(spans);
        if is_focus {
            let used: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            line.spans.push(Span::raw(" ".repeat(width.saturating_sub(used))));
            line = line.style(Style::new().bg(theme::SELECTED_BG).add_modifier(Modifier::BOLD));
            focus_line = lines.len();
        }
        lines.push(line);
    };
    let dim = |t: &str| vec![Span::styled(t.to_string(), Style::new().fg(DIM))];
    let text = |t: &str| Span::styled(t.to_string(), Style::new().fg(TEXT));
    let error = |lines: &mut Vec<Line<'static>>, e: &str| lines.push(Line::styled(format!("{}✗ {e}", " ".repeat(2 + LABEL)), Style::new().fg(ERR)));

    if form.mode == FormMode::Create {
        let radio = |on: bool, label: &str| Span::styled(format!("{} {label}", if on { "●" } else { "○" }), Style::new().fg(if on { ACCENT } else { DIM }));
        row(&mut lines, TagField::Kind, "Type", vec![radio(!form.is_dynamic(), "Static"), Span::raw("   "), radio(form.is_dynamic(), "Dynamic"), Span::styled(if form.field == TagField::Kind { "   ←→ switch" } else { "" }, Style::new().fg(DIM))]);
        lines.push(Line::styled(format!("{}{}", " ".repeat(2 + LABEL), if form.is_dynamic() { "Fills itself from a rule." } else { "Holds the devices you choose." }), Style::new().fg(DIM)));
    }

    let editing = |f: TagField| form.editing && form.field == f;
    let field_value = |value: &str, f: TagField, placeholder: &str| -> Vec<Span<'static>> {
        if value.is_empty() && !editing(f) {
            vec![Span::styled(placeholder.to_string(), Style::new().fg(DIM))]
        } else {
            let shown: String = value.chars().rev().take(value_width.saturating_sub(2)).collect::<Vec<_>>().into_iter().rev().collect();
            vec![Span::styled(shown, Style::new().fg(TEXT)), Span::styled(cursor(editing(f)).to_string(), Style::new().fg(ACCENT))]
        }
    };
    row(&mut lines, TagField::Name, "Name", field_value(&form.data.name, TagField::Name, "letters, numbers, - and _"));
    if let Some(e) = problems.name {
        error(&mut lines, e);
    }
    row(&mut lines, TagField::Description, "Description", field_value(&form.data.description, TagField::Description, "optional"));
    if form.mode == FormMode::Create {
        row(&mut lines, TagField::Category, "Category", field_value(&form.data.category, TagField::Category, "optional"));
        row(&mut lines, TagField::Access, "Usable in", match &form.data.org {
            Some(o) => vec![text(&truncate(&o.name, value_width.saturating_sub(4))), Span::styled(" ▾", Style::new().fg(DIM))],
            None => dim("choose where it can be used ▾"),
        });
        if let Some(e) = problems.access {
            error(&mut lines, e);
        }
    }

    if form.is_dynamic() {
        lines.push(Line::from(""));
        if form.rule_editable {
            row(&mut lines, TagField::Match, "Match", vec![text(if form.data.rule.r#match == nvoke_core::rules::Match::All { "all" } else { "any" }), Span::styled(" ▾  of these conditions", Style::new().fg(DIM))]);
            for (i, c) in form.data.rule.conditions.iter().enumerate() {
                let focus = form.field == TagField::Condition(i);
                let part = |p: CondPart, label: String, wide: usize| {
                    let selected = focus && form.part == p;
                    let style = if selected { Style::new().fg(theme::BG).bg(ACCENT).add_modifier(Modifier::BOLD) } else { Style::new().fg(TEXT) };
                    Span::styled(format!(" {:<w$} ", truncate(&label, wide), w = wide), style)
                };
                let editing_value = editing(TagField::Condition(i)) && form.part == CondPart::Value;
                let value = if c.value.is_empty() && !editing_value { "…".to_string() } else { format!("{}{}", c.value, cursor(editing_value)) };
                row(&mut lines, TagField::Condition(i), &format!("  {}", i + 1), vec![
                    part(CondPart::Field, c.field.label().to_string(), 16),
                    Span::raw(" "),
                    part(CondPart::Op, c.op.label().to_string(), 11),
                    Span::raw(" "),
                    part(CondPart::Value, value, value_width.saturating_sub(34).clamp(12, 28)),
                ]);
            }
            if matches!(form.field, TagField::Condition(_)) {
                // Said in full when there is room; a shorter version otherwise, so it never wraps under the highlight.
                let room = width.saturating_sub(2 + LABEL);
                let hint = if room >= 52 { "←→ part · enter changes · + adds a row · x removes" } else { "←→ part · + add · x remove" };
                lines.push(Line::styled(format!("{}{}", " ".repeat(2 + LABEL), truncate(hint, room)), Style::new().fg(DIM)));
            }
            if let Some(e) = problems.rule {
                error(&mut lines, e);
            }
        } else {
            lines.push(Line::styled("  Rule: kept as it is: it uses conditions this editor can't show.", Style::new().fg(DIM)));
        }
    } else if form.mode == FormMode::Create {
        let n = form.data.devices.len();
        row(&mut lines, TagField::Devices, "Devices", if n == 0 {
            dim("add devices (optional) ▾")
        } else {
            vec![text(&format!("{n} device{}", if n == 1 { "" } else { "s" })), Span::styled(if form.field == TagField::Devices { " ▾   c clears" } else { " ▾" }, Style::new().fg(DIM))]
        });
    }

    // The button.
    lines.push(Line::from(""));
    let (label, style) = if form.busy {
        (format!("{} Saving…", spinner(tick)), Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))
    } else if form.confirming {
        (format!("▶ Confirm: {}", summary(form)), Style::new().fg(WARN).add_modifier(Modifier::BOLD))
    } else {
        (if form.mode == FormMode::Create { "▶ Create tag".to_string() } else { "▶ Save changes".to_string() }, Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))
    };
    row(&mut lines, TagField::Submit, "", vec![Span::styled(label, style)]);
    if form.confirming {
        lines.push(Line::styled("  enter again to confirm · esc to cancel", Style::new().fg(DIM)));
    }
    if let Some(n) = &form.notice {
        lines.push(Line::from(""));
        lines.push(Line::styled(format!("  {}", n.text), Style::new().fg(if n.kind == NoticeKind::Error { ERR } else { ACCENT })));
    }

    let scroll = (focus_line + 3).saturating_sub(inner.height as usize) as u16;
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)).wrap(Wrap { trim: false }), inner);
}

fn draw_side(frame: &mut Frame, area: Rect, form: &TagForm, tick: u64) {
    if form.is_dynamic() {
        return draw_preview(frame, area, form, tick);
    }
    let block = theme::panel_padded("devices", false);
    if form.mode == FormMode::Edit {
        let text = vec![Line::from(""), Line::styled("Static tags keep the devices you choose.", Style::new().fg(DIM)), Line::styled("To change them, use + and x on the tag's own page.", Style::new().fg(DIM))];
        return frame.render_widget(Paragraph::new(text).alignment(Alignment::Center).block(block), area);
    }
    if form.data.devices.is_empty() {
        let text = vec![Line::from(""), Line::styled("No devices yet.", Style::new().fg(TEXT)), Line::styled("Add some now, or later from the tag's page.", Style::new().fg(DIM))];
        return frame.render_widget(Paragraph::new(text).alignment(Alignment::Center).block(block), area);
    }
    device_table(frame, area, &form.data.devices, block);
}

fn device_table(frame: &mut Frame, area: Rect, devices: &[nvoke_core::model::Asset], block: ratatui::widgets::Block<'static>) {
    let unit = (area.width as usize).saturating_sub(8) / 7;
    let rows: Vec<Row> = devices
        .iter()
        .map(|d| {
            let (glyph, color) = agent_glyph(d);
            let location = [d.customer.as_ref().map(|c| c.name.as_str()), d.site.as_ref().map(|s| s.name.as_str())].into_iter().flatten().collect::<Vec<_>>().join(" · ");
            Row::new(vec![
                Cell::from(Line::from(vec![Span::styled(glyph.to_string(), Style::new().fg(color)), Span::styled(format!(" {}", truncate(&d.name, (unit * 3).saturating_sub(2))), Style::new().fg(TEXT))])),
                Cell::from(truncate(&location, unit * 2)).style(Style::new().fg(DIM)),
                Cell::from(truncate(d.os_name().unwrap_or(""), unit * 2)).style(Style::new().fg(DIM)),
            ])
        })
        .collect();
    let table = Table::new(rows, [Constraint::Fill(3), Constraint::Fill(2), Constraint::Fill(2)])
        .column_spacing(2)
        .header(Row::new(["Device", "Location", "Operating system"]).style(Style::new().fg(DIM).add_modifier(Modifier::BOLD)))
        .block(block);
    frame.render_widget(table, area);
}

/// The devices a dynamic rule matches right now.
fn draw_preview(frame: &mut Frame, area: Rect, form: &TagForm, tick: u64) {
    let block = theme::panel_padded("matches now", false);
    let center = |frame: &mut Frame, lines: Vec<Line<'static>>| frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center).wrap(Wrap { trim: true }).block(theme::panel_padded("matches now", false)), area);

    if !form.rule_editable {
        let raw = form.tag.as_ref().and_then(|t| t.filter.clone()).unwrap_or_default();
        return frame.render_widget(Paragraph::new(vec![Line::styled("Stored rule (kept as is)", Style::new().fg(DIM).add_modifier(Modifier::BOLD)), Line::from(""), Line::styled(raw, Style::new().fg(TEXT))]).wrap(Wrap { trim: false }).block(block), area);
    }
    if nvoke_core::rules::active_condition_count(&form.data.rule) == 0 {
        return center(frame, vec![Line::from(""), Line::styled("Add a condition to see which devices match.", Style::new().fg(DIM))]);
    }
    if form.org_id().is_none() {
        return center(frame, vec![Line::from(""), Line::styled("Choose where the tag can be used to preview its devices.", Style::new().fg(DIM))]);
    }
    if let Some(e) = &form.preview.error {
        return center(frame, vec![Line::from(""), Line::styled(e.clone(), Style::new().fg(ERR))]);
    }
    match &form.preview.result {
        Some((devices, total)) if !devices.is_empty() => {
            let footer = if *total > devices.len() as i64 { format!("{} of {total}", devices.len()) } else { format!("{total} match{}", if *total == 1 { "" } else { "es" }) };
            device_table(frame, area, devices, block.title_bottom(theme::footer(vec![Span::styled(footer, Style::new().fg(OK))])));
        }
        Some(_) => center(frame, vec![Line::from(""), Line::styled("No devices match yet.", Style::new().fg(DIM)), Line::styled(describe_rule(&form.data.rule), Style::new().fg(DIM))]),
        None => center(frame, vec![Line::from(""), Line::styled(format!("{} Finding matching devices…", spinner(tick)), Style::new().fg(ACCENT))]),
    }
}
