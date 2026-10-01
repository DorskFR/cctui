//! The instance panel: what this deployment is running, what is available, and
//! the server's own self-update.
//!
//! This is the *server's* update, not the TUI binary's (`selfupdate.rs`). The
//! trigger is admin-scoped server-side, so the panel refuses locally for the
//! same reason: a non-admin never sees the panel at all, which is also what a
//! 403 on the status request means.
//!
//! Once a hook run is in flight the panel polls it until the run reports a
//! terminal phase, then stops — the same rule the web UI's modal follows.

use cctui_client::{ReleaseNote, SelfUpdateLaunch, SelfUpdateRun, VersionInfo};
use cctui_clientcore::instance::{
    PhaseTone, badge_message, can_launch, confirm_message, hint_message, message_text,
    phase_message, phase_tone, update_available,
};
use cctui_proto::release_sig::Channel;

use super::action::Effect;
use super::state::App;
use super::toast::Level;

/// The scope the server requires to launch an update.
pub const ADMIN_SCOPE: &str = "admin";

/// How often a run in flight is polled.
pub const POLL_MS: i64 = 3_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Browse,
    /// The consent step; what it promises depends on `self_update_hook`.
    Confirm,
}

#[derive(Debug, Default)]
pub struct Instance {
    pub info: Option<VersionInfo>,
    pub run: Option<SelfUpdateRun>,
    /// Releases newer than the running build, newest first. Empty before the
    /// server's probe has answered once.
    pub releases: Vec<ReleaseNote>,
    pub open: bool,
    pub loading: bool,
    pub launching: bool,
    pub error: Option<String>,
    pub mode: Option<Mode>,
    pub polled_ms: i64,
}

impl Instance {
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode.clone().unwrap_or(Mode::Browse)
    }

    #[must_use]
    pub fn name(&self) -> String {
        self.info
            .as_ref()
            .and_then(|i| i.instance_name.clone())
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| "cctui".to_owned())
    }

    #[must_use]
    pub fn version(&self) -> &str {
        self.info.as_ref().map_or("-", |i| i.version.as_str())
    }

    /// The release channel the running build is on, as the release signer
    /// classifies it: any semver pre-release is beta.
    #[must_use]
    pub fn channel(&self) -> &'static str {
        self.info.as_ref().map_or("-", |i| Channel::of_version(&i.version).as_str())
    }

    #[must_use]
    pub fn latest(&self) -> Option<&str> {
        self.info.as_ref().and_then(|i| i.latest_version.as_deref())
    }

    #[must_use]
    pub fn update_available(&self) -> bool {
        self.info
            .as_ref()
            .is_some_and(|i| update_available(&i.version, i.latest_version.as_deref()))
    }

    #[must_use]
    pub fn self_update_ready(&self) -> bool {
        self.info.as_ref().is_some_and(|i| i.self_update_ready)
    }

    #[must_use]
    pub fn self_update_hook(&self) -> bool {
        self.info.as_ref().is_some_and(|i| i.self_update_hook)
    }

    /// The run readout: phase wording and its tone.
    #[must_use]
    pub fn run_readout(&self) -> Option<(&'static str, PhaseTone)> {
        let run = self.run.as_ref()?;
        Some((message_text(phase_message(run.phase)), phase_tone(Some(run.phase))))
    }

    /// Whether a hook run is still going, which is the only thing worth polling.
    #[must_use]
    pub fn run_in_flight(&self) -> bool {
        self.run.as_ref().is_some_and(|r| !r.done && !r.phase.is_terminal())
    }

    #[must_use]
    pub fn hint(&self, is_admin: bool) -> &'static str {
        message_text(hint_message(is_admin, self.self_update_ready(), self.self_update_hook()))
    }

    #[must_use]
    pub fn confirm_text(&self) -> &'static str {
        message_text(confirm_message(self.self_update_hook()))
    }

    #[must_use]
    pub fn badge_text(&self) -> &'static str {
        message_text(badge_message(self.self_update_hook()))
    }
}

