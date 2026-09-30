//! The composer's `#session` completion, on `cctui-clientcore`'s filter.

use cctui_clientcore::mention::{
    MentionSession, MentionTrigger, apply_mention, filter_mentions, find_trigger,
    mentionable_sessions, move_selection,
};

use super::action::Effect;
use super::state::App;

/// How many rows the popup shows at once.
pub const VISIBLE_ROWS: usize = 6;

/// An open completion: the `#query` it answers and the sessions that match.
#[derive(Debug)]
pub struct MentionPopup {
    pub trigger: MentionTrigger,
    pub matches: Vec<MentionSession>,
    pub selected: usize,
}

impl MentionPopup {
    pub fn current(&self) -> Option<&MentionSession> {
        self.matches.get(self.selected)
    }

    /// The label a row shows: the session's name, else its id, with the target
    /// as a dim lead.
    #[must_use]
    pub fn row_label(session: &MentionSession) -> String {
        let name = session.name.as_deref().map(str::trim).filter(|n| !n.is_empty());
        name.map_or_else(|| session.id.clone(), str::to_owned)
    }

    #[must_use]
    pub fn row_detail(session: &MentionSession) -> String {
        let mut parts = Vec::new();
        if let Some(machine) = session.machine_name.as_deref().filter(|m| !m.is_empty()) {
            parts.push(machine.to_owned());
        }
        if let Some(dir) = session.working_dir.as_deref().filter(|d| !d.is_empty()) {
            parts.push(dir.rsplit('/').next().unwrap_or(dir).to_owned());
        }
        parts.join(" · ")
    }
}

#[derive(Debug, Default)]
pub struct MentionState {
    pub popup: Option<MentionPopup>,
}

/// Every session an agent could be pointed at, in the shape the shared filter
/// reads.
fn candidates(app: &App) -> Vec<MentionSession> {
    app.sessions
        .iter()
        .map(|s| MentionSession {
            id: s.id.clone(),
            status: status_str(s.status).to_owned(),
            name: s.name.clone(),
            working_dir: Some(s.working_dir.clone()),
            machine_name: s.machine_name.clone(),
        })
        .collect()
}

const fn status_str(status: cctui_proto::models::SessionStatus) -> &'static str {
    use cctui_proto::models::SessionStatus as S;
    match status {
        S::New => "new",
        S::Active => "active",
        S::Inactive => "inactive",
        S::Archived => "archived",
        S::Draft => "draft",
    }
}

/// Recompute the popup after a composer edit: it opens, narrows and closes
/// with the `#query` under the caret.
pub fn refresh(app: &mut App) {
    let text = app.message_input.lines().join("\n");
    let caret = super::drafts::caret_offset(app);
    let Some(trigger) = find_trigger(&text, caret) else {
        app.mentions.popup = None;
        return;
    };
    let exclude = app.drafts.composer_session.clone();
    let pool = mentionable_sessions(&candidates(app), exclude.as_deref());
    let matches = filter_mentions(&pool, &trigger.query);
    if matches.is_empty() {
        app.mentions.popup = None;
        return;
    }
    let selected =
        app.mentions.popup.as_ref().map_or(0, |popup| popup.selected.min(matches.len() - 1));
    app.mentions.popup = Some(MentionPopup { trigger, matches, selected });
}

/// True when the popup took the key.
pub const fn walk(app: &mut App, delta: i32) -> bool {
    let Some(popup) = app.mentions.popup.as_mut() else { return false };
    popup.selected = move_selection(popup.selected, delta, popup.matches.len());
    true
}

/// `None` when no popup is open and the key means what it usually means.
pub fn accept(app: &mut App) -> Option<Vec<Effect>> {
    let popup = app.mentions.popup.take()?;
    let session = popup.current().or_else(|| popup.matches.first())?;
    let text = app.message_input.lines().join("\n");
    let caret = super::drafts::caret_offset(app);
    let insertion =
        apply_mention(&text, caret, &popup.trigger, &session.id, session.name.as_deref());
    app.set_input_text_at(&insertion.text, insertion.caret);
    app.input_active = true;
    Some(super::drafts::on_input(app))
}

