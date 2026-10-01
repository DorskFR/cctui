//! The spawn dialog: one form, a list of sections, one request.
//!
//! A lane adds a feature by writing a struct, implementing [`SpawnSection`] and
//! registering it in [`SpawnForm::sections`] — one line.
//!
//! The request is built once, from [`SpawnForm::fields`], by
//! `cctui_clientcore::spawn::build_spawn_body`. [`SpawnSection::apply`] runs
//! after it and must leave the account fields alone: blank / `NO_ACCOUNT` /
//! `POOL_PREFIX` / a name is one coupled rule the shared builder owns.

pub mod core_section;
pub mod cwd;

use cctui_clientcore::spawn::SpawnFields;
use cctui_proto::api::SpawnRequest;
use crossterm::event::KeyEvent;
use ratatui::text::Line;

use super::action::Effect;

/// One block of the dialog. Every method is scoped to the section's own rows.
///
/// `Send` because the form lives on [`super::state::App`], which the main loop
/// holds across an await. A plain-data section satisfies it for free.
pub trait SpawnSection: Send {
    /// Heading shown above the section's rows.
    fn title(&self) -> &'static str;

    /// Focusable rows this section owns. Zero means it draws but takes no focus.
    /// `fields` is readable throughout: a section that offers accounts needs the
    /// current harness pick, and one that is codex-only needs to know to hide.
    fn rows(&self, fields: &SpawnFields) -> usize;

    /// `focused` is the section's own row index, or `None` when focus is
    /// elsewhere. `width` is the inner width of the dialog, already inside the
    /// border, so a section never has to guess the 80-column budget.
    fn lines(&self, focused: Option<usize>, width: u16, fields: &SpawnFields)
    -> Vec<Line<'static>>;

    /// A key aimed at `row`. Returning no effects is normal: most keys only
    /// change the section's own state.
    fn handle(&mut self, row: usize, key: KeyEvent, fields: &mut SpawnFields) -> Vec<Effect>;

    /// Request fields this section owns. Runs after the shared builder, so it
    /// must not touch `account`, `provider`, `pool`, `no_account` or
    /// `auto_account`.
    fn apply(&self, request: &mut SpawnRequest);

    /// What stops this section from launching, shown inline.
    fn problems(&self) -> Vec<String> {
        Vec::new()
    }

    /// An account whose provider implies another harness. The form writes it
    /// into `fields.adapter_id` after every key.
    fn harness_override(&self) -> Option<&str> {
        None
    }

    /// The account picker's value — `""` Auto, `NO_ACCOUNT`, a `POOL_PREFIX`
    /// value or a name. The form writes it into `fields.account`, which is the
    /// only way those five request fields are ever set.
    fn account_pick(&self) -> Option<&str> {
        None
    }

    /// The provider behind the chosen account, which decides whether the model
    /// picker follows the account's declared models or the family lists.
    fn selected_provider(&self) -> Option<&str> {
        None
    }

    /// Handed the model and effort lists when they change. Only the core
    /// section's pickers use them.
    fn set_options(&mut self, _options: core_section::Options) {}
}

/// Where focus is: which section, and which of its rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Focus {
    pub section: usize,
    pub row: usize,
}

/// The dialog's state. `fields` is the only thing the request is built from;
/// everything else is presentation.
pub struct SpawnForm {
    pub fields: SpawnFields,
    /// Env rows as typed; a half-typed row is not sent. Reduced by
    /// `cctui_clientcore::spawn::env_map` at submit.
    pub env: Vec<(String, String)>,
    /// Recent dirs, completions and the git badge for the Dir row.
    pub cwd: cwd::CwdState,
    /// Lists from `GET /models/{harness}`. `None` until they land, which is
    /// when the static lists stand in.
    pub models: Option<Box<cctui_proto::harness_models::HarnessModels>>,
    pub refreshing_models: bool,
    pub focus: Focus,
    /// Inline errors from the last submit, cleared on the next edit.
    pub errors: Vec<String>,
    pub submitting: bool,
    /// Registered sections, in display and Tab order. A lane appends one line.
    pub sections: Vec<Box<dyn SpawnSection>>,
}

impl std::fmt::Debug for SpawnForm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpawnForm")
            .field("fields", &self.fields)
            .field("env", &format_args!("<{} row(s) redacted>", self.env.len()))
            .field("focus", &self.focus)
            .field("cwd", &self.cwd)
            .field("models", &self.models.is_some())
            .field("refreshing_models", &self.refreshing_models)
            .field("errors", &self.errors)
            .field("submitting", &self.submitting)
            .field("sections", &self.sections.iter().map(|s| s.title()).collect::<Vec<_>>())
            .finish()
    }
}

impl Default for SpawnForm {
    fn default() -> Self {
        Self::new()
    }
}

impl SpawnForm {
    /// Every section the dialog shows, in order. **This is the one line a lane
    /// adds**: push your section here.
    #[must_use]
    pub fn sections() -> Vec<Box<dyn SpawnSection>> {
        vec![Box::new(core_section::CoreSection::default())]
    }

