//! The features the connected server runs with. Process-global like
//! [`crate::servercaps`]; a server that announces nothing has every feature on.

use std::sync::RwLock;

use cctui_proto::features::{Feature, FeatureSet};

static ANNOUNCED: RwLock<FeatureSet> = RwLock::new(FeatureSet::all());

fn store(features: FeatureSet) {
    let mut slot = ANNOUNCED.write().unwrap_or_else(std::sync::PoisonError::into_inner);
    *slot = features;
}

pub fn reset() {
    store(FeatureSet::all());
}

pub fn announce(off: &[String]) {
    let features = from_off(off);
    tracing::info!(%features, "server features announced");
    store(features);
}

#[must_use]
pub fn current() -> FeatureSet {
    *ANNOUNCED.read().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[must_use]
pub fn from_off(off: &[String]) -> FeatureSet {
    let set = off
        .iter()
        .filter_map(|name| Feature::parse(name))
        .fold(FeatureSet::all(), FeatureSet::without);
    Feature::ALL
        .into_iter()
        .filter(|f| f.requires().iter().any(|r| !set.contains(*r)))
        .fold(set, FeatureSet::without)
}

// `announce` / `reset` are not tested here: they mutate state the whole test
// binary reads. Callers take a `FeatureSet` so they are tested without it.
#[cfg(test)]
mod tests {
    use super::{Feature, FeatureSet, from_off};

    fn names(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn nothing_off_is_every_feature_on() {
        assert!(from_off(&[]).is_all());
    }

    #[test]
    fn named_features_are_off_and_unknown_names_are_skipped() {
        let set = from_off(&names(&["rooms", "from_the_future"]));
        assert!(!set.contains(Feature::Rooms));
        assert_eq!(set, FeatureSet::all().without(Feature::Rooms));
    }

    #[test]
    fn a_feature_whose_prerequisite_is_off_is_off() {
        let set = from_off(&names(&["machine_spawn"]));
        assert!(!set.contains(Feature::Files));
        assert!(set.contains(Feature::Rooms));
    }
}
