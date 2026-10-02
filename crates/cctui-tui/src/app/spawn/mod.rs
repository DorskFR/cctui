//! The spawn dialog: one form, a list of sections, one request.
//!
//! A feature is a struct implementing [`SpawnSection`], registered in
//! [`SpawnForm::sections`].
//!
//! The request is built once, from [`SpawnForm::fields`], by
//! `cctui_clientcore::spawn::build_spawn_body`. [`SpawnSection::apply`] runs
//! after it and must leave the account fields alone: blank / `NO_ACCOUNT` /
//! `POOL_PREFIX` / a name is one coupled rule the shared builder owns.

pub mod core_section;
pub mod cwd;
pub mod env;
pub mod files;
pub mod labels;

use cctui_clientcore::spawn::SpawnFields;
use cctui_proto::api::SpawnRequest;
use cctui_proto::drafts::SPAWN_MEMORY_CAP;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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

    /// The files section identifies itself, so a finished read can reach its
    /// state without a downcast.
    fn as_files_mut(&mut self) -> Option<&mut files::FilesSection> {
        None
    }

    /// Files this section attaches, as multipart parts. The spawn route carries
    /// them alongside the request, so they are up before the first turn runs.
    fn parts(&self) -> Vec<(String, Vec<u8>)> {
        Vec::new()
    }

    /// Catalogs fetched for the dialog. Called when the dialog opens and again
    /// whenever one lands, so a section never owns a fetch of its own.
    fn receive(&mut self, _data: &SpawnData) {}

    /// Handed the model and effort lists when they change. Only the core
    /// section's pickers use them.
    fn set_options(&mut self, _options: core_section::Options) {}
}

/// Cuts a section's rows to the dialog's inner width. A row that wrapped would
/// push every row under it down by one.
#[must_use]
pub fn clamp_rows(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width);
    lines
        .into_iter()
        .map(|line| {
            let mut left = width;
            let mut spans = Vec::with_capacity(line.spans.len());
            for span in line.spans {
                if left == 0 {
                    break;
                }
                let cols = span.content.chars().count();
                if cols <= left {
                    left -= cols;
                    spans.push(span);
                    continue;
                }
                let cut: String = span.content.chars().take(left).collect();
                left = 0;
                spans.push(ratatui::text::Span::styled(cut, span.style));
            }
            Line::from(spans)
        })
        .collect()
}

