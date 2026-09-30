use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::PromptFocus;
use crate::app::action::Action;
use crate::app::attention::{AttentionAction, Decision};
use crate::app::conversation::ConversationAction;
use crate::app::drafts::DraftAction;
use crate::app::prompt::PromptAction;
use crate::app::send::SendAction;
use crate::app::state::View;
use crate::config::chord::Chord;
use crate::config::keymap::{ActionId, Context, Keymap};

#[derive(Debug, Clone, Copy)]
pub enum InputEvent {
    Key(KeyEvent),
    ScrollUp,
    ScrollDown,
}

/// A live card outranks the transcript but never the open composer; the recall
/// picker outranks both, being modal over whatever opened it.
///
/// Which card holds the keyboard is [`App::prompt_focus`]'s call, not this
/// function's: a permission request blocks the turn and is answered in one
/// keystroke, so it outranks an ask or plan card, which can be deferred.
pub const fn context_for(view: View, input_active: bool, prompt: Option<PromptFocus>) -> Context {
    if matches!(view, View::HistoryPicker) {
        return Context::History;
    }
    if input_active {
        return Context::Composer;
    }
    if let (View::Conversation, Some(focus)) = (view, prompt) {
        return match focus {
            PromptFocus::Permission => Context::Permission,
            PromptFocus::Ask => Context::Ask,
            PromptFocus::AskText => Context::AskText,
            PromptFocus::Plan => Context::Plan,
            PromptFocus::PlanText => Context::PlanText,
        };
    }
    match view {
        View::SessionList => Context::SessionList,
        View::Conversation => Context::Conversation,
        View::Help => Context::Help,
        View::HistoryPicker => Context::History,
    }
}

/// Pure: terminal input in, at most one [`Action`] out. Every binding resolves
/// through the keymap, so a user override needs no code change here.
pub fn map_input(
    keys: &Keymap,
    view: View,
    input_active: bool,
    prompt: Option<PromptFocus>,
    input: InputEvent,
) -> Option<Action> {
    match input {
        InputEvent::Key(key) => {
            let context = context_for(view, input_active, prompt);
            let chord = Chord::from_event(key);
            keys.lookup(context, chord)
                .and_then(|id| to_action(id, chord))
                .or_else(|| unbound(context, key))
        }
        InputEvent::ScrollUp => match view {
            View::Conversation => Some(Action::Scroll { lines: -3, release_follow: true }),
            View::SessionList => Some(Action::SelectPrev),
            View::Help | View::HistoryPicker => None,
        },
        InputEvent::ScrollDown => match view {
            View::Conversation => Some(Action::Scroll { lines: 3, release_follow: false }),
            View::SessionList => Some(Action::SelectNext),
            View::Help | View::HistoryPicker => None,
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
        ActionId::LineCursor => Action::Conversation(ConversationAction::ToggleLineCursor),
        ActionId::ToggleExpand => Action::Conversation(ConversationAction::ToggleExpand),
        ActionId::ToggleExpandAll => Action::Conversation(ConversationAction::ToggleExpandAll),
        ActionId::Interrupt => Action::InterruptSelected,
        ActionId::RetrySend => Action::Send(SendAction::Retry(chord.event())),
        ActionId::EditSend => Action::Send(SendAction::Edit(chord.event())),
        ActionId::DiscardSend => Action::Send(SendAction::Discard(chord.event())),
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

        ActionId::PermissionAllow => Action::Attention(AttentionAction::Respond(Decision::Allow)),
        ActionId::PermissionDeny => Action::Attention(AttentionAction::Respond(Decision::Deny)),
        ActionId::PermissionAllowAlways => {
            Action::Attention(AttentionAction::Respond(Decision::AllowAlways))
        }
        ActionId::JumpToPending => Action::Attention(AttentionAction::JumpToPending),

        ActionId::FocusPrompt => Action::Prompt(PromptAction::Focus),
        ActionId::PromptDefer => Action::Prompt(PromptAction::Defer),
        ActionId::AskNextOption => Action::Prompt(PromptAction::NextOption),
        ActionId::AskPrevOption => Action::Prompt(PromptAction::PrevOption),
        ActionId::AskPickIndex => Action::Prompt(PromptAction::PickIndex(chord.digit()?)),
        ActionId::AskToggleOption => Action::Prompt(PromptAction::Toggle),
        ActionId::AskNextQuestion => Action::Prompt(PromptAction::NextQuestion),
        ActionId::AskPrevQuestion => Action::Prompt(PromptAction::PrevQuestion),
        ActionId::AskEditOther => Action::Prompt(PromptAction::EditOther),
        ActionId::AskSubmit => Action::Prompt(PromptAction::Submit),
        ActionId::PromptTextCommit => Action::Prompt(PromptAction::TextCommit),
        ActionId::PromptTextCancel => Action::Prompt(PromptAction::TextCancel),
        ActionId::PlanApproveAuto => Action::Prompt(PromptAction::PlanChoose(0)),
        ActionId::PlanApproveManual => Action::Prompt(PromptAction::PlanChoose(1)),
        ActionId::PlanKeepPlanning => Action::Prompt(PromptAction::PlanChoose(2)),
        ActionId::PlanRefine => Action::Prompt(PromptAction::PlanRefine),
        ActionId::PlanScrollDown => Action::Prompt(PromptAction::PlanScroll(1)),
        ActionId::PlanScrollUp => Action::Prompt(PromptAction::PlanScroll(-1)),

        _ => return None,
    })
}

