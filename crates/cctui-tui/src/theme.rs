//! One palette, chosen at startup and read through these accessors.

use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};

use crate::config::ThemeChoice;

pub struct Palette {
    pub active: Style,
    pub new: Style,
    pub inactive: Style,
    pub border_focused: Style,
    pub border_dim: Style,
    pub selected: Style,
    pub hotkey: Style,
    pub hotkey_desc: Style,
    pub status_bar_bg: Style,
    pub dim: Style,
    pub bold: Style,
    pub error: Style,
    pub model: Style,
    pub cost: Style,
    pub branch: Style,
    pub header_bg: Style,
    pub section_title: Style,
    pub stale: Style,
    pub attention: Style,
    pub unread: Style,
}

const fn dark() -> Palette {
    Palette {
        active: Style::new().fg(Color::Green),
        new: Style::new().fg(Color::Cyan),
        inactive: Style::new().fg(Color::DarkGray),
        border_focused: Style::new().fg(Color::Blue),
        border_dim: Style::new().fg(Color::DarkGray),
        selected: Style::new().bg(Color::DarkGray).add_modifier(Modifier::BOLD),
        hotkey: Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        hotkey_desc: Style::new().fg(Color::DarkGray),
        status_bar_bg: Style::new().fg(Color::White).bg(Color::DarkGray),
        dim: Style::new().fg(Color::DarkGray),
        bold: Style::new().add_modifier(Modifier::BOLD),
        error: Style::new().fg(Color::Red),
        model: Style::new().fg(Color::DarkGray),
        cost: Style::new().fg(Color::Yellow),
        branch: Style::new().fg(Color::DarkGray),
        header_bg: Style::new().fg(Color::White).bg(Color::DarkGray),
        section_title: Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD),
        stale: Style::new().fg(Color::Yellow),
        attention: Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        unread: Style::new().fg(Color::Magenta),
    }
}

/// Dimmed text has to darken, not lighten, on a light terminal, and the
/// bright-on-dark chrome inverts with it.
const fn light() -> Palette {
    Palette {
        active: Style::new().fg(Color::Green),
        new: Style::new().fg(Color::Blue),
        inactive: Style::new().fg(Color::Gray),
        border_focused: Style::new().fg(Color::Blue),
        border_dim: Style::new().fg(Color::Gray),
        selected: Style::new().bg(Color::Gray).add_modifier(Modifier::BOLD),
        hotkey: Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD),
        hotkey_desc: Style::new().fg(Color::DarkGray),
        status_bar_bg: Style::new().fg(Color::Black).bg(Color::Gray),
        dim: Style::new().fg(Color::DarkGray),
        bold: Style::new().add_modifier(Modifier::BOLD),
        error: Style::new().fg(Color::Red),
        model: Style::new().fg(Color::DarkGray),
        cost: Style::new().fg(Color::Magenta),
        branch: Style::new().fg(Color::DarkGray),
        header_bg: Style::new().fg(Color::Black).bg(Color::Gray),
        section_title: Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD),
        stale: Style::new().fg(Color::Yellow),
        attention: Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        unread: Style::new().fg(Color::Magenta),
    }
}

static PALETTE: OnceLock<Palette> = OnceLock::new();

pub fn init(choice: ThemeChoice) {
    let _ = PALETTE.set(match resolve(choice) {
        ThemeChoice::Light => light(),
        _ => dark(),
    });
}

/// `COLORFGBG` is the one background hint a terminal reliably exports; without
/// it, dark is the safer guess.
pub fn resolve(choice: ThemeChoice) -> ThemeChoice {
    match choice {
        ThemeChoice::Auto => {
            match std::env::var("COLORFGBG").ok().as_deref().and_then(background_index) {
                Some(bg) if (7..=15).contains(&bg) => ThemeChoice::Light,
                _ => ThemeChoice::Dark,
            }
        }
        pinned => pinned,
    }
}

fn background_index(value: &str) -> Option<u8> {
    value.rsplit(';').next()?.trim().parse().ok()
}

pub fn palette() -> &'static Palette {
    PALETTE.get_or_init(dark)
}

macro_rules! accessors {
    ($($name:ident),* $(,)?) => {
        $(
            pub fn $name() -> Style {
                palette().$name
            }
        )*
    };
}

accessors!(
    active,
    new,
    inactive,
    border_focused,
    border_dim,
    selected,
    hotkey,
    hotkey_desc,
    status_bar_bg,
    dim,
    bold,
    error,
    model,
    cost,
    branch,
    header_bg,
    section_title,
    stale,
    attention,
    unread,
);

