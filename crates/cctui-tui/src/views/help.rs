use crossterm::event::KeyCode;
use ratatui::Frame;
use ratatui::layout::Margin;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::config::chord::Chord;
use crate::config::keymap::{CONTEXTS, Keymap};
use crate::keys::is_wired;
use crate::theme;

const KEYS_WIDTH: usize = 12;

enum Row {
    Heading(&'static str),
    Binding { keys: String, desc: &'static str },
}

/// Consecutive single characters collapse to a range, so nine digit bindings
/// read as `1-9` rather than filling the line.
fn keys_label(chords: &[Chord]) -> String {
    let mut groups: Vec<Vec<Chord>> = Vec::new();
    for chord in chords {
        let extends = groups.last().and_then(|g| g.last()).is_some_and(|prev| {
            match (prev.code, chord.code) {
                (KeyCode::Char(a), KeyCode::Char(b)) => {
                    prev.mods == chord.mods
                        && a.is_ascii_alphanumeric()
                        && b.is_ascii_alphanumeric()
                        && a as u32 + 1 == b as u32
                }
                _ => false,
            }
        });
        if extends {
            groups.last_mut().expect("a group to extend").push(*chord);
        } else {
            groups.push(vec![*chord]);
        }
    }
    let mut labels: Vec<String> = Vec::with_capacity(groups.len());
    for group in &groups {
        let label = match group.len() {
            n if n > 2 => format!("{}-{}", group[0].label(), group[n - 1].label()),
            _ => group.iter().map(|c| c.label()).collect::<Vec<_>>().join(" / "),
        };
        // Distinct chords can share a label (Tab and BackTab with a stray
        // modifier); the sheet must not print it twice.
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    labels.join(" / ")
}

fn rows(keys: &Keymap) -> Vec<Row> {
    let mut rows = Vec::new();
    for context in CONTEXTS {
        let entries: Vec<_> =
            keys.entries(*context).into_iter().filter(|(action, _)| is_wired(*action)).collect();
        if entries.is_empty() {
            continue;
        }
        rows.push(Row::Heading(context.title()));
        for (action, chords) in entries {
            rows.push(Row::Binding { keys: keys_label(&chords), desc: action.description() });
        }
    }
    rows.push(Row::Heading("Row glyphs"));
    for &(glyph, desc) in crate::app::session_status::GLYPH_LEGEND {
        rows.push(Row::Binding { keys: glyph.to_owned(), desc });
    }
    rows
}

/// Clip to `width` display columns, then pad to it: a label never pushes the
/// next column along, and a two-cell glyph like ⚡ does not leave it ragged.
fn fit(text: &str, width: usize) -> String {
    let clipped = crate::app::session_status::truncate(text, width);
    let pad = width.saturating_sub(UnicodeWidthStr::width(clipped.as_str()));
    format!("{clipped}{}", " ".repeat(pad))
}

fn spans(row: &Row, width: usize) -> Vec<Span<'static>> {
    match row {
        Row::Heading(title) => vec![Span::styled(fit(title, width), theme::section_title())],
        Row::Binding { keys, desc } => {
            let desc_width = width.saturating_sub(KEYS_WIDTH + 2);
            vec![
                Span::raw("  "),
                Span::styled(fit(keys, KEYS_WIDTH - 1) + " ", theme::hotkey()),
                Span::raw(fit(desc, desc_width)),
            ]
        }
    }
}

/// The cheat sheet is the keymap: a rebound or unbound key shows up here with
/// no second list to keep in step.
pub fn draw(frame: &mut Frame, keys: &Keymap, scroll: &mut usize) {
    let area = frame.area().inner(Margin { horizontal: 2, vertical: 1 });
    frame.render_widget(Clear, area);

    let rows = rows(keys);
    let height = (area.height.saturating_sub(2) as usize).max(1);
    let columns = usize::from(rows.len() > height) + 1;
    let per_column = rows.len().div_ceil(columns);
    let max_scroll = per_column.saturating_sub(height);
    *scroll = (*scroll).min(max_scroll);

    let title =
        if max_scroll == 0 { " Keys ".to_owned() } else { " Keys · j/k to scroll ".to_owned() };
    let block =
        Block::default().borders(Borders::ALL).border_style(theme::border_focused()).title(title);
    let inner = block.inner(area);
    let column_width = (inner.width as usize) / columns;

    let lines: Vec<Line> = (*scroll..per_column.min(*scroll + height))
        .map(|i| {
            let mut out = spans(&rows[i], column_width);
            if let Some(right) = rows.get(i + per_column) {
                out.extend(spans(right, column_width));
            }
            Line::from(out)
        })
        .collect();

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::{Keymap, Row, keys_label, rows};
    use crate::config::chord::Chord;
    use crate::config::keymap::Context;

    #[test]
    fn a_digit_run_collapses_to_a_range() {
        let chords = Chord::parse_list("1-9").expect("parses");
        assert_eq!(keys_label(&chords), "1-9");
        assert_eq!(keys_label(&Chord::parse_list("j, down").expect("parses")), "j / ↓");
    }

    #[test]
    fn a_label_two_chords_share_is_printed_once() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let plain = Chord::new(KeyCode::BackTab, KeyModifiers::NONE);
        let shifted = Chord::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(plain.label(), "Shift+Tab");
        assert_eq!(shifted.label(), "Shift+Tab", "BackTab already carries the shift");
        assert_eq!(keys_label(&[plain, shifted]), "Shift+Tab");
    }

    #[test]
    fn the_sheet_follows_the_keymap() {
        let mut keys = Keymap::default();
        keys.set(Context::SessionList, "enter", "none").expect("valid");
        keys.set(Context::SessionList, "o", "open-conversation").expect("valid");
        let listed: Vec<String> = rows(&keys)
            .iter()
            .filter_map(|row| match row {
                Row::Binding { keys, desc } if *desc == "Open the conversation" => {
                    Some(keys.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(listed, vec!["o".to_owned()]);
    }
}
