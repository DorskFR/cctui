use ratatui::Frame;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::config::keymap::{ActionId, Context, Keymap};
use crate::theme;

/// The footer is a digest of the keymap, not a second list of bindings: each
/// entry is the first key still bound to that action, or nothing at all.
const FOOTER: &[(&[ActionId], &str)] = &[
    (&[ActionId::SelectNext, ActionId::SelectPrev], "nav"),
    (&[ActionId::OpenConversation], "open"),
    (&[ActionId::SelectFirst, ActionId::SelectLast], "top/bottom"),
    (&[ActionId::Help], "help"),
    (&[ActionId::Quit], "quit"),
];

fn first_key(keys: &Keymap, action: ActionId) -> Option<String> {
    let own = keys.chords_for(Context::SessionList, action);
    let chords = if own.is_empty() { keys.chords_for(Context::Global, action) } else { own };
    chords.first().map(|chord| chord.label())
}

pub fn draw_session_hotkeys(frame: &mut Frame, area: ratatui::layout::Rect, keys: &Keymap) {
    let mut spans = Vec::new();
    for (actions, label) in FOOTER {
        let bound: Vec<String> = actions.iter().filter_map(|a| first_key(keys, *a)).collect();
        if bound.is_empty() {
            continue;
        }
        let prefix = if spans.is_empty() { " " } else { "  " };
        spans.push(Span::styled(format!("{prefix}{}", bound.join("/")), theme::hotkey()));
        spans.push(Span::styled(format!(":{label}"), theme::hotkey_desc()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests {
    use super::{Keymap, first_key};
    use crate::config::keymap::{ActionId, Context};

    #[test]
    fn the_footer_reads_the_current_binding() {
        let mut keys = Keymap::default();
        assert_eq!(first_key(&keys, ActionId::SelectNext).as_deref(), Some("j"));
        assert_eq!(first_key(&keys, ActionId::Quit).as_deref(), Some("q"));
        keys.set(Context::SessionList, "j", "none").expect("valid");
        assert_eq!(first_key(&keys, ActionId::SelectNext).as_deref(), Some("↓"));
    }
}
