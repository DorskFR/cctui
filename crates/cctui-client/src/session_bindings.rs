//! Deserializable mirrors of the session-binding wire types.
//!
//! The `cctui-proto` originals are `Serialize`-only, so a client that reads
//! them back needs its own structs.

use serde::Deserialize;
use uuid::Uuid;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct SessionBinding {
    pub family: String,
    pub credential_id: Uuid,
    pub account_id: Uuid,
    pub account_name: String,
}
