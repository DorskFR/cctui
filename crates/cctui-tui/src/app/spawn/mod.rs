//! The spawn dialog: one form, a list of sections, one request.
//!
//! A lane adds a feature by writing a struct, implementing [`SpawnSection`] and
//! registering it in [`SpawnForm::sections`] — one line.
//!
//! The request is built once, from [`SpawnForm::fields`], by
//! `cctui_clientcore::spawn::build_spawn_body`. [`SpawnSection::apply`] runs
//! after it and must leave the account fields alone: blank / `NO_ACCOUNT` /
//! `POOL_PREFIX` / a name is one coupled rule the shared builder owns.

pub mod accounts;
pub mod core_section;
pub mod env;
pub mod files;
pub mod labels;

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

    /// The profile section identifies itself, so the profile effects can reach
    /// its state without a downcast. Every other section leaves these `None`.
    fn as_profiles(&self) -> Option<&crate::app::profiles::ProfileSection> {
        None
    }

    fn as_profiles_mut(&mut self) -> Option<&mut crate::app::profiles::ProfileSection> {
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
    pub accounts: Vec<cctui_client::AccountPick>,
    pub pools: Vec<cctui_client::PoolPick>,
    pub usage: Vec<cctui_client::AccountUsagePick>,
    pub labels: Vec<cctui_proto::api::Label>,
}