/// A key no binding claimed: the conversation opens the composer with it, the
/// composer types it, the recall picker filters on it, every other view
/// ignores it. A permission card claims only its answer keys, so anything
/// else still reaches the composer.
const fn unbound(context: Context, key: KeyEvent) -> Option<Action> {
    match context {
        Context::Conversation | Context::Permission => Some(Action::ActivateInputWith(key)),
        Context::Composer => Some(Action::InputKey(key)),
        Context::History => Some(Action::Drafts(DraftAction::PickerKey(key))),
        Context::AskText | Context::PlanText => Some(Action::Prompt(PromptAction::TextKey(key))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    use super::{
        Action, AttentionAction, Decision, DraftAction, InputEvent, Keymap, PromptFocus, View,
        map_input,
    };
    use crate::app::prompt::PromptAction;
    use crate::config::keymap::Context;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn map(view: View, input_active: bool, code: KeyCode) -> Option<Action> {
        map_input(&Keymap::default(), view, input_active, None, InputEvent::Key(key(code)))
    }

    fn map_event(view: View, input_active: bool, event: KeyEvent) -> Option<Action> {
        map_input(&Keymap::default(), view, input_active, None, InputEvent::Key(event))
    }

    fn map_card(code: KeyCode) -> Option<Action> {
        map_prompt(PromptFocus::Permission, code)
    }

    fn map_prompt(focus: PromptFocus, code: KeyCode) -> Option<Action> {
        map_input(
            &Keymap::default(),
            View::Conversation,
            false,
            Some(focus),
            InputEvent::Key(key(code)),
        )
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
    fn the_transcript_collapse_keys_are_claimed_only_in_the_conversation() {
        use crate::app::conversation::ConversationAction;
        assert!(matches!(
            map(View::Conversation, false, KeyCode::Char('v')),
            Some(Action::Conversation(ConversationAction::ToggleLineCursor))
        ));
        assert!(matches!(
            map(View::Conversation, false, KeyCode::Char('o')),
            Some(Action::Conversation(ConversationAction::ToggleExpand))
        ));
        assert!(matches!(
            map(View::Conversation, false, KeyCode::Char('z')),
            Some(Action::Conversation(ConversationAction::ToggleExpandAll))
        ));
        assert!(map(View::SessionList, false, KeyCode::Char('v')).is_none());
        assert!(matches!(
            map(View::Conversation, true, KeyCode::Char('v')),
            Some(Action::InputKey(_))
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
    fn the_delivery_affordances_are_bound_in_the_conversation() {
        for (code, expected) in [
            (KeyCode::Char('R'), "retry"),
            (KeyCode::Char('e'), "edit"),
            (KeyCode::Char('x'), "discard"),
        ] {
            let action = map(View::Conversation, false, code);
            let got = match action {
                Some(Action::Send(crate::app::send::SendAction::Retry(_))) => "retry",
                Some(Action::Send(crate::app::send::SendAction::Edit(_))) => "edit",
                Some(Action::Send(crate::app::send::SendAction::Discard(_))) => "discard",
                _ => "none",
            };
            assert_eq!(got, expected, "{code:?}");
        }
        assert!(map(View::SessionList, false, KeyCode::Char('R')).is_none());
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
        assert!(matches!(map_card(KeyCode::Char('w')), Some(Action::ActivateInputWith(_))));
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
                Some(PromptFocus::Permission),
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
            map_input(&keys, View::SessionList, false, None, InputEvent::ScrollDown),
            Some(Action::SelectNext)
        ));
        assert!(matches!(
            map_input(&keys, View::Conversation, false, None, InputEvent::ScrollUp),
            Some(Action::Scroll { lines: -3, release_follow: true })
        ));
        assert!(map_input(&keys, View::Help, false, None, InputEvent::ScrollUp).is_none());
    }

    #[test]
    fn a_live_question_card_takes_the_conversation_keys() {
        assert!(matches!(
            map_prompt(PromptFocus::Ask, KeyCode::Char('2')),
            Some(Action::Prompt(PromptAction::PickIndex(1)))
        ));
        assert!(matches!(
            map_prompt(PromptFocus::Ask, KeyCode::Char(' ')),
            Some(Action::Prompt(PromptAction::Toggle))
        ));
        assert!(matches!(
            map_prompt(PromptFocus::Ask, KeyCode::Tab),
            Some(Action::Prompt(PromptAction::NextQuestion))
        ));
        assert!(matches!(
            map_prompt(PromptFocus::Ask, KeyCode::Enter),
            Some(Action::Prompt(PromptAction::Submit))
        ));
        assert!(matches!(
            map_prompt(PromptFocus::Ask, KeyCode::Esc),
            Some(Action::Prompt(PromptAction::Defer))
        ));
        assert!(map_prompt(PromptFocus::Ask, KeyCode::Char('z')).is_none());
    }

    #[test]
    fn the_free_text_field_types_everything_the_card_does_not_claim() {
        assert!(matches!(
            map_prompt(PromptFocus::AskText, KeyCode::Char('o')),
            Some(Action::Prompt(PromptAction::TextKey(_)))
        ));
        assert!(matches!(
            map_prompt(PromptFocus::AskText, KeyCode::Enter),
            Some(Action::Prompt(PromptAction::TextCommit))
        ));
        assert!(matches!(
            map_prompt(PromptFocus::PlanText, KeyCode::Esc),
            Some(Action::Prompt(PromptAction::TextCancel))
        ));
    }

    #[test]
    fn the_plan_card_answers_by_digit_or_refines() {
        assert!(matches!(
            map_prompt(PromptFocus::Plan, KeyCode::Char('1')),
            Some(Action::Prompt(PromptAction::PlanChoose(0)))
        ));
        assert!(matches!(
            map_prompt(PromptFocus::Plan, KeyCode::Char('2')),
            Some(Action::Prompt(PromptAction::PlanChoose(1)))
        ));
        assert!(matches!(
            map_prompt(PromptFocus::Plan, KeyCode::Char('3')),
            Some(Action::Prompt(PromptAction::PlanChoose(2)))
        ));
        assert!(matches!(
            map_prompt(PromptFocus::Plan, KeyCode::Char('r')),
            Some(Action::Prompt(PromptAction::PlanRefine))
        ));
        assert!(matches!(
            map_prompt(PromptFocus::Plan, KeyCode::Char('j')),
            Some(Action::Prompt(PromptAction::PlanScroll(1)))
        ));
    }

    #[test]
    fn an_open_composer_outranks_a_live_card() {
        assert!(matches!(
            map_input(
                &Keymap::default(),
                View::Conversation,
                true,
                Some(PromptFocus::Ask),
                InputEvent::Key(key(KeyCode::Char('2'))),
            ),
            Some(Action::InputKey(_))
        ));
    }

    #[test]
    fn tab_takes_a_deferred_card_back() {
        assert!(matches!(
            map(View::Conversation, false, KeyCode::Tab),
            Some(Action::Prompt(PromptAction::Focus))
        ));
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
            map_input(&keys, View::SessionList, false, None, InputEvent::Key(ctrl('n'))),
            Some(Action::SelectNext)
        ));
    }
}