/// True when the popup was open and the key only closed it.
pub fn close(app: &mut App) -> bool {
    app.mentions.popup.take().is_some()
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::app::state::App;
    use crate::app::{Action, reduce};
    use crate::testsupport::session;

    /// The mention token carries the session's own name, so one fixture has
    /// one and another does not.
    fn named(id: &str, project: &str, name: &str) -> cctui_proto::api::SessionListItem {
        let mut item = session(id, project, "active", "working");
        item.name = Some(name.to_owned());
        item
    }

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![
            session("s-a", "alpha", "active", "working"),
            named("s-b", "beta", "beta"),
            session("s-c", "gamma", "archived", "done"),
        ];
        app.update_aggregates();
        reduce(&mut app, Action::OpenSelectedConversation);
        app.input_active = true;
        app
    }

    fn tab() -> KeyEvent {
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            reduce(app, Action::InputKey(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)));
        }
    }

    fn composer(app: &App) -> String {
        app.message_input.lines().join("\n")
    }

    fn matches(app: &App) -> Vec<String> {
        app.mentions
            .popup
            .as_ref()
            .map(|p| p.matches.iter().map(|m| m.id.clone()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn hash_opens_the_popup_over_every_mentionable_session() {
        let mut app = app();
        type_text(&mut app, "ping #");
        assert_eq!(matches(&app), vec!["s-b".to_owned()], "not this session, not archived ones");
    }

    #[test]
    fn typing_narrows_the_popup_and_a_miss_closes_it() {
        let mut app = app();
        type_text(&mut app, "#bet");
        assert_eq!(matches(&app), vec!["s-b".to_owned()]);
        type_text(&mut app, "zzz");
        assert!(app.mentions.popup.is_none(), "no match closes the popup");
    }

    #[test]
    fn a_hash_that_is_not_a_trigger_opens_nothing() {
        let mut app = app();
        type_text(&mut app, "C# is a language");
        assert!(app.mentions.popup.is_none());
        type_text(&mut app, " # then a space ");
        assert!(app.mentions.popup.is_none());
    }

    #[test]
    fn enter_accepts_the_highlighted_session_instead_of_sending() {
        let mut app = app();
        type_text(&mut app, "look at #bet");
        let effects = reduce(&mut app, Action::SubmitInput);
        assert_eq!(composer(&app), "look at #s-b (beta) ", "the web UI's token, plus a space");
        assert!(app.mentions.popup.is_none());
        assert!(app.input_active, "the composer stays open for the rest of the message");
        assert!(
            !effects.iter().any(|e| matches!(e, crate::app::action::Effect::SendMessage { .. })),
            "the message is not sent by accepting a mention"
        );
    }

    #[test]
    fn the_arrows_walk_the_popup_rather_than_the_prompt_history() {
        let mut app = app();
        app.sessions.push(named("s-d", "beta-two", "beta-two"));
        type_text(&mut app, "#bet");
        assert_eq!(matches(&app).len(), 2);

        reduce(&mut app, Action::Drafts(crate::app::drafts::DraftAction::HistoryNext));
        assert_eq!(app.mentions.popup.as_ref().expect("popup").selected, 1);
        reduce(&mut app, Action::Drafts(crate::app::drafts::DraftAction::HistoryNext));
        assert_eq!(app.mentions.popup.as_ref().expect("popup").selected, 0, "the walk wraps");
        reduce(&mut app, Action::Drafts(crate::app::drafts::DraftAction::HistoryPrev));
        assert_eq!(app.mentions.popup.as_ref().expect("popup").selected, 1);
        assert_eq!(composer(&app), "#bet", "the composer text is untouched");
    }

    #[test]
    fn escape_closes_the_popup_and_keeps_the_composer() {
        let mut app = app();
        type_text(&mut app, "#bet");
        reduce(&mut app, Action::CancelInput);
        assert!(app.mentions.popup.is_none());
        assert!(app.input_active, "the first escape only dismisses the popup");
        assert_eq!(composer(&app), "#bet");

        reduce(&mut app, Action::CancelInput);
        assert!(!app.input_active, "the next one closes the composer");
    }

    #[test]
    fn tab_accepts_the_mention_and_types_a_tab_when_there_is_none() {
        let mut app = app();
        type_text(&mut app, "#bet");
        reduce(&mut app, Action::AcceptMention(tab()));
        assert_eq!(composer(&app), "#s-b (beta) ");

        reduce(&mut app, Action::AcceptMention(tab()));
        assert!(composer(&app).starts_with("#s-b (beta) "), "no popup: the key types instead");
    }

    #[test]
    fn an_accepted_mention_leaves_the_caret_after_the_token() {
        let mut typed = app();
        type_text(&mut typed, "#bet and then some");
        assert!(typed.mentions.popup.is_none(), "the space ended the trigger");

        let mut app = app();
        type_text(&mut app, "#bet");
        reduce(&mut app, Action::AcceptMention(tab()));
        type_text(&mut app, "please");
        assert_eq!(composer(&app), "#s-b (beta) please");
    }
}