pub enum InstanceAction {
    Open,
    Close,
    Refresh,
    /// `GET /version/refresh`'s sibling: probe upstream now.
    Probe,
    Loaded(Box<VersionInfo>),
    RunLoaded(Option<Box<SelfUpdateRun>>),
    ChangelogLoaded(Vec<ReleaseNote>),
    Failed(String),
    /// The status request came back 403: this key is not an admin.
    Forbidden,
    StartUpdate,
    Confirm,
    Cancel,
    Launched(Box<SelfUpdateLaunch>),
    LaunchFailed(String),
}

pub fn reduce_instance(app: &mut App, action: InstanceAction) -> Vec<Effect> {
    match action {
        InstanceAction::Open => {
            if !is_admin(app) {
                app.toast(Level::Warn, "the instance panel needs the admin scope");
                return Vec::new();
            }
            app.instance.open = true;
            app.router.push(super::state::View::Instance);
            refresh(app)
        }
        InstanceAction::Close => {
            app.instance.mode = None;
            if std::mem::take(&mut app.instance.open) {
                app.router.pop();
            }
            Vec::new()
        }
        InstanceAction::Refresh => refresh(app),
        InstanceAction::Probe => {
            app.toast(Level::Info, "checking upstream for a newer release");
            vec![Effect::RefreshVersion]
        }
        InstanceAction::Loaded(info) => {
            app.instance.info = Some(*info);
            app.instance.loading = false;
            app.instance.error = None;
            Vec::new()
        }
        InstanceAction::ChangelogLoaded(releases) => {
            app.instance.releases = releases;
            Vec::new()
        }
        InstanceAction::RunLoaded(run) => {
            app.instance.run = run.map(|r| *r);
            app.instance.loading = false;
            Vec::new()
        }
        InstanceAction::Failed(message) => {
            app.instance.loading = false;
            app.instance.error = Some(message);
            Vec::new()
        }
        // A refused status request is the same answer as a missing scope: the
        // panel is not for this key, so it closes rather than showing a wall.
        InstanceAction::Forbidden => {
            app.instance.loading = false;
            app.instance.info = None;
            app.instance.run = None;
            app.instance.releases.clear();
            app.toast(Level::Warn, "the server refused: the instance panel is admin-only");
            reduce_instance(app, InstanceAction::Close)
        }
        InstanceAction::StartUpdate => {
            if !can_launch(
                is_admin(app),
                app.instance.self_update_ready(),
                app.instance.update_available(),
            ) {
                app.toast(Level::Warn, app.instance.hint(is_admin(app)));
                return Vec::new();
            }
            app.instance.mode = Some(Mode::Confirm);
            Vec::new()
        }
        InstanceAction::Confirm => {
            if app.instance.mode() != Mode::Confirm {
                return Vec::new();
            }
            app.instance.mode = None;
            app.instance.launching = true;
            vec![Effect::LaunchSelfUpdate]
        }
        InstanceAction::Cancel => {
            if app.instance.mode.take().is_some() {
                return Vec::new();
            }
            reduce_instance(app, InstanceAction::Close)
        }
        InstanceAction::Launched(launch) => {
            app.instance.launching = false;
            let version = launch.version().to_owned();
            if launch.is_hook() {
                app.toast(Level::Info, format!("update to v{version} handed to the update hook"));
                return poll_run(app);
            }
            app.toast(Level::Info, format!("update to v{version} handed to an agent"));
            Vec::new()
        }
        InstanceAction::LaunchFailed(message) => {
            app.instance.launching = false;
            app.toast(Level::Error, format!("could not start the update: {message}"));
            Vec::new()
        }
    }
}

/// Whether the key may launch: the same `Scope::Admin` the server requires.
#[must_use]
pub fn is_admin(app: &App) -> bool {
    match &app.auth {
        super::identity::AuthState::Identified(identity) => {
            identity.role == ADMIN_SCOPE || identity.scopes.iter().any(|s| s == ADMIN_SCOPE)
        }
        _ => false,
    }
}

/// Polls a hook run while one is in flight, and nothing otherwise: an idle
/// panel costs no requests.
pub fn on_tick(app: &mut App) -> Vec<Effect> {
    if !app.instance.open || app.instance.loading || !app.instance.run_in_flight() {
        return Vec::new();
    }
    if app.clock_ms - app.instance.polled_ms < POLL_MS {
        return Vec::new();
    }
    poll_run(app)
}

