//! The spawn dialog's profile strip: pick a saved kit, adjust it for one
//! launch, save it back, add, delete, reorder.
//!
//! The section owns its state, as the dialog's trait expects, and writes only
//! the compute knobs: harness, account binding, model, effort, permission mode
//! and tier. Where it runs, the prompt, the labels and the env stay whatever
//! the rest of the form holds.

use cctui_clientcore::profiles::{
    AccountRef, ChainLabels, PoolRef, ProfileSpec, apply_spec, initial_profile, move_profile,
    same_spec, spec_chain, spec_changes, spec_from_form, unique_profile_name,
};
use cctui_clientcore::spawn::SpawnFields;
use cctui_proto::api::SpawnRequest;
use cctui_proto::api::profiles::SessionProfile;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::text::{Line, Span};
use uuid::Uuid;

use super::action::Effect;
use super::spawn::{SpawnForm, SpawnSection};
use super::state::App;
use super::toast::Level;
use crate::theme;

/// How the chain line words a knob left to the harness or the account.
const LABELS: ChainLabels<'static> = ChainLabels {
    auto: "auto",
    no_account: "no account",
    default_model: "default model",
    default_effort: "default effort",
    default_mode: "default mode",
};

/// A name prompt: creating a profile from the form, or renaming one.
#[derive(Debug)]
pub struct NamePrompt {
    pub buffer: String,
    /// `None` while creating, else the profile being renamed.
    pub editing: Option<Uuid>,
}

#[derive(Debug, Default)]
pub struct ProfileSection {
    pub list: Vec<SessionProfile>,
    /// The picked profile; `None` leaves the form's own knobs in charge.
    pub selected: Option<Uuid>,
    /// The spec in effect: the picked profile's, plus any one-off adjust.
    pub spec: ProfileSpec,
    pub prompt: Option<NamePrompt>,
    /// A delete waiting on its confirm.
    pub confirm: Option<Uuid>,
    pub loaded: bool,
    /// The machine's last-used profile, as spawn memory remembers it.
    pub last_used: Option<String>,
    /// Names the chain line resolves against; the accounts view fills these,
    /// and an empty list simply reads as Auto.
    pub accounts: Vec<AccountRef>,
    pub pools: Vec<PoolRef>,
}

/// A toast the section asks the reducer to raise, having no app of its own.
type Said = Option<(Level, String)>;

impl ProfileSection {
    #[must_use]
    pub fn selected_profile(&self) -> Option<&SessionProfile> {
        let id = self.selected?;
        self.list.iter().find(|p| p.id == id)
    }

    #[must_use]
    pub fn saved_spec(&self) -> Option<ProfileSpec> {
        self.selected_profile().map(spec_of)
    }

    /// How many knobs a one-off adjust moved away from the saved profile.
    #[must_use]
    pub fn changes(&self) -> usize {
        self.saved_spec().map_or(0, |saved| spec_changes(&saved, &self.spec))
    }

    #[must_use]
    pub fn is_adjusted(&self) -> bool {
        self.saved_spec().is_some_and(|saved| !same_spec(&saved, &self.spec))
    }

    fn names(&self) -> Vec<String> {
        self.list.iter().map(|p| p.name.clone()).collect()
    }

    fn ids(&self) -> Vec<String> {
        self.list.iter().map(|p| p.id.to_string()).collect()
    }

    /// The knobs in effect, in the web UI's wording and order.
    #[must_use]
    pub fn chain(&self) -> String {
        spec_chain(&self.spec, &self.accounts, &self.pools, LABELS, &|_, alias| alias.to_owned())
    }

    /// The form's own knobs moved: re-read the spec in effect so the change
    /// count and the star follow.
    pub fn form_changed(&mut self, fields: Box<SpawnFields>, said: &mut Said) -> Vec<Effect> {
        self.reduce(ProfileAction::FormChanged(fields), said)
    }

