//! Switchable server features and the set resolved from `CCTUI_FEATURES_FILE`.
//! No file means every feature on.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

/// The `snake_case` names are a contract: feature file, capabilities, clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    MachineSpawn,
    Files,
    Uploads,
    Drafts,
    Profiles,
    Terminal,
    TranscriptSearch,
    ChildSessions,
    Context,
    Plugins,
    AutoApprove,
    Scheduling,
    Automation,
    Rooms,
    Voice,
    Previews,
    Federation,
    DeviceLogin,
    Passkeys,
    SelfUpdate,
    AccountSharing,
    Observability,
}

impl Feature {
    pub const ALL: [Self; 22] = [
        Self::MachineSpawn,
        Self::Files,
        Self::Uploads,
        Self::Drafts,
        Self::Profiles,
        Self::Terminal,
        Self::TranscriptSearch,
        Self::ChildSessions,
        Self::Context,
        Self::Plugins,
        Self::AutoApprove,
        Self::Scheduling,
        Self::Automation,
        Self::Rooms,
        Self::Voice,
        Self::Previews,
        Self::Federation,
        Self::DeviceLogin,
        Self::Passkeys,
        Self::SelfUpdate,
        Self::AccountSharing,
        Self::Observability,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::MachineSpawn => "machine_spawn",
            Self::Files => "files",
            Self::Uploads => "uploads",
            Self::Drafts => "drafts",
            Self::Profiles => "profiles",
            Self::Terminal => "terminal",
            Self::TranscriptSearch => "transcript_search",
            Self::ChildSessions => "child_sessions",
            Self::Context => "context",
            Self::Plugins => "plugins",
            Self::AutoApprove => "auto_approve",
            Self::Scheduling => "scheduling",
            Self::Automation => "automation",
            Self::Rooms => "rooms",
            Self::Voice => "voice",
            Self::Previews => "previews",
            Self::Federation => "federation",
            Self::DeviceLogin => "device_login",
            Self::Passkeys => "passkeys",
            Self::SelfUpdate => "self_update",
            Self::AccountSharing => "account_sharing",
            Self::Observability => "observability",
        }
    }

    /// `None` for a name this server does not know.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.name() == raw)
    }

    /// Features that must be on for this one to be on.
    #[must_use]
    pub const fn requires(self) -> &'static [Self] {
        match self {
            Self::Files | Self::Uploads | Self::Drafts | Self::Profiles | Self::Terminal => {
                &[Self::MachineSpawn]
            }
            _ => &[],
        }
    }

    const fn bit(self) -> u32 {
        1 << self as u32
    }
}

impl fmt::Display for Feature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The features a server runs with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeatureSet(u32);

impl FeatureSet {
    #[must_use]
    pub const fn all() -> Self {
        Self((1 << Feature::ALL.len()) - 1)
    }

    #[must_use]
    pub const fn none() -> Self {
        Self(0)
    }

    #[must_use]
    pub const fn contains(self, feature: Feature) -> bool {
        self.0 & feature.bit() != 0
    }

    #[must_use]
    pub const fn with(self, feature: Feature) -> Self {
        Self(self.0 | feature.bit())
    }

    #[must_use]
    pub const fn without(self, feature: Feature) -> Self {
        Self(self.0 & !feature.bit())
    }

    #[must_use]
    pub const fn is_all(self) -> bool {
        self.0 == Self::all().0
    }

    /// Whether something classified under `feature` is mounted: `None` is core
    /// and always is.
    #[must_use]
    pub fn allows(self, feature: Option<Feature>) -> bool {
        feature.is_none_or(|f| self.contains(f))
    }

    /// Enabled features, in [`Feature::ALL`] order.
    pub fn enabled(self) -> impl Iterator<Item = Feature> {
        Feature::ALL.into_iter().filter(move |f| self.contains(*f))
    }

    /// Disabled features, in [`Feature::ALL`] order.
    pub fn disabled(self) -> impl Iterator<Item = Feature> {
        Feature::ALL.into_iter().filter(move |f| !self.contains(*f))
    }
}

impl Default for FeatureSet {
    fn default() -> Self {
        Self::all()
    }
}