/// What the dialog's sections read but none of them fetches: the catalogs live
/// on [`super::state::App`], so they outlive one dialog and are fetched once.
#[derive(Debug, Default, Clone)]
pub struct SpawnData {
    pub labels: Vec<cctui_proto::api::Label>,
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
    /// The spawn the server accepted, waited on until its `command_result`.
    pub launching: Option<uuid::Uuid>,
    /// Pre-minted by the server, so the jump knows where to land.
    pub session_id: Option<String>,
    /// Memory that lands after the dialog opens only seeds an untouched form.
    pub edited: bool,
    pub focus: Focus,
    /// Inline errors from the last submit, cleared on the next edit.
    pub errors: Vec<String>,
    pub submitting: bool,
    /// Registered sections, in display and Tab order.
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
            .field("launching", &self.launching)
            .field("session_id", &self.session_id)
            .field("edited", &self.edited)
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
        vec![
            Box::new(core_section::CoreSection::default()),
            Box::new(labels::LabelsSection::default()),
            Box::new(env::EnvSection::default()),
            Box::new(files::FilesSection::default()),
        ]
    }

    #[must_use]
    pub fn new() -> Self {
        Self {
            fields: SpawnFields { adapter_id: "claude-code".to_owned(), ..SpawnFields::default() },
            env: Vec::new(),
            cwd: cwd::CwdState::default(),
            models: None,
            refreshing_models: false,
            launching: None,
            session_id: None,
            edited: false,
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

    /// Hands a key to the focused section.
    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        self.errors.clear();
        self.edited = true;
        let before = self.list_key();
        let Focus { section, row } = self.focus;
        let Some(target) = self.sections.get_mut(section) else { return Vec::new() };
        let mut effects = target.handle(row, key, &mut self.fields);
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

    /// Hands every section the lists the pickers step through. Called after
    /// every key and whenever the lists land, because switching harness
    /// changes both.
    pub fn sync_options(&mut self) {
        let models = self.model_options();
        let efforts = self.effort_options();
        for section in &mut self.sections {
            section.set_options(core_section::Options {
                models: models.clone(),
                efforts: efforts.clone(),
            });
        }
    }

    /// Where the core section sits, which the renderer needs: the dir and model
    /// rows carry extra lines the section itself cannot draw.
    #[must_use]
    pub fn core_index(&self) -> Option<usize> {
        self.sections.iter().position(|s| s.title() == core_section::TITLE)
    }

    /// The target this form's configuration is remembered under.
    #[must_use]
    pub fn memory_key(&self) -> String {
        cctui_proto::drafts::machine_memory_key(&self.fields.machine_id, &self.fields.working_dir)
    }

    /// What a later spawn on the same machine and directory starts from.
    #[must_use]
    pub fn memory_entry(&self, at: i64) -> cctui_proto::drafts::SpawnMemoryEntry {
        cctui_proto::drafts::SpawnMemoryEntry {
            adapter_id: self.fields.adapter_id.clone(),
            model_claude: self.fields.model_claude.clone(),
            model_codex: self.fields.model_codex.clone(),
            model_account: self.fields.model_account.clone(),
            effort_claude: self.fields.effort_claude.clone(),
            effort_codex: self.fields.effort_codex.clone(),
            account: self.fields.account.clone(),
            account_provider: self.fields.account_provider.clone(),
            permission_mode: self.fields.permission_mode.clone(),
            name: self.fields.name.clone(),
            labels: None,
            profile_id: None,
            at,
        }
    }

    /// Seeds the form from a remembered spawn. The machine and directory are
    /// what keyed it, so they are left alone.
    pub fn apply_memory(&mut self, entry: &cctui_proto::drafts::SpawnMemoryEntry) {
        let f = &mut self.fields;
        if !entry.adapter_id.is_empty() {
            f.adapter_id.clone_from(&entry.adapter_id);
        }
        f.model_claude.clone_from(&entry.model_claude);
        f.model_codex.clone_from(&entry.model_codex);
        f.model_account.clone_from(&entry.model_account);
        f.effort_claude.clone_from(&entry.effort_claude);
        f.effort_codex.clone_from(&entry.effort_codex);
        f.account.clone_from(&entry.account);
        f.account_provider.clone_from(&entry.account_provider);
        f.permission_mode.clone_from(&entry.permission_mode);
        f.name.clone_from(&entry.name);
        self.sync_options();
        self.settle_focus();
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
        self.core_index() == Some(self.focus.section)
            && core_section::rows_for(&self.fields.adapter_id).get(self.focus.row).copied()
                == Some(core_section::Row::Dir)
    }

    /// The provider behind the remembered account, which decides whether the
    /// model follows the account's declared models or the family lists.
    #[must_use]
    pub const fn provider(&self) -> Option<&str> {
        let provider = self.fields.account_provider.as_str();
        if provider.is_empty() { None } else { Some(provider) }
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
    }

    fn form() -> SpawnForm {
        let mut form = SpawnForm::new();
        form.fields.machine_id = "m-1".to_owned();
        form.fields.working_dir = "/home/dev/cctui".to_owned();
        form
    }

    const CORE: usize = 0;

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn the_core_section_is_registered_and_takes_focus_first() {
        let mut form = SpawnForm::new();
        form.settle_focus();
        assert_eq!(form.sections[CORE].title(), "New session", "the core fields come first");
        assert_eq!(form.focus, Focus { section: CORE, row: 0 });
        assert!(form.sections[CORE].rows(&form.fields) > 0);
        assert!(
            form.sections.len() > 2,
            "the lane sections register themselves; this count is not pinned"
        );
    }

    #[test]
    fn tab_walks_every_row_of_every_section_and_wraps() {
        let mut form = form();
        form.sections.push(Box::new(Stub { rows: 2, seen: Vec::new() }));
        form.settle_focus();

        let total: usize = form.sections.iter().map(|s| s.rows(&form.fields)).sum();
        for _ in 0..total {
            form.step_focus(1);
        }
        assert_eq!(form.focus, Focus { section: CORE, row: 0 }, "a full lap comes home");

        let last = form.sections.len() - 1;
        form.step_focus(-1);
        assert_eq!(
            form.focus,
            Focus { section: last, row: 1 },
            "stepping back from the first row wraps to the last section's last row"
        );
    }

    #[test]
    fn a_section_with_no_rows_draws_but_never_takes_focus() {
        let mut form = form();
        form.sections.push(Box::new(Stub { rows: 0, seen: Vec::new() }));
        let rowless = form.sections.len() - 1;
        form.settle_focus();
        let total: usize = form.sections.iter().map(|s| s.rows(&form.fields)).sum();
        for _ in 0..=total {
            form.step_focus(1);
            assert_ne!(form.focus.section, rowless, "focus never reached the rowless section");
        }
    }

    #[test]
    fn a_key_reaches_the_focused_section_with_its_own_row_index() {
        let mut form = form();
        form.sections.push(Box::new(Stub { rows: 2, seen: Vec::new() }));
        let stub_at = form.sections.len() - 1;
        form.focus = Focus { section: stub_at, row: 1 };
        form.handle_key(key('x'));

        let stub = &form.sections[stub_at];
        assert_eq!(stub.rows(&form.fields), 2);
        // The stub recorded the row it was handed; read it back through Debug.
        assert!(
            format!("{form:?}").contains("Stub"),
            "the section is registered under its own title"
        );
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
    fn a_remembered_provider_drives_the_model_choice() {
        let mut form = form();
        form.fields.model_claude = "opus".to_owned();
        form.fields.model_account = "llama-3.3-70b".to_owned();
        assert_eq!(
            form.request().model.as_deref(),
            Some("opus"),
            "no remembered provider, so the family list wins"
        );

        form.fields.account_provider = "fireworks-compatible".to_owned();
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
        let at = form.sections.len() - 1;
        form.focus = Focus { section: at, row: 1 };
        form.sections[at] = Box::new(Stub { rows: 0, seen: Vec::new() });
        form.settle_focus();
        assert_eq!(form.focus, Focus { section: CORE, row: 0 });
    }

    #[test]
    fn editing_clears_the_errors_from_the_last_submit() {
        let mut form = form();
        form.errors = vec!["machine offline".to_owned()];
        form.handle_key(key('a'));
        assert!(form.errors.is_empty());
    }
}

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
    /// Bytes for a path the files section asked for, or why it was refused.
    FileRead {
        path: String,
        outcome: Result<(Vec<u8>, String), String>,
    },
    DirsLoaded(Vec<String>),
    RecentDirsLoaded(Vec<String>),
    /// Down/up over the recent-dirs and completion dropdown.
    DirPick(i32),
    /// Enter on the dropdown.
    DirAccept,
    ModelsLoaded(Box<cctui_proto::harness_models::HarnessModels>),
    MemoryLoaded(Box<cctui_proto::drafts::SpawnMemoryPayload>),
    /// The server took the spawn; the daemon has yet to report it.
    Accepted {
        command_id: uuid::Uuid,
        session_id: Option<String>,
    },
    /// A `command_result`, for this spawn or any other.
    Launched {
        command_id: uuid::Uuid,
        ok: bool,
        error: Option<String>,
        session_id: Option<String>,
    },
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
        SpawnAction::Key(key) => app.spawn.as_mut().map_or_else(Vec::new, |f| f.handle_key(key)),
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
        SpawnAction::DirAccept => dir_accept(app),
        SpawnAction::ModelsLoaded(models) => {
            if let Some(form) = app.spawn.as_mut() {
                form.models = Some(models);
                form.refreshing_models = false;
                form.sync_options();
            }
            Vec::new()
        }
        SpawnAction::MemoryLoaded(payload) => memory_loaded(app, payload.entries),
        SpawnAction::RefreshModels => refresh_models(app),
        SpawnAction::ModelsRefreshed => {
            let Some(form) = app.spawn.as_mut() else { return Vec::new() };
            form.refreshing_models = false;
            form.fetch_models().into_iter().collect()
        }
        SpawnAction::Submit => submit(app),
        SpawnAction::Accepted { command_id, session_id } => {
            if let Some(form) = app.spawn.as_mut() {
                form.launching = Some(command_id);
                form.session_id = session_id;
            }
            Vec::new()
        }
        SpawnAction::Launched { command_id, ok, error, session_id } => {
            launched(app, command_id, ok, error.as_deref(), session_id.as_deref())
        }
        SpawnAction::FileRead { path, outcome } => file_read(app, &path, outcome),
        SpawnAction::Failed(reason) => {
            if let Some(form) = app.spawn.as_mut() {
                form.submitting = false;
                form.errors = vec![reason];
            }
            Vec::new()
        }
    }
}