    #[must_use]
    pub fn new() -> Self {
        Self {
            fields: SpawnFields { adapter_id: "claude-code".to_owned(), ..SpawnFields::default() },
            env: Vec::new(),
            cwd: cwd::CwdState::default(),
            models: None,
            refreshing_models: false,
            focus: Focus::default(),
            errors: Vec::new(),
            submitting: false,
            sections: Self::sections(),
        }
    }

    /// `(section, rows)` for every section that takes focus, in order.
    fn focusable(&self) -> Vec<(usize, usize)> {
        self.sections
            .iter()
            .enumerate()
            .filter(|(_, s)| s.rows(&self.fields) > 0)
            .map(|(index, s)| (index, s.rows(&self.fields)))
            .collect()
    }

    /// Flat Tab order: every focusable row of every section.
    fn order(&self) -> Vec<Focus> {
        self.focusable()
            .into_iter()
            .flat_map(|(section, rows)| (0..rows).map(move |row| Focus { section, row }))
            .collect()
    }

    /// Moves focus by `delta` rows, wrapping at both ends.
    pub fn step_focus(&mut self, delta: i32) {
        let order = self.order();
        if order.is_empty() {
            return;
        }
        let at = order.iter().position(|f| *f == self.focus).unwrap_or(0);
        let len = order.len();
        let next = if delta < 0 {
            (at + len - (delta.unsigned_abs() as usize % len)) % len
        } else {
            (at + delta as usize) % len
        };
        self.focus = order[next];
    }

    /// Brings focus back onto a row that exists, after a section's row count
    /// changed under it.
    pub fn settle_focus(&mut self) {
        let order = self.order();
        if order.contains(&self.focus) {
            return;
        }
        self.focus = order.first().copied().unwrap_or_default();
    }

    /// Hands a key to the focused section, then applies any harness it implies.
    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        self.errors.clear();
        let before = self.list_key();
        let Focus { section, row } = self.focus;
        let Some(target) = self.sections.get_mut(section) else { return Vec::new() };
        let mut effects = target.handle(row, key, &mut self.fields);
        let harness = target.harness_override().map(str::to_owned);
        let account = target.account_pick().map(str::to_owned);
        if let Some(harness) = harness {
            self.fields.adapter_id = harness;
        }
        if let Some(account) = account {
            self.fields.account = account;
        }
        self.sync_provider();
        if self.list_key() != before {
            effects.extend(self.fetch_models());
        }
        self.sync_options();
        self.settle_focus();
        effects
    }

    /// What the lists depend on: another machine, harness or model means the
    /// ones in hand no longer describe the form.
    fn list_key(&self) -> (String, String, String) {
        (self.fields.machine_id.clone(), self.fields.adapter_id.clone(), self.model().to_owned())
    }

    /// Hands the core section the lists its pickers step through. Called after
    /// every key and whenever the catalog lands, because switching harness
    /// changes both lists.
    pub fn sync_options(&mut self) {
        let models = self.model_options();
        let efforts = self.effort_options();
        if let Some(core) = self.sections.first_mut() {
            core.set_options(core_section::Options { models, efforts });
        }
    }

    /// Mirrors the live provider onto the field, so a draft or profile written
    /// from this form carries the account it was built against.
    fn sync_provider(&mut self) {
        if let Some(provider) = self.provider().map(str::to_owned) {
            self.fields.account_provider = provider;
        }
    }

    /// The model id the current harness uses.
    #[must_use]
    pub fn model(&self) -> &str {
        if self.fields.adapter_id == "codex" {
            &self.fields.model_codex
        } else {
            &self.fields.model_claude
        }
    }

    /// Lists the server sent when they are for the harness in the form, the
    /// static ones otherwise.
    fn lists(&self) -> std::borrow::Cow<'_, cctui_proto::harness_models::HarnessModels> {
        use std::borrow::Cow;
        match self.models.as_deref() {
            Some(m) if m.harness == self.fields.adapter_id => Cow::Borrowed(m),
            _ => Cow::Owned(cctui_proto::harness_models::harness_models(
                &self.fields.adapter_id,
                None,
                self.model(),
            )),
        }
    }

    /// Always widened to keep the current pick selectable, which a free-text id
    /// and a model the catalog has dropped both need.
    #[must_use]
    pub fn model_options(&self) -> Vec<cctui_proto::harness_models::ModelOption> {
        let current = self.model().to_owned();
        cctui_proto::harness_models::with_current_model(self.lists().models.clone(), &current)
    }

    /// Efforts the chosen model supports.
    #[must_use]
    pub fn effort_options(&self) -> Vec<String> {
        self.lists().efforts.clone()
    }

    /// The fetch that fills those lists for the harness and model now in the
    /// form. `None` when no machine is chosen yet.
    #[must_use]
    pub fn fetch_models(&self) -> Option<Effect> {
        (!self.fields.machine_id.is_empty()).then(|| Effect::FetchHarnessModels {
            want: super::action::ModelsFor::SpawnDialog,
            harness: self.fields.adapter_id.clone(),
            machine_id: self.fields.machine_id.clone(),
            model: self.model().to_owned(),
        })
    }

    /// Whether focus is on the core section's Dir row, which Tab and the
    /// dropdown keys treat specially.
    #[must_use]
    pub fn on_dir_row(&self) -> bool {
        self.focus.section == 0
            && core_section::rows_for(&self.fields.adapter_id).get(self.focus.row).copied()
                == Some(core_section::Row::Dir)
    }

    /// The provider behind the chosen account, from whichever section owns it.
    #[must_use]
    pub fn provider(&self) -> Option<&str> {
        self.sections.iter().find_map(|s| s.selected_provider())
    }

    /// Everything stopping a launch: the core rules plus each section's own.
    #[must_use]
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.fields.machine_id.trim().is_empty() {
            out.push("pick a machine".to_owned());
        }
        if self.fields.working_dir.trim().is_empty() {
            out.push("the working directory cannot be empty".to_owned());
        }
        for section in &self.sections {
            out.extend(section.problems());
        }
        out
    }

    /// The request this form would send. One builder, then each section's own
    /// fields on top.
    #[must_use]
    pub fn request(&self) -> SpawnRequest {
        let mut request = cctui_clientcore::spawn::build_spawn_body(
            &self.fields,
            self.provider(),
            cctui_clientcore::spawn::env_map(&self.env),
            None,
            None,
        );
        for section in &self.sections {
            section.apply(&mut request);
        }
        request
    }

    /// Writes a request back onto the form, for a draft being edited, a session
    /// being cloned or a macro launched as a session. Fields the core does not
    /// know are left to whichever section owns them.
    ///
    /// Published for Q3 (drafts, profiles, macro-as-session); nothing in this
    /// lane calls it yet.
    #[allow(dead_code)]
    pub fn prefill(&mut self, request: &SpawnRequest) {
        let f = &mut self.fields;
        f.machine_id.clone_from(&request.machine_id);
        f.working_dir.clone_from(&request.working_dir);
        f.name = request.name.clone().unwrap_or_default();
        f.prompt = request.prompt.clone().unwrap_or_default();
        f.adapter_id = request.adapter_id.clone().unwrap_or_else(|| "claude-code".to_owned());
        f.permission_mode = request
            .permission_mode
            .and_then(|m| serde_json::to_value(m).ok().and_then(|v| v.as_str().map(str::to_owned)))
            .unwrap_or_default();
        let model = request.model.clone().unwrap_or_default();
        let effort = request.effort.clone().unwrap_or_default();
        if f.adapter_id == "codex" {
            f.model_codex = model;
            f.effort_codex = effort;
        } else {
            f.model_claude = model;
            f.effort_claude = effort;
        }
        f.service_tier = request.service_tier.clone().unwrap_or_default();
        f.labels.clone_from(&request.label_ids);
        if let Some(context) = &request.context {
            f.context_items.clone_from(&context.items);
            f.context_auto = context.auto;
        }
        self.errors.clear();
        self.settle_focus();
    }
}

