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

/// A matched search term: the same tint the transcript search uses, so a hit
/// reads the same wherever it is found.
#[must_use]
pub const fn search_hit() -> Style {
    Style::new().fg(Color::Rgb(20, 20, 24)).bg(Color::Rgb(210, 180, 90))
}

/// Saturation and lightness every hue-derived accent shares: mid-tones that
/// stay legible on both palettes.
const ACCENT_SAT: f64 = 0.60;
const ACCENT_LIGHT: f64 = 0.65;

/// Whether the terminal took 24-bit colour. Probed once: the answer cannot
/// change under a running process, and a per-row probe would read the
/// environment thousands of times a second.
fn truecolor() -> bool {
    static TRUECOLOR: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *TRUECOLOR.get_or_init(|| {
        supports_color::on(supports_color::Stream::Stdout).is_some_and(|level| level.has_16m)
    })
}

/// The 8 bright ANSI colours, by 45° hue sector: what a terminal without
/// truecolor gets instead of an exact hue.
const ANSI_BY_SECTOR: [Color; 8] = [
    Color::LightRed,
    Color::LightYellow,
    Color::LightGreen,
    Color::LightGreen,
    Color::LightCyan,
    Color::LightBlue,
    Color::LightMagenta,
    Color::LightMagenta,
];

#[must_use]
pub fn hue_to_rgb(hue: u32, sat: f64, light: f64) -> (u8, u8, u8) {
    let sector = f64::from(hue % 360) / 60.0;
    let chroma = (1.0 - 2.0f64.mul_add(light, -1.0).abs()) * sat;
    let second = chroma * (1.0 - (sector % 2.0 - 1.0).abs());
    let base = chroma.mul_add(-0.5, light);
    let (red, green, blue) = match sector as u32 {
        0 => (chroma, second, 0.0),
        1 => (second, chroma, 0.0),
        2 => (0.0, chroma, second),
        3 => (0.0, second, chroma),
        4 => (second, 0.0, chroma),
        _ => (chroma, 0.0, second),
    };
    let to_byte = |v: f64| ((v + base) * 255.0).round().clamp(0.0, 255.0) as u8;
    (to_byte(red), to_byte(green), to_byte(blue))
}

/// Foreground style for a dimension's hue, truecolor when the terminal has it
/// and the nearest bright ANSI colour when it does not.
#[must_use]
pub fn hue_style(hue: u32) -> Style {
    if truecolor() {
        let (r, g, b) = hue_to_rgb(hue, ACCENT_SAT, ACCENT_LIGHT);
        return Style::new().fg(Color::Rgb(r, g, b));
    }
    let sector = ((hue % 360) / 45) as usize;
    Style::new().fg(ANSI_BY_SECTOR[sector.min(ANSI_BY_SECTOR.len() - 1)])
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

    #[test]
    fn a_hue_becomes_the_rgb_its_hsl_describes() {
        assert_eq!(super::hue_to_rgb(0, 1.0, 0.5), (255, 0, 0));
        assert_eq!(super::hue_to_rgb(120, 1.0, 0.5), (0, 255, 0));
        assert_eq!(super::hue_to_rgb(240, 1.0, 0.5), (0, 0, 255));
        assert_eq!(super::hue_to_rgb(0, 0.0, 0.5), (128, 128, 128), "no saturation is grey");
        assert_eq!(super::hue_to_rgb(360, 1.0, 0.5), (255, 0, 0), "the wheel wraps");
    }

    #[test]
    fn the_accent_palette_stays_inside_the_byte_range_for_every_hue() {
        for hue in 0..360 {
            let (r, g, b) = super::hue_to_rgb(hue, super::ACCENT_SAT, super::ACCENT_LIGHT);
            assert!(
                r > 80 && g > 80 && b > 80 || r > 150 || g > 150 || b > 150,
                "hue {hue} is too dark"
            );
        }
    }

    #[test]
    fn every_hue_sector_has_an_ansi_fallback() {
        for hue in [0_u32, 44, 45, 180, 359] {
            let sector = ((hue % 360) / 45) as usize;
            assert!(sector < super::ANSI_BY_SECTOR.len(), "hue {hue} has no sector");
        }
    }
}

#[cfg(test)]
mod hue_tests {
    use super::{ANSI_BY_SECTOR, hue_style, hue_to_rgb};

    #[test]
    fn every_hue_maps_to_one_of_the_eight_sectors() {
        for hue in 0..360 {
            let sector = (hue / 45) as usize;
            assert!(sector < ANSI_BY_SECTOR.len(), "hue {hue} fell outside the table");
        }
    }

    #[test]
    fn a_hue_past_one_turn_still_resolves() {
        assert_eq!(hue_style(720), hue_style(0));
        assert_eq!(hue_style(361), hue_style(1));
    }

    #[test]
    fn rgb_hits_the_primaries_and_never_goes_unreadably_dark() {
        assert_eq!(hue_to_rgb(0, 1.0, 0.5), (255, 0, 0));
        assert_eq!(hue_to_rgb(120, 1.0, 0.5), (0, 255, 0));
        assert_eq!(hue_to_rgb(240, 1.0, 0.5), (0, 0, 255));
        for hue in (0..360).step_by(7) {
            let (r, g, b) = hue_to_rgb(hue, super::ACCENT_SAT, super::ACCENT_LIGHT);
            assert!(
                u32::from(r) + u32::from(g) + u32::from(b) > 180,
                "hue {hue} is too dark to read"
            );
        }
    }
}
