//! Drawing the full-screen output viewer.

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};

use crate::{
    app::{App, TOAST_TICKS},
    output_view::{self, Kind, OutLine, Visual},
    theme::{self, ACCENT, BG, BORDER, DIM, ERR, TEXT, WARN, panel_padded},
    ui::{centered, clear, truncate},
};

pub fn draw_viewer(frame: &mut Frame, area: Rect, app: &mut App) {
    // Everything between the header bar and the key hints.
    let popup = Rect { x: area.x, y: area.y + 1, width: area.width, height: area.height.saturating_sub(2) };
    clear(frame, popup);

    let Some(task) = app.viewer_task().cloned() else {
        let text = Paragraph::new("This run is no longer in the list.\n\nPress esc to close.").style(Style::new().fg(DIM)).alignment(Alignment::Center).block(panel_padded("output", true));
        frame.render_widget(text, centered(popup, 50.min(popup.width), 7.min(popup.height)));
        return;
    };
    let tick = app.tick;
    let script = app.current_job().map(|j| j.script_name.clone()).unwrap_or_default();
    let Some(viewer) = app.viewer.as_mut() else { return };

    let lines = output_view::build(&task);
    let many = viewer.task_ids.len() > 1;
    let title = if many {
        format!("{} · {} · device {} of {}", script, task.device_name, viewer.device + 1, viewer.task_ids.len())
    } else {
        format!("{} · {}", script, task.device_name)
    };
    let block = panel_padded(&truncate(&title, popup.width.saturating_sub(8) as usize), true);
    let inner = block.inner(popup);

    let [body, status_area] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);
    let numbered_digits = lines.iter().filter_map(|l| l.number).max().map_or(0, |n| n.to_string().len());
    let gutter = if numbered_digits > 0 { numbered_digits + 2 } else { 0 };
    let text_width = (body.width as usize).saturating_sub(gutter + 1).max(1);

    // The sizes the keys need, and a view that is always inside the content.
    viewer.width = text_width;
    viewer.height = body.height as usize;
    let visuals = output_view::layout(&lines, text_width, viewer.wrap, viewer.hscroll);
    viewer.total = visuals.len();
    viewer.widest = output_view::widest(&lines);
    viewer.clamp();
    let visuals = if viewer.wrap { visuals } else { output_view::layout(&lines, text_width, false, viewer.hscroll) };

    let hits = output_view::find(&lines, &viewer.search);
    let current_line = hits.get(viewer.match_at).copied();
    let top = viewer.top;
    let shown: Vec<Line> = visuals.iter().skip(top).take(body.height as usize).map(|v| visual_line(v, &lines, gutter, text_width, &viewer.search, current_line)).collect();

    // Position, in the panel's own border.
    let (first, last) = (top + 1, (top + body.height as usize).min(viewer.total));
    let percent = if viewer.total <= viewer.height { 100 } else { top * 100 / viewer.max_top().max(1) };
    let wrap_note = if viewer.wrap { "wrap on" } else { "wrap off" };
    let footer = if viewer.total == 0 { theme::footer_text(wrap_note) } else { theme::footer_text(&format!("lines {first}–{last} of {} · {percent}% · {wrap_note}", viewer.total)) };
    frame.render_widget(block.title_bottom(footer), popup);

    frame.render_widget(Paragraph::new(shown), Rect { width: body.width.saturating_sub(1), ..body });
    if viewer.total > viewer.height {
        let mut state = ScrollbarState::new(viewer.max_top() + 1).position(top);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight).begin_symbol(None).end_symbol(None).track_symbol(None).thumb_symbol("█").style(Style::new().fg(BORDER)),
            body.inner(Margin { vertical: 0, horizontal: 0 }),
            &mut state,
        );
    }

    // What the reader needs to know right now: the search box, its result, a message, or nothing.
    let status = if viewer.searching {
        Line::from(vec![Span::styled("/", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)), Span::styled(viewer.search.clone(), Style::new().fg(TEXT)), Span::styled(if theme::cursor_visible(tick) { "▏" } else { " " }, Style::new().fg(ACCENT))])
    } else if let Some((text, _)) = viewer.toast.as_ref().filter(|(_, at)| tick.saturating_sub(*at) < TOAST_TICKS) {
        Line::styled(text.clone(), Style::new().fg(ACCENT))
    } else if !viewer.search.is_empty() {
        if hits.is_empty() {
            Line::styled(format!("No matches for “{}”. Esc clears the search.", viewer.search), Style::new().fg(WARN))
        } else {
            Line::from(vec![
                Span::styled(format!("Match {} of {} for “{}”", viewer.match_at + 1, hits.len(), viewer.search), Style::new().fg(ACCENT)),
                Span::styled("   n next · N previous · esc clears", Style::new().fg(DIM)),
            ])
        }
    } else {
        Line::styled(if viewer.wrap { "Long lines are wrapped: w shows them whole, and < > then scroll sideways." } else { "Lines are shown whole: < > scroll sideways, w wraps them again." }, Style::new().fg(DIM))
    };
    frame.render_widget(Paragraph::new(status), status_area);
}

/// One row of the viewer: the line-number gutter, then the text with its kind's colour and any search hits marked.
fn visual_line(v: &Visual, lines: &[OutLine], gutter: usize, width: usize, search: &str, current: Option<usize>) -> Line<'static> {
    let line = &lines[v.line];

    // A section heading is a rule across the view, which is what separates the sections.
    if line.kind == Kind::Heading {
        let rule_len = width.saturating_sub(line.text.chars().count() + 4);
        return Line::from(vec![
            Span::styled(" ".repeat(gutter), Style::new()),
            Span::styled(format!("── {} ", line.text), Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("─".repeat(rule_len), Style::new().fg(BORDER)),
        ]);
    }

    let mut spans = vec![];
    if gutter > 0 {
        let number = if v.first { line.number.map(|n| format!("{n:>w$} ", w = gutter - 1)).unwrap_or_else(|| " ".repeat(gutter)) } else { " ".repeat(gutter) };
        spans.push(Span::styled(number, Style::new().fg(DIM)));
    }
    let base = match line.kind {
        Kind::Stdout | Kind::Variable | Kind::Meta => Style::new().fg(TEXT),
        Kind::Stderr => Style::new().fg(ERR),
        Kind::Message => Style::new().fg(WARN),
        Kind::Note | Kind::Heading => Style::new().fg(DIM),
    };

    // Split into marked and unmarked stretches.
    let chars: Vec<char> = v.text.chars().collect();
    let ranges = output_view::match_ranges(&v.text, search);
    let is_current = current == Some(v.line);
    let mark = Style::new().fg(BG).bg(if is_current { ACCENT } else { WARN }).add_modifier(Modifier::BOLD);
    let label_len = if v.first && line.label.is_some() { 11.min(chars.len()) } else { 0 };
    let mut at = 0;
    let push = |spans: &mut Vec<Span<'static>>, from: usize, to: usize, marked: bool| {
        if from >= to {
            return;
        }
        // The label of a detail row is dimmer than its value.
        let mut cut = from;
        while cut < to {
            let in_label = cut < label_len;
            let end = if in_label { to.min(label_len) } else { to };
            let style = if marked { mark } else if in_label { Style::new().fg(DIM) } else { base };
            spans.push(Span::styled(chars[cut..end].iter().collect::<String>(), style));
            cut = end;
        }
    };
    for (start, end) in ranges {
        push(&mut spans, at, start, false);
        push(&mut spans, start, end, true);
        at = end;
    }
    push(&mut spans, at, chars.len(), false);
    Line::from(spans)
}