fn refresh(app: &mut App) -> Vec<Effect> {
    app.instance.loading = true;
    app.instance.polled_ms = app.clock_ms;
    vec![Effect::FetchVersion, Effect::FetchSelfUpdateRun, Effect::FetchChangelog]
}

fn poll_run(app: &mut App) -> Vec<Effect> {
    app.instance.polled_ms = app.clock_ms;
    vec![Effect::FetchSelfUpdateRun]
}

#[cfg(test)]
mod tests {
    use cctui_client::{SelfUpdateLaunch, SelfUpdateRun, VersionInfo};
    use cctui_clientcore::instance::PhaseTone;
    use cctui_proto::updatehook::UpdateHookPhase;

    use super::{InstanceAction, Mode, POLL_MS, is_admin};
    use crate::app::action::Effect;
    use crate::app::{Action, App, reduce};

    fn info(version: &str, latest: Option<&str>, ready: bool, hook: bool) -> VersionInfo {
        VersionInfo {
            version: version.to_owned(),
            git_hash: "deadbeef".to_owned(),
            commit_url: "https://example.invalid/c".to_owned(),
            latest_version: latest.map(ToOwned::to_owned),
            latest_url: latest.map(|_| "https://example.invalid/r".to_owned()),
            instance_name: Some("cyberia".to_owned()),
            self_update_ready: ready,
            self_update_hook: hook,
        }
    }

    fn run(phase: UpdateHookPhase, done: bool) -> SelfUpdateRun {
        SelfUpdateRun {
            id: uuid::Uuid::nil(),
            version: "1.2.4".to_owned(),
            from_version: "1.2.3".to_owned(),
            phase,
            done,
            exit_code: None,
            detail: "kubectl rollout".to_owned(),
            output_tail: None,
            started_at: chrono::DateTime::from_timestamp_millis(0).expect("stamp"),
            updated_at: chrono::DateTime::from_timestamp_millis(0).expect("stamp"),
        }
    }

    fn app_as(role: &str) -> App {
        let mut app = App::new();
        app.auth = crate::app::identity::AuthState::Identified(crate::app::identity::Identity {
            role: role.to_owned(),
            user_name: Some("dev".to_owned()),
            scopes: vec!["read".to_owned()],
            token_preview: "abc".to_owned(),
        });
        app
    }

    fn act(app: &mut App, action: InstanceAction) -> Vec<Effect> {
        reduce(app, Action::Instance(action))
    }

    fn opened(update: bool) -> App {
        let mut app = app_as("admin");
        act(&mut app, InstanceAction::Open);
        act(
            &mut app,
            InstanceAction::Loaded(Box::new(info(
                "1.2.3",
                if update { Some("1.2.4") } else { None },
                true,
                true,
            ))),
        );
        act(&mut app, InstanceAction::RunLoaded(None));
        app
    }

    #[test]
    fn opening_fetches_the_version_and_the_last_run() {
        let mut app = app_as("admin");
        let effects = act(&mut app, InstanceAction::Open);
        assert!(effects.iter().any(|e| matches!(e, Effect::FetchVersion)));
        assert!(effects.iter().any(|e| matches!(e, Effect::FetchSelfUpdateRun)));
        assert_eq!(app.view(), crate::app::View::Instance);
        act(&mut app, InstanceAction::Close);
        assert!(!app.instance.open);
        assert_ne!(app.view(), crate::app::View::Instance);
    }

    #[test]
    fn a_non_admin_never_gets_the_panel() {
        let mut app = app_as("user");
        assert!(!is_admin(&app));
        assert!(act(&mut app, InstanceAction::Open).is_empty());
        assert!(!app.instance.open);
        assert!(app.toasts.latest().expect("a toast").text.contains("admin scope"));
    }

    #[test]
    fn a_403_on_the_status_hides_the_view() {
        let mut app = app_as("admin");
        act(&mut app, InstanceAction::Open);
        act(&mut app, InstanceAction::Forbidden);
        assert!(app.instance.info.is_none());
        assert!(!app.instance.open, "the panel closes rather than showing a wall");
        assert_ne!(app.view(), crate::app::View::Instance);
    }

