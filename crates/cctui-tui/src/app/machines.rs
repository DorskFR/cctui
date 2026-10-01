//! The machines slice: every daemon the caller can spawn on, and whether it is
//! alive.
//!
//! The fetched tier is a snapshot; `machine_liveness` is the live one, so a row
//! always reads the socket's answer when it has one. That is what makes a
//! machine flip within one event instead of on the next refresh.

use cctui_client::MachineResourcesRow;
use cctui_proto::models::MachineLiveness;

use super::action::Effect;
use super::state::App;
use super::toast::Level;

/// One row of the table, after the live tier and the session count are folded in.
#[derive(Debug, Clone, PartialEq)]
pub struct MachineRow {
    pub id: String,
    pub name: String,
    pub liveness: MachineLiveness,
    pub last_seen_at: chrono::DateTime<chrono::Utc>,
    /// Hue for the name, operator-set or hashed from it.
    pub hue: u32,
    /// Live sessions running on it, counted from the list the TUI already holds.
    pub sessions: usize,
    pub cpu_pct: Option<f32>,
    pub mem_used_bytes: Option<u64>,
    pub mem_total_bytes: Option<u64>,
}

impl MachineRow {
    /// `31%`, or `-` when the daemon has sent no snapshot.
    #[must_use]
    pub fn cpu_text(&self) -> String {
        self.cpu_pct.map_or_else(|| "-".to_owned(), |pct| format!("{}%", pct.round() as i64))
    }

    /// `12G/64G`, or `-` without a snapshot.
    #[must_use]
    pub fn mem_text(&self) -> String {
        let (Some(used), Some(total)) = (self.mem_used_bytes, self.mem_total_bytes) else {
            return "-".to_owned();
        };
        if total == 0 {
            return "-".to_owned();
        }
        format!("{}/{}", gigabytes(used), gigabytes(total))
    }

    /// Age of the last heartbeat, in the compact form the rows already use.
    #[must_use]
    pub fn seen_text(&self, now_ms: i64) -> String {
        let age = (now_ms - self.last_seen_at.timestamp_millis()).max(0);
        super::session_status::format_ago(age)
    }

    #[must_use]
    pub const fn state_text(&self) -> &'static str {
        match self.liveness {
            MachineLiveness::Online => "online",
            MachineLiveness::Stale => "stale",
            MachineLiveness::Offline => "offline",
        }
    }

    /// An offline machine is not a spawn target, and the view greys it.
    #[must_use]
    pub const fn reachable(&self) -> bool {
        matches!(self.liveness, MachineLiveness::Online | MachineLiveness::Stale)
    }
}

fn gigabytes(bytes: u64) -> String {
    let gib = bytes / (1 << 30);
    format!("{gib}G")
}

#[derive(Debug, Default)]
pub struct Machines {
    pub rows: Vec<MachineRow>,
    pub selected: usize,
    /// A fetch is in flight and nothing has arrived yet.
    pub loading: bool,
    /// Why the last fetch failed, kept so the view says so rather than looking
    /// like a machine-less install.
    pub error: Option<String>,
    /// Whether the list has ever been fetched, so an empty list can be told
    /// apart from an unasked one.
    pub loaded: bool,
}

impl Machines {
    #[must_use]
    pub fn selected_row(&self) -> Option<&MachineRow> {
        self.rows.get(self.selected)
    }

    /// Online over total, for the Overview tile.
    #[must_use]
    pub fn counts(&self) -> (usize, usize) {
        let online = self.rows.iter().filter(|r| r.liveness == MachineLiveness::Online).count();
        (online, self.rows.len())
    }

    /// Re-read the live tiers into the rows. Called when a `machine_liveness`
    /// event lands, so the table does not wait for a refetch.
    pub fn apply_liveness(&mut self, live: &std::collections::HashMap<String, MachineLiveness>) {
        for row in &mut self.rows {
            if let Some(tier) = live.get(&row.id) {
                row.liveness = *tier;
            }
        }
    }
}

pub enum MachineAction {
    /// `M`, or the tab.
    Open,
    Close,
    Refresh,
    Loaded(Vec<MachineResourcesRow>),
    Failed(String),
    SelectNext,
    SelectPrev,
    /// `Enter`: aim the spawn dialog at this machine.
    SpawnHere,
}

pub fn reduce_machines(app: &mut App, action: MachineAction) -> Vec<Effect> {
    match action {
        MachineAction::Open => {
            let mut effects = super::slice::go_to(app, super::slice::Slice::Machines);
            if !app.machines.loaded {
                effects.extend(refresh(app));
            }
            effects
        }
        MachineAction::Close => super::slice::go_to(app, super::slice::Slice::Sessions),
        MachineAction::Refresh => refresh(app),
        MachineAction::Loaded(rows) => {
            app.machines.rows = rows.into_iter().map(|row| merge(app, row)).collect();
            app.machines.apply_liveness(&app.machine_liveness.clone());
            app.machines.loading = false;
            app.machines.loaded = true;
            app.machines.error = None;
            clamp(app);
            Vec::new()
        }
        MachineAction::Failed(message) => {
            app.machines.loading = false;
            app.machines.loaded = true;
            app.machines.error = Some(message);
            Vec::new()
        }
        MachineAction::SelectNext => {
            let len = app.machines.rows.len();
            if len > 0 {
                app.machines.selected = (app.machines.selected + 1).min(len - 1);
            }
            Vec::new()
        }
        MachineAction::SelectPrev => {
            app.machines.selected = app.machines.selected.saturating_sub(1);
            Vec::new()
        }
        MachineAction::SpawnHere => spawn_here(app),
    }
}

