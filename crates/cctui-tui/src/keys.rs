use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::action::Action;
use crate::app::state::View;

#[derive(Debug, Clone)]
pub(crate) enum InputEvent {
    Key(KeyEvent),
    ScrollUp,
    ScrollDown,
}

/// Pure: terminal input in, at most one [`Action`] out. No state is touched
/// here, so every binding is testable without a terminal.
pub(crate) fn map_input(view: View, input_active: bool, input: InputEvent) -> Option<Action> {
    match input {
        InputEvent::Key(key) if input_active => Some(map_composer(key)),
        InputEvent::Key(key) => match view {
            View::SessionList => map_session_list(key.code),
            View::Conversation => Some(map_conversation(key)),
            View::Help => map_help(key.code),
            View::PermissionDialog => map_permission(key.code),
        },
        InputEvent::ScrollUp => match view {
            View::Conversation => Some(Action::Scroll { lines: -3, release_follow: true }),
            View::SessionList => Some(Action::SelectPrev),
            View::Help | View::PermissionDialog => None,
        },
        InputEvent::ScrollDown => match view {
            View::Conversation => Some(Action::Scroll { lines: 3, release_follow: false }),
            View::SessionList => Some(Action::SelectNext),
            View::Help | View::PermissionDialog => None,
        },
    }
}

fn map_composer(key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::CancelInput,
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => Action::InputNewline,
        KeyCode::Enter => Action::SubmitInput,
        _ => Action::InputKey(key),
    }
}

fn map_session_list(code: KeyCode) -> Option<Action> {
    Some(match code {
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Char('j') | KeyCode::Down => Action::SelectNext,
        KeyCode::Char('k') | KeyCode::Up => Action::SelectPrev,
        KeyCode::Char('g') => Action::SelectFirst,
        KeyCode::Char('G') => Action::SelectLast,
        KeyCode::Char('a') => Action::ToggleShowAllSessions,
        KeyCode::Char('?') => Action::OpenHelp,
        KeyCode::Enter => Action::OpenSelectedConversation,
        _ => return None,
    })
}

/// Ctrl-modified actions are matched first, then navigation; anything left over
/// opens the composer and types itself.
fn map_conversation(key: KeyEvent) -> Action {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('c') => return Action::InterruptSelected,
            KeyCode::Char('a') => return Action::ToggleAutoApproveSelected,
            _ => {}
        }
    }
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => Action::LeaveConversation,
        KeyCode::Char('j') | KeyCode::Down => Action::Scroll { lines: 1, release_follow: true },
        KeyCode::Char('k') | KeyCode::Up => Action::Scroll { lines: -1, release_follow: true },
        KeyCode::PageUp => Action::Scroll { lines: -15, release_follow: true },
        KeyCode::PageDown => Action::Scroll { lines: 15, release_follow: false },
        KeyCode::Char('g') => Action::ScrollToTop,
        KeyCode::Char('G') => Action::ScrollToBottom,
        KeyCode::Char('?') => Action::OpenHelp,
        KeyCode::Char('t') => Action::ToggleTimestamps,
        KeyCode::Char(c @ '1'..='9') => Action::SelectIndex((c as usize) - ('1' as usize)),
        _ => Action::ActivateInputWith(key),
    }
}

fn map_help(code: KeyCode) -> Option<Action> {
    matches!(code, KeyCode::Esc | KeyCode::Char('?' | 'q')).then_some(Action::CloseHelp)
}