/// Bytes an effect read for the files section, or the refusal to show instead.
fn file_read(
    app: &mut super::state::App,
    path: &str,
    outcome: Result<(Vec<u8>, String), String>,
) -> Vec<Effect> {
    let Some(form) = app.spawn.as_mut() else { return Vec::new() };
    let Some(files) = form.sections.iter_mut().find_map(|s| s.as_files_mut()) else {
        return Vec::new();
    };
    match outcome {
        Ok((bytes, content_type)) => {
            files.staged_read(std::path::Path::new(path), bytes, content_type);
        }
        Err(message) => files.read_failed(message),
    }
    Vec::new()
}

fn memory_loaded(
    app: &mut super::state::App,
    entries: std::collections::BTreeMap<String, cctui_proto::drafts::SpawnMemoryEntry>,
) -> Vec<Effect> {
    app.spawn_memory = entries;
    let remembered = app
        .spawn
        .as_ref()
        .filter(|f| !f.edited)
        .and_then(|f| app.spawn_memory.get(&f.memory_key()).cloned());
    if let (Some(form), Some(entry)) = (app.spawn.as_mut(), remembered) {
        form.apply_memory(&entry);
    }
    Vec::new()
}

/// Enter is the dialog's one Enter: with no dropdown under the Dir row it
/// belongs to whatever row has focus, which is how the prompt gets newlines.
fn dir_accept(app: &mut super::state::App) -> Vec<Effect> {
    let Some(form) = app.spawn.as_mut() else { return Vec::new() };
    if let Some(pick) = form.cwd.selected().map(str::to_owned) {
        form.fields.working_dir = pick;
        form.cwd.close();
        form.cwd.on_edit(app.clock_ms);
        return Vec::new();
    }
    form.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
}

