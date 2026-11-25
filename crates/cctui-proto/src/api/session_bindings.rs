//! A session's active per-family gateway credential bindings.

/// One of a session's active per-family gateway credential bindings.
#[derive(serde::Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct SessionBinding {
    #[cfg_attr(test, ts(type = "\"anthropic\" | \"openai\" | \"fireworks\""))]
    pub family: String,
    pub credential_id: uuid::Uuid,
    pub account_id: uuid::Uuid,
    pub account_name: String,
    /// The accounts the caller may switch this binding to.
    pub options: Vec<SwitchOption>,
}

/// A credential the server accepts as a binding's new target.
#[derive(serde::Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct SwitchOption {
    pub credential_id: uuid::Uuid,
    pub account_id: uuid::Uuid,
    pub account_name: String,
    pub provider: String,
    /// The account is the caller's, not the runner's: switching moves the
    /// binding over to the caller.
    pub takeover: bool,
}
