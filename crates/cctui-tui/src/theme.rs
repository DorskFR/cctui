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