impl fmt::Display for FeatureSet {
    /// `on=[..] off=[..]`, the one-line boot summary.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let join = |it: &mut dyn Iterator<Item = Feature>| {
            it.map(Feature::name).collect::<Vec<_>>().join(",")
        };
        write!(f, "on=[{}] off=[{}]", join(&mut self.enabled()), join(&mut self.disabled()))
    }
}

const fn default_on() -> bool {
    true
}

/// The TOML document `CCTUI_FEATURES_FILE` points at.
///
/// ```toml
/// default = true
/// [features]
/// machine_spawn = false
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeatureFile {
    /// State of every feature not listed in [`Self::features`].
    #[serde(default = "default_on")]
    pub default: bool,
    #[serde(default)]
    pub features: BTreeMap<String, bool>,
}

impl Default for FeatureFile {
    fn default() -> Self {
        Self { default: true, features: BTreeMap::new() }
    }
}

/// A resolved feature file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub set: FeatureSet,
    /// Features left unlisted that resolved off because a requirement is off,
    /// each with that requirement.
    pub forced_off: Vec<(Feature, Feature)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeatureFileError {
    Unknown(String),
    /// `feature` is explicitly on but `requires` resolved off.
    Unmet {
        feature: Feature,
        requires: Feature,
    },
}

impl fmt::Display for FeatureFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(name) => write!(
                f,
                "unknown feature `{name}` (known: {})",
                Feature::ALL.map(Feature::name).join(", ")
            ),
            Self::Unmet { feature, requires } => write!(
                f,
                "feature `{feature}` is set to true but requires `{requires}`, which is off"
            ),
        }
    }
}

impl std::error::Error for FeatureFileError {}

