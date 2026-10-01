//! Account pools: the pane in the accounts slice that shows membership and
//! changes it.
//!
//! Membership is replaced wholesale by `PATCH /account-pools/{id}`, so every
//! gesture here computes the whole ordered list; the arithmetic is
//! `cctui_clientcore::accounts`, shared with the webui.

use cctui_client::AccountPool;
use cctui_clientcore::accounts::{
    MemberRef, PoolRef, accepts_member, membership_after_move, ordered_members,
};

use super::action::Effect;
use super::state::App;
use super::toast::Level;

pub const STRATEGIES: [&str; 2] = ["headroom", "ordered"];

/// One line of the pane: a pool header, or a member under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub pool: usize,
    /// Index into the pool's members in election order; `None` is the header.
    pub member: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Name,
    Strategy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    New {
        name: String,
        strategy: usize,
        field: Field,
    },
    AddMember {
        pool_id: String,
        pool_name: String,
        candidates: Vec<Candidate>,
        selected: usize,
    },
    /// Deleting a pool asks for its name, not a keystroke.
    ConfirmDelete {
        pool_id: String,
        name: String,
        typed: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Default)]
pub struct Pools {
    pub rows: Vec<AccountPool>,
    pub cursor: usize,
    pub loading: bool,
    pub loaded: bool,
    pub error: Option<String>,
    pub mode: Option<Mode>,
}

impl Pools {
    /// Headers and members, flattened in the order the pane draws them.
    #[must_use]
    pub fn entries(&self) -> Vec<Entry> {
        let mut out = Vec::new();
        for (pool, row) in self.rows.iter().enumerate() {
            out.push(Entry { pool, member: None });
            for member in 0..row.members.len() {
                out.push(Entry { pool, member: Some(member) });
            }
        }
        out
    }

    #[must_use]
    pub fn entry(&self) -> Option<Entry> {
        self.entries().get(self.cursor).copied()
    }

    #[must_use]
    pub fn selected_pool(&self) -> Option<&AccountPool> {
        self.rows.get(self.entry()?.pool)
    }

    /// The member under the cursor, `None` on a header.
    #[must_use]
    pub fn selected_member(&self) -> Option<(String, String)> {
        let entry = self.entry()?;
        let pool = self.rows.get(entry.pool)?;
        let ids = ordered_members(&pool_ref(pool));
        let id = ids.get(entry.member?)?.clone();
        let name = pool
            .members
            .iter()
            .find(|m| m.account_id == id)
            .map_or_else(|| id.clone(), |m| m.name.clone());
        Some((id, name))
    }

    #[must_use]
    pub const fn form_open(&self) -> bool {
        self.mode.is_some()
    }

    #[must_use]
    pub fn refs(&self) -> Vec<PoolRef> {
        self.rows.iter().map(pool_ref).collect()
    }

    /// Members in election order, by pool index, for the pane.
    #[must_use]
    pub fn ordered(&self, pool: usize) -> Vec<cctui_client::AccountPoolMember> {
        let Some(row) = self.rows.get(pool) else { return Vec::new() };
        let mut members = row.members.clone();
        members.sort_by_key(|m| m.position);
        members
    }
}

/// The name of the pool an account sits in, for the accounts table.
#[must_use]
pub fn name_of(app: &App, account_id: &str) -> Option<String> {
    app.pools
        .rows
        .iter()
        .find(|pool| pool.members.iter().any(|m| m.account_id == account_id))
        .map(|pool| pool.name.clone())
}

fn pool_ref(pool: &AccountPool) -> PoolRef {
    PoolRef {
        id: pool.id.clone(),
        user_id: pool.user_id.clone(),
        members: pool
            .members
            .iter()
            .map(|m| MemberRef { account_id: m.account_id.clone(), position: m.position })
            .collect(),
    }
}