    /// Read the profiles when the dialog opens, once per run.
    pub fn on_open(&mut self) -> Vec<Effect> {
        if self.loaded { Vec::new() } else { vec![Effect::LoadProfiles] }
    }

    fn adopt(&mut self, list: Vec<SessionProfile>) {
        self.list = list;
        self.loaded = true;
        let ids = self.ids();
        let Some(id) = initial_profile(&ids, self.last_used.as_deref()) else { return };
        if let Some(index) = ids.iter().position(|known| *known == id) {
            self.pick(index);
        }
    }

    fn pick(&mut self, index: usize) {
        let Some(profile) = self.list.get(index) else { return };
        self.selected = Some(profile.id);
        self.spec = spec_of(profile);
    }

    fn save(&self, said: &mut Said) -> Vec<Effect> {
        let Some(profile) = self.selected_profile() else {
            *said = Some((Level::Info, "no profile picked — press + to make one".to_owned()));
            return Vec::new();
        };
        if !self.is_adjusted() {
            *said = Some((Level::Info, "nothing to save".to_owned()));
            return Vec::new();
        }
        vec![Effect::UpdateProfile {
            id: profile.id.to_string(),
            name: None,
            spec: Box::new(wire_spec(&self.spec)),
        }]
    }

    fn move_selected(&mut self, delta: i32) -> Vec<Effect> {
        let Some(id) = self.selected else { return Vec::new() };
        let ids = self.ids();
        let Some(at) = ids.iter().position(|known| *known == id.to_string()) else {
            return Vec::new();
        };
        let target = if delta < 0 { at.saturating_sub(1) } else { at + 1 };
        if target == at || target >= ids.len() {
            return Vec::new();
        }
        let order = move_profile(&ids, &id.to_string(), target);
        // Reorder locally so the strip moves under the key; the server's
        // answer replaces the list either way.
        let mut next = Vec::with_capacity(self.list.len());
        for wanted in &order {
            if let Some(found) = self.list.iter().find(|p| p.id.to_string() == *wanted) {
                next.push(found.clone());
            }
        }
        self.list = next;
        let ids = order.iter().filter_map(|id| Uuid::parse_str(id).ok()).collect();
        vec![Effect::ReorderProfiles { ids }]
    }

    fn prompt_commit(&mut self, said: &mut Said) -> Vec<Effect> {
        let Some(prompt) = self.prompt.take() else { return Vec::new() };
        let name = prompt.buffer.trim().to_owned();
        if name.is_empty() {
            *said = Some((Level::Warn, "a profile needs a name".to_owned()));
            return Vec::new();
        }
        let spec = Box::new(wire_spec(&self.spec));
        match prompt.editing {
            Some(id) => vec![Effect::UpdateProfile { id: id.to_string(), name: Some(name), spec }],
            None => vec![Effect::CreateProfile { name, spec }],
        }
    }

    fn stored(&mut self, profile: &SessionProfile, said: &mut Said) {
        match self.list.iter_mut().find(|p| p.id == profile.id) {
            Some(row) => row.clone_from(profile),
            None => self.list.push(profile.clone()),
        }
        self.selected = Some(profile.id);
        self.spec = spec_of(profile);
        *said = Some((Level::Info, format!("saved “{}”", profile.name)));
    }