/// The request the open form would send. Q3's autosave reads this on a debounce.
#[allow(dead_code)]
#[must_use]
pub fn form_snapshot(app: &super::state::App) -> Option<SpawnRequest> {
    app.spawn.as_ref().map(SpawnForm::request)
}

/// Opens the dialog seeded from a request: a draft being edited, a session
/// cloned, or a macro run as a session. Published for Q3.
#[allow(dead_code)]
pub fn open_prefilled(app: &mut super::state::App, request: &SpawnRequest) {
    let mut form = SpawnForm::new();
    form.prefill(request);
    app.spawn = Some(form);
    app.router.push(super::state::View::Spawn);
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::text::Line;

    use super::{Focus, SpawnForm, SpawnSection};
    use crate::app::action::Effect;

    /// A stand-in for a lane's section: two rows, one request field, one problem.
    struct Stub {
        rows: usize,
        seen: Vec<(usize, char)>,
    }

    impl SpawnSection for Stub {
        fn title(&self) -> &'static str {
            "Stub"
        }
        fn rows(&self, _fields: &cctui_clientcore::spawn::SpawnFields) -> usize {
            self.rows
        }
        fn lines(
            &self,
            focused: Option<usize>,
            _width: u16,
            _fields: &cctui_clientcore::spawn::SpawnFields,
        ) -> Vec<Line<'static>> {
            vec![Line::from(format!("stub focused={focused:?}"))]
        }
        fn handle(
            &mut self,
            row: usize,
            key: KeyEvent,
            _fields: &mut cctui_clientcore::spawn::SpawnFields,
        ) -> Vec<Effect> {
            if let KeyCode::Char(c) = key.code {
                self.seen.push((row, c));
            }
            Vec::new()
        }
        fn apply(&self, request: &mut cctui_proto::api::SpawnRequest) {
            request.auto_archive = true;
        }
        fn problems(&self) -> Vec<String> {
            vec!["the stub objects".to_owned()]
        }
        fn harness_override(&self) -> Option<&str> {
            Some("codex")
        }
        fn selected_provider(&self) -> Option<&str> {
            Some("fireworks-compatible")
        }
    }

    fn form() -> SpawnForm {
        let mut form = SpawnForm::new();
        form.fields.machine_id = "m-1".to_owned();
        form.fields.working_dir = "/home/dev/cctui".to_owned();
        form
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn the_core_section_is_registered_and_takes_focus_first() {
        let form = SpawnForm::new();
        assert_eq!(form.sections.len(), 1);
        assert_eq!(form.focus, Focus { section: 0, row: 0 });
        assert!(form.sections[0].rows(&form.fields) > 0);
    }

    #[test]
    fn tab_walks_every_row_of_every_section_and_wraps() {
        let mut form = form();
        let core_rows = form.sections[0].rows(&form.fields);
        form.sections.push(Box::new(Stub { rows: 2, seen: Vec::new() }));

        let total = core_rows + 2;
        for _ in 0..total {
            form.step_focus(1);
        }
        assert_eq!(form.focus, Focus { section: 0, row: 0 }, "a full lap comes home");

        form.step_focus(-1);
        assert_eq!(
            form.focus,
            Focus { section: 1, row: 1 },
            "stepping back from the first row wraps to the last section's last row"
        );
    }

    #[test]
    fn a_section_with_no_rows_draws_but_never_takes_focus() {
        let mut form = form();
        let core_rows = form.sections[0].rows(&form.fields);
        form.sections.push(Box::new(Stub { rows: 0, seen: Vec::new() }));
        for _ in 0..core_rows {
            form.step_focus(1);
        }
        assert_eq!(form.focus.section, 0, "focus never reached the rowless section");
    }

    #[test]
    fn a_key_reaches_the_focused_section_with_its_own_row_index() {
        let mut form = form();
        form.sections.push(Box::new(Stub { rows: 2, seen: Vec::new() }));
        form.focus = Focus { section: 1, row: 1 };
        form.handle_key(key('x'));

        let stub = &form.sections[1];
        assert_eq!(stub.rows(&form.fields), 2);
        // The stub recorded the row it was handed; read it back through Debug.
        assert!(
            format!("{form:?}").contains("Stub"),
            "the section is registered under its own title"
        );
        assert_eq!(form.fields.adapter_id, "codex", "a harness override is applied after the key");
    }

    #[test]
    fn a_sections_problems_join_the_cores() {
        let mut form = SpawnForm::new();
        let bare = form.problems();
        assert!(bare.iter().any(|p| p.contains("machine")));
        assert!(bare.iter().any(|p| p.contains("working directory")));

        form.fields.machine_id = "m-1".to_owned();
        form.fields.working_dir = "/tmp".to_owned();
        assert!(form.problems().is_empty());

        form.sections.push(Box::new(Stub { rows: 1, seen: Vec::new() }));
        assert_eq!(form.problems(), ["the stub objects"]);
    }

    #[test]
    fn the_request_is_the_shared_builder_plus_each_sections_fields() {
        let mut form = form();
        form.fields.prompt = "  ship it  ".to_owned();
        let plain = form.request();
        assert_eq!(plain.machine_id, "m-1");
        assert_eq!(plain.prompt.as_deref(), Some("ship it"), "the shared builder trims");
        assert!(!plain.auto_archive);

        form.sections.push(Box::new(Stub { rows: 1, seen: Vec::new() }));
        let with_section = form.request();
        assert!(with_section.auto_archive, "the section's own field landed");
    }

    #[test]
    fn a_sections_provider_drives_the_model_choice() {
        let mut form = form();
        form.fields.model_claude = "opus".to_owned();
        form.fields.model_account = "llama-3.3-70b".to_owned();
        assert_eq!(
            form.request().model.as_deref(),
            Some("opus"),
            "no account section, so the family list wins"
        );

        form.sections.push(Box::new(Stub { rows: 1, seen: Vec::new() }));
        assert_eq!(form.provider(), Some("fireworks-compatible"));
        assert_eq!(
            form.request().model.as_deref(),
            Some("llama-3.3-70b"),
            "a compatible endpoint takes the account's own model"
        );
    }

    #[test]
    fn focus_comes_back_inside_after_a_section_shrinks() {
        let mut form = form();
        form.sections.push(Box::new(Stub { rows: 2, seen: Vec::new() }));
        form.focus = Focus { section: 1, row: 1 };
        form.sections[1] = Box::new(Stub { rows: 0, seen: Vec::new() });
        form.settle_focus();
        assert_eq!(form.focus, Focus { section: 0, row: 0 });
    }

    #[test]
    fn editing_clears_the_errors_from_the_last_submit() {
        let mut form = form();
        form.errors = vec!["machine offline".to_owned()];
        form.handle_key(key('a'));
        assert!(form.errors.is_empty());
    }
}