pub enum PoolAction {
    Loaded(Vec<AccountPool>),
    Failed(String),
    SelectNext,
    SelectPrev,
    /// `a`: enrol an account in the pool under the cursor.
    StartAddMember,
    /// `d`: drop the member under the cursor.
    RemoveMember,
    /// `J` / `K`: move the member under the cursor in election order.
    Move {
        down: bool,
    },
    /// `n`.
    StartNew,
    /// `D`, which asks for the pool's name.
    StartDelete,
    Commit,
    Cancel,
    NextField,
    PickNext,
    PickPrev,
    Key(crossterm::event::KeyEvent),
    Refused(String),
}

pub fn reduce_pools(app: &mut App, action: PoolAction) -> Vec<Effect> {
    match action {
        PoolAction::Loaded(rows) => {
            app.pools.rows = rows;
            app.pools.loading = false;
            app.pools.loaded = true;
            app.pools.error = None;
            clamp(app);
            Vec::new()
        }
        PoolAction::Failed(message) => {
            app.pools.loading = false;
            app.pools.loaded = true;
            app.pools.error = Some(message);
            Vec::new()
        }
        PoolAction::SelectNext => {
            let len = app.pools.entries().len();
            if len > 0 {
                app.pools.cursor = (app.pools.cursor + 1).min(len - 1);
            }
            Vec::new()
        }
        PoolAction::SelectPrev => {
            app.pools.cursor = app.pools.cursor.saturating_sub(1);
            Vec::new()
        }
        PoolAction::StartAddMember => start_add_member(app),
        PoolAction::RemoveMember => remove_member(app),
        PoolAction::Move { down } => move_member(app, down),
        PoolAction::StartNew => {
            if !writable(app) {
                return Vec::new();
            }
            app.pools.mode =
                Some(Mode::New { name: String::new(), strategy: 0, field: Field::Name });
            Vec::new()
        }
        PoolAction::StartDelete => start_delete(app),
        PoolAction::Commit => commit(app),
        PoolAction::Cancel => {
            app.pools.mode = None;
            Vec::new()
        }
        PoolAction::NextField => {
            if let Some(Mode::New { field, strategy, .. }) = app.pools.mode.as_mut() {
                match field {
                    Field::Name => *field = Field::Strategy,
                    // On the strategy field, Tab cycles the value rather than
                    // leaving a two-field form.
                    Field::Strategy => *strategy = (*strategy + 1) % STRATEGIES.len(),
                }
            }
            Vec::new()
        }
        PoolAction::PickNext => {
            if let Some(Mode::AddMember { candidates, selected, .. }) = app.pools.mode.as_mut()
                && !candidates.is_empty()
            {
                *selected = (*selected + 1).min(candidates.len() - 1);
            }
            Vec::new()
        }
        PoolAction::PickPrev => {
            if let Some(Mode::AddMember { selected, .. }) = app.pools.mode.as_mut() {
                *selected = selected.saturating_sub(1);
            }
            Vec::new()
        }
        PoolAction::Key(key) => {
            type_into(app, key);
            Vec::new()
        }
        PoolAction::Refused(message) => {
            app.accounts.read_only = true;
            app.toast(Level::Warn, message);
            Vec::new()
        }
    }
}

fn clamp(app: &mut App) {
    let len = app.pools.entries().len();
    app.pools.cursor = if len == 0 { 0 } else { app.pools.cursor.min(len - 1) };
}

fn writable(app: &mut App) -> bool {
    if app.accounts.read_only {
        app.toast(Level::Warn, "this key may not edit pools");
        return false;
    }
    true
}

fn type_into(app: &mut App, key: crossterm::event::KeyEvent) {
    use crossterm::event::{KeyCode, KeyModifiers};
    let Some(mode) = app.pools.mode.as_mut() else { return };
    let target = match mode {
        Mode::New { name, field: Field::Name, .. } => name,
        Mode::ConfirmDelete { typed, .. } => typed,
        Mode::New { .. } | Mode::AddMember { .. } => return,
    };
    match key.code {
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => target.push(c),
        KeyCode::Backspace => {
            target.pop();
        }
        _ => {}
    }
}