impl FeatureFile {
    /// Applies [`Self::default`] to unlisted features, then turns off every
    /// unlisted feature whose requirement is off.
    ///
    /// # Errors
    ///
    /// An unknown feature name, or a feature set to `true` whose requirement
    /// resolved off.
    pub fn resolve(&self) -> Result<Resolution, FeatureFileError> {
        let mut explicit = BTreeMap::new();
        for (name, on) in &self.features {
            let feature =
                Feature::parse(name).ok_or_else(|| FeatureFileError::Unknown(name.clone()))?;
            explicit.insert(feature, *on);
        }
        let mut set = FeatureSet::none();
        for feature in Feature::ALL {
            if explicit.get(&feature).copied().unwrap_or(self.default) {
                set = set.with(feature);
            }
        }
        let mut forced_off = Vec::new();
        loop {
            let unmet = set.enabled().find_map(|feature| {
                feature.requires().iter().find(|r| !set.contains(**r)).map(|r| (feature, *r))
            });
            let Some((feature, requires)) = unmet else { break };
            if explicit.get(&feature) == Some(&true) {
                return Err(FeatureFileError::Unmet { feature, requires });
            }
            set = set.without(feature);
            forced_off.push((feature, requires));
        }
        Ok(Resolution { set, forced_off })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(text: &str) -> Result<Resolution, String> {
        let file: FeatureFile = toml::from_str(text).map_err(|e| e.to_string())?;
        file.resolve().map_err(|e| e.to_string())
    }

    #[test]
    fn names_round_trip_and_match_serde() {
        for feature in Feature::ALL {
            assert_eq!(Feature::parse(feature.name()), Some(feature));
            assert_eq!(serde_json::to_value(feature).unwrap(), feature.name());
        }
        assert_eq!(Feature::parse("nope"), None);
    }

    #[test]
    fn all_lists_every_variant_once_in_bit_order() {
        for (i, feature) in Feature::ALL.into_iter().enumerate() {
            assert_eq!(feature as usize, i);
        }
        assert_eq!(FeatureSet::all().enabled().count(), Feature::ALL.len());
        assert_eq!(FeatureSet::none().disabled().count(), Feature::ALL.len());
    }

    #[test]
    fn requirements_are_switchable_and_acyclic() {
        for feature in Feature::ALL {
            for req in feature.requires() {
                assert_ne!(*req, feature);
                assert!(req.requires().iter().all(|r| *r != feature), "{feature} <-> {req}");
            }
        }
        for child in [
            Feature::Files,
            Feature::Uploads,
            Feature::Drafts,
            Feature::Profiles,
            Feature::Terminal,
        ] {
            assert_eq!(child.requires(), &[Feature::MachineSpawn]);
        }
    }

    #[test]
    fn empty_file_turns_every_feature_on() {
        let r = resolve("").unwrap();
        assert!(r.set.is_all());
        assert!(r.forced_off.is_empty());
        assert_eq!(FeatureFile::default().resolve().unwrap().set, FeatureSet::all());
    }

    #[test]
    fn default_on_with_listed_exceptions() {
        let r = resolve("default = true\n[features]\ntranscript_search = false\nvoice = false\n")
            .unwrap();
        assert!(!r.set.contains(Feature::TranscriptSearch));
        assert!(!r.set.contains(Feature::Voice));
        assert_eq!(r.set.enabled().count(), Feature::ALL.len() - 2);
    }

    #[test]
    fn default_off_with_listed_exceptions() {
        let r =
            resolve("default = false\n[features]\nmachine_spawn = true\nfiles = true\n").unwrap();
        assert_eq!(r.set.enabled().collect::<Vec<_>>(), [Feature::MachineSpawn, Feature::Files]);
        assert!(r.forced_off.is_empty());
    }

    #[test]
    fn unknown_feature_is_refused_by_name() {
        let err = resolve("[features]\nteleport = false\n").unwrap_err();
        assert!(err.contains("unknown feature `teleport`"), "{err}");
    }

    #[test]
    fn unknown_top_level_key_is_refused() {
        let err = resolve("defaults = false\n").unwrap_err();
        assert!(err.contains("defaults"), "{err}");
    }

    #[test]
    fn wrong_type_is_refused() {
        let err = resolve("[features]\nvoice = \"off\"\n").unwrap_err();
        assert!(err.contains("invalid type"), "{err}");
        let err = resolve("default = 1\n").unwrap_err();
        assert!(err.contains("invalid type"), "{err}");
    }

    #[test]
    fn explicit_child_under_an_off_parent_is_refused_naming_both() {
        let err = resolve("[features]\nmachine_spawn = false\nterminal = true\n").unwrap_err();
        assert!(err.contains("`terminal`") && err.contains("`machine_spawn`"), "{err}");
        let err = resolve("default = false\n[features]\nprofiles = true\n").unwrap_err();
        assert!(err.contains("`profiles`") && err.contains("`machine_spawn`"), "{err}");
    }

    #[test]
    fn unlisted_children_under_an_off_parent_resolve_off() {
        let r = resolve("[features]\nmachine_spawn = false\n").unwrap();
        for child in [
            Feature::Files,
            Feature::Uploads,
            Feature::Drafts,
            Feature::Profiles,
            Feature::Terminal,
        ] {
            assert!(!r.set.contains(child), "{child}");
            assert!(r.forced_off.contains(&(child, Feature::MachineSpawn)), "{child}");
        }
        assert!(r.set.contains(Feature::Voice));
    }

    #[test]
    fn explicit_child_off_under_an_off_parent_is_not_reported_as_forced() {
        let r = resolve("[features]\nmachine_spawn = false\nfiles = false\n").unwrap();
        assert!(!r.forced_off.iter().any(|(f, _)| *f == Feature::Files));
    }

    #[test]
    fn allows_treats_none_as_core() {
        assert!(FeatureSet::none().allows(None));
        assert!(!FeatureSet::none().allows(Some(Feature::Voice)));
        assert!(FeatureSet::all().allows(Some(Feature::Voice)));
    }

    #[test]
    fn display_is_one_line() {
        let set = FeatureSet::all().without(Feature::Voice);
        let line = set.to_string();
        assert!(!line.contains('\n'));
        assert!(line.ends_with("off=[voice]"), "{line}");
    }

    #[test]
    fn shipped_core_file_parses_and_turns_every_switchable_feature_off() {
        let r = resolve(include_str!("../../../deploy/features/core.toml")).unwrap();
        assert_eq!(r.set, FeatureSet::none());
    }
}
