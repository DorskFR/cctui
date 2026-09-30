//! Key chords: the `ctrl+c` / `pageup` / `G` syntax used in `tui.toml`.

use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chord {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl Chord {
    pub const fn new(code: KeyCode, mods: KeyModifiers) -> Self {
        Self { code, mods }
    }

    /// A pressed key reduced to its lookup form: a character already carries
    /// its own case, so `SHIFT` on `G` must not make it a different chord.
    pub fn from_event(event: KeyEvent) -> Self {
        let mut mods = event.modifiers;
        if matches!(event.code, KeyCode::Char(_)) {
            mods.remove(KeyModifiers::SHIFT);
        }
        Self { code: event.code, mods }
    }

    /// The digit of a `1`..`9` chord, for the bindings that carry an index.
    pub const fn digit(self) -> Option<usize> {
        match self.code {
            KeyCode::Char(c @ '1'..='9') => Some(c as usize - '1' as usize),
            _ => None,
        }
    }

    /// How a chord is shown to the user, as opposed to how it is written in
    /// `tui.toml` ([`Display`](std::fmt::Display)).
    pub fn label(self) -> String {
        let mut out = String::new();
        if self.mods.contains(KeyModifiers::CONTROL) {
            out.push_str("Ctrl+");
        }
        if self.mods.contains(KeyModifiers::ALT) {
            out.push_str("Alt+");
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            out.push_str("Shift+");
        }
        out.push_str(match self.code {
            KeyCode::Char(' ') => "Space",
            KeyCode::Char(c) => return out + &c.to_string(),
            KeyCode::F(n) => return out + &format!("F{n}"),
            KeyCode::Enter => "Enter",
            KeyCode::Esc => "Esc",
            KeyCode::Tab => "Tab",
            KeyCode::BackTab => "Shift+Tab",
            KeyCode::Backspace => "Bksp",
            KeyCode::Delete => "Del",
            KeyCode::Insert => "Ins",
            KeyCode::Up => "↑",
            KeyCode::Down => "↓",
            KeyCode::Left => "←",
            KeyCode::Right => "→",
            KeyCode::Home => "Home",
            KeyCode::End => "End",
            KeyCode::PageUp => "PgUp",
            KeyCode::PageDown => "PgDn",
            _ => return out + &self.to_string(),
        });
        out
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        if let Some(c) = single_char(text) {
            return Ok(Self::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let mut mods = KeyModifiers::NONE;
        let mut rest = text;
        while let Some((head, tail)) = rest.split_once('+') {
            if head.is_empty() || tail.is_empty() {
                return Err(format!("`{text}` is not a key"));
            }
            match head.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => mods |= KeyModifiers::CONTROL,
                "alt" | "meta" => mods |= KeyModifiers::ALT,
                "shift" => mods |= KeyModifiers::SHIFT,
                other => return Err(format!("`{other}` is not a modifier (in `{text}`)")),
            }
            rest = tail;
        }
        let code =
            parse_code(rest).ok_or_else(|| format!("`{rest}` is not a key (in `{text}`)"))?;
        if matches!(code, KeyCode::Char(_)) {
            mods.remove(KeyModifiers::SHIFT);
        }
        Ok(Self { code, mods })
    }

    /// Every chord a spec string names: comma-separated, with `a-z` ranges.
    pub fn parse_list(text: &str) -> Result<Vec<Self>, String> {
        let mut out = Vec::new();
        for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            match char_range(part) {
                Some((from, to)) => {
                    for c in from..=to {
                        out.push(Self::new(KeyCode::Char(c), KeyModifiers::NONE));
                    }
                }
                None => out.push(Self::parse(part)?),
            }
        }
        if out.is_empty() {
            return Err(format!("`{text}` names no key"));
        }
        Ok(out)
    }
}

/// `1-9` / `a-f`: a plain ascending range of single characters.
fn char_range(part: &str) -> Option<(char, char)> {
    let (a, b) = part.split_once('-')?;
    let (a, b) = (single_char(a)?, single_char(b)?);
    (a < b && a.is_ascii_alphanumeric() && b.is_ascii_alphanumeric()).then_some((a, b))
}

fn single_char(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let c = chars.next()?;
    chars.next().is_none().then_some(c)
}

fn parse_code(text: &str) -> Option<KeyCode> {
    if let Some(c) = single_char(text) {
        return Some(KeyCode::Char(c));
    }
    let lower = text.to_ascii_lowercase();
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok())
        && (1..=12).contains(&n)
    {
        return Some(KeyCode::F(n));
    }
    Some(match lower.as_str() {
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "space" => KeyCode::Char(' '),
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "backspace" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "insert" | "ins" => KeyCode::Insert,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "pgup" => KeyCode::PageUp,
        "pagedown" | "pgdn" => KeyCode::PageDown,
        _ => return None,
    })
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.mods.contains(KeyModifiers::CONTROL) {
            write!(f, "ctrl+")?;
        }
        if self.mods.contains(KeyModifiers::ALT) {
            write!(f, "alt+")?;
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            write!(f, "shift+")?;
        }
        match self.code {
            KeyCode::Char(' ') => f.write_str("space"),
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::F(n) => write!(f, "f{n}"),
            KeyCode::Enter => f.write_str("enter"),
            KeyCode::Esc => f.write_str("esc"),
            KeyCode::Tab => f.write_str("tab"),
            KeyCode::BackTab => f.write_str("backtab"),
            KeyCode::Backspace => f.write_str("backspace"),
            KeyCode::Delete => f.write_str("delete"),
            KeyCode::Insert => f.write_str("insert"),
            KeyCode::Up => f.write_str("up"),
            KeyCode::Down => f.write_str("down"),
            KeyCode::Left => f.write_str("left"),
            KeyCode::Right => f.write_str("right"),
            KeyCode::Home => f.write_str("home"),
            KeyCode::End => f.write_str("end"),
            KeyCode::PageUp => f.write_str("pageup"),
            KeyCode::PageDown => f.write_str("pagedown"),
            other => write!(f, "{other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::Chord;

    #[test]
    fn modifiers_and_named_keys_round_trip() {
        for text in ["ctrl+c", "alt+enter", "shift+tab", "pageup", "g", "?", "f5", "space"] {
            let chord = Chord::parse(text).expect("parses");
            assert_eq!(chord.to_string(), text, "round trip of {text}");
        }
    }

    #[test]
    fn a_capital_letter_needs_no_shift_modifier() {
        let parsed = Chord::parse("G").expect("parses");
        assert_eq!(parsed, Chord::new(KeyCode::Char('G'), KeyModifiers::NONE));
        let pressed = Chord::from_event(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT));
        assert_eq!(pressed, parsed);
    }

    #[test]
    fn a_list_expands_ranges_and_commas() {
        let chords = Chord::parse_list("1-9").expect("parses");
        assert_eq!(chords.len(), 9);
        assert_eq!(chords[2].digit(), Some(2));
        assert_eq!(Chord::parse_list("j, down").expect("parses").len(), 2);
    }

    #[test]
    fn unknown_keys_and_modifiers_are_rejected() {
        assert!(Chord::parse("hyper+x").is_err());
        assert!(Chord::parse("nosuchkey").is_err());
        assert!(Chord::parse_list("  ").is_err());
    }
}