/// The `PATCH`es a membership change implies, as effects.
fn patches(changes: Vec<cctui_clientcore::accounts::MembershipChange>) -> Vec<Effect> {
    changes
        .into_iter()
        .map(|change| Effect::UpdatePool {
            id: change.pool_id,
            request: Box::new(cctui_client::UpdatePool {
                accounts: Some(change.accounts),
                ..Default::default()
            }),
        })
        .collect()
}

fn start_add_member(app: &mut App) -> Vec<Effect> {
    if !writable(app) {
        return Vec::new();
    }
    let Some(pool) = app.pools.selected_pool().cloned() else {
        app.toast(Level::Warn, "no pool under the cursor");
        return Vec::new();
    };
    let accounts = app.accounts.refs();
    let reference = pool_ref(&pool);
    let candidates: Vec<Candidate> = accounts
        .iter()
        .filter(|a| accepts_member(&reference, &a.id, &accounts))
        .map(|a| Candidate { id: a.id.clone(), name: a.name.clone() })
        .collect();
    if candidates.is_empty() {
        app.toast(Level::Warn, format!("no account may join {}", pool.name));
        return Vec::new();
    }
    app.pools.mode =
        Some(Mode::AddMember { pool_id: pool.id, pool_name: pool.name, candidates, selected: 0 });
    Vec::new()
}

fn remove_member(app: &mut App) -> Vec<Effect> {
    if !writable(app) {
        return Vec::new();
    }
    let Some((id, name)) = app.pools.selected_member() else {
        app.toast(Level::Warn, "no member under the cursor");
        return Vec::new();
    };
    app.toast(Level::Info, format!("{name} left the pool"));
    patches(membership_after_move(&app.pools.refs(), &id, None))
}

fn move_member(app: &mut App, down: bool) -> Vec<Effect> {
    if !writable(app) {
        return Vec::new();
    }
    let Some(entry) = app.pools.entry() else { return Vec::new() };
    let Some(index) = entry.member else {
        app.toast(Level::Warn, "reordering moves a member, not a pool");
        return Vec::new();
    };
    let Some(pool) = app.pools.rows.get(entry.pool) else { return Vec::new() };
    let mut ids = ordered_members(&pool_ref(pool));
    let swap = if down { index + 1 } else { index.checked_sub(1).unwrap_or(usize::MAX) };
    if swap >= ids.len() {
        return Vec::new();
    }
    ids.swap(index, swap);
    // The cursor follows the row it was on, so a held J walks it down.
    app.pools.cursor = if down { app.pools.cursor + 1 } else { app.pools.cursor - 1 };
    vec![Effect::UpdatePool {
        id: pool.id.clone(),
        request: Box::new(cctui_client::UpdatePool { accounts: Some(ids), ..Default::default() }),
    }]
}

fn start_delete(app: &mut App) -> Vec<Effect> {
    if !writable(app) {
        return Vec::new();
    }
    let Some(pool) = app.pools.selected_pool() else {
        app.toast(Level::Warn, "no pool under the cursor");
        return Vec::new();
    };
    app.pools.mode = Some(Mode::ConfirmDelete {
        pool_id: pool.id.clone(),
        name: pool.name.clone(),
        typed: String::new(),
    });
    Vec::new()
}

fn commit(app: &mut App) -> Vec<Effect> {
    let Some(mode) = app.pools.mode.clone() else { return Vec::new() };
    match mode {
        Mode::New { name, strategy, .. } => {
            if name.trim().is_empty() {
                app.toast(Level::Warn, "a pool needs a name");
                return Vec::new();
            }
            app.pools.mode = None;
            vec![Effect::CreatePool {
                request: Box::new(cctui_client::CreatePool {
                    name: name.trim().to_owned(),
                    strategy: Some(STRATEGIES[strategy].to_owned()),
                    failover: None,
                    accounts: Vec::new(),
                }),
            }]
        }
        Mode::AddMember { pool_id, candidates, selected, .. } => {
            let Some(candidate) = candidates.get(selected) else { return Vec::new() };
            let (id, name) = (candidate.id.clone(), candidate.name.clone());
            app.pools.mode = None;
            app.toast(Level::Info, format!("{name} joined the pool"));
            patches(membership_after_move(&app.pools.refs(), &id, Some(&pool_id)))
        }
        Mode::ConfirmDelete { pool_id, name, typed } => {
            if typed.trim() != name {
                app.toast(Level::Warn, format!("type {name} to delete it"));
                return Vec::new();
            }
            app.pools.mode = None;
            app.toast(Level::Info, format!("deleted {name}"));
            vec![Effect::DeletePool { id: pool_id }]
        }
    }
}

