//! Colours and small styling helpers, so the look lives in one place.
//! Colours are 24-bit; any modern terminal shows them (set `COLORTERM=truecolor`
//! if yours doesn't advertise it).

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType},
};

/// The palette comes from the N-voke logo: deep violet, white and bright cyan.
/// Those are the colours sampled from `src/assets/logo.png` (about #300070 and
/// #18D0F0), not an official N-able brand specification.
pub const BRAND_VIOLET: Color = Color::Rgb(48, 0, 112);
pub const BRAND_CYAN: Color = Color::Rgb(24, 208, 240);

/// The app paints its own background, so every colour below is chosen against it
/// and stays readable whatever the terminal's theme, transparency or wallpaper.
/// Set `NVOKE_TRANSPARENT=1` to let the terminal's own background show through.
pub const BG: Color = Color::Rgb(17, 9, 38);
/// The top bar, in the logo's violet.
pub const HEADER_BG: Color = BRAND_VIOLET;
pub const SELECTED_BG: Color = Color::Rgb(62, 18, 132);

// Text colours. Each is at least 4.5:1 against BG, SELECTED_BG and HEADER_BG (WCAG AA); a test checks it.
pub const TEXT: Color = Color::Rgb(242, 238, 252);
pub const DIM: Color = Color::Rgb(176, 166, 208);
pub const ACCENT: Color = BRAND_CYAN;
// Meaning colours stay green, amber and red whatever the brand: they have to read as success, caution and failure.
pub const OK: Color = Color::Rgb(120, 226, 140);
pub const ERR: Color = Color::Rgb(255, 128, 142);
pub const WARN: Color = Color::Rgb(255, 205, 80);
pub const SCRIPT: Color = Color::Rgb(204, 170, 255);

/// Lines and bars (at least 3:1 against BG, the AA bar for graphics).
pub const BORDER: Color = Color::Rgb(126, 106, 192);

/// The page background, unless the user asked for the terminal's own.
pub fn background() -> Style {
    static TRANSPARENT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *TRANSPARENT.get_or_init(|| std::env::var_os("NVOKE_TRANSPARENT").is_some_and(|v| !v.is_empty() && v != "0")) {
        Style::new()
    } else {
        Style::new().bg(BG).fg(TEXT)
    }
}

type Rgb = (f32, f32, f32);

fn mix(a: Rgb, b: Rgb, t: f32) -> Color {
    let f = |x: f32, y: f32| (x + (y - x) * t.clamp(0.0, 1.0)).round() as u8;
    Color::Rgb(f(a.0, b.0), f(a.1, b.1), f(a.2, b.2))
}

/// Bar colour for an amount from 0.0 to 1.0: soft violet for a little, the
/// logo's cyan for a lot. Deliberately no red or orange: those mean "failed" here.
pub fn volume(t: f32) -> Color {
    mix((156.0, 120.0, 255.0), (24.0, 208.0, 240.0), t)
}

/// Colour for a success rate from 0.0 to 1.0.
pub fn health(rate: f64) -> Color {
    if rate >= 0.9 {
        OK
    } else if rate >= 0.6 {
        WARN
    } else {
        ERR
    }
}

/// A colour that drifts along the wordmark over time (`tick` advances every frame tick),
/// between the logo's cyan and white.
pub fn shimmer(position: usize, tick: u64) -> Color {
    let wave = ((position as f32 * 0.55 - tick as f32 * 0.12).sin() + 1.0) / 2.0;
    mix((24.0, 208.0, 240.0), (232.0, 250.0, 255.0), wave)
}

/// Whether the logo's underscore cursor is showing: it blinks about twice a second.
pub fn cursor_visible(tick: u64) -> bool {
    (tick / 5).is_multiple_of(2)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(v: u8) -> f64 {
        let v = v as f64 / 255.0;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    }

    /// WCAG relative luminance.
    fn luminance(c: Color) -> f64 {
        let Color::Rgb(r, g, b) = c else { panic!("theme colours are RGB") };
        0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
    }

    fn contrast(a: Color, b: Color) -> f64 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn text_colours_are_readable_on_both_backgrounds() {
        for (name, color) in [("TEXT", TEXT), ("DIM", DIM), ("ACCENT", ACCENT), ("OK", OK), ("ERR", ERR), ("WARN", WARN), ("SCRIPT", SCRIPT)] {
            for (bg_name, bg) in [("BG", BG), ("SELECTED_BG", SELECTED_BG), ("HEADER_BG", HEADER_BG)] {
                let ratio = contrast(color, bg);
                assert!(ratio >= 4.5, "{name} on {bg_name} is only {ratio:.1}:1 (needs 4.5)");
            }
        }
    }

    #[test]
    fn the_palette_is_the_logos() {
        assert_eq!(BRAND_VIOLET, Color::Rgb(48, 0, 112), "#300070, sampled from the logo");
        assert_eq!(ACCENT, BRAND_CYAN);
        assert_eq!(HEADER_BG, BRAND_VIOLET);
    }

    #[test]
    fn selection_is_visibly_different_from_the_page() {
        // Luminance contrast is low between two dark violets, so the hue difference has to be real.
        let (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) = (SELECTED_BG, BG) else { unreachable!() };
        let distance = (r1 as i32 - r2 as i32).abs() + (g1 as i32 - g2 as i32).abs() + (b1 as i32 - b2 as i32).abs();
        assert!(distance >= 120, "selection bar too close to the background: {distance}");
    }

    #[test]
    fn the_cursor_blinks_and_is_visible_at_the_start() {
        assert!(cursor_visible(0) && !cursor_visible(5) && cursor_visible(10));
    }

    #[test]
    fn the_rejected_key_badge_is_readable() {
        assert!(contrast(BG, ERR) >= 4.5, "{:.1}:1", contrast(BG, ERR));
    }

    #[test]
    fn the_dry_run_banner_is_readable() {
        // Dark text on the warning colour.
        assert!(contrast(BG, WARN) >= 4.5, "{:.1}:1", contrast(BG, WARN));
    }

    #[test]
    fn lines_and_bars_stand_out_from_the_background() {
        assert!(contrast(BORDER, BG) >= 3.0, "borders are {:.1}:1", contrast(BORDER, BG));
        for t in [0.0, 0.5, 1.0] {
            assert!(contrast(volume(t), BG) >= 3.0, "volume({t}) is {:.1}:1", contrast(volume(t), BG));
        }
        // The wordmark sits on the header bar.
        for i in 0..40 {
            for tick in [0, 17, 50] {
                assert!(contrast(shimmer(i, tick), HEADER_BG) >= 4.5, "wordmark colour at {i},{tick}");
            }
        }
    }

    #[test]
    fn red_is_never_used_for_volume() {
        // Anything the chart can draw for "a lot of activity" must be clearly not the failure colour.
        for t in [0.0, 0.5, 1.0] {
            let Color::Rgb(r, _, b) = volume(t) else { unreachable!() };
            assert!(b > r, "volume({t}) looks warm");
        }
    }

    #[test]
    fn health_colours_follow_the_rate() {
        assert_eq!(health(1.0), OK);
        assert_eq!(health(0.75), WARN);
        assert_eq!(health(0.2), ERR);
    }
}