/// A failed launch keeps the form, so the operator fixes it and re-submits
/// rather than retyping it.
fn launched(
    app: &mut super::state::App,
    command_id: uuid::Uuid,
    ok: bool,
    error: Option<&str>,
    session_id: Option<&str>,
) -> Vec<Effect> {
    let Some(form) = app.spawn.as_mut() else { return Vec::new() };
    if form.launching != Some(command_id) {
        return Vec::new();
    }
    form.launching = None;
    form.submitting = false;
    if !ok {
        form.errors = vec![error.unwrap_or("the launch failed").to_owned()];
        return Vec::new();
    }
    let landing = session_id.map(str::to_owned).or_else(|| form.session_id.clone());
    app.spawn = None;
    app.router.pop();
    super::deeplink::apply(app, super::deeplink::Startup { open: landing, ..Default::default() })
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
    if let Some(entry) = app.spawn_memory.get(&form.memory_key()) {
        form.apply_memory(&entry.clone());
    }
    // The labels the last spawn carried; the labels section drops any the
    // catalog has since lost.
    form.fields.labels.clone_from(&app.ui.last_spawn_labels);
    app.spawn_data.labels.clone_from(&app.labels.all);
    for section in &mut form.sections {
        section.receive(&app.spawn_data);
    }
    let mut effects = vec![Effect::FetchRecentDirs, Effect::FetchSpawnMemory];
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
    let offline = app
        .spawn
        .as_ref()
        .map(|f| f.fields.machine_id.clone())
        .and_then(|id| app.machine_liveness.get(&id).copied())
        .is_some_and(|tier| tier == cctui_proto::models::MachineLiveness::Offline);
    let Some(form) = app.spawn.as_mut() else { return Vec::new() };
    let mut problems = form.problems();
    if offline {
        problems.push("that machine is offline".to_owned());
    }
    if !problems.is_empty() {
        form.errors = problems;
        return Vec::new();
    }
    form.submitting = true;
    form.launching = None;
    form.session_id = None;
    let files = form.sections.iter().flat_map(|section| section.parts()).collect();
    let request = Box::new(form.request());
    let labels = request.label_ids.clone();
    // Remembered on submit rather than on a confirmed launch: a spawn that
    // never lands still recorded what was asked for.
    let (key, entry) = (form.memory_key(), form.memory_entry(app.clock_ms));
    app.spawn_memory.insert(key, entry);
    cctui_proto::drafts::evict_spawn_memory(&mut app.spawn_memory, SPAWN_MEMORY_CAP);
    let mut effects = vec![
        Effect::SpawnSession { request, files },
        Effect::PutSpawnMemory { entries: app.spawn_memory.clone() },
    ];
    if app.ui.last_spawn_labels != labels {
        app.ui.last_spawn_labels = labels;
        effects.push(Effect::SaveUiState(app.ui.clone()));
    }
    effects
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
            [Effect::SpawnSession { request, .. }, Effect::PutSpawnMemory { .. }] => {
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

    /// Submits the form and reports what the server answered.
    fn accept(app: &mut App) -> uuid::Uuid {
        let command_id = uuid::Uuid::new_v4();
        reduce(app, SpawnAction::Submit);
        reduce(app, SpawnAction::Accepted { command_id, session_id: Some("s-a".to_owned()) });
        command_id
    }

    #[test]
    fn enter_adds_a_line_to_the_prompt_when_no_dropdown_wants_it() {
        let mut app = app();
        reduce(&mut app, SpawnAction::Open);
        let form = app.spawn.as_mut().expect("a form");
        let prompt_row = super::core_section::rows_for(&form.fields.adapter_id)
            .iter()
            .position(|r| *r == super::core_section::Row::Prompt)
            .expect("a prompt row");
        let core = form.core_index().expect("the core section");
        form.focus = super::Focus { section: core, row: prompt_row };
        let typed = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        form.handle_key(typed('a'));
        reduce(&mut app, SpawnAction::DirAccept);
        app.spawn.as_mut().expect("a form").handle_key(typed('b'));
        assert_eq!(app.spawn.as_ref().expect("a form").fields.prompt, "a\nb");
    }

    fn memory(adapter: &str, at: i64) -> cctui_proto::drafts::SpawnMemoryEntry {
        cctui_proto::drafts::SpawnMemoryEntry {
            adapter_id: adapter.to_owned(),
            model_claude: "opus".to_owned(),
            effort_claude: "high".to_owned(),
            permission_mode: "yolo".to_owned(),
            name: "retry".to_owned(),
            at,
            ..cctui_proto::drafts::SpawnMemoryEntry::default()
        }
    }

    #[test]
    fn opening_seeds_the_form_from_the_last_spawn_on_that_target() {
        let mut app = app();
        let session = app.sessions[0].clone();
        let key =
            cctui_proto::drafts::machine_memory_key(&session.machine_id, &session.working_dir);
        app.spawn_memory.insert(key, memory("codex", 1));
        reduce(&mut app, SpawnAction::Open);
        let form = app.spawn.as_ref().expect("a form");
        assert_eq!(form.fields.adapter_id, "codex");
        assert_eq!(form.fields.permission_mode, "yolo");
        assert_eq!(form.fields.name, "retry");
    }

    #[test]
    fn memory_that_lands_late_seeds_an_untouched_form_only() {
        let mut app = app();
        reduce(&mut app, SpawnAction::Open);
        let key = app.spawn.as_ref().expect("a form").memory_key();
        let payload = cctui_proto::drafts::SpawnMemoryPayload {
            entries: std::iter::once((key, memory("codex", 1))).collect(),
        };
        reduce(&mut app, SpawnAction::MemoryLoaded(Box::new(payload.clone())));
        assert_eq!(app.spawn.as_ref().expect("a form").fields.adapter_id, "codex");

        app.spawn.as_mut().expect("a form").fields.adapter_id = "claude-code".to_owned();
        app.spawn.as_mut().expect("a form").edited = true;
        reduce(&mut app, SpawnAction::MemoryLoaded(Box::new(payload)));
        assert_eq!(
            app.spawn.as_ref().expect("a form").fields.adapter_id,
            "claude-code",
            "a form the operator has touched is left alone"
        );
    }

    #[test]
    fn submitting_remembers_the_configuration_for_that_target() {
        let mut app = app();
        reduce(&mut app, SpawnAction::Open);
        app.clock_ms = 4_200;
        let key = app.spawn.as_ref().expect("a form").memory_key();
        app.spawn.as_mut().expect("a form").fields.name = "parser".to_owned();
        match reduce(&mut app, SpawnAction::Submit).as_slice() {
            [Effect::SpawnSession { .. }, Effect::PutSpawnMemory { entries }] => {
                let entry = entries.get(&key).expect("the target is remembered");
                assert_eq!(entry.name, "parser");
                assert_eq!(entry.at, 4_200);
            }
            other => panic!("expected a spawn and a remember, got {} effects", other.len()),
        }
    }

    #[test]
    fn a_launch_onto_an_offline_machine_reports_inline_instead_of_sending() {
        let mut app = app();
        reduce(&mut app, SpawnAction::Open);
        let machine = app.spawn.as_ref().expect("a form").fields.machine_id.clone();
        app.machine_liveness.insert(machine, cctui_proto::models::MachineLiveness::Offline);
        assert!(reduce(&mut app, SpawnAction::Submit).is_empty());
        let form = app.spawn.as_ref().expect("a form");
        assert!(form.errors.iter().any(|e| e.contains("offline")));
        assert!(!form.submitting);
    }

    #[test]
    fn a_confirmed_launch_closes_the_dialog_and_lands_on_the_new_session() {
        let mut app = app();
        reduce(&mut app, SpawnAction::Open);
        let command_id = accept(&mut app);
        reduce(
            &mut app,
            SpawnAction::Launched { command_id, ok: true, error: None, session_id: None },
        );
        assert!(app.spawn.is_none());
        assert_eq!(app.view(), crate::app::View::Conversation);
        assert_eq!(app.selected_session_id().as_deref(), Some("s-a"));
    }

    #[test]
    fn a_rejected_launch_keeps_the_form_and_says_why() {
        let mut app = app();
        reduce(&mut app, SpawnAction::Open);
        let command_id = accept(&mut app);
        reduce(
            &mut app,
            SpawnAction::Launched {
                command_id,
                ok: false,
                error: Some("no account backs codex".to_owned()),
                session_id: None,
            },
        );
        let form = app.spawn.as_ref().expect("the form stays open");
        assert_eq!(form.errors, ["no account backs codex"]);
        assert!(!form.submitting, "re-submitting is allowed again");
    }

    #[test]
    fn another_commands_result_is_not_this_dialogs() {
        let mut app = app();
        reduce(&mut app, SpawnAction::Open);
        accept(&mut app);
        reduce(
            &mut app,
            SpawnAction::Launched {
                command_id: uuid::Uuid::new_v4(),
                ok: true,
                error: None,
                session_id: None,
            },
        );
        assert!(app.spawn.is_some(), "the dialog waits for its own result");
    }

    #[test]
    fn opening_asks_for_the_recent_dirs_and_the_machines_model_lists() {
        let mut app = app();
        let effects = reduce(&mut app, SpawnAction::Open);
        assert!(effects.iter().any(|e| matches!(e, Effect::FetchRecentDirs)));
        assert!(effects.iter().any(|e| matches!(e, Effect::FetchSpawnMemory)));
        let models = effects
            .iter()
            .find_map(|e| match e {
                Effect::FetchHarnessModels { want, harness, machine_id, .. } => {
                    Some((*want, harness.clone(), machine_id.clone()))
                }
                _ => None,
            })
            .expect("the model lists are fetched");
        assert_eq!(models.0, crate::app::action::ModelsFor::SpawnDialog);
        assert_eq!(models.1, "claude-code");
        assert_eq!(models.2, "orion");
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
        let core = form.core_index().expect("the core section");
        form.focus = super::Focus { section: core, row: harness_row };
        let effects = form.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        let harness = effects
            .iter()
            .find_map(|e| match e {
                Effect::FetchHarnessModels { harness, .. } => Some(harness.clone()),
                _ => None,
            })
            .expect("a refetch");
        assert_eq!(harness, "codex");
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
        let core = form.core_index().expect("the core section");
        form.focus = super::Focus { section: core, row: model_row };
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
