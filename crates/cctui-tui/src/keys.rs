use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::action::Action;
use crate::app::attention::{AttentionAction, Decision};
use crate::app::state::View;
use crate::config::chord::Chord;
use crate::config::keymap::{ActionId, Context, Keymap};

#[derive(Debug, Clone, Copy)]
pub enum InputEvent {
    Key(KeyEvent),
    ScrollUp,
    ScrollDown,
}

/// A card only claims keys while it is on screen and the composer is closed,
/// so typing in one session can never be eaten by another's request.
pub const fn context_for(view: View, input_active: bool, card_pending: bool) -> Context {
    if input_active {
        return Context::Composer;
    }
    match view {
        View::SessionList => Context::SessionList,
        View::Conversation if card_pending => Context::Permission,
        View::Conversation => Context::Conversation,
        View::Help => Context::Help,
    }
}

/// Pure: terminal input in, at most one [`Action`] out. Every binding resolves
/// through the keymap, so a user override needs no code change here.
pub fn map_input(
    keys: &Keymap,
    view: View,
    input_active: bool,
    card_pending: bool,
    input: InputEvent,
) -> Option<Action> {
    match input {
        InputEvent::Key(key) => {
            let context = context_for(view, input_active, card_pending);
            let chord = Chord::from_event(key);
            keys.lookup(context, chord)
                .and_then(|id| to_action(id, chord))
                .or_else(|| unbound(context, key))
        }
        InputEvent::ScrollUp => match view {
            View::Conversation => Some(Action::Scroll { lines: -3, release_follow: true }),
            View::SessionList => Some(Action::SelectPrev),
            View::Help => None,
        },
        InputEvent::ScrollDown => match view {
            View::Conversation => Some(Action::Scroll { lines: 3, release_follow: false }),
            View::SessionList => Some(Action::SelectNext),
            View::Help => None,
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

        ActionId::PermissionAllow => Action::Attention(AttentionAction::Respond(Decision::Allow)),
        ActionId::PermissionDeny => Action::Attention(AttentionAction::Respond(Decision::Deny)),
        ActionId::PermissionAllowAlways => {
            Action::Attention(AttentionAction::Respond(Decision::AllowAlways))
        }
        ActionId::JumpToPending => Action::Attention(AttentionAction::JumpToPending),

        _ => return None,
    })
}

/// A key no binding claimed: the conversation opens the composer with it, the
/// composer types it, every other view ignores it.
const fn unbound(context: Context, key: KeyEvent) -> Option<Action> {
    match context {
        Context::Conversation | Context::Permission => Some(Action::ActivateInputWith(key)),
        Context::Composer => Some(Action::InputKey(key)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    use super::{Action, AttentionAction, Decision, InputEvent, Keymap, View, map_input};
    use crate::config::keymap::Context;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn map(view: View, input_active: bool, code: KeyCode) -> Option<Action> {
        map_input(&Keymap::default(), view, input_active, false, InputEvent::Key(key(code)))
    }

    fn map_card(code: KeyCode) -> Option<Action> {
        map_input(&Keymap::default(), View::Conversation, false, true, InputEvent::Key(key(code)))
    }

    fn map_event(view: View, input_active: bool, event: KeyEvent) -> Option<Action> {
        map_input(&Keymap::default(), view, input_active, false, InputEvent::Key(event))
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
    fn a_pending_card_claims_only_the_answer_keys() {
        assert!(matches!(
            map_card(KeyCode::Char('y')),
            Some(Action::Attention(AttentionAction::Respond(Decision::Allow)))
        ));
        assert!(matches!(
            map_card(KeyCode::Char('n')),
            Some(Action::Attention(AttentionAction::Respond(Decision::Deny)))
        ));
        assert!(matches!(
            map_card(KeyCode::Char('A')),
            Some(Action::Attention(AttentionAction::Respond(Decision::AllowAlways)))
        ));
        assert!(matches!(
            map_card(KeyCode::Char('j')),
            Some(Action::Scroll { lines: 1, release_follow: true })
        ));
        assert!(matches!(map_card(KeyCode::Esc), Some(Action::LeaveConversation)));
        assert!(matches!(map_card(KeyCode::Char('z')), Some(Action::ActivateInputWith(_))));
    }

    /// The whole point of retiring the modal: a request raised elsewhere must
    /// not turn the composer's next keystroke into an answer.
    #[test]
    fn an_open_composer_types_the_answer_keys_instead_of_answering() {
        for code in [KeyCode::Char('y'), KeyCode::Char('n'), KeyCode::Char('A')] {
            let action = map_input(
                &Keymap::default(),
                View::Conversation,
                true,
                true,
                InputEvent::Key(key(code)),
            );
            assert!(matches!(action, Some(Action::InputKey(_))), "{code:?} must be typed");
        }
    }

    #[test]
    fn ctrl_g_jumps_to_the_next_pending_approval_from_anywhere() {
        for view in [View::SessionList, View::Conversation] {
            assert!(matches!(
                map_event(view, false, ctrl('g')),
                Some(Action::Attention(AttentionAction::JumpToPending))
            ));
        }
    }

    #[test]
    fn the_mouse_wheel_navigates_the_list_and_scrolls_the_conversation() {
        let keys = Keymap::default();
        assert!(matches!(
            map_input(&keys, View::SessionList, false, false, InputEvent::ScrollDown),
            Some(Action::SelectNext)
        ));
        assert!(matches!(
            map_input(&keys, View::Conversation, false, false, InputEvent::ScrollUp),
            Some(Action::Scroll { lines: -3, release_follow: true })
        ));
        assert!(map_input(&keys, View::Help, false, false, InputEvent::ScrollUp).is_none());
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
            map_input(&keys, View::SessionList, false, false, InputEvent::Key(ctrl('n'))),
            Some(Action::SelectNext)
        ));
    }
}