fn refresh(app: &mut App) -> Vec<Effect> {
    app.machines.loading = true;
    vec![Effect::FetchMachines]
}

/// Fold the REST row together with what the TUI already knows: the live tier and
/// how many of its own sessions are running there.
fn merge(app: &App, row: MachineResourcesRow) -> MachineRow {
    let id = row.machine_id.to_string();
    let name = row.display_name.filter(|n| !n.trim().is_empty()).unwrap_or(row.name);
    let hue = row
        .hue
        .and_then(|h| u32::try_from(h).ok())
        .unwrap_or_else(|| cctui_clientcore::format::hash_hue(&name));
    let sessions = app
        .sessions
        .iter()
        .filter(|s| s.machine_id == id && s.status == cctui_proto::models::SessionStatus::Active)
        .count();
    MachineRow {
        id,
        name,
        liveness: row.liveness,
        last_seen_at: row.last_seen_at,
        hue,
        sessions,
        cpu_pct: row.resources.as_ref().map(|r| r.cpu_pct),
        mem_used_bytes: row.resources.as_ref().map(|r| r.mem_used_bytes),
        mem_total_bytes: row.resources.as_ref().map(|r| r.mem_total_bytes),
    }
}

fn clamp(app: &mut App) {
    let len = app.machines.rows.len();
    app.machines.selected = if len == 0 { 0 } else { app.machines.selected.min(len - 1) };
}

/// Aiming the spawn dialog is all this lane can do: the dialog itself belongs to
/// the spawn lane, which reads `spawn_target`.
fn spawn_here(app: &mut App) -> Vec<Effect> {
    let Some(row) = app.machines.selected_row() else { return Vec::new() };
    if !row.reachable() {
        let name = row.name.clone();
        app.toast(Level::Warn, format!("{name} is offline — nothing would start there"));
        return Vec::new();
    }
    let (id, name) = (row.id.clone(), row.name.clone());
    app.spawn_target = Some(id);
    app.toast(Level::Info, format!("spawn target: {name}"));
    Vec::new()
}

#[cfg(test)]
mod tests {
    use cctui_proto::models::MachineLiveness;

    use super::{MachineAction, MachineRow};
    use crate::app::action::Effect;
    use crate::app::{Action, App, reduce};
    use crate::testsupport::session;

    fn row(
        id: &str,
        name: &str,
        tier: &str,
        cpu: Option<f32>,
    ) -> cctui_client::MachineResourcesRow {
        let liveness = match tier {
            "online" => MachineLiveness::Online,
            "stale" => MachineLiveness::Stale,
            _ => MachineLiveness::Offline,
        };
        cctui_client::MachineResourcesRow {
            machine_id: uuid::Uuid::parse_str(id).expect("a uuid"),
            name: name.to_owned(),
            display_name: None,
            hue: None,
            liveness,
            last_seen_at: chrono::DateTime::from_timestamp_millis(1_000).expect("stamp"),
            resources: cpu.map(|cpu_pct| cctui_proto::resources::MachineResources {
                cpu_pct,
                mem_pct: 20.0,
                mem_used_bytes: 12 << 30,
                mem_total_bytes: 64 << 30,
                disk_pct: 10.0,
                disk_used_bytes: 0,
                disk_total_bytes: 0,
                disk_path: String::new(),
                load1: None,
            }),
            updated_at: None,
        }
    }

    const A: &str = "11111111-1111-4111-8111-111111111111";
    const B: &str = "22222222-2222-4222-8222-222222222222";

    fn app() -> App {
        let mut app = App::new();
        let mut on_a = session("s-a", "alpha", "active", "working");
        on_a.machine_id = A.to_owned();
        let mut idle = session("s-idle", "beta", "inactive", "done");
        idle.machine_id = A.to_owned();
        app.sessions = vec![on_a, idle];
        app.update_aggregates();
        app
    }

    fn act(app: &mut App, action: MachineAction) -> Vec<Effect> {
        reduce(app, Action::Machines(action))
    }

    fn loaded(app: &mut App) {
        act(
            app,
            MachineAction::Loaded(vec![
                row(A, "cyberia-ws", "online", Some(31.4)),
                row(B, "macbook", "offline", None),
            ]),
        );
    }

