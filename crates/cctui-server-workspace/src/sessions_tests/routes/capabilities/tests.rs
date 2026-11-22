use super::*;

#[test]
fn a_core_server_announces_only_always_on_features() {
    let core =
        crate::config::parse_features(include_str!("../../../../../../deploy/features/core.toml"))
            .unwrap()
            .set;
    let caps = CapabilitiesResponse::full(LangfuseCapability::default(), core);
    let v = serde_json::to_value(&caps).unwrap();
    assert_eq!(v["workspace"], true);
    assert_eq!(v["distribution"], "full");
    let features: Vec<&str> =
        v["features"].as_array().unwrap().iter().filter_map(|f| f.as_str()).collect();
    assert!(features.contains(&"dispatch") && features.contains(&"workspace"));
    for feature in cctui_proto::features::Feature::ALL {
        assert!(!features.contains(&feature.name()), "{feature} announced on core");
        assert!(!caps.has(feature), "{feature}");
    }
    assert_eq!(
        v["switchable"].as_array().unwrap().len(),
        cctui_proto::features::Feature::ALL.len()
    );
}