    #[allow(clippy::too_many_lines)]
    fn reduce(&mut self, action: ProfileAction, said: &mut Said) -> Vec<Effect> {
        match action {
            ProfileAction::Loaded(list) => {
                self.adopt(list);
                Vec::new()
            }
            ProfileAction::Failed => {
                *said = Some((Level::Warn, "could not load the profiles".to_owned()));
                Vec::new()
            }
            ProfileAction::Pick(index) => {
                self.pick(index);
                Vec::new()
            }
            ProfileAction::FormChanged(fields) => {
                self.spec = spec_from_form(&fields, &self.accounts, &self.pools);
                Vec::new()
            }
            ProfileAction::Save => self.save(said),
            ProfileAction::New => {
                let base = self.selected_profile().map_or("Profile", |p| p.name.as_str());
                let name = unique_profile_name(base, &self.names());
                self.prompt = Some(NamePrompt { buffer: name, editing: None });
                Vec::new()
            }
            ProfileAction::Delete => {
                self.confirm = self.selected;
                if self.confirm.is_none() {
                    *said = Some((Level::Info, "pick a profile first".to_owned()));
                }
                Vec::new()
            }
            ProfileAction::DeleteConfirm => self
                .confirm
                .take()
                .map(|id| vec![Effect::DeleteProfile { id: id.to_string() }])
                .unwrap_or_default(),
            ProfileAction::DeleteCancel => {
                self.confirm = None;
                Vec::new()
            }
            ProfileAction::Move(delta) => self.move_selected(delta),
            ProfileAction::Reordered(list) => {
                self.list = list;
                Vec::new()
            }
            ProfileAction::PromptKey(key) => {
                if let Some(prompt) = self.prompt.as_mut() {
                    match key.code {
                        KeyCode::Char(c) => prompt.buffer.push(c),
                        KeyCode::Backspace => {
                            prompt.buffer.pop();
                        }
                        _ => {}
                    }
                }
                Vec::new()
            }
            ProfileAction::PromptCommit => self.prompt_commit(said),
            ProfileAction::PromptCancel => {
                self.prompt = None;
                Vec::new()
            }
            ProfileAction::Stored(profile) => {
                self.stored(&profile, said);
                Vec::new()
            }
            ProfileAction::Deleted(id) => {
                self.list.retain(|p| p.id != id);
                if self.selected == Some(id) {
                    self.selected = None;
                }
                *said = Some((Level::Info, "deleted the profile".to_owned()));
                Vec::new()
            }
        }
    }

    /// The numbered names, and the chain line with its change count.
    #[must_use]
    pub fn strip(&self) -> (Vec<(usize, String, bool)>, String) {
        let rows = self
            .list
            .iter()
            .take(9)
            .enumerate()
            .map(|(index, profile)| {
                let picked = self.selected == Some(profile.id);
                let adjusted = picked && self.is_adjusted();
                let name =
                    if adjusted { format!("{}*", profile.name) } else { profile.name.clone() };
                (index + 1, name, picked)
            })
            .collect();
        let mut chain = self.chain();
        match self.changes() {
            0 => {}
            1 => chain.push_str("   (1 change)"),
            n => {
                use std::fmt::Write;
                let _ = write!(chain, "   ({n} changes)");
            }
        }
        (rows, chain)
    }
}

/// The spec a stored profile carries.
#[must_use]
pub fn spec_of(profile: &SessionProfile) -> ProfileSpec {
    ProfileSpec {
        harness: profile.harness.clone(),
        account_id: profile.account_id.map(|id| id.to_string()),
        pool_id: profile.pool_id.map(|id| id.to_string()),
        no_account: profile.no_account,
        model_alias: blank(profile.model_alias.as_deref()),
        effort: blank(profile.effort.as_deref()),
        permission_mode: blank(profile.permission_mode.as_deref()),
        service_tier: blank(profile.service_tier.as_deref()),
    }
}

fn blank(value: Option<&str>) -> Option<String> {
    value.map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned)
}

fn wire_spec(spec: &ProfileSpec) -> cctui_proto::api::profiles::ProfileSpec {
    cctui_proto::api::profiles::ProfileSpec {
        harness: spec.harness.clone(),
        account_id: spec.account_id.as_deref().and_then(|id| Uuid::parse_str(id).ok()),
        pool_id: spec.pool_id.as_deref().and_then(|id| Uuid::parse_str(id).ok()),
        no_account: spec.no_account,
        model_alias: spec.model_alias.clone(),
        effort: spec.effort.clone(),
        permission_mode: spec.permission_mode.clone(),
        service_tier: spec.service_tier.clone(),
        context_items: Vec::new(),
    }
}