    #[test]
    fn opening_fetches_once_and_the_tab_follows() {
        let mut app = app();
        let effects = act(&mut app, MachineAction::Open);
        assert!(effects.iter().any(|e| matches!(e, Effect::FetchMachines)));
        assert_eq!(app.slice, crate::app::slice::Slice::Machines);
        assert!(app.machines.loading);

        loaded(&mut app);
        assert!(!app.machines.loading);
        // Reopening uses what is already there; `r` is how you ask again.
        assert!(act(&mut app, MachineAction::Open).is_empty());
        assert!(!act(&mut app, MachineAction::Refresh).is_empty());
    }

    #[test]
    fn a_row_carries_its_live_session_count_not_every_session() {
        let mut app = app();
        loaded(&mut app);
        assert_eq!(app.machines.rows[0].sessions, 1, "the inactive one does not count");
        assert_eq!(app.machines.rows[1].sessions, 0);
    }

    #[test]
    fn resources_render_or_say_nothing_rather_than_zero() {
        let mut app = app();
        loaded(&mut app);
        assert_eq!(app.machines.rows[0].cpu_text(), "31%");
        assert_eq!(app.machines.rows[0].mem_text(), "12G/64G");
        assert_eq!(app.machines.rows[1].cpu_text(), "-");
        assert_eq!(app.machines.rows[1].mem_text(), "-");
    }

    #[test]
    fn a_live_event_flips_a_row_without_a_refetch() {
        let mut app = app();
        loaded(&mut app);
        assert_eq!(app.machines.rows[1].liveness, MachineLiveness::Offline);

        reduce(
            &mut app,
            Action::SessionLive(crate::app::session_live::SessionLiveAction::MachineLiveness {
                machine_id: B.to_owned(),
                liveness: MachineLiveness::Online,
            }),
        );
        assert_eq!(
            app.machines.rows[1].liveness,
            MachineLiveness::Online,
            "the socket's answer wins over the fetched snapshot"
        );
    }

    #[test]
    fn a_fetched_snapshot_never_overwrites_a_newer_live_tier() {
        let mut app = app();
        app.machine_liveness.insert(B.to_owned(), MachineLiveness::Online);
        loaded(&mut app);
        assert_eq!(app.machines.rows[1].liveness, MachineLiveness::Online);
    }

    #[test]
    fn the_cursor_stays_inside_the_table() {
        let mut app = app();
        loaded(&mut app);
        for _ in 0..5 {
            act(&mut app, MachineAction::SelectNext);
        }
        assert_eq!(app.machines.selected, 1);
        for _ in 0..5 {
            act(&mut app, MachineAction::SelectPrev);
        }
        assert_eq!(app.machines.selected, 0);
    }

    #[test]
    fn a_shorter_list_brings_the_cursor_back() {
        let mut app = app();
        loaded(&mut app);
        act(&mut app, MachineAction::SelectNext);
        assert_eq!(app.machines.selected, 1);
        act(&mut app, MachineAction::Loaded(vec![row(A, "cyberia-ws", "online", None)]));
        assert_eq!(app.machines.selected, 0);
    }

    #[test]
    fn enter_aims_the_spawn_dialog_at_a_reachable_machine() {
        let mut app = app();
        loaded(&mut app);
        act(&mut app, MachineAction::SpawnHere);
        assert_eq!(app.spawn_target.as_deref(), Some(A));
        assert!(app.toasts.latest().is_some());
    }

    #[test]
    fn enter_on_an_offline_machine_says_so_instead_of_aiming_at_it() {
        let mut app = app();
        loaded(&mut app);
        act(&mut app, MachineAction::SelectNext);
        act(&mut app, MachineAction::SpawnHere);
        assert_eq!(app.spawn_target, None);
        assert!(app.toasts.latest().expect("a toast").text.contains("offline"));
    }

    #[test]
    fn a_failed_fetch_is_reported_rather_than_looking_machine_less() {
        let mut app = app();
        act(&mut app, MachineAction::Open);
        act(&mut app, MachineAction::Failed("forbidden".to_owned()));
        assert!(!app.machines.loading);
        assert!(app.machines.loaded, "asked and answered, even though it failed");
        assert_eq!(app.machines.error.as_deref(), Some("forbidden"));
        assert!(app.machines.rows.is_empty());
    }

    #[test]
    fn counts_are_online_over_total() {
        let mut app = app();
        loaded(&mut app);
        assert_eq!(app.machines.counts(), (1, 2));
    }

    #[test]
    fn ages_and_states_read_as_the_table_shows_them() {
        let row = MachineRow {
            id: A.to_owned(),
            name: "cyberia".to_owned(),
            liveness: MachineLiveness::Stale,
            last_seen_at: chrono::DateTime::from_timestamp_millis(0).expect("stamp"),
            hue: 0,
            sessions: 0,
            cpu_pct: None,
            mem_used_bytes: None,
            mem_total_bytes: None,
        };
        assert_eq!(row.state_text(), "stale");
        assert_eq!(row.seen_text(3 * 60 * 1000), "3m");
        assert!(row.reachable(), "a stale daemon may still take a spawn");
    }
}