    #[test]
    fn the_status_rows_read_from_the_version_reply() {
        let app = opened(true);
        assert_eq!(app.instance.name(), "cyberia");
        assert_eq!(app.instance.version(), "1.2.3");
        assert_eq!(app.instance.channel(), "stable");
        assert_eq!(app.instance.latest(), Some("1.2.4"));
        assert!(app.instance.update_available());
    }

    #[test]
    fn a_prerelease_build_reads_as_the_beta_channel() {
        let mut app = app_as("admin");
        act(&mut app, InstanceAction::Open);
        act(&mut app, InstanceAction::Loaded(Box::new(info("1.2.3-beta.4", None, true, true))));
        assert_eq!(app.instance.channel(), "beta");
        assert!(!app.instance.update_available());
    }

    #[test]
    fn an_unnamed_deployment_falls_back_to_the_product_name() {
        let mut app = app_as("admin");
        act(&mut app, InstanceAction::Open);
        let mut reply = info("1.2.3", None, true, true);
        reply.instance_name = Some("  ".to_owned());
        act(&mut app, InstanceAction::Loaded(Box::new(reply)));
        assert_eq!(app.instance.name(), "cctui");
    }

    #[test]
    fn the_trigger_sits_behind_a_confirm() {
        let mut app = opened(true);
        act(&mut app, InstanceAction::StartUpdate);
        assert_eq!(app.instance.mode(), Mode::Confirm);
        assert!(act(&mut app, InstanceAction::Cancel).is_empty());
        assert_eq!(app.instance.mode, None, "Esc backs out of the confirm first");
        assert!(app.instance.open, "and leaves the panel up");

        act(&mut app, InstanceAction::StartUpdate);
        match act(&mut app, InstanceAction::Confirm).as_slice() {
            [Effect::LaunchSelfUpdate] => {}
            other => panic!("expected one launch effect, got {}", other.len()),
        }
        assert!(app.instance.launching);
        assert_eq!(app.instance.mode, None);
    }

    #[test]
    fn confirming_without_the_dialog_launches_nothing() {
        let mut app = opened(true);
        assert!(act(&mut app, InstanceAction::Confirm).is_empty());
        assert!(!app.instance.launching);
    }

    #[test]
    fn nothing_to_install_or_no_target_refuses_before_the_confirm() {
        let mut app = opened(false);
        act(&mut app, InstanceAction::StartUpdate);
        assert_eq!(app.instance.mode, None, "no newer release, no dialog");

        let mut app = app_as("admin");
        act(&mut app, InstanceAction::Open);
        act(&mut app, InstanceAction::Loaded(Box::new(info("1.2.3", Some("1.2.4"), false, false))));
        act(&mut app, InstanceAction::StartUpdate);
        assert_eq!(app.instance.mode, None);
        assert!(app.toasts.latest().expect("a toast").text.contains("no self-update machine"));
    }

    #[test]
    fn a_hook_launch_starts_polling_the_run_and_an_agent_one_does_not() {
        let mut app = opened(true);
        let effects = act(
            &mut app,
            InstanceAction::Launched(Box::new(SelfUpdateLaunch::Hook {
                run_id: uuid::Uuid::nil(),
                version: "1.2.4".to_owned(),
            })),
        );
        assert!(effects.iter().any(|e| matches!(e, Effect::FetchSelfUpdateRun)));
        assert!(!app.instance.launching);

        let mut app = opened(true);
        let effects = act(
            &mut app,
            InstanceAction::Launched(Box::new(SelfUpdateLaunch::Agent {
                command_id: uuid::Uuid::nil(),
                session_id: None,
                version: "1.2.4".to_owned(),
                account: None,
            })),
        );
        assert!(effects.is_empty(), "an agent reports through its session, not a run");
    }

