use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::action::Action;
use crate::app::drafts::DraftAction;
use crate::app::state::View;
use crate::config::chord::Chord;
use crate::config::keymap::{ActionId, Context, Keymap};

#[derive(Debug, Clone, Copy)]
pub enum InputEvent {
    Key(KeyEvent),
    ScrollUp,
    ScrollDown,
}

/// The recall picker is modal: it keeps its own keys even though the composer
/// it was opened from is still active underneath.
pub const fn context_for(view: View, input_active: bool) -> Context {
    match view {
        View::HistoryPicker => Context::History,
        _ if input_active => Context::Composer,
        View::SessionList => Context::SessionList,
        View::Conversation => Context::Conversation,
        View::Help => Context::Help,
        View::PermissionDialog => Context::Permission,
    }
}

/// Pure: terminal input in, at most one [`Action`] out. Every binding resolves
/// through the keymap, so a user override needs no code change here.
pub fn map_input(
    keys: &Keymap,
    view: View,
    input_active: bool,
    input: InputEvent,
) -> Option<Action> {
    match input {
        InputEvent::Key(key) => {
            let context = context_for(view, input_active);
            let chord = Chord::from_event(key);
            keys.lookup(context, chord)
                .and_then(|id| to_action(id, chord))
                .or_else(|| unbound(context, key))
        }
        InputEvent::ScrollUp => match view {
            View::Conversation => Some(Action::Scroll { lines: -3, release_follow: true }),
            View::SessionList => Some(Action::SelectPrev),
            View::Help | View::PermissionDialog | View::HistoryPicker => None,
        },
        InputEvent::ScrollDown => match view {
            View::Conversation => Some(Action::Scroll { lines: 3, release_follow: false }),
            View::SessionList => Some(Action::SelectNext),
            View::Help | View::PermissionDialog | View::HistoryPicker => None,
        },
    }
}

/// Whether an action does anything yet; the cheat sheet lists only these.
pub fn is_wired(id: ActionId) -> bool {
    to_action(id, Chord::new(KeyCode::Char('1'), KeyModifiers::NONE)).is_some()
}

/// Actions a later wave still owns return `None`: the key then behaves as if it
/// were unbound rather than being silently swallowed.
fn to_action(id: ActionId, chord: Chord) -> Option<Action> {
    Some(match id {
        ActionId::Help => Action::OpenHelp,
        ActionId::Quit => Action::Quit,
        ActionId::CloseHelp => Action::CloseHelp,

        ActionId::SelectNext => Action::SelectNext,
        ActionId::SelectPrev => Action::SelectPrev,
        ActionId::SelectFirst => Action::SelectFirst,
        ActionId::SelectLast => Action::SelectLast,
        ActionId::SelectIndex => Action::SelectIndex(chord.digit()?),
        ActionId::OpenConversation => Action::OpenSelectedConversation,

        ActionId::LeaveConversation => Action::LeaveConversation,
        ActionId::ScrollDown => Action::Scroll { lines: 1, release_follow: true },
        ActionId::ScrollUp => Action::Scroll { lines: -1, release_follow: true },
        ActionId::PageDown => Action::Scroll { lines: 15, release_follow: false },
        ActionId::PageUp => Action::Scroll { lines: -15, release_follow: true },
        ActionId::ScrollToTop => Action::ScrollToTop,
        ActionId::ScrollToBottom => Action::ScrollToBottom,
        ActionId::ToggleTimestamps => Action::ToggleTimestamps,
        ActionId::Interrupt => Action::InterruptSelected,
        ActionId::ToggleAutoApprove => Action::ToggleAutoApproveSelected,

        ActionId::CancelInput => Action::CancelInput,
        ActionId::SubmitInput => Action::SubmitInput,
        ActionId::InputNewline => Action::InputNewline,

        ActionId::HistoryPrev => Action::Drafts(DraftAction::HistoryPrev),
        ActionId::HistoryNext => Action::Drafts(DraftAction::HistoryNext),
        ActionId::HistoryOpen => Action::Drafts(DraftAction::OpenPicker),
        ActionId::HistoryClose => Action::Drafts(DraftAction::ClosePicker),
        ActionId::HistorySelectNext => Action::Drafts(DraftAction::PickerSelectNext),
        ActionId::HistorySelectPrev => Action::Drafts(DraftAction::PickerSelectPrev),
        ActionId::HistoryRecall => Action::Drafts(DraftAction::PickerRecall),

        ActionId::PermissionAllow => Action::ResolvePermission { allow: true },
        ActionId::PermissionDeny => Action::ResolvePermission { allow: false },

        _ => return None,
    })
}