#[cfg(test)]
mod tests {
    use cctui_client::{AccountPool, AccountPoolMember};

    use super::{Field, Mode, PoolAction};
    use crate::app::accounts::AccountAction;
    use crate::app::accounts::tests::account;
    use crate::app::action::Effect;
    use crate::app::{Action, App, reduce};

    fn member(id: &str, name: &str, position: i32) -> AccountPoolMember {
        AccountPoolMember {
            account_id: id.to_owned(),
            name: name.to_owned(),
            position,
            owned: true,
            pool_eligible: true,
        }
    }

    fn pool(id: &str, name: &str, members: Vec<AccountPoolMember>) -> AccountPool {
        AccountPool {
            id: id.to_owned(),
            user_id: "u1".to_owned(),
            name: name.to_owned(),
            strategy: "headroom".to_owned(),
            failover: false,
            members,
        }
    }

    fn act(app: &mut App, action: PoolAction) -> Vec<Effect> {
        reduce(app, Action::Pools(action))
    }

    /// Two accounts in `default`, out of order on purpose, plus a spare.
    fn loaded() -> App {
        let mut app = App::new();
        reduce(
            &mut app,
            Action::Accounts(AccountAction::Loaded(vec![
                account("a1", "alice@max", &["anthropic"]),
                account("a2", "bob@max", &["anthropic"]),
                account("a3", "ops-codex", &["openai"]),
            ])),
        );
        act(
            &mut app,
            PoolAction::Loaded(vec![
                pool(
                    "p1",
                    "default",
                    vec![member("a2", "bob@max", 1), member("a1", "alice@max", 0)],
                ),
                pool("p2", "codex", vec![]),
            ]),
        );
        app
    }

