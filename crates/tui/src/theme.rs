//! Colours and small styling helpers, so the look lives in one place.
//! Colours are 24-bit; any modern terminal shows them (set `COLORTERM=truecolor`
//! if yours doesn't advertise it).

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType},
};

pub const ACCENT: Color = Color::Rgb(100, 200, 255);
pub const BORDER: Color = Color::Rgb(70, 78, 96);
pub const DIM: Color = Color::Rgb(125, 133, 152);
pub const TEXT: Color = Color::Rgb(220, 226, 240);
pub const OK: Color = Color::Rgb(110, 220, 130);
pub const ERR: Color = Color::Rgb(250, 100, 100);
pub const WARN: Color = Color::Rgb(250, 210, 80);
pub const SELECTED_BG: Color = Color::Rgb(36, 46, 68);

type Rgb = (f32, f32, f32);

fn mix(a: Rgb, b: Rgb, t: f32) -> Color {
    let f = |x: f32, y: f32| (x + (y - x) * t.clamp(0.0, 1.0)).round() as u8;
    Color::Rgb(f(a.0, b.0), f(a.1, b.1), f(a.2, b.2))
}

/// Cool-to-hot colour for a value from 0.0 to 1.0, like btop's graphs.
pub fn heat(t: f32) -> Color {
    const STOPS: [Rgb; 5] = [(60.0, 120.0, 255.0), (60.0, 220.0, 200.0), (120.0, 230.0, 100.0), (250.0, 210.0, 70.0), (250.0, 100.0, 80.0)];
    let scaled = t.clamp(0.0, 1.0) * (STOPS.len() - 1) as f32;
    let i = (scaled as usize).min(STOPS.len() - 2);
    mix(STOPS[i], STOPS[i + 1], scaled - i as f32)
}

/// A colour that drifts along text over time (`tick` advances every frame tick).
pub fn shimmer(position: usize, tick: u64) -> Color {
    let wave = ((position as f32 * 0.55 - tick as f32 * 0.12).sin() + 1.0) / 2.0;
    mix((90.0, 140.0, 255.0), (120.0, 255.0, 220.0), wave)
}

/// A rounded panel with its title set into the top border: ╭┤ title ├──╮.
/// The focused panel is drawn in the accent colour.
pub fn panel(title: &str, focused: bool) -> Block<'static> {
    let border = Style::new().fg(if focused { ACCENT } else { BORDER });
    let name = Style::new().fg(if focused { ACCENT } else { TEXT }).add_modifier(Modifier::BOLD);
    Block::bordered().border_type(BorderType::Rounded).border_style(border).title(Line::from(vec![
        Span::styled("┤ ", border),
        Span::styled(title.to_string(), name),
        Span::styled(" ├", border),
    ]))
}

/// "↑↓ move  → open  q quit": keys in the accent colour, descriptions dim.
pub fn key_hints(pairs: &[(&str, &str)]) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for (key, what) in pairs {
        spans.push(Span::styled(key.to_string(), Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(format!(" {what}   "), Style::new().fg(DIM)));
    }
    Line::from(spans)
}
