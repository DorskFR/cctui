//! A session's active per-family gateway credential bindings.

/// One of a session's active per-family gateway credential bindings.
#[derive(serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SessionBinding {
    #[cfg_attr(feature = "ts", ts(type = "\"anthropic\" | \"openai\" | \"fireworks\""))]
    pub family: String,
    pub credential_id: uuid::Uuid,
    pub account_id: uuid::Uuid,
    pub account_name: String,
}