    fn typed(app: &mut App, text: &str) {
        for c in text.chars() {
            act(
                app,
                PoolAction::Key(crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Char(c),
                    crossterm::event::KeyModifiers::NONE,
                )),
            );
        }
    }

    #[test]
    fn the_pane_is_headers_and_their_members_in_election_order() {
        let app = loaded();
        let entries = app.pools.entries();
        assert_eq!(entries.len(), 4, "two headers, two members, one empty pool");
        assert_eq!(
            app.pools.ordered(0).iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
            vec!["alice@max", "bob@max"]
        );
    }

    #[test]
    fn the_cursor_stays_inside_the_pane() {
        let mut app = loaded();
        for _ in 0..9 {
            act(&mut app, PoolAction::SelectNext);
        }
        assert_eq!(app.pools.cursor, 3);
        for _ in 0..9 {
            act(&mut app, PoolAction::SelectPrev);
        }
        assert_eq!(app.pools.cursor, 0);
    }

    #[test]
    fn j_reorders_the_member_under_the_cursor_and_patches_the_whole_list() {
        let mut app = loaded();
        act(&mut app, PoolAction::SelectNext);
        let effects = act(&mut app, PoolAction::Move { down: true });
        let [Effect::UpdatePool { id, request }] = effects.as_slice() else {
            panic!("one PATCH");
        };
        assert_eq!(id, "p1");
        assert_eq!(
            request.accounts.as_deref(),
            Some(["a2".to_owned(), "a1".to_owned()].as_slice()),
            "the body is the whole ordered membership"
        );
        assert_eq!(request.name, None, "a reorder renames nothing");
        assert_eq!(app.pools.cursor, 2, "the cursor follows the row it moved");
    }

    #[test]
    fn reordering_stops_at_the_ends_and_refuses_a_header() {
        let mut app = loaded();
        act(&mut app, PoolAction::SelectNext);
        assert!(act(&mut app, PoolAction::Move { down: false }).is_empty());
        act(&mut app, PoolAction::SelectNext);
        assert!(act(&mut app, PoolAction::Move { down: true }).is_empty());

        app.pools.cursor = 0;
        assert!(act(&mut app, PoolAction::Move { down: true }).is_empty());
        assert!(app.toasts.latest().expect("a toast").text.contains("not a pool"));
    }

    #[test]
    fn a_adds_only_accounts_the_pool_may_take() {
        let mut app = loaded();
        act(&mut app, PoolAction::StartAddMember);
        let Some(Mode::AddMember { candidates, pool_name, .. }) = app.pools.mode.clone() else {
            panic!("a picker");
        };
        assert_eq!(pool_name, "default");
        assert_eq!(
            candidates.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            vec!["ops-codex"],
            "the two members are already in"
        );

        let effects = act(&mut app, PoolAction::Commit);
        let [Effect::UpdatePool { id, request }] = effects.as_slice() else {
            panic!("one PATCH");
        };
        assert_eq!(id, "p1");
        assert_eq!(
            request.accounts.as_deref(),
            Some(["a1".to_owned(), "a2".to_owned(), "a3".to_owned()].as_slice())
        );
    }

    #[test]
    fn adding_a_member_that_is_in_another_pool_moves_it() {
        let mut app = loaded();
        act(
            &mut app,
            PoolAction::Loaded(vec![
                pool("p1", "default", vec![member("a1", "alice@max", 0)]),
                pool("p2", "codex", vec![member("a3", "ops-codex", 0)]),
            ]),
        );
        act(&mut app, PoolAction::StartAddMember);
        let Some(Mode::AddMember { candidates, .. }) = app.pools.mode.clone() else {
            panic!("a picker");
        };
        let ops = candidates.iter().position(|c| c.name == "ops-codex").expect("ops-codex");
        for _ in 0..ops {
            act(&mut app, PoolAction::PickNext);
        }
        let effects = act(&mut app, PoolAction::Commit);
        assert_eq!(effects.len(), 2, "it leaves one pool and joins the other");
        let Some(Effect::UpdatePool { id, request }) = effects.first() else { panic!("a PATCH") };
        assert_eq!(id, "p2");
        assert_eq!(request.accounts.as_deref(), Some([].as_slice()));
    }

    #[test]
    fn an_account_withheld_from_pools_is_not_offered_to_someone_elses_pool() {
        let mut app = loaded();
        app.accounts.rows[2].pool_eligible = false;
        app.accounts.rows[2].user_id = "u2".to_owned();
        act(&mut app, PoolAction::StartAddMember);
        assert!(app.pools.mode.is_none());
        assert!(app.toasts.latest().expect("a toast").text.contains("no account may join"));
    }

    #[test]
    fn d_drops_the_member_under_the_cursor_only() {
        let mut app = loaded();
        act(&mut app, PoolAction::SelectNext);
        act(&mut app, PoolAction::SelectNext);
        let effects = act(&mut app, PoolAction::RemoveMember);
        let [Effect::UpdatePool { id, request }] = effects.as_slice() else {
            panic!("one PATCH");
        };
        assert_eq!(id, "p1");
        assert_eq!(request.accounts.as_deref(), Some(["a1".to_owned()].as_slice()));
    }

    #[test]
    fn d_on_a_header_says_so_rather_than_emptying_the_pool() {
        let mut app = loaded();
        assert!(act(&mut app, PoolAction::RemoveMember).is_empty());
        assert!(app.toasts.latest().expect("a toast").text.contains("no member"));
    }

    #[test]
    fn n_creates_a_pool_with_the_typed_name_and_chosen_strategy() {
        let mut app = loaded();
        act(&mut app, PoolAction::StartNew);
        typed(&mut app, "overflow");
        act(&mut app, PoolAction::NextField);
        act(&mut app, PoolAction::NextField);
        let Some(Mode::New { field, strategy, .. }) = app.pools.mode.clone() else {
            panic!("a form")
        };
        assert_eq!(field, Field::Strategy);
        assert_eq!(super::STRATEGIES[strategy], "ordered");

        let effects = act(&mut app, PoolAction::Commit);
        let [Effect::CreatePool { request }] = effects.as_slice() else { panic!("one create") };
        assert_eq!(request.name, "overflow");
        assert_eq!(request.strategy.as_deref(), Some("ordered"));
        assert!(request.accounts.is_empty());
        assert!(app.pools.mode.is_none());
    }

    #[test]
    fn a_nameless_pool_is_not_created() {
        let mut app = loaded();
        act(&mut app, PoolAction::StartNew);
        assert!(act(&mut app, PoolAction::Commit).is_empty());
        assert!(app.pools.mode.is_some(), "the form stays up");
    }

    #[test]
    fn deleting_a_pool_needs_its_name_typed() {
        let mut app = loaded();
        act(&mut app, PoolAction::StartDelete);
        typed(&mut app, "defaul");
        assert!(act(&mut app, PoolAction::Commit).is_empty());
        assert!(app.toasts.latest().expect("a toast").text.contains("type default"));
        typed(&mut app, "t");
        let effects = act(&mut app, PoolAction::Commit);
        let [Effect::DeletePool { id }] = effects.as_slice() else { panic!("one delete") };
        assert_eq!(id, "p1");
    }

    #[test]
    fn a_refused_write_makes_every_pool_gesture_a_no_op() {
        let mut app = loaded();
        act(&mut app, PoolAction::Refused("this key may not edit pools".to_owned()));
        assert!(app.accounts.read_only);
        assert!(act(&mut app, PoolAction::StartNew).is_empty());
        assert!(act(&mut app, PoolAction::StartAddMember).is_empty());
        assert!(act(&mut app, PoolAction::StartDelete).is_empty());
        assert!(act(&mut app, PoolAction::RemoveMember).is_empty());
        assert!(act(&mut app, PoolAction::Move { down: true }).is_empty());
        assert!(app.pools.mode.is_none());
    }

    #[test]
    fn the_shared_form_keys_reach_the_pool_form_when_the_accounts_pane_has_none() {
        let mut app = loaded();
        act(&mut app, PoolAction::StartAddMember);
        reduce(&mut app, Action::Accounts(AccountAction::PickNext));
        let Some(Mode::AddMember { selected, .. }) = app.pools.mode.clone() else {
            panic!("a picker")
        };
        assert_eq!(selected, 0, "one candidate, so the cursor cannot move");

        reduce(&mut app, Action::Accounts(AccountAction::Cancel));
        assert!(app.pools.mode.is_none(), "Esc backs out of the pool form");

        act(&mut app, PoolAction::StartNew);
        typed(&mut app, "overflow");
        let effects = reduce(&mut app, Action::Accounts(AccountAction::Commit));
        assert!(matches!(effects.as_slice(), [Effect::CreatePool { .. }]), "Enter commits it");
    }

    #[test]
    fn a_failed_fetch_is_reported_rather_than_looking_pool_less() {
        let mut app = App::new();
        reduce(&mut app, Action::Accounts(AccountAction::Refresh));
        assert!(app.pools.loading, "the slice's refresh fills both panes");
        act(&mut app, PoolAction::Failed("forbidden".to_owned()));
        assert!(!app.pools.loading);
        assert!(app.pools.loaded);
        assert_eq!(app.pools.error.as_deref(), Some("forbidden"));
    }

    #[test]
    fn a_shorter_pane_brings_the_cursor_back() {
        let mut app = loaded();
        app.pools.cursor = 3;
        act(&mut app, PoolAction::Loaded(vec![pool("p2", "codex", vec![])]));
        assert_eq!(app.pools.cursor, 0);
    }
}