pub fn liveness_style(liveness: crate::app::session_status::RowLiveness) -> Style {
    use crate::app::session_status::RowLiveness;
    match liveness {
        RowLiveness::Active => active(),
        RowLiveness::Stale => stale(),
        RowLiveness::Hibernated => new(),
        RowLiveness::Dead => inactive(),
    }
}

#[cfg(test)]
mod tests {
    use super::{ThemeChoice, light, resolve};

    #[test]
    fn a_pinned_choice_is_not_second_guessed() {
        assert_eq!(resolve(ThemeChoice::Light), ThemeChoice::Light);
        assert_eq!(resolve(ThemeChoice::Dark), ThemeChoice::Dark);
    }

    #[test]
    fn the_light_palette_dims_downwards() {
        assert_eq!(light().dim, super::dark().dim);
        assert_ne!(light().selected, super::dark().selected);
    }
}

/// Whether the terminal takes 24-bit colour. Probed once: a hue is a continuous
/// value and the 16-colour fallback has to bucket it.
fn truecolor() -> bool {
    static TRUECOLOR: OnceLock<bool> = OnceLock::new();
    *TRUECOLOR.get_or_init(|| {
        supports_color::on(supports_color::Stream::Stdout).is_some_and(|s| s.has_16m)
    })
}

/// A hue (0–359, as `cctui_clientcore::labels::label_hue` gives it) as a
/// foreground colour: exact on a truecolor terminal, bucketed to the nearest of
/// the six ANSI hues otherwise.
#[must_use]
pub fn hue_fg(hue: u32) -> Style {
    if truecolor() {
        let (r, g, b) = hsl_to_rgb(hue, 0.65, 0.65);
        return Style::new().fg(Color::Rgb(r, g, b));
    }
    Style::new().fg(ansi_bucket(hue))
}

/// The six ANSI hues sit 60° apart, so a bucket is the nearest multiple of 60.
const fn ansi_bucket(hue: u32) -> Color {
    match ((hue % 360) + 30) / 60 {
        1 => Color::Yellow,
        2 => Color::Green,
        3 => Color::Cyan,
        4 => Color::Blue,
        5 => Color::Magenta,
        _ => Color::Red,
    }
}

/// HSL with `s`/`l` in 0..=1, matching the webui's chip saturation and lightness
/// closely enough that a label reads as the same colour in both clients.
#[allow(clippy::many_single_char_names)]
fn hsl_to_rgb(hue: u32, s: f64, l: f64) -> (u8, u8, u8) {
    let h = f64::from(hue % 360) / 60.0;
    let c = (1.0 - 2.0f64.mul_add(l, -1.0).abs()) * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let byte = |v: f64| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (byte(r), byte(g), byte(b))
}

#[cfg(test)]
mod hue_tests {
    use super::{ansi_bucket, hsl_to_rgb};
    use ratatui::style::Color;

    #[test]
    fn the_six_ansi_hues_sit_at_their_own_centres() {
        assert_eq!(ansi_bucket(0), Color::Red);
        assert_eq!(ansi_bucket(60), Color::Yellow);
        assert_eq!(ansi_bucket(120), Color::Green);
        assert_eq!(ansi_bucket(180), Color::Cyan);
        assert_eq!(ansi_bucket(240), Color::Blue);
        assert_eq!(ansi_bucket(300), Color::Magenta);
    }

    #[test]
    fn a_bucket_rounds_to_the_nearest_hue_and_wraps() {
        assert_eq!(ansi_bucket(29), Color::Red);
        assert_eq!(ansi_bucket(31), Color::Yellow);
        assert_eq!(ansi_bucket(359), Color::Red, "just short of 360 is still red");
        assert_eq!(ansi_bucket(720), Color::Red, "a hue past one turn still buckets");
    }

    #[test]
    fn hsl_hits_the_primaries_and_stays_in_range() {
        let (r, g, b) = hsl_to_rgb(0, 1.0, 0.5);
        assert_eq!((r, g, b), (255, 0, 0));
        assert_eq!(hsl_to_rgb(120, 1.0, 0.5), (0, 255, 0));
        assert_eq!(hsl_to_rgb(240, 1.0, 0.5), (0, 0, 255));
        for hue in (0..360).step_by(7) {
            let (r, g, b) = hsl_to_rgb(hue, 0.65, 0.65);
            assert!(r > 60 && g > 60 && b > 60, "hue {hue} is too dark to read");
        }
    }
}