#[derive(Debug, Clone)]
pub enum SpawnAction {
    Open,
    Close,
    NextField,
    PrevField,
    Submit,
    Failed(String),
    Key(KeyEvent),
    /// The clock tick: fires the debounced git lookup when it comes due.
    Tick,
    GitInfo {
        machine_id: String,
        path: String,
        info: Option<Box<cctui_proto::git::GitInfo>>,
    },
    DirsLoaded(Vec<String>),
    RecentDirsLoaded(Vec<String>),
    /// Down/up over the recent-dirs and completion dropdown.
    DirPick(i32),
    /// Enter on the dropdown.
    DirAccept,
    ModelsLoaded(Box<cctui_proto::harness_models::HarnessModels>),
    /// `Ctrl+r` in the dialog.
    RefreshModels,
    ModelsRefreshed,
}

pub fn reduce(app: &mut super::state::App, action: SpawnAction) -> Vec<Effect> {
    match action {
        SpawnAction::Open => open(app),
        SpawnAction::Close => {
            app.spawn = None;
            app.router.pop();
            Vec::new()
        }
        SpawnAction::NextField => {
            // On the Dir row, Tab completes the path first and only moves on
            // when there is nothing left to complete — what the ticket asks for
            // and what the web UI's dir field does.
            if let Some(effects) = complete_dir(app) {
                return effects;
            }
            if let Some(form) = app.spawn.as_mut() {
                form.step_focus(1);
            }
            Vec::new()
        }
        SpawnAction::PrevField => {
            if let Some(form) = app.spawn.as_mut() {
                form.step_focus(-1);
            }
            Vec::new()
        }
        SpawnAction::Key(key) => {
            let mut form = app.spawn.take();
            let effects = form.as_mut().map_or_else(Vec::new, |f| f.handle_key(key));
            app.spawn = form;
            effects
        }
        SpawnAction::Tick => {
            let Some(form) = app.spawn.as_mut() else { return Vec::new() };
            let (machine, path) = (form.fields.machine_id.clone(), form.fields.working_dir.clone());
            form.cwd.on_tick(app.clock_ms, &machine, &path)
        }
        SpawnAction::GitInfo { machine_id, path, info } => {
            if let Some(form) = app.spawn.as_mut() {
                match info {
                    Some(info) => form.cwd.git_loaded(&machine_id, &path, Some(&info)),
                    None => form.cwd.git_failed(&machine_id, &path),
                }
            }
            Vec::new()
        }
        SpawnAction::DirsLoaded(dirs) => {
            if let Some(form) = app.spawn.as_mut() {
                form.cwd.completions = dirs;
            }
            Vec::new()
        }
        SpawnAction::RecentDirsLoaded(dirs) => {
            if let Some(form) = app.spawn.as_mut() {
                form.cwd.recent = dirs;
            }
            Vec::new()
        }
        SpawnAction::DirPick(delta) => {
            if let Some(form) = app.spawn.as_mut() {
                form.cwd.step(delta);
            }
            Vec::new()
        }
        SpawnAction::DirAccept => {
            let Some(form) = app.spawn.as_mut() else { return Vec::new() };
            if let Some(pick) = form.cwd.selected().map(str::to_owned) {
                form.fields.working_dir = pick;
                form.cwd.close();
                form.cwd.on_edit(app.clock_ms);
            }
            Vec::new()
        }
        SpawnAction::ModelsLoaded(models) => {
            if let Some(form) = app.spawn.as_mut() {
                form.models = Some(models);
                form.refreshing_models = false;
                form.sync_options();
            }
            Vec::new()
        }
        SpawnAction::RefreshModels => refresh_models(app),
        SpawnAction::ModelsRefreshed => {
            let Some(form) = app.spawn.as_mut() else { return Vec::new() };
            form.refreshing_models = false;
            form.fetch_models().into_iter().collect()
        }
        SpawnAction::Submit => submit(app),
        SpawnAction::Failed(reason) => {
            if let Some(form) = app.spawn.as_mut() {
                form.submitting = false;
                form.errors = vec![reason];
            }
            Vec::new()
        }
    }
}

