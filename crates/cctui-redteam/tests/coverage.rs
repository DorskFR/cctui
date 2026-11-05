//! The ADR ratchet, checked in-process: every probe belongs to a taxonomy
//! class and every class with probes has a scenario that lists each of them.

use std::collections::BTreeMap;
use std::path::PathBuf;

const CLASSES: &[&str] = &["EGR", "LAT", "SEC", "PRIV", "GUARD", "AUTH", "INJ", "JUDGE", "RES"];

fn redteam_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("redteam")
}

fn probe_ids() -> BTreeMap<String, Vec<String>> {
    let mut by_class: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in std::fs::read_dir(redteam_dir().join("probes")).expect("probes dir") {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        if !name.ends_with(".sh") || name == "lib.sh" || name == "run-all.sh" {
            continue;
        }
        let stem = name.trim_end_matches(".sh");
        let mut parts = stem.splitn(3, '-');
        let class = parts.next().unwrap_or_default().to_owned();
        let num = parts.next().unwrap_or_default();
        assert!(CLASSES.contains(&class.as_str()), "{name}: unknown class `{class}`");
        assert!(num.chars().all(|c| c.is_ascii_digit()) && !num.is_empty(), "{name}: bad id");
        by_class.entry(class.clone()).or_default().push(format!("{class}-{num}"));
    }
    by_class
}

#[test]
fn every_probe_has_a_valid_taxonomy_id() {
    let by_class = probe_ids();
    assert!(!by_class.is_empty(), "no probes found");
}

#[test]
fn every_probed_class_has_a_scenario_listing_each_of_its_probes() {
    for (class, ids) in probe_ids() {
        let path = redteam_dir().join("scenarios").join(format!("{class}.yaml"));
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("class {class} has probes but no scenarios/{class}.yaml"));
        for id in ids {
            assert!(text.contains(&id), "scenarios/{class}.yaml does not list {id}");
        }
    }
}