/// A key no binding claimed: the conversation opens the composer with it, the
/// composer types it, the recall picker filters on it, every other view
/// ignores it.
const fn unbound(context: Context, key: KeyEvent) -> Option<Action> {
    match context {
        Context::Conversation => Some(Action::ActivateInputWith(key)),
        Context::Composer => Some(Action::InputKey(key)),
        Context::History => Some(Action::Drafts(DraftAction::PickerKey(key))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    use super::{Action, DraftAction, InputEvent, Keymap, View, map_input};
    use crate::config::keymap::Context;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn map(view: View, input_active: bool, code: KeyCode) -> Option<Action> {
        map_input(&Keymap::default(), view, input_active, InputEvent::Key(key(code)))
    }

    fn map_event(view: View, input_active: bool, event: KeyEvent) -> Option<Action> {
        map_input(&Keymap::default(), view, input_active, InputEvent::Key(event))
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

    /// A global bound to an action no wave implements yet must not eat the key.
    #[test]
    fn a_reserved_global_still_reaches_the_composer() {
        for code in [KeyCode::Char('i'), KeyCode::Char('/'), KeyCode::Char('n')] {
            assert!(
                matches!(map(View::Conversation, false, code), Some(Action::ActivateInputWith(_))),
                "{code:?} should fall through"
            );
        }
        assert!(map(View::SessionList, false, KeyCode::Char('i')).is_none());
    }

    #[test]
    fn ctrl_bindings_win_over_navigation() {
        assert!(matches!(
            map_event(View::Conversation, false, ctrl('c')),
            Some(Action::InterruptSelected)
        ));
        assert!(matches!(
            map_event(View::Conversation, false, ctrl('a')),
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
        assert!(matches!(
            map(View::Conversation, true, KeyCode::Char('j')),
            Some(Action::InputKey(_))
        ));
        assert!(matches!(map(View::Conversation, true, KeyCode::Esc), Some(Action::CancelInput)));
        assert!(matches!(map(View::Conversation, true, KeyCode::Enter), Some(Action::SubmitInput)));
    }

    #[test]
    fn shift_enter_inserts_a_newline_instead_of_sending() {
        let event = KeyEvent {
            kind: KeyEventKind::Press,
            ..KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)
        };
        assert!(matches!(map_event(View::Conversation, true, event), Some(Action::InputNewline)));
    }

    #[test]
    fn the_composer_recalls_prompts_with_the_arrows_and_ctrl_r() {
        assert!(matches!(
            map(View::Conversation, true, KeyCode::Up),
            Some(Action::Drafts(DraftAction::HistoryPrev))
        ));
        assert!(matches!(
            map(View::Conversation, true, KeyCode::Down),
            Some(Action::Drafts(DraftAction::HistoryNext))
        ));
        assert!(matches!(
            map_event(View::Conversation, true, ctrl('r')),
            Some(Action::Drafts(DraftAction::OpenPicker))
        ));
        assert!(matches!(
            map_event(View::Conversation, false, ctrl('r')),
            Some(Action::Drafts(DraftAction::OpenPicker))
        ));
    }

    /// `shift+enter` never arrives in most terminals, so the alternatives are
    /// the ones that matter.
    #[test]
    fn a_newline_has_bindings_a_terminal_actually_reports() {
        let ctrl_j = map_event(View::Conversation, true, ctrl('j'));
        assert!(matches!(ctrl_j, Some(Action::InputNewline)));
        let alt = KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT);
        assert!(matches!(map_event(View::Conversation, true, alt), Some(Action::InputNewline)));
    }

    #[test]
    fn the_recall_picker_keeps_its_keys_over_an_active_composer() {
        for active in [true, false] {
            assert!(matches!(
                map(View::HistoryPicker, active, KeyCode::Esc),
                Some(Action::Drafts(DraftAction::ClosePicker))
            ));
            assert!(matches!(
                map(View::HistoryPicker, active, KeyCode::Enter),
                Some(Action::Drafts(DraftAction::PickerRecall))
            ));
            assert!(matches!(
                map(View::HistoryPicker, active, KeyCode::Down),
                Some(Action::Drafts(DraftAction::PickerSelectNext))
            ));
            assert!(matches!(
                map(View::HistoryPicker, active, KeyCode::Char('x')),
                Some(Action::Drafts(DraftAction::PickerKey(_)))
            ));
            assert!(matches!(
                map(View::HistoryPicker, active, KeyCode::Backspace),
                Some(Action::Drafts(DraftAction::PickerKey(_)))
            ));
        }
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
        assert!(map(View::PermissionDialog, false, KeyCode::Char('q')).is_none());
    }

    #[test]
    fn the_mouse_wheel_navigates_the_list_and_scrolls_the_conversation() {
        let keys = Keymap::default();
        assert!(matches!(
            map_input(&keys, View::SessionList, false, InputEvent::ScrollDown),
            Some(Action::SelectNext)
        ));
        assert!(matches!(
            map_input(&keys, View::Conversation, false, InputEvent::ScrollUp),
            Some(Action::Scroll { lines: -3, release_follow: true })
        ));
        assert!(map_input(&keys, View::Help, false, InputEvent::ScrollUp).is_none());
    }

    #[test]
    fn help_dismisses_on_escape_question_mark_or_q() {
        for code in [KeyCode::Esc, KeyCode::Char('?'), KeyCode::Char('q')] {
            assert!(matches!(map(View::Help, false, code), Some(Action::CloseHelp)));
        }
        assert!(map(View::Help, false, KeyCode::Char('x')).is_none());
    }

    #[test]
    fn a_user_override_changes_the_binding_without_touching_this_module() {
        let mut keys = Keymap::default();
        keys.set(Context::SessionList, "ctrl+n", "select-next").expect("valid");
        assert!(matches!(
            map_input(&keys, View::SessionList, false, InputEvent::Key(ctrl('n'))),
            Some(Action::SelectNext)
        ));
    }
}
