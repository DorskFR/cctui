//! `/api/v1/auth/passkeys` — `WebAuthn` enrolment and login ceremonies.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(test)]
use ts_rs::TS;
use uuid::Uuid;

#[derive(Serialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct PasskeyChallenge {
    /// Echoed back on finish.
    pub ceremony: String,
    /// The `WebAuthn` options as kagi serializes them.
    #[cfg_attr(
        test,
        ts(
            type = "import(\"@dorsk/kagi\").CreationOptionsJSON | import(\"@dorsk/kagi\").RequestOptionsJSON"
        )
    )]
    pub options: Value,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct PasskeyRegisterFinish {
    pub ceremony: String,
    /// Human label for the key ("iPhone", "`YubiKey`", "Bitwarden").
    pub label: Option<String>,
    #[cfg_attr(test, ts(type = "import(\"@dorsk/kagi\").RegistrationResponseJSON"))]
    pub credential: Value,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct PasskeyAssertion {
    pub ceremony: String,
    #[cfg_attr(test, ts(type = "import(\"@dorsk/kagi\").AuthenticationResponseJSON"))]
    pub credential: Value,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct PasskeyRow {
    pub id: Uuid,
    pub label: String,
    /// False when the authenticator declined to store a discoverable
    /// credential: the key still works as a second factor but will not appear
    /// at the login screen, and the UI says so.
    pub discoverable: bool,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct PasskeyListResponse {
    pub passkeys: Vec<PasskeyRow>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct PasskeyConfig {
    /// Whether this server can run a passkey ceremony at all (relying party
    /// configured). False means the login screen shows only the token box.
    pub available: bool,
    /// Whether anyone has enrolled a key. The login screen offers the passkey
    /// button only when a ceremony could actually succeed.
    pub enrolled: bool,
    /// Server-wide admin setting: attempt the passkey read as soon as the login
    /// screen opens instead of waiting for a click. The user can always dismiss
    /// it and type a token.
    pub auto_prompt: bool,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct PasskeyAutoPromptRequest {
    pub auto_prompt: bool,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct PasskeyTestResult {
    /// The label of the key that answered, so the UI can say which one.
    pub label: String,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct RelabelPasskeyRequest {
    pub label: String,
}
