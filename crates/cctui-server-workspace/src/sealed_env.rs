use std::collections::BTreeMap;

pub type EnvMap = BTreeMap<String, String>;

pub const MAX_VARS: usize = 64;
pub const MAX_VALUE_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealError {
    NeedsVaultKey,
    Unsealable,
}

impl SealError {
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::NeedsVaultKey => {
                "env values can only be stored when the server has a vault key (CCTUI_VAULT_KEY)"
            }
            Self::Unsealable => {
                "the stored env does not decrypt with the vault key; re-enter its values"
            }
        }
    }
}

#[must_use]
pub fn valid_name(name: &str) -> bool {
    name.bytes().next().is_some_and(|b| b == b'_' || b.is_ascii_uppercase())
        && name.bytes().all(|b| b == b'_' || b.is_ascii_uppercase() || b.is_ascii_digit())
}

pub fn check<'a>(names: impl IntoIterator<Item = &'a str>, set: &'a EnvMap) -> Result<(), String> {
    let mut all = std::collections::BTreeSet::new();
    for name in names.into_iter().chain(set.keys().map(String::as_str)) {
        if !valid_name(name) {
            return Err(format!("invalid env key {name:?} (want ^[A-Z_][A-Z0-9_]*$)"));
        }
        all.insert(name);
    }
    bounded(all.len(), set)
}

/// Size limits only: a draft may hold a name its launch will refuse.
pub fn bounded(count: usize, set: &EnvMap) -> Result<(), String> {
    if count > MAX_VARS {
        return Err(format!("at most {MAX_VARS} env vars"));
    }
    if set.values().any(|v| v.len() > MAX_VALUE_BYTES) {
        return Err(format!("an env value must be at most {MAX_VALUE_BYTES} bytes"));
    }
    Ok(())
}

pub fn seal(env: &EnvMap, key: impl FnOnce() -> Vec<u8>) -> Result<Option<String>, SealError> {
    if env.is_empty() {
        return Ok(None);
    }
    let key = key();
    if key.is_empty() {
        return Err(SealError::NeedsVaultKey);
    }
    let plain = serde_json::to_string(env).map_err(|_| SealError::Unsealable)?;
    Ok(Some(crate::crypto::encrypt(&plain, &key)))
}

pub fn unseal(sealed: Option<&str>, key: impl FnOnce() -> Vec<u8>) -> Result<EnvMap, SealError> {
    let Some(sealed) = sealed.filter(|s| !s.is_empty()) else { return Ok(EnvMap::new()) };
    let key = key();
    if key.is_empty() {
        return Err(SealError::Unsealable);
    }
    let plain = crate::crypto::decrypt(sealed, &key).ok_or(SealError::Unsealable)?;
    serde_json::from_str(&plain).map_err(|_| SealError::Unsealable)
}

/// The env after an edit: a name in `keep` keeps its stored value, a name in
/// `set` takes the new one, and every other stored name is dropped.
#[must_use]
pub fn edit(mut stored: EnvMap, keep: &[String], set: &EnvMap) -> EnvMap {
    stored.retain(|name, _| keep.contains(name));
    stored.extend(set.iter().map(|(k, v)| (k.clone(), v.clone())));
    stored
}

/// [`edit`] over a sealed map, opened only when a kept name needs its value.
pub fn apply_edit(
    sealed: Option<&str>,
    keep: &[String],
    set: &EnvMap,
    key: impl FnOnce() -> Vec<u8>,
) -> Result<EnvMap, SealError> {
    let stored = if keep.iter().any(|name| !set.contains_key(name)) {
        unseal(sealed, key)?
    } else {
        EnvMap::new()
    };
    Ok(edit(stored, keep, set))
}

/// `over` wins per name.
#[must_use]
pub fn overlay(mut base: EnvMap, over: EnvMap) -> EnvMap {
    base.extend(over);
    base
}