pub enum ProfileAction {
    /// `GET /profiles` answered.
    Loaded(Vec<SessionProfile>),
    Failed,
    /// `1`-`9`: pick the nth profile and apply its spec.
    Pick(usize),
    /// The form's own knobs changed under the strip.
    FormChanged(Box<SpawnFields>),
    /// `s`: write the spec in effect back to the picked profile.
    Save,
    /// `+`: create a profile from the spec in effect.
    New,
    Delete,
    DeleteConfirm,
    DeleteCancel,
    /// `K` / `J`: move the picked profile up or down.
    Move(i32),
    Reordered(Vec<SessionProfile>),
    PromptKey(KeyEvent),
    PromptCommit,
    PromptCancel,
    Stored(Box<SessionProfile>),
    Deleted(Uuid),
}

/// The strip's state lives in the section; toasts need the app. This threads
/// the two together.
pub fn reduce_profiles(app: &mut App, action: ProfileAction) -> Vec<Effect> {
    let Some(section) = app.spawn.as_mut().and_then(SpawnForm::profiles_mut) else {
        return Vec::new();
    };
    let mut said = None;
    let effects = section.reduce(action, &mut said);
    if let Some((level, text)) = said {
        app.toast(level, text);
    }
    effects
}

impl SpawnSection for ProfileSection {
    fn title(&self) -> &'static str {
        "Profile"
    }

    fn rows(&self, _fields: &SpawnFields) -> usize {
        usize::from(!self.list.is_empty())
    }

    fn lines(
        &self,
        focused: Option<usize>,
        width: u16,
        _fields: &SpawnFields,
    ) -> Vec<Line<'static>> {
        let room = usize::from(width);
        let clip = |text: String| -> String { text.chars().take(room).collect() };
        let (rows, chain) = self.strip();
        if rows.is_empty() {
            let empty = clip("   no profiles yet — press + to save this setup".to_owned());
            return vec![Line::from(Span::styled(empty, theme::dim()))];
        }
        // The names row is built span by span, so it is budgeted as it grows:
        // a long profile list must not push the row past the dialog.
        let mut spans = vec![Span::raw(if focused.is_some() { " ❯ " } else { "   " })];
        let mut used = 3;
        for (number, name, picked) in rows {
            let cell = format!("{number} {name}  ");
            if used + cell.chars().count() > room {
                spans.push(Span::styled("…", theme::dim()));
                break;
            }
            used += cell.chars().count();
            let style = if picked { theme::hotkey() } else { theme::dim() };
            spans.push(Span::styled(cell, style));
        }
        vec![
            Line::from(spans),
            Line::from(Span::styled(clip(format!("     {chain}")), theme::dim())),
            Line::from(Span::styled(
                clip("     1-9 pick · s save · + new · D delete · K/J move".to_owned()),
                theme::dim(),
            )),
        ]
    }

    /// Adjusting a knob is the rest of the form's job, so the strip claims
    /// only its own keys and leaves the arrows and Tab to the dialog.
    fn handle(&mut self, _row: usize, key: KeyEvent, _fields: &mut SpawnFields) -> Vec<Effect> {
        let action = match key.code {
            KeyCode::Char(c @ '1'..='9') => ProfileAction::Pick(c as usize - '1' as usize),
            KeyCode::Char('s') => ProfileAction::Save,
            KeyCode::Char('+') => ProfileAction::New,
            KeyCode::Char('D') => ProfileAction::Delete,
            KeyCode::Char('K') => ProfileAction::Move(-1),
            KeyCode::Char('J') => ProfileAction::Move(1),
            _ => return Vec::new(),
        };
        let mut said = None;
        self.reduce(action, &mut said)
    }

    /// The launch names the profile it came from, so the server applies its
    /// context set. Every other knob travels as its own field.
    fn apply(&self, request: &mut SpawnRequest) {
        request.profile_id = self.selected;
    }

    fn as_profiles(&self) -> Option<&Self> {
        Some(self)
    }

    fn as_profiles_mut(&mut self) -> Option<&mut Self> {
        Some(self)
    }
}

