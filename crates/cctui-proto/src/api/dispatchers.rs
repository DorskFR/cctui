//! `/api/v1/dispatchers` — enrolled-dispatcher management.

/// A dispatcher edit. `name` renames; each binding field is left untouched when
/// absent, cleared by an empty string, and set otherwise — so the one-control
/// UI can express "bind this pool, drop the account" in a single call.
#[derive(Debug, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct RenameDispatcher {
    pub name: String,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string | null", optional))]
    pub default_account: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string | null", optional))]
    pub default_pool: Option<String>,
}