#[must_use]
pub fn names(env: &EnvMap) -> Vec<String> {
    env.keys().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::{EnvMap, SealError, apply_edit, check, edit, names, overlay, seal, unseal};

    fn env(pairs: &[(&str, &str)]) -> EnvMap {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    fn key() -> Vec<u8> {
        vec![9; 32]
    }

    #[test]
    fn a_sealed_env_is_not_plaintext_and_unseals_with_its_key() {
        let plain = env(&[("FOO", "bar-secret-value"), ("TOKEN", "t0k3n")]);
        let sealed = seal(&plain, key).unwrap().expect("something to seal");
        assert!(!sealed.contains("bar-secret-value") && !sealed.contains("t0k3n"), "{sealed}");
        assert!(!sealed.contains("FOO"), "names are sealed with the values");
        assert_eq!(unseal(Some(sealed.as_str()), key).unwrap(), plain);
        assert_eq!(unseal(Some(sealed.as_str()), || vec![1; 32]), Err(SealError::Unsealable));
    }

    #[test]
    fn an_empty_env_seals_to_nothing_without_reading_the_key() {
        let sealed = seal(&EnvMap::new(), || panic!("the key is not needed")).unwrap();
        assert_eq!(sealed, None);
        assert!(unseal(None, || panic!("nothing to unseal")).unwrap().is_empty());
    }

    #[test]
    fn sealing_without_a_vault_key_is_refused_rather_than_stored_plain() {
        assert_eq!(seal(&env(&[("FOO", "bar")]), Vec::new), Err(SealError::NeedsVaultKey));
    }

    #[test]
    fn an_edit_keeps_replaces_and_deletes_per_name() {
        let stored = env(&[("KEEP", "old-k"), ("SWAP", "old-s"), ("GONE", "old-g")]);
        let keep = vec!["KEEP".to_owned(), "SWAP".to_owned(), "NEVER_SET".to_owned()];
        let set = env(&[("SWAP", "new-s"), ("ADDED", "new-a")]);
        assert_eq!(
            edit(stored, &keep, &set),
            env(&[("ADDED", "new-a"), ("KEEP", "old-k"), ("SWAP", "new-s")]),
            "a kept name with no stored value is not invented"
        );
        assert!(edit(env(&[("A", "1")]), &[], &EnvMap::new()).is_empty());
    }

    #[test]
    fn an_edit_opens_the_seal_only_when_a_kept_name_needs_it() {
        let sealed = seal(&env(&[("A", "1"), ("B", "2")]), key).unwrap();
        let kept = apply_edit(sealed.as_deref(), &["A".to_owned()], &EnvMap::new(), key).unwrap();
        assert_eq!(kept, env(&[("A", "1")]));
        let replaced =
            apply_edit(Some("v1:not-a-seal"), &["A".to_owned()], &env(&[("A", "9")]), || {
                panic!("every kept name has a new value")
            })
            .unwrap();
        assert_eq!(replaced, env(&[("A", "9")]));
        let cleared =
            apply_edit(Some("v1:not-a-seal"), &[], &EnvMap::new(), || panic!("nothing kept"));
        assert!(cleared.unwrap().is_empty());
    }

    #[test]
    fn launch_time_env_wins_per_name() {
        let merged = overlay(env(&[("A", "stored"), ("B", "stored")]), env(&[("B", "launch")]));
        assert_eq!(merged, env(&[("A", "stored"), ("B", "launch")]));
        assert_eq!(names(&merged), ["A", "B"]);
    }

    #[test]
    fn names_follow_the_shell_var_pattern() {
        assert!(check(["FOO", "_X", "A1_B"], &EnvMap::new()).is_ok());
        for bad in ["foo", "1A", "", "A-B"] {
            assert!(check([bad], &EnvMap::new()).is_err(), "{bad:?}");
        }
        assert!(check([], &env(&[("lower", "v")])).is_err());
        let big = "x".repeat(super::MAX_VALUE_BYTES + 1);
        assert!(check([], &env(&[("BIG", big.as_str())])).is_err());
    }
}
