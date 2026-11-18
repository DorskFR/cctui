//! `idx_sessions_live` (migration 117) is partial on exactly
//! `status <> 'archived'`. The planner matches an identical conjunct
//! reliably; it will not reliably prove that `status NOT IN ('archived',
//! 'ended')` implies it. So every live-session query ANDs this conjunct in
//! verbatim on top of its own stricter status filter, and rewording it here
//! requires rewording migration 117.

/// Expands to the `status <> 'archived'` conjunct, optionally table-qualified.
/// A macro, not a `const`, so it composes inside `concat!` in const SQL.
#[macro_export]
macro_rules! live_sessions_predicate {
    () => {
        "status <> 'archived'"
    };
    ($alias:literal) => {
        concat!($alias, ".status <> 'archived'")
    };
}

/// `SET` assignments of an update that ends a session for good, so the row no
/// longer reads as busy.
///
/// A `blocked` or `hibernated` tempo is kept: it says
/// what the session was waiting on. A `daemon_lost` or `machine_offline` end
/// leaves the signals alone, as a daemon that re-registers the session revives
/// it without reporting them again.
#[macro_export]
macro_rules! ended_signals {
    () => {
        "tempo = NULLIF(tempo, 'active'), agent_state = 'stopped'"
    };
}