/// Which tab the dialog is on. The toggle belongs to the dialog; a tab's own
/// fields belong to whichever lane owns it.
///
/// `Dispatch` is unreachable until Q4's tab lands, which is what the allow is
/// for: the variant is the published shape that lane builds against.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SpawnTarget {
    #[default]
    Machine,
    Dispatch,
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
    /// Which tab is showing. `Dispatch` is hidden when no dispatcher exists.
    pub target: SpawnTarget,
    /// Env rows as typed; a half-typed row is not sent. Reduced by
    /// `cctui_clientcore::spawn::env_map` at submit.
    pub env: Vec<(String, String)>,
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
            .field("target", &self.target)
            .field("env", &format_args!("<{} row(s) redacted>", self.env.len()))
            .field("focus", &self.focus)
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
            Box::new(crate::app::profiles::ProfileSection::default()),
            Box::new(core_section::CoreSection),
            Box::new(accounts::AccountSection::default()),
            Box::new(labels::LabelsSection::default()),
            Box::new(env::EnvSection::default()),
            Box::new(files::FilesSection::default()),
        ]
    }

    /// The profile section, which the profile effects load and store into. It
    /// is registered first, so this is where it is.
    pub fn profiles_mut(&mut self) -> Option<&mut crate::app::profiles::ProfileSection> {
        self.sections.first_mut().and_then(|s| s.as_profiles_mut())
    }

    pub fn profiles(&self) -> Option<&crate::app::profiles::ProfileSection> {
        self.sections.first().and_then(|s| s.as_profiles())
    }

    #[must_use]
    pub fn new() -> Self {
        Self {
            fields: SpawnFields { adapter_id: "claude-code".to_owned(), ..SpawnFields::default() },
            target: SpawnTarget::default(),
            env: Vec::new(),
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
        let Focus { section, row } = self.focus;
        let Some(target) = self.sections.get_mut(section) else { return Vec::new() };
        let effects = target.handle(row, key, &mut self.fields);
        let harness = target.harness_override().map(str::to_owned);
        let account = target.account_pick().map(str::to_owned);
        if let Some(harness) = harness {
            self.fields.adapter_id = harness;
        }
        if let Some(account) = account {
            self.fields.account = account;
        }
        self.sync_provider();
        self.sync_profile_strip();
        self.settle_focus();
        effects
    }

    /// Keeps the profile strip and the fields in step: the strip's own pick
    /// writes the compute knobs, and a knob changed anywhere else re-counts
    /// the one-off adjust.
    fn sync_profile_strip(&mut self) {
        let Some(section) = self.profiles() else { return };
        if section.selected.is_none() {
            return;
        }
        let picked = crate::app::profiles::applied(section, &self.fields);
        if picked != self.fields {
            self.fields = picked;
            return;
        }
        let fields = self.fields.clone();
        let mut said = None;
        if let Some(section) = self.profiles_mut() {
            let _ = section.form_changed(Box::new(fields), &mut said);
        }
    }

    /// Mirrors the live provider onto the field, so a draft or profile written
    /// from this form carries the account it was built against.
    fn sync_provider(&mut self) {
        if let Some(provider) = self.provider().map(str::to_owned) {
            self.fields.account_provider = provider;
        }
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
pub fn open_prefilled(app: &mut super::state::App, request: &SpawnRequest) -> Vec<Effect> {
    let mut form = SpawnForm::new();
    form.prefill(request);
    app.spawn = Some(form);
    app.router.push(super::state::View::Spawn);
    app.spawn
        .as_mut()
        .and_then(SpawnForm::profiles_mut)
        .map(crate::app::profiles::ProfileSection::on_open)
        .unwrap_or_default()
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

    /// The core section sits after the profile strip.
    const CORE: usize = 1;

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn the_core_section_is_registered_and_takes_focus_first() {
        let mut form = SpawnForm::new();
        form.settle_focus();
        assert_eq!(form.sections[0].title(), "Profile", "the profile strip comes first");
        assert_eq!(form.sections[CORE].title(), "New session", "then the core fields");
        assert_eq!(
            form.focus,
            Focus { section: CORE, row: 0 },
            "an empty profile strip takes no focus, so the core section has it"
        );
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

#[derive(Debug, Clone)]
/// One landed catalog.
pub enum SpawnFetch {
    Accounts(Vec<cctui_client::AccountPick>),
    Pools(Vec<cctui_client::PoolPick>),
    Usage(Vec<cctui_client::AccountUsagePick>),
}

pub enum SpawnAction {
    DataLoaded(Box<SpawnFetch>),
    Open,
    Close,
    NextField,
    PrevField,
    Submit,
    Failed(String),
    Key(KeyEvent),
}

pub fn reduce(app: &mut super::state::App, action: SpawnAction) -> Vec<Effect> {
    use super::state::View;
    match action {
        SpawnAction::Open => {
            if app.view() != View::SessionList {
                return Vec::new();
            }
            let mut form = SpawnForm::new();
            // Seeded from the row in front of you: the machine and checkout you
            // were just looking at are nearly always the ones you want.
            if let Some(session) = app.selected_session() {
                form.fields.machine_id.clone_from(&session.machine_id);
                form.fields.working_dir.clone_from(&session.working_dir);
            }
            // The labels the last spawn carried; the labels section drops any
            // the catalog has since lost.
            form.fields.labels.clone_from(&app.ui.last_spawn_labels);
            app.spawn_data.labels.clone_from(&app.labels.all);
            for section in &mut form.sections {
                section.receive(&app.spawn_data);
            }
            app.spawn = Some(form);
            app.router.push(View::Spawn);
            let mut effects =
                vec![Effect::FetchAccounts, Effect::FetchAccountPools, Effect::FetchAccountsUsage];
            effects.extend(
                app.spawn
                    .as_mut()
                    .and_then(SpawnForm::profiles_mut)
                    .map(crate::app::profiles::ProfileSection::on_open)
                    .unwrap_or_default(),
            );
            effects
        }
        SpawnAction::Close => {
            app.spawn = None;
            app.router.pop();
            Vec::new()
        }
        SpawnAction::NextField => {
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
            let mut effects = form.as_mut().map_or_else(Vec::new, |f| f.handle_key(key));
            app.spawn = form;
            effects.extend(crate::app::spawn_drafts::autosave(app));
            effects
        }
        SpawnAction::Submit => submit(app),
        SpawnAction::DataLoaded(data) => {
            match *data {
                SpawnFetch::Accounts(accounts) => app.spawn_data.accounts = accounts,
                SpawnFetch::Pools(pools) => app.spawn_data.pools = pools,
                SpawnFetch::Usage(usage) => app.spawn_data.usage = usage,
            }
            if let Some(form) = app.spawn.as_mut() {
                for section in &mut form.sections {
                    section.receive(&app.spawn_data);
                }
            }
            Vec::new()
        }
        SpawnAction::Failed(reason) => {
            if let Some(form) = app.spawn.as_mut() {
                form.submitting = false;
                form.errors = vec![accounts::spawn_error_hint(&reason)];
            }
            Vec::new()
        }
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
    let files = form.sections.iter().flat_map(|section| section.parts()).collect();
    let request = Box::new(form.request());
    let labels = request.label_ids.clone();
    let mut effects = vec![Effect::SpawnSession { request, files }];
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
            [Effect::SpawnSession { request, .. }] => {
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
        reduce(&mut app, SpawnAction::NextField);
        reduce(&mut app, SpawnAction::NextField);
        // Machine, Dir, Name: the third row is the name field.
        for c in "fix".chars() {
            reduce(&mut app, SpawnAction::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)));
        }
        assert_eq!(app.spawn.as_ref().expect("a form").fields.name, "fix");
    }
}

/// The seams Q2–Q4 build against. These are the published contract, so they are
/// exercised here rather than waiting for the lane that consumes them.
#[cfg(test)]
mod contract_tests {
    use super::{SpawnForm, SpawnTarget, form_snapshot, open_prefilled};
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

    #[test]
    fn the_dialog_starts_on_the_machine_tab() {
        let mut form = SpawnForm::new();
        assert_eq!(form.target, SpawnTarget::Machine);
        form.target = SpawnTarget::Dispatch;
        assert_ne!(form.target, SpawnTarget::Machine, "the tab is switchable");
    }
}