fn open(app: &mut super::state::App) -> Vec<Effect> {
    use super::state::View;
    if app.view() != View::SessionList {
        return Vec::new();
    }
    let mut form = SpawnForm::new();
    // Seeded from the row in front of you: the machine and checkout you were
    // just looking at are nearly always the ones you want.
    if let Some(session) = app.selected_session() {
        form.fields.machine_id.clone_from(&session.machine_id);
        form.fields.working_dir.clone_from(&session.working_dir);
    }
    let mut effects = vec![Effect::FetchRecentDirs];
    effects.extend(form.fetch_models());
    app.spawn = Some(form);
    app.router.push(View::Spawn);
    effects
}

/// Only codex has a catalog to re-read upstream; every other harness just
/// asks the server again.
fn refresh_models(app: &mut super::state::App) -> Vec<Effect> {
    let Some(form) = app.spawn.as_mut() else { return Vec::new() };
    if form.fields.machine_id.is_empty() || form.refreshing_models {
        return Vec::new();
    }
    if form.fields.adapter_id != "codex" {
        return form.fetch_models().into_iter().collect();
    }
    form.refreshing_models = true;
    vec![Effect::RefreshCodexModels { machine_id: form.fields.machine_id.clone() }]
}

/// `Some` when Tab was consumed by dir completion.
fn complete_dir(app: &mut super::state::App) -> Option<Vec<Effect>> {
    let form = app.spawn.as_mut()?;
    if !form.on_dir_row() {
        return None;
    }
    let path = form.fields.working_dir.clone();
    match cwd::complete(&form.cwd, &path) {
        cwd::Complete::Replace(full) => {
            form.fields.working_dir = full;
            form.cwd.on_edit(app.clock_ms);
            Some(Vec::new())
        }
        cwd::Complete::Fetch => Some(vec![Effect::FetchMachineDirs {
            machine_id: form.fields.machine_id.clone(),
            path,
        }]),
        // Nothing more to complete: let Tab do its usual job.
        cwd::Complete::Ambiguous | cwd::Complete::None => None,
    }
}