fn map_permission(code: KeyCode) -> Option<Action> {
    Some(match code {
        KeyCode::Char('y') | KeyCode::Enter => Action::ResolvePermission { allow: true },
        KeyCode::Char('n') | KeyCode::Esc => Action::ResolvePermission { allow: false },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    use super::{Action, InputEvent, View, map_input};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn map(view: View, input_active: bool, code: KeyCode) -> Option<Action> {
        map_input(view, input_active, InputEvent::Key(key(code)))
    }

    #[test]
    fn the_session_list_navigates_and_opens() {
        assert!(matches!(
            map(View::SessionList, false, KeyCode::Char('j')),
            Some(Action::SelectNext)
        ));
        assert!(matches!(map(View::SessionList, false, KeyCode::Up), Some(Action::SelectPrev)));
        assert!(matches!(
            map(View::SessionList, false, KeyCode::Enter),
            Some(Action::OpenSelectedConversation)
        ));
        assert!(matches!(map(View::SessionList, false, KeyCode::Char('q')), Some(Action::Quit)));
        assert!(map(View::SessionList, false, KeyCode::Char('z')).is_none());
    }

    #[test]
    fn conversation_scroll_keys_keep_their_follow_tail_behaviour() {
        assert!(matches!(
            map(View::Conversation, false, KeyCode::Char('j')),
            Some(Action::Scroll { lines: 1, release_follow: true })
        ));
        assert!(matches!(
            map(View::Conversation, false, KeyCode::PageDown),
            Some(Action::Scroll { lines: 15, release_follow: false })
        ));
        assert!(matches!(
            map(View::Conversation, false, KeyCode::PageUp),
            Some(Action::Scroll { lines: -15, release_follow: true })
        ));
    }

    #[test]
    fn an_unclaimed_conversation_key_opens_the_composer_with_it() {
        match map(View::Conversation, false, KeyCode::Char('h')) {
            Some(Action::ActivateInputWith(k)) => assert_eq!(k.code, KeyCode::Char('h')),
            _ => panic!("expected the composer to open"),
        }
    }

    #[test]
    fn ctrl_bindings_win_over_navigation() {
        assert!(matches!(
            map_input(View::Conversation, false, InputEvent::Key(ctrl('c'))),
            Some(Action::InterruptSelected)
        ));
        assert!(matches!(
            map_input(View::Conversation, false, InputEvent::Key(ctrl('a'))),
            Some(Action::ToggleAutoApproveSelected)
        ));
    }

    #[test]
    fn digits_jump_to_a_session_row() {
        assert!(matches!(
            map(View::Conversation, false, KeyCode::Char('3')),
            Some(Action::SelectIndex(2))
        ));
    }

    #[test]
    fn an_active_composer_swallows_navigation_keys() {
        assert!(matches!(map(View::Conversation, true, KeyCode::Char('j')), Some(Action::InputKey(_))));
        assert!(matches!(map(View::Conversation, true, KeyCode::Esc), Some(Action::CancelInput)));
        assert!(matches!(map(View::Conversation, true, KeyCode::Enter), Some(Action::SubmitInput)));
    }

    #[test]
    fn shift_enter_inserts_a_newline_instead_of_sending() {
        let event = KeyEvent {
            kind: KeyEventKind::Press,
            ..KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)
        };
        assert!(matches!(
            map_input(View::Conversation, true, InputEvent::Key(event)),
            Some(Action::InputNewline)
        ));
    }

    #[test]
    fn the_permission_dialog_only_answers_yes_or_no() {
        assert!(matches!(
            map(View::PermissionDialog, false, KeyCode::Char('y')),
            Some(Action::ResolvePermission { allow: true })
        ));
        assert!(matches!(
            map(View::PermissionDialog, false, KeyCode::Esc),
            Some(Action::ResolvePermission { allow: false })
        ));
        assert!(map(View::PermissionDialog, false, KeyCode::Char('j')).is_none());
    }

    #[test]
    fn the_mouse_wheel_navigates_the_list_and_scrolls_the_conversation() {
        assert!(matches!(
            map_input(View::SessionList, false, InputEvent::ScrollDown),
            Some(Action::SelectNext)
        ));
        assert!(matches!(
            map_input(View::Conversation, false, InputEvent::ScrollUp),
            Some(Action::Scroll { lines: -3, release_follow: true })
        ));
        assert!(map_input(View::Help, false, InputEvent::ScrollUp).is_none());
    }

    #[test]
    fn help_dismisses_on_escape_question_mark_or_q() {
        for code in [KeyCode::Esc, KeyCode::Char('?'), KeyCode::Char('q')] {
            assert!(matches!(map(View::Help, false, code), Some(Action::CloseHelp)));
        }
        assert!(map(View::Help, false, KeyCode::Char('x')).is_none());
    }
}