    #[test]
    fn a_run_is_polled_until_it_is_terminal_and_then_left_alone() {
        let mut app = opened(true);
        act(
            &mut app,
            InstanceAction::RunLoaded(Some(Box::new(run(UpdateHookPhase::Running, false)))),
        );
        assert!(app.instance.run_in_flight());
        assert!(super::on_tick(&mut app).is_empty(), "nothing is due yet");

        app.clock_ms += POLL_MS;
        assert!(super::on_tick(&mut app).iter().any(|e| matches!(e, Effect::FetchSelfUpdateRun)));

        act(
            &mut app,
            InstanceAction::RunLoaded(Some(Box::new(run(UpdateHookPhase::Succeeded, true)))),
        );
        app.clock_ms += 10 * POLL_MS;
        assert!(super::on_tick(&mut app).is_empty(), "a finished run is not polled over");
    }

    #[test]
    fn a_closed_panel_costs_no_requests() {
        let mut app = opened(true);
        act(
            &mut app,
            InstanceAction::RunLoaded(Some(Box::new(run(UpdateHookPhase::Running, false)))),
        );
        act(&mut app, InstanceAction::Close);
        app.clock_ms += 10 * POLL_MS;
        assert!(super::on_tick(&mut app).is_empty());
    }

    #[test]
    fn the_run_readout_pairs_the_wording_with_its_tone() {
        let mut app = opened(true);
        act(
            &mut app,
            InstanceAction::RunLoaded(Some(Box::new(run(UpdateHookPhase::Verifying, false)))),
        );
        let (text, tone) = app.instance.run_readout().expect("a readout");
        assert_eq!(text, "waiting for the new version to answer");
        assert_eq!(tone, PhaseTone::Faint);

        act(
            &mut app,
            InstanceAction::RunLoaded(Some(Box::new(run(UpdateHookPhase::Failed, true)))),
        );
        assert_eq!(app.instance.run_readout().expect("a readout").1, PhaseTone::Danger);
    }

    #[test]
    fn the_confirm_says_which_mechanism_will_run() {
        let app = opened(true);
        assert!(app.instance.confirm_text().contains("own update command"));
        assert_eq!(app.instance.badge_text(), "deterministic update");

        let mut app = app_as("admin");
        act(&mut app, InstanceAction::Open);
        act(&mut app, InstanceAction::Loaded(Box::new(info("1.2.3", Some("1.2.4"), true, false))));
        assert!(app.instance.confirm_text().contains("YOLO"));
        assert_eq!(app.instance.badge_text(), "agent fallback");
    }

    #[test]
    fn opening_also_loads_the_release_notes_the_server_collected() {
        let mut app = app_as("admin");
        let effects = act(&mut app, InstanceAction::Open);
        assert!(effects.iter().any(|e| matches!(e, Effect::FetchChangelog)));
        act(
            &mut app,
            InstanceAction::ChangelogLoaded(vec![cctui_client::ReleaseNote {
                version: "1.2.4".to_owned(),
                url: "https://example.invalid/r".to_owned(),
                body: "fixed the thing".to_owned(),
                published_at: None,
            }]),
        );
        assert_eq!(app.instance.releases.len(), 1);
        assert_eq!(app.instance.releases[0].version, "1.2.4");
    }

    #[test]
    fn probing_upstream_posts_a_refresh() {
        let mut app = opened(false);
        match act(&mut app, InstanceAction::Probe).as_slice() {
            [Effect::RefreshVersion] => {}
            other => panic!("expected a refresh, got {}", other.len()),
        }
    }

    #[test]
    fn a_failed_fetch_is_reported() {
        let mut app = app_as("admin");
        act(&mut app, InstanceAction::Open);
        act(&mut app, InstanceAction::Failed("connection refused".to_owned()));
        assert!(!app.instance.loading);
        assert_eq!(app.instance.error.as_deref(), Some("connection refused"));
    }

    #[test]
    fn a_failed_launch_clears_the_spinner_and_says_why() {
        let mut app = opened(true);
        act(&mut app, InstanceAction::StartUpdate);
        act(&mut app, InstanceAction::Confirm);
        act(&mut app, InstanceAction::LaunchFailed("cooldown".to_owned()));
        assert!(!app.instance.launching);
        assert!(app.toasts.latest().expect("a toast").text.contains("cooldown"));
    }
}