fn submit(app: &mut super::state::App) -> Vec<Effect> {
    let Some(form) = app.spawn.as_mut() else { return Vec::new() };
    let problems = form.problems();
    if !problems.is_empty() {
        form.errors = problems;
        return Vec::new();
    }
    form.submitting = true;
    vec![Effect::SpawnSession { request: Box::new(form.request()) }]
}

#[cfg(test)]
mod reduce_tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{SpawnAction, reduce};
    use crate::app::action::Effect;
    use crate::app::state::{App, View};
    use crate::testsupport::session;

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![session("s-a", "alpha", "active", "working")];
        app.update_aggregates();
        app
    }

    #[test]
    fn opening_seeds_the_machine_and_dir_from_the_row_in_front_of_you() {
        let mut app = app();
        reduce(&mut app, SpawnAction::Open);
        assert_eq!(app.view(), View::Spawn);
        let form = app.spawn.as_ref().expect("a form");
        assert_eq!(form.fields.machine_id, "orion");
        assert_eq!(form.fields.working_dir, "/home/dev/alpha");
        assert_eq!(form.fields.adapter_id, "claude-code");
    }

    #[test]
    fn the_dialog_only_opens_from_the_list() {
        let mut app = app();
        app.router.push(View::Conversation);
        reduce(&mut app, SpawnAction::Open);
        assert!(app.spawn.is_none());
    }

    #[test]
    fn closing_drops_the_form_and_returns_to_the_list() {
        let mut app = app();
        reduce(&mut app, SpawnAction::Open);
        reduce(&mut app, SpawnAction::Close);
        assert!(app.spawn.is_none());
        assert_eq!(app.view(), View::SessionList);
    }

    #[test]
    fn a_launch_with_an_empty_dir_reports_inline_instead_of_sending() {
        let mut app = app();
        reduce(&mut app, SpawnAction::Open);
        app.spawn.as_mut().expect("a form").fields.working_dir.clear();
        assert!(reduce(&mut app, SpawnAction::Submit).is_empty());
        let form = app.spawn.as_ref().expect("a form");
        assert!(form.errors.iter().any(|e| e.contains("working directory")));
        assert!(!form.submitting);
    }

    #[test]
    fn a_complete_form_sends_one_spawn_and_marks_itself_in_flight() {
        let mut app = app();
        reduce(&mut app, SpawnAction::Open);
        match reduce(&mut app, SpawnAction::Submit).as_slice() {
            [Effect::SpawnSession { request }] => {
                assert_eq!(request.machine_id, "orion");
                assert_eq!(request.working_dir, "/home/dev/alpha");
                assert_eq!(request.adapter_id.as_deref(), Some("claude-code"));
            }
            other => panic!("expected one spawn, got {} effects", other.len()),
        }
        assert!(app.spawn.as_ref().expect("a form").submitting);
    }

    #[test]
    fn tab_walks_the_fields_and_a_key_reaches_the_focused_one() {
        let mut app = app();
        reduce(&mut app, SpawnAction::Open);
        // Nothing the Dir row can complete, so Tab walks past it as usual.
        app.spawn.as_mut().expect("a form").cwd.completions = vec!["/nowhere".to_owned()];
        reduce(&mut app, SpawnAction::NextField);
        reduce(&mut app, SpawnAction::NextField);
        // Machine, Dir, Name: the third row is the name field.
        for c in "fix".chars() {
            reduce(&mut app, SpawnAction::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)));
        }
        assert_eq!(app.spawn.as_ref().expect("a form").fields.name, "fix");
    }

    #[test]
    fn tab_on_the_dir_row_completes_before_it_moves_on() {
        let mut app = app();
        reduce(&mut app, SpawnAction::Open);
        reduce(&mut app, SpawnAction::NextField);
        assert!(app.spawn.as_ref().expect("a form").on_dir_row());

        // Nothing held yet: Tab asks the machine and focus stays put.
        match reduce(&mut app, SpawnAction::NextField).as_slice() {
            [Effect::FetchMachineDirs { machine_id, path }] => {
                assert_eq!(machine_id, "orion");
                assert_eq!(path, "/home/dev/alpha");
            }
            other => panic!("expected a dir listing, got {} effects", other.len()),
        }
        assert!(app.spawn.as_ref().expect("a form").on_dir_row(), "focus did not move");

        // With one offer, Tab finishes the path.
        reduce(&mut app, SpawnAction::DirsLoaded(vec!["/home/dev/alpha-worktree".to_owned()]));
        reduce(&mut app, SpawnAction::NextField);
        let form = app.spawn.as_ref().expect("a form");
        assert_eq!(form.fields.working_dir, "/home/dev/alpha-worktree");
        assert!(form.on_dir_row());
    }

    #[test]
    fn the_dropdown_accepts_a_recent_dir_and_rearms_the_badge() {
        let mut app = app();
        app.clock_ms = 500;
        reduce(&mut app, SpawnAction::Open);
        reduce(
            &mut app,
            SpawnAction::RecentDirsLoaded(vec!["/srv/one".to_owned(), "/srv/two".to_owned()]),
        );
        reduce(&mut app, SpawnAction::DirPick(1));
        reduce(&mut app, SpawnAction::DirPick(1));
        reduce(&mut app, SpawnAction::DirAccept);
        let form = app.spawn.as_ref().expect("a form");
        assert_eq!(form.fields.working_dir, "/srv/two");
        assert!(form.cwd.picking.is_none(), "the dropdown closes");
        assert!(form.cwd.due_ms.is_some(), "and the badge lookup is re-armed");
    }

    #[test]
    fn the_badge_lookup_goes_out_once_the_debounce_passes() {
        let mut app = app();
        app.clock_ms = 1_000;
        reduce(&mut app, SpawnAction::Open);
        app.spawn.as_mut().expect("a form").cwd.on_edit(app.clock_ms);
        assert!(reduce(&mut app, SpawnAction::Tick).is_empty());

        app.clock_ms += super::cwd::GIT_DEBOUNCE_MS;
        match reduce(&mut app, SpawnAction::Tick).as_slice() {
            [Effect::FetchGitInfo { machine_id, path }] => {
                assert_eq!(machine_id, "orion");
                assert_eq!(path, "/home/dev/alpha");
            }
            other => panic!("expected one lookup, got {} effects", other.len()),
        }
    }

    /// The lists `GET /models/codex` answers with for a machine whose codex is
    /// too old for one of them.
    fn codex_lists() -> cctui_proto::harness_models::HarnessModels {
        use cctui_proto::harness_models::{HarnessModels, ModelHint, ModelOption};
        HarnessModels {
            harness: "codex".to_owned(),
            models: vec![
                ModelOption {
                    v: String::new(),
                    label: "Default".to_owned(),
                    hint: None,
                    disabled: false,
                },
                ModelOption {
                    v: "gpt-5.6-sol".to_owned(),
                    label: "GPT-5.6 Sol".to_owned(),
                    hint: None,
                    disabled: false,
                },
                ModelOption {
                    v: "gpt-6-preview".to_owned(),
                    label: "GPT-6 preview".to_owned(),
                    hint: Some(ModelHint::Gated {
                        version: "0.200.0".to_owned(),
                        current: "0.150.0".to_owned(),
                    }),
                    disabled: true,
                },
            ],
            efforts: vec![String::new(), "high".to_owned()],
        }
    }

    #[test]
    fn opening_asks_for_the_recent_dirs_and_the_machines_model_lists() {
        let mut app = app();
        match reduce(&mut app, SpawnAction::Open).as_slice() {
            [
                Effect::FetchRecentDirs,
                Effect::FetchHarnessModels { want, harness, machine_id, .. },
            ] => {
                assert_eq!(*want, crate::app::action::ModelsFor::SpawnDialog);
                assert_eq!(harness, "claude-code");
                assert_eq!(machine_id, "orion");
            }
            other => panic!("expected both fetches, got {} effects", other.len()),
        }
    }

    #[test]
    fn switching_harness_asks_for_that_harnesss_lists() {
        let mut app = app();
        reduce(&mut app, SpawnAction::Open);
        let form = app.spawn.as_mut().expect("a form");
        let harness_row = super::core_section::rows_for("claude-code")
            .iter()
            .position(|r| *r == super::core_section::Row::Harness)
            .expect("a harness row");
        form.focus = super::Focus { section: 0, row: harness_row };
        let effects = form.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        match effects.as_slice() {
            [Effect::FetchHarnessModels { harness, .. }] => assert_eq!(harness, "codex"),
            other => panic!("expected a refetch, got {} effects", other.len()),
        }
    }

    #[test]
    fn a_gated_model_cannot_be_stepped_onto() {
        let mut app = app();
        reduce(&mut app, SpawnAction::Open);
        let form = app.spawn.as_mut().expect("a form");
        form.fields.adapter_id = "codex".to_owned();
        reduce(&mut app, SpawnAction::ModelsLoaded(Box::new(codex_lists())));

        let form = app.spawn.as_mut().expect("a form");
        let gated: Vec<String> =
            form.model_options().iter().filter(|o| o.disabled).map(|o| o.v.clone()).collect();
        assert_eq!(gated, ["gpt-6-preview"], "the newer model is gated by the client version");

        let model_row = super::core_section::rows_for("codex")
            .iter()
            .position(|r| *r == super::core_section::Row::Model)
            .expect("a model row");
        form.focus = super::Focus { section: 0, row: model_row };
        let mut seen = Vec::new();
        for _ in 0..6 {
            form.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
            seen.push(form.fields.model_codex.clone());
        }
        assert!(seen.contains(&"gpt-5.6-sol".to_owned()));
        assert!(!seen.iter().any(|m| m == "gpt-6-preview"), "stepping skips the gated model");
    }

    #[test]
    fn a_remembered_model_the_lists_dropped_stays_selectable() {
        let mut app = app();
        reduce(&mut app, SpawnAction::Open);
        app.spawn.as_mut().expect("a form").fields.adapter_id = "codex".to_owned();
        reduce(&mut app, SpawnAction::ModelsLoaded(Box::new(codex_lists())));
        let form = app.spawn.as_mut().expect("a form");
        form.fields.model_codex = "gpt-5.1-retired".to_owned();
        let last = form.model_options().last().cloned().expect("an option");
        assert_eq!(last.v, "gpt-5.1-retired");
        assert!(!last.disabled);
    }

    #[test]
    fn refreshing_the_catalog_asks_upstream_then_re_reads_the_lists() {
        let mut app = app();
        reduce(&mut app, SpawnAction::Open);
        app.spawn.as_mut().expect("a form").fields.adapter_id = "codex".to_owned();
        match reduce(&mut app, SpawnAction::RefreshModels).as_slice() {
            [Effect::RefreshCodexModels { machine_id }] => assert_eq!(machine_id, "orion"),
            other => panic!("expected a refresh, got {} effects", other.len()),
        }
        assert!(app.spawn.as_ref().expect("a form").refreshing_models);
        assert!(
            reduce(&mut app, SpawnAction::RefreshModels).is_empty(),
            "not while one is in flight"
        );

        match reduce(&mut app, SpawnAction::ModelsRefreshed).as_slice() {
            [Effect::FetchHarnessModels { harness, .. }] => assert_eq!(harness, "codex"),
            other => panic!("expected a re-read, got {} effects", other.len()),
        }
        assert!(!app.spawn.as_ref().expect("a form").refreshing_models);
    }

    #[test]
    fn lists_for_another_harness_are_ignored_in_favour_of_the_static_ones() {
        let mut app = app();
        reduce(&mut app, SpawnAction::Open);
        reduce(&mut app, SpawnAction::ModelsLoaded(Box::new(codex_lists())));
        let form = app.spawn.as_ref().expect("a form");
        assert_eq!(form.fields.adapter_id, "claude-code");
        assert!(
            form.model_options().iter().any(|o| o.v == "opus"),
            "the static claude list stands in until its own lists land"
        );
        assert!(!form.effort_options().is_empty());
    }
}

