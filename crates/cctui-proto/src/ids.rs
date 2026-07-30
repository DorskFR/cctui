//! String newtypes for the identifiers on the spawn / gateway path.
//!
//! They exist to stop transposition bugs: `rebind_spawn_key` takes a spawn key
//! and a session id of the same underlying type, and passing them in the wrong
//! order silently re-keys tokens onto the wrong id.

macro_rules! string_id {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
        pub struct $name(String);

        impl $name {
            #[must_use]
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            #[must_use]
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_owned())
            }
        }

        impl std::ops::Deref for $name {
            type Target = str;
            fn deref(&self) -> &str {
                &self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

string_id!(SessionId);
string_id!(LocalId);
string_id!(SpawnKey);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_types_carry_the_same_string() {
        let key = SpawnKey::new("abc");
        let id = SessionId::from("abc");
        assert_eq!(key.as_str(), id.as_str());
        assert_eq!(id.to_string(), "abc");
    }
}