/// The form with the picked profile's knobs written over it.
#[must_use]
pub fn applied(section: &ProfileSection, fields: &SpawnFields) -> SpawnFields {
    apply_spec(fields, &section.spec, &section.accounts, &section.pools)
}

#[cfg(test)]
mod tests {
    use cctui_clientcore::profiles::AccountRef;
    use cctui_clientcore::spawn::SpawnFields;
    use cctui_proto::api::profiles::SessionProfile;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use uuid::Uuid;

    use super::{ProfileAction, ProfileSection, applied, spec_of, wire_spec};
    use crate::app::action::Effect;
    use crate::app::spawn::SpawnSection;

    fn uuid(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn profile(n: u128, name: &str, harness: &str, effort: Option<&str>) -> SessionProfile {
        SessionProfile {
            id: uuid(n),
            user_id: uuid(999),
            name: name.to_owned(),
            harness: harness.to_owned(),
            account_id: None,
            pool_id: None,
            no_account: false,
            model_alias: None,
            effort: effort.map(str::to_owned),
            permission_mode: None,
            service_tier: None,
            context_items: Vec::new(),
            sort_order: 0,
            created_at: chrono::DateTime::from_timestamp(0, 0).expect("epoch"),
            updated_at: chrono::DateTime::from_timestamp(0, 0).expect("epoch"),
        }
    }

    fn act(section: &mut ProfileSection, action: ProfileAction) -> Vec<Effect> {
        let mut said = None;
        section.reduce(action, &mut said)
    }

    fn told(section: &mut ProfileSection, action: ProfileAction) -> Option<String> {
        let mut said = None;
        section.reduce(action, &mut said);
        said.map(|(_, text)| text)
    }

    fn section(list: Vec<SessionProfile>) -> ProfileSection {
        let mut section = ProfileSection {
            accounts: vec![AccountRef {
                id: uuid(1).to_string(),
                name: "personal".to_owned(),
                emoji: None,
                providers: vec!["anthropic".to_owned()],
            }],
            ..ProfileSection::default()
        };
        act(&mut section, ProfileAction::Loaded(list));
        section
    }

    fn fields() -> SpawnFields {
        SpawnFields {
            adapter_id: "claude-code".to_owned(),
            model_claude: "fable".to_owned(),
            effort_claude: "medium".to_owned(),
            prompt: "keep me".to_owned(),
            ..SpawnFields::default()
        }
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn the_strip_opens_on_the_first_profile_and_a_digit_picks_one() {
        let mut section = section(vec![
            profile(1, "Default", "claude-code", None),
            profile(2, "codex-hi", "codex", Some("high")),
        ]);
        assert_eq!(section.selected, Some(uuid(1)));

        section.handle(0, key('2'), &mut fields());
        assert_eq!(section.selected, Some(uuid(2)));
        assert_eq!(section.spec.harness, "codex");
        assert_eq!(section.spec.effort.as_deref(), Some("high"));

        let (rows, chain) = section.strip();
        assert_eq!(rows[0], (1, "Default".to_owned(), false));
        assert_eq!(rows[1], (2, "codex-hi".to_owned(), true));
        assert_eq!(chain, "Codex · auto · default model · high · default mode");
    }

    #[test]
    fn the_strip_lands_on_the_machines_last_used_profile() {
        let mut section =
            ProfileSection { last_used: Some(uuid(2).to_string()), ..ProfileSection::default() };
        act(
            &mut section,
            ProfileAction::Loaded(vec![
                profile(1, "Default", "claude-code", None),
                profile(2, "codex-hi", "codex", None),
            ]),
        );
        assert_eq!(section.selected, Some(uuid(2)), "spawn memory decides, not the order");
    }

    #[test]
    fn a_pick_patches_only_the_compute_fields_of_the_form() {
        let section = section(vec![profile(2, "codex-hi", "codex", Some("high"))]);
        let out = applied(&section, &fields());
        assert_eq!(out.adapter_id, "codex");
        assert_eq!(out.effort_codex, "high");
        assert_eq!(out.model_claude, "fable", "the claude knobs are left as they were");
        assert_eq!(out.effort_claude, "medium");
        assert_eq!(out.prompt, "keep me", "the prompt is none of the strip's business");
    }

    #[test]
    fn adjusting_the_form_marks_the_profile_and_counts_the_changes() {
        let mut section = section(vec![profile(1, "Default", "claude-code", None)]);
        assert_eq!(section.changes(), 0);

        let adjusted = SpawnFields { effort_claude: "high".to_owned(), ..fields() };
        act(&mut section, ProfileAction::FormChanged(Box::new(adjusted)));
        assert_eq!(section.changes(), 2, "the model and the effort moved");
        assert!(section.is_adjusted());

        let (rows, chain) = section.strip();
        assert_eq!(rows[0].1, "Default*", "the star marks a one-off adjust");
        assert!(chain.ends_with("(2 changes)"), "{chain}");
    }

    #[test]
    fn s_writes_the_adjusted_spec_back_and_refuses_when_nothing_moved() {
        let mut section = section(vec![profile(1, "Default", "claude-code", None)]);
        assert_eq!(told(&mut section, ProfileAction::Save).as_deref(), Some("nothing to save"));

        let adjusted = SpawnFields { effort_claude: "high".to_owned(), ..fields() };
        act(&mut section, ProfileAction::FormChanged(Box::new(adjusted)));
        match section.handle(0, key('s'), &mut fields()).as_slice() {
            [Effect::UpdateProfile { id, name, spec }] => {
                assert_eq!(*id, uuid(1).to_string());
                assert!(name.is_none(), "a save keeps the name");
                assert_eq!(spec.effort.as_deref(), Some("high"));
                assert_eq!(spec.model_alias.as_deref(), Some("fable"));
            }
            _ => panic!("expected a profile update"),
        }
    }

    #[test]
    fn plus_names_a_new_profile_from_the_spec_in_effect() {
        let mut section = section(vec![profile(1, "Default", "claude-code", None)]);
        section.handle(0, key('+'), &mut fields());
        assert_eq!(
            section.prompt.as_ref().expect("a prompt").buffer,
            "Default 2",
            "the proposed name is the first one free"
        );
        for c in "-x".chars() {
            act(&mut section, ProfileAction::PromptKey(key(c)));
        }
        match act(&mut section, ProfileAction::PromptCommit).as_slice() {
            [Effect::CreateProfile { name, spec }] => {
                assert_eq!(name, "Default 2-x");
                assert_eq!(spec.harness, "claude-code");
            }
            _ => panic!("expected a profile create"),
        }
    }

    #[test]
    fn a_nameless_profile_is_refused() {
        let mut section = section(vec![profile(1, "Default", "claude-code", None)]);
        act(&mut section, ProfileAction::New);
        for _ in 0.."Default 2".len() {
            act(
                &mut section,
                ProfileAction::PromptKey(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)),
            );
        }
        assert_eq!(
            told(&mut section, ProfileAction::PromptCommit).as_deref(),
            Some("a profile needs a name")
        );
    }

    #[test]
    fn a_stored_profile_joins_the_strip_and_becomes_the_pick() {
        let mut section = section(vec![profile(1, "Default", "claude-code", None)]);
        let fresh = profile(7, "codex-hi", "codex", Some("high"));
        act(&mut section, ProfileAction::Stored(Box::new(fresh)));
        assert_eq!(section.list.len(), 2);
        assert_eq!(section.selected, Some(uuid(7)));
        assert_eq!(section.changes(), 0, "a fresh save is not adjusted");
    }

    #[test]
    fn delete_waits_for_a_confirm_and_drops_the_row() {
        let mut section = section(vec![profile(1, "Default", "claude-code", None)]);
        section.handle(0, key('D'), &mut fields());
        assert_eq!(section.confirm, Some(uuid(1)));
        act(&mut section, ProfileAction::DeleteCancel);
        assert!(
            act(&mut section, ProfileAction::DeleteConfirm).is_empty(),
            "nothing is deleted without a standing confirm"
        );

        act(&mut section, ProfileAction::Delete);
        assert!(matches!(
            act(&mut section, ProfileAction::DeleteConfirm).as_slice(),
            [Effect::DeleteProfile { .. }]
        ));
        act(&mut section, ProfileAction::Deleted(uuid(1)));
        assert!(section.list.is_empty());
        assert_eq!(section.selected, None);
    }

    #[test]
    fn k_and_j_reorder_the_picked_profile_and_tell_the_server() {
        let mut section = section(vec![
            profile(1, "a", "claude-code", None),
            profile(2, "b", "claude-code", None),
            profile(3, "c", "claude-code", None),
        ]);
        section.handle(0, key('3'), &mut fields());
        match section.handle(0, key('K'), &mut fields()).as_slice() {
            [Effect::ReorderProfiles { ids }] => assert_eq!(ids, &vec![uuid(1), uuid(3), uuid(2)]),
            _ => panic!("expected a reorder"),
        }
        assert_eq!(
            section.list.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["a", "c", "b"],
            "the strip moves under the key"
        );

        section.handle(0, key('1'), &mut fields());
        assert!(
            section.handle(0, key('K'), &mut fields()).is_empty(),
            "the first profile cannot go up"
        );
    }

    #[test]
    fn the_profiles_are_read_once_per_run() {
        let mut section = ProfileSection::default();
        assert!(matches!(section.on_open().as_slice(), [Effect::LoadProfiles]));
        act(&mut section, ProfileAction::Loaded(Vec::new()));
        assert!(section.on_open().is_empty());
    }

    #[test]
    fn the_launch_names_the_profile_it_came_from() {
        let section = section(vec![profile(1, "Default", "claude-code", None)]);
        let mut request = cctui_clientcore::spawn::build_spawn_body(
            &fields(),
            None,
            std::collections::BTreeMap::new(),
            None,
            None,
        );
        assert_eq!(request.profile_id, None, "the form itself never writes it");
        section.apply(&mut request);
        assert_eq!(request.profile_id, Some(uuid(1)));
    }

    #[test]
    fn an_empty_strip_says_how_to_fill_it_and_takes_no_focus() {
        let section = ProfileSection::default();
        assert_eq!(section.rows(&fields()), 0, "nothing to pick, nothing to focus");
        let lines = section.lines(None, 80, &fields());
        assert_eq!(lines.len(), 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("press + to save this setup"), "{text}");
    }

    #[test]
    fn the_strip_fits_eighty_columns() {
        let mut section = section(vec![
            profile(1, "Default", "claude-code", None),
            profile(2, "codex-hi", "codex", Some("high")),
            profile(3, "review-ask", "claude-code", Some("low")),
        ]);
        section.handle(0, key('2'), &mut fields());
        for line in section.lines(Some(0), 80, &fields()) {
            let width: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            assert!(width <= 80, "{width} columns: {line:?}");
        }
    }

    #[test]
    fn a_profile_spec_round_trips_through_the_wire_shape() {
        let mut profile = profile(1, "p", "codex", Some("high"));
        profile.account_id = Some(uuid(1));
        profile.model_alias = Some("  ".to_owned());
        profile.service_tier = Some("fast".to_owned());
        let spec = spec_of(&profile);
        assert_eq!(spec.model_alias, None, "a blank alias is no alias");
        let wire = wire_spec(&spec);
        assert_eq!(wire.account_id, Some(uuid(1)));
        assert_eq!(wire.service_tier.as_deref(), Some("fast"));
        assert_eq!(wire.harness, "codex");
    }
}