/// The seams Q2–Q4 build against. These are the published contract, so they are
/// exercised here rather than waiting for the lane that consumes them.
#[cfg(test)]
mod contract_tests {
    use super::{SpawnForm, form_snapshot, open_prefilled};
    use crate::app::state::{App, View};
    use crate::testsupport::session;

    /// `SpawnRequest` has no `PartialEq`; the wire form is what matters anyway.
    fn json(request: &cctui_proto::api::SpawnRequest) -> serde_json::Value {
        serde_json::to_value(request).expect("a request serialises")
    }

    fn request() -> cctui_proto::api::SpawnRequest {
        let mut form = SpawnForm::new();
        form.fields.machine_id = "cyberia".to_owned();
        form.fields.working_dir = "/srv/work".to_owned();
        form.fields.adapter_id = "codex".to_owned();
        form.fields.model_codex = "gpt-5.6-sol".to_owned();
        form.fields.effort_codex = "high".to_owned();
        form.fields.service_tier = "fast".to_owned();
        form.fields.name = "nightly".to_owned();
        form.fields.prompt = "run the suite".to_owned();
        form.fields.labels = vec!["l-1".to_owned()];
        form.request()
    }

    #[test]
    fn a_request_round_trips_through_prefill() {
        let original = request();
        let mut form = SpawnForm::new();
        form.prefill(&original);
        assert_eq!(form.fields.machine_id, "cyberia");
        assert_eq!(form.fields.working_dir, "/srv/work");
        assert_eq!(form.fields.adapter_id, "codex");
        assert_eq!(form.fields.model_codex, "gpt-5.6-sol", "the codex field, not the claude one");
        assert!(form.fields.model_claude.is_empty());
        assert_eq!(form.fields.effort_codex, "high");
        assert_eq!(form.fields.service_tier, "fast");
        assert_eq!(form.fields.name, "nightly");
        assert_eq!(form.fields.prompt, "run the suite");
        assert_eq!(form.fields.labels, ["l-1"]);
        assert_eq!(json(&form.request()), json(&original), "and rebuilds the same request");
    }

