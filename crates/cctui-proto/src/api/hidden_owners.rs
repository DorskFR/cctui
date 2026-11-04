//! `/api/v1/me/hidden-owners` — users and machines whose sessions an admin
//! keeps out of their own session views.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum HiddenOwnerKind {
    User,
    Machine,
}

impl HiddenOwnerKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Machine => "machine",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "user" => Some(Self::User),
            "machine" => Some(Self::Machine),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct HiddenOwner {
    pub kind: HiddenOwnerKind,
    pub target_id: String,
    /// The user's name or the machine's display name; `None` once it is gone.
    pub label: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct HiddenOwners {
    pub owners: Vec<HiddenOwner>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_owner_kind_round_trips_as_lowercase() {
        for kind in [HiddenOwnerKind::User, HiddenOwnerKind::Machine] {
            let json = serde_json::to_value(kind).unwrap();
            assert_eq!(json, serde_json::json!(kind.as_str()));
            assert_eq!(HiddenOwnerKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(HiddenOwnerKind::parse("room"), None);
    }
}
