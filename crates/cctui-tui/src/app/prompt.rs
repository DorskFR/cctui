//! Live `AskUserQuestion` and plan-approval prompts, answered in-session.

use std::collections::BTreeSet;

use crossterm::event::{KeyCode, KeyEvent};
use serde_json::Value;

use super::action::Effect;
use super::state::App;

/// The plan continuations, in the order the harness's own form lists them. The
/// index is what rides along as the structured pick.
pub const PLAN_OPTIONS: [&str; 3] =
    ["Yes, and auto-accept edits", "Yes, and manually approve edits", "No, keep planning"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskOption {
    pub label: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskQuestion {
    pub header: Option<String>,
    pub question: String,
    pub multi_select: bool,
    pub options: Vec<AskOption>,
}

/// A live question set plus the answer being assembled for it.
#[derive(Debug, Clone)]
pub struct AskCard {
    pub questions: Vec<AskQuestion>,
    pub preamble: Option<String>,
    pub current: usize,
    pub cursor: Vec<usize>,
    pub chosen: Vec<BTreeSet<usize>>,
    pub other: Vec<String>,
    pub editing_other: bool,
    /// Esc keeps the card on screen but hands the keys back to the transcript.
    pub deferred: bool,
}

#[derive(Debug, Clone)]
pub struct PlanCard {
    pub plan: String,
    pub preamble: Option<String>,
    pub scroll: usize,
    pub refining: bool,
    pub refine: String,
    pub deferred: bool,
}

/// Which card owns the keyboard, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptFocus {
    Ask,
    AskText,
    Plan,
    PlanText,
}

impl AskCard {
    pub fn new(question: String, questions: Option<&Value>, preamble: Option<String>) -> Self {
        let questions =
            parse_questions(questions).unwrap_or_else(|| vec![free_text_question(question)]);
        let n = questions.len();
        Self {
            questions,
            preamble,
            current: 0,
            cursor: vec![0; n],
            chosen: vec![BTreeSet::new(); n],
            other: vec![String::new(); n],
            editing_other: false,
            deferred: false,
        }
    }

    pub fn answered_all(&self) -> bool {
        (0..self.questions.len())
            .all(|i| !self.chosen[i].is_empty() || !self.other[i].trim().is_empty())
    }

    /// The transcript echo, formatted exactly as the web card formats it.
    pub fn answer_text(&self) -> String {
        self.questions
            .iter()
            .enumerate()
            .map(|(qi, q)| {
                let mut picks: Vec<&str> = self.chosen[qi]
                    .iter()
                    .filter_map(|oi| q.options.get(*oi).map(|o| o.label.as_str()))
                    .collect();
                let other = self.other[qi].trim();
                if !other.is_empty() {
                    picks.push(other);
                }
                let head = q.header.as_ref().map_or_else(String::new, |h| format!("**{h}** — "));
                format!("{head}{}\n→ {}", q.question, picks.join(", "))
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Structured picks, or `None` when any answer used the free-text field —
    /// that is the signal for the dismiss-then-reply fallback.
    pub fn picks(&self) -> Option<Vec<Vec<usize>>> {
        if self.other.iter().any(|t| !t.trim().is_empty()) {
            return None;
        }
        Some(self.chosen.iter().map(|set| set.iter().copied().collect()).collect())
    }

    fn pick(&mut self, oi: usize) {
        let qi = self.current;
        let Some(q) = self.questions.get(qi) else { return };
        if oi >= q.options.len() {
            return;
        }
        if q.multi_select {
            if !self.chosen[qi].remove(&oi) {
                self.chosen[qi].insert(oi);
            }
        } else {
            self.chosen[qi].clear();
            self.chosen[qi].insert(oi);
        }
        self.cursor[qi] = oi;
    }

    fn move_cursor(&mut self, delta: i32) {
        let qi = self.current;
        let len = self.questions.get(qi).map_or(0, |q| q.options.len());
        if len == 0 {
            return;
        }
        let next = if delta < 0 {
            self.cursor[qi].checked_sub(1).unwrap_or(len - 1)
        } else {
            (self.cursor[qi] + 1) % len
        };
        self.cursor[qi] = next;
    }
}

impl PlanCard {
    pub const fn new(plan: String, preamble: Option<String>) -> Self {
        Self { plan, preamble, scroll: 0, refining: false, refine: String::new(), deferred: false }
    }
}

const fn free_text_question(question: String) -> AskQuestion {
    AskQuestion { header: None, question, multi_select: false, options: Vec::new() }
}

/// The hook payload is the harness's own JSON, so every field is optional and a
/// shape that carries no usable options falls back to the free-text path.
fn parse_questions(value: Option<&Value>) -> Option<Vec<AskQuestion>> {
    let items = value?.as_array()?;
    let questions: Vec<AskQuestion> = items
        .iter()
        .filter_map(|item| {
            let question = item.get("question")?.as_str()?.to_owned();
            let options = item
                .get("options")
                .and_then(Value::as_array)
                .map(|opts| {
                    opts.iter()
                        .filter_map(|o| {
                            Some(AskOption {
                                label: o.get("label")?.as_str()?.to_owned(),
                                description: o
                                    .get("description")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(AskQuestion {
                header: item.get("header").and_then(Value::as_str).map(str::to_owned),
                question,
                multi_select: item.get("multiSelect").and_then(Value::as_bool).unwrap_or(false),
                options,
            })
        })
        .collect();
    if questions.is_empty() { None } else { Some(questions) }
}

/// A read-only one-liner for a historical `AskUserQuestion` / `ExitPlanMode`
/// tool call: the transcript renders a tool detail as a single row, so this
/// must not contain newlines.
pub fn historical_tool_text(tool: &str, input: &Value) -> Option<String> {
    match tool {
        "AskUserQuestion" => {
            let questions = parse_questions(input.get("questions"))?;
            Some(
                questions
                    .iter()
                    .map(|q| {
                        let labels = q
                            .options
                            .iter()
                            .map(|o| o.label.as_str())
                            .collect::<Vec<_>>()
                            .join(" | ");
                        if labels.is_empty() {
                            q.question.clone()
                        } else {
                            format!("{} [{labels}]", q.question)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" · "),
            )
        }
        "ExitPlanMode" => {
            let plan = input.get("plan").and_then(Value::as_str)?;
            let first = plan.lines().find(|l| !l.trim().is_empty())?.trim();
            let rest = plan.lines().filter(|l| !l.trim().is_empty()).count().saturating_sub(1);
            Some(if rest == 0 { first.to_owned() } else { format!("{first} (+{rest} lines)") })
        }
        _ => None,
    }
}

pub enum PromptAction {
    AskRequested {
        session_id: String,
        question: String,
        questions: Option<Value>,
        preamble: Option<String>,
    },
    AskResolved {
        session_id: String,
    },
    PlanRequested {
        session_id: String,
        plan: String,
        preamble: Option<String>,
    },
    PlanResolved {
        session_id: String,
    },

    /// Take the keyboard back after an Esc deferred the card.
    Focus,
    Defer,
    NextOption,
    PrevOption,
    PickIndex(usize),
    Toggle,
    NextQuestion,
    PrevQuestion,
    EditOther,
    Submit,
    TextKey(KeyEvent),
    TextCommit,
    TextCancel,
    PlanScroll(i32),
    PlanChoose(usize),
    PlanRefine,
}

#[allow(clippy::too_many_lines)]
pub fn reduce_prompt(app: &mut App, action: PromptAction) -> Vec<Effect> {
    match action {
        PromptAction::AskRequested { session_id, question, questions, preamble } => {
            app.asks.insert(session_id, AskCard::new(question, questions.as_ref(), preamble));
            Vec::new()
        }
        PromptAction::AskResolved { session_id } => {
            app.asks.remove(&session_id);
            Vec::new()
        }
        PromptAction::PlanRequested { session_id, plan, preamble } => {
            app.plans.insert(session_id, PlanCard::new(plan, preamble));
            Vec::new()
        }
        PromptAction::PlanResolved { session_id } => {
            app.plans.remove(&session_id);
            Vec::new()
        }

        PromptAction::Focus => {
            let Some(id) = app.selected_session_id() else { return Vec::new() };
            if let Some(card) = app.asks.get_mut(&id) {
                card.deferred = false;
            }
            if let Some(card) = app.plans.get_mut(&id) {
                card.deferred = false;
            }
            Vec::new()
        }
        PromptAction::Defer => {
            let Some(id) = app.selected_session_id() else { return Vec::new() };
            if let Some(card) = app.asks.get_mut(&id) {
                card.deferred = true;
                card.editing_other = false;
            }
            if let Some(card) = app.plans.get_mut(&id) {
                card.deferred = true;
                card.refining = false;
            }
            Vec::new()
        }

        PromptAction::NextOption => with_ask(app, |card| card.move_cursor(1)),
        PromptAction::PrevOption => with_ask(app, |card| card.move_cursor(-1)),
        PromptAction::PickIndex(index) => with_ask(app, |card| card.pick(index)),
        PromptAction::Toggle => with_ask(app, |card| {
            let oi = card.cursor[card.current];
            card.pick(oi);
        }),
        PromptAction::NextQuestion => with_ask(app, |card| {
            if card.current + 1 < card.questions.len() {
                card.current += 1;
            }
        }),
        PromptAction::PrevQuestion => with_ask(app, |card| {
            card.current = card.current.saturating_sub(1);
        }),
        PromptAction::EditOther => with_ask(app, |card| card.editing_other = true),
        PromptAction::TextCancel => {
            let Some(id) = app.selected_session_id() else { return Vec::new() };
            if let Some(card) = app.asks.get_mut(&id) {
                card.editing_other = false;
                card.other[card.current].clear();
            }
            if let Some(card) = app.plans.get_mut(&id) {
                card.refining = false;
                card.refine.clear();
            }
            Vec::new()
        }
        PromptAction::TextKey(key) => {
            let Some(id) = app.selected_session_id() else { return Vec::new() };
            if let Some(card) = app.asks.get_mut(&id).filter(|c| c.editing_other) {
                let qi = card.current;
                edit(&mut card.other[qi], key);
            }
            if let Some(card) = app.plans.get_mut(&id).filter(|c| c.refining) {
                edit(&mut card.refine, key);
            }
            Vec::new()
        }
        // Enter closes the free-text field; the answer still goes out through
        // the one submit path.
        PromptAction::TextCommit => {
            let Some(id) = app.selected_session_id() else { return Vec::new() };
            if let Some(card) = app.asks.get_mut(&id) {
                card.editing_other = false;
                return Vec::new();
            }
            let Some(card) = app.plans.get(&id).filter(|c| !c.refine.trim().is_empty()) else {
                return Vec::new();
            };
            let content = card.refine.trim().to_owned();
            app.plans.remove(&id);
            super::send::submit(app, id, content, None)
        }
        PromptAction::Submit => {
            let Some(id) = app.selected_session_id() else { return Vec::new() };
            let Some(card) = app.asks.get(&id).filter(|c| c.answered_all()) else {
                return Vec::new();
            };
            let content = card.answer_text();
            let ask_picks = card.picks();
            app.asks.remove(&id);
            super::send::submit(app, id, content, ask_picks)
        }

        PromptAction::PlanScroll(lines) => {
            let Some(id) = app.selected_session_id() else { return Vec::new() };
            let Some(card) = app.plans.get_mut(&id) else { return Vec::new() };
            card.scroll = if lines < 0 {
                card.scroll.saturating_sub(lines.unsigned_abs() as usize)
            } else {
                card.scroll.saturating_add(lines as usize)
            };
            Vec::new()
        }
        PromptAction::PlanRefine => {
            let Some(id) = app.selected_session_id() else { return Vec::new() };
            if let Some(card) = app.plans.get_mut(&id) {
                card.refining = true;
            }
            Vec::new()
        }
        PromptAction::PlanChoose(index) => {
            let Some(id) = app.selected_session_id() else { return Vec::new() };
            let Some(label) = PLAN_OPTIONS.get(index) else { return Vec::new() };
            if app.plans.remove(&id).is_none() {
                return Vec::new();
            }
            super::send::submit(app, id, (*label).to_owned(), Some(vec![vec![index]]))
        }
    }
}

fn with_ask(app: &mut App, f: impl FnOnce(&mut AskCard)) -> Vec<Effect> {
    if let Some(id) = app.selected_session_id()
        && let Some(card) = app.asks.get_mut(&id)
    {
        f(card);
    }
    Vec::new()
}

fn edit(buffer: &mut String, key: KeyEvent) {
    match key.code {
        KeyCode::Char(c) => buffer.push(c),
        KeyCode::Backspace => {
            buffer.pop();
        }
        _ => {}
    }
}

impl App {
    /// The card holding the keyboard for the selected session, if any.
    pub fn prompt_focus(&self) -> Option<PromptFocus> {
        let id = self.selected_session_id()?;
        if let Some(card) = self.asks.get(&id).filter(|c| !c.deferred) {
            return Some(if card.editing_other { PromptFocus::AskText } else { PromptFocus::Ask });
        }
        let card = self.plans.get(&id).filter(|c| !c.deferred)?;
        Some(if card.refining { PromptFocus::PlanText } else { PromptFocus::Plan })
    }

    /// Header/list badge for a session with a prompt still waiting.
    pub fn prompt_marker(&self, session_id: &str) -> Option<&'static str> {
        if self.asks.contains_key(session_id) {
            Some("?")
        } else if self.plans.contains_key(session_id) {
            Some("P")
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{AskCard, PLAN_OPTIONS, PromptAction, PromptFocus, historical_tool_text};
    use crate::app::action::{Action, Effect};
    use crate::app::reduce;
    use crate::app::state::App;
    use crate::testsupport::session;

    fn questions() -> serde_json::Value {
        json!([
            {
                "header": "Database",
                "question": "Which database?",
                "options": [
                    {"label": "Postgres", "description": "the default"},
                    {"label": "SQLite"}
                ]
            },
            {
                "question": "Which features?",
                "multiSelect": true,
                "options": [{"label": "auth"}, {"label": "billing"}, {"label": "search"}]
            }
        ])
    }

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![session("s-a", "alpha", "active", "working")];
        app.update_aggregates();
        app
    }

    fn ask(app: &mut App) {
        reduce(
            app,
            Action::Prompt(PromptAction::AskRequested {
                session_id: "s-a".to_owned(),
                question: "Which database?".to_owned(),
                questions: Some(questions()),
                preamble: Some("I need a couple of decisions.".to_owned()),
            }),
        );
    }

    fn plan(app: &mut App) {
        reduce(
            app,
            Action::Prompt(PromptAction::PlanRequested {
                session_id: "s-a".to_owned(),
                plan: "# Plan\n\n- do the thing".to_owned(),
                preamble: None,
            }),
        );
    }

    fn dispatch(app: &mut App, action: PromptAction) -> Vec<Effect> {
        reduce(app, Action::Prompt(action))
    }

    /// Session, content and picks of the dispatch an answer produced; a send
    /// also clears the composer's draft and records the prompt.
    fn sent(effects: &[Effect]) -> (&str, &str, Option<&[Vec<usize>]>) {
        effects
            .iter()
            .find_map(|e| match e {
                Effect::SendMessage { session_id, content, ask_picks, .. } => {
                    Some((session_id.as_str(), content.as_str(), ask_picks.as_deref()))
                }
                _ => None,
            })
            .expect("expected one tracked send")
    }

    #[test]
    fn a_structured_ask_becomes_picks_and_the_web_cards_text() {
        let mut app = app();
        ask(&mut app);
        dispatch(&mut app, PromptAction::PickIndex(0));
        dispatch(&mut app, PromptAction::NextQuestion);
        dispatch(&mut app, PromptAction::PickIndex(2));
        dispatch(&mut app, PromptAction::PickIndex(0));

        let card = app.asks.get("s-a").expect("a live ask");
        assert_eq!(card.picks(), Some(vec![vec![0], vec![0, 2]]));
        assert_eq!(
            card.answer_text(),
            "**Database** — Which database?\n→ Postgres\n\nWhich features?\n→ auth, search"
        );

        let effects = dispatch(&mut app, PromptAction::Submit);
        let (session_id, _, picks) = sent(&effects);
        assert_eq!(session_id, "s-a");
        assert_eq!(picks, Some([vec![0], vec![0, 2]].as_slice()));
        assert!(app.asks.is_empty(), "answering clears the card optimistically");
    }

    #[test]
    fn a_single_select_question_keeps_only_the_last_pick() {
        let mut app = app();
        ask(&mut app);
        dispatch(&mut app, PromptAction::PickIndex(0));
        dispatch(&mut app, PromptAction::PickIndex(1));
        assert_eq!(app.asks["s-a"].chosen[0].iter().copied().collect::<Vec<_>>(), vec![1]);
    }

    #[test]
    fn free_text_drops_the_structured_picks() {
        let mut app = app();
        ask(&mut app);
        dispatch(&mut app, PromptAction::EditOther);
        assert_eq!(app.prompt_focus(), Some(PromptFocus::AskText));
        for c in "mysql".chars() {
            dispatch(&mut app, PromptAction::TextKey(key(c)));
        }
        dispatch(&mut app, PromptAction::TextCommit);
        dispatch(&mut app, PromptAction::NextQuestion);
        dispatch(&mut app, PromptAction::PickIndex(1));

        let card = app.asks.get("s-a").expect("a live ask");
        assert!(card.picks().is_none(), "free text forces the reply fallback");
        assert!(card.answer_text().contains("→ mysql"));
    }

    #[test]
    fn submitting_needs_every_question_answered() {
        let mut app = app();
        ask(&mut app);
        dispatch(&mut app, PromptAction::PickIndex(0));
        assert!(dispatch(&mut app, PromptAction::Submit).is_empty());
        assert!(app.asks.contains_key("s-a"), "the card survives a refused submit");
    }

    #[test]
    fn a_payload_without_options_falls_back_to_free_text() {
        let card = AskCard::new("What now?".to_owned(), None, None);
        assert_eq!(card.questions.len(), 1);
        assert!(card.questions[0].options.is_empty());
        assert!(!card.answered_all());
    }

    #[test]
    fn resolving_clears_the_card_however_it_was_answered() {
        let mut app = app();
        ask(&mut app);
        dispatch(&mut app, PromptAction::AskResolved { session_id: "s-a".to_owned() });
        assert!(app.asks.is_empty());
        assert!(app.prompt_focus().is_none());

        plan(&mut app);
        assert_eq!(app.prompt_marker("s-a"), Some("P"));
        dispatch(&mut app, PromptAction::PlanResolved { session_id: "s-a".to_owned() });
        assert!(app.plans.is_empty());
        assert!(app.prompt_marker("s-a").is_none());
    }

    #[test]
    fn deferring_hands_the_keys_back_and_focus_takes_them_again() {
        let mut app = app();
        ask(&mut app);
        assert_eq!(app.prompt_focus(), Some(PromptFocus::Ask));
        dispatch(&mut app, PromptAction::Defer);
        assert!(app.prompt_focus().is_none());
        assert!(app.asks.contains_key("s-a"), "deferring keeps the card on screen");
        dispatch(&mut app, PromptAction::Focus);
        assert_eq!(app.prompt_focus(), Some(PromptFocus::Ask));
    }

    #[test]
    fn a_plan_pick_maps_to_its_index_and_label() {
        let mut app = app();
        plan(&mut app);
        let effects = dispatch(&mut app, PromptAction::PlanChoose(1));
        let (_, content, picks) = sent(&effects);
        assert_eq!(content, PLAN_OPTIONS[1]);
        assert_eq!(picks, Some([vec![1]].as_slice()));
        assert!(app.plans.is_empty());
        assert!(dispatch(&mut app, PromptAction::PlanChoose(0)).is_empty());
    }

    #[test]
    fn refining_a_plan_sends_free_text_with_no_picks() {
        let mut app = app();
        plan(&mut app);
        dispatch(&mut app, PromptAction::PlanRefine);
        assert_eq!(app.prompt_focus(), Some(PromptFocus::PlanText));
        for c in "smaller".chars() {
            dispatch(&mut app, PromptAction::TextKey(key(c)));
        }
        dispatch(
            &mut app,
            PromptAction::TextKey(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Backspace,
                crossterm::event::KeyModifiers::NONE,
            )),
        );
        let effects = dispatch(&mut app, PromptAction::TextCommit);
        let (_, content, picks) = sent(&effects);
        assert_eq!(content, "smalle");
        assert!(picks.is_none());
    }

    #[test]
    fn the_option_cursor_wraps_within_the_current_question() {
        let mut app = app();
        ask(&mut app);
        dispatch(&mut app, PromptAction::PrevOption);
        assert_eq!(app.asks["s-a"].cursor[0], 1);
        dispatch(&mut app, PromptAction::NextOption);
        assert_eq!(app.asks["s-a"].cursor[0], 0);
        dispatch(&mut app, PromptAction::Toggle);
        assert!(app.asks["s-a"].chosen[0].contains(&0));
    }

    #[test]
    fn a_historical_ask_renders_its_questions_instead_of_json() {
        let text = historical_tool_text("AskUserQuestion", &json!({"questions": questions()}))
            .expect("a rendering");
        assert_eq!(
            text,
            "Which database? [Postgres | SQLite] · Which features? [auth | billing | search]"
        );
        assert!(!text.contains('\n'), "a tool detail renders as one row");
        assert_eq!(
            historical_tool_text("ExitPlanMode", &json!({"plan": "# P\n\n- a"})).as_deref(),
            Some("# P (+1 lines)")
        );
        assert!(historical_tool_text("Bash", &json!({"command": "ls"})).is_none());
    }

    fn key(c: char) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char(c),
            crossterm::event::KeyModifiers::NONE,
        )
    }
}