    #[test]
    fn prefill_routes_a_claude_model_to_the_claude_field() {
        let mut form = SpawnForm::new();
        let mut request = request();
        request.adapter_id = Some("claude-code".to_owned());
        request.model = Some("opus".to_owned());
        form.prefill(&request);
        assert_eq!(form.fields.model_claude, "opus");
        assert!(form.fields.model_codex.is_empty());
    }

    #[test]
    fn form_snapshot_is_none_until_the_dialog_is_open() {
        let mut app = App::new();
        app.sessions = vec![session("s-a", "alpha", "active", "working")];
        app.update_aggregates();
        assert!(form_snapshot(&app).is_none());

        super::reduce(&mut app, super::SpawnAction::Open);
        let snapshot = form_snapshot(&app).expect("a snapshot once it is open");
        assert_eq!(snapshot.machine_id, "orion");
    }

    #[test]
    fn open_prefilled_opens_the_dialog_seeded_from_a_request() {
        let mut app = App::new();
        let original = request();
        open_prefilled(&mut app, &original);
        assert_eq!(app.view(), View::Spawn);
        let form = app.spawn.as_ref().expect("a form");
        assert_eq!(form.fields.machine_id, "cyberia");
        assert_eq!(json(&form_snapshot(&app).expect("a snapshot")), json(&original));
    }
}
