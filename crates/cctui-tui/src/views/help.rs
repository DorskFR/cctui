use crossterm::event::KeyCode;
use ratatui::Frame;
use ratatui::layout::Margin;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

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
    groups
        .iter()
        .map(|group| match group.len() {
            n if n > 2 => format!("{}-{}", group[0].label(), group[n - 1].label()),
            _ => group.iter().map(|c| c.label()).collect::<Vec<_>>().join(" / "),
        })
        .collect::<Vec<_>>()
        .join(" / ")
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

fn spans(row: &Row, width: usize) -> Vec<Span<'static>> {
    match row {
        Row::Heading(title) => {
            vec![Span::styled(format!("{title:<width$}"), theme::section_title())]
        }
        Row::Binding { keys, desc } => {
            let desc_width = width.saturating_sub(KEYS_WIDTH + 2);
            vec![
                Span::raw("  "),
                Span::styled(format!("{keys:<KEYS_WIDTH$}"), theme::hotkey()),
                Span::raw(format!("{desc:<desc_width$}")),
            ]
        }
    }
}

/// The cheat sheet is the keymap: a rebound or unbound key shows up here with
/// no second list to keep in step.
pub fn draw(frame: &mut Frame, keys: &Keymap) {
    let area = frame.area().inner(Margin { horizontal: 2, vertical: 1 });
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border_focused())
        .title(" Keys ");
    let inner = block.inner(area);
    let rows = rows(keys);

    let height = (inner.height as usize).max(1);
    let columns = usize::from(rows.len() > height) + 1;
    let per_column = rows.len().div_ceil(columns);
    let column_width = (inner.width as usize) / columns;

    let lines: Vec<Line> = (0..per_column.min(height))
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
