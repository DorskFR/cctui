use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use super::*;
use crate::testing::{Fixture, KINDS, capture_logs, eventually};

fn no_ttl() -> TtlLookup {
    Arc::new(|_: &str| None)
}

fn ttl_env(vars: &[(&str, &str)]) -> TtlLookup {
    let vars: HashMap<String, String> =
        vars.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
    Arc::new(move |k: &str| vars.get(k).cloned())
}

fn registry_for(fixture: &Fixture, ttl: TtlLookup) -> SecretRegistry {
    SecretRegistry::new(fixture.resolver()).with_ttl_lookup(ttl)
}

#[tokio::test]
async fn literal_is_fixed_and_not_tracked() {
    let fixture = Fixture::start().await;
    let registry = registry_for(&fixture, no_ttl());
    let secret = registry.register("CCTUI_LANGFUSE_SECRET_KEY", "sk-literal").await.unwrap();
    assert_eq!(&*secret.get(), "sk-literal");
    assert!(secret.reference().is_none());
    assert!(registry.get("CCTUI_LANGFUSE_SECRET_KEY").is_none());
    assert_eq!(registry.reload().await, ReloadReport::default());
    let escaped = registry.register("X", "literal:file:/etc/x").await.unwrap();
    assert_eq!(&*escaped.get(), "file:/etc/x");
}

#[tokio::test]
async fn every_kind_resolves_and_rotates_on_reload() {
    let fixture = Fixture::start().await;
    let registry = registry_for(&fixture, no_ttl());
    let mut secrets = Vec::new();
    for kind in KINDS {
        let key = format!("rot-{}", kind.replace('-', ""));
        fixture.set(&key, &format!("{kind}-v1"));
        let secret = registry.register(kind, &fixture.reference(kind, &key)).await.unwrap();
        assert_eq!(&*secret.get(), format!("{kind}-v1"), "{kind}");
        secrets.push((kind, key, secret));
    }
    let mut watchers: Vec<_> = secrets.iter().map(|(_, _, s)| s.subscribe()).collect();
    for (kind, key, _) in &secrets {
        fixture.set(key, &format!("{kind}-v2"));
    }
    assert_eq!(registry.reload().await, ReloadReport { changed: 4, unchanged: 0, failed: 0 });
    for (kind, _, secret) in &secrets {
        assert_eq!(&*secret.get(), format!("{kind}-v2"), "{kind}");
        assert_eq!(&*registry.get(kind).unwrap().get(), format!("{kind}-v2"), "{kind}");
    }
    for watcher in &mut watchers {
        assert!(watcher.has_changed().unwrap(), "subscribers see the swap");
    }
    assert_eq!(registry.reload().await, ReloadReport { changed: 0, unchanged: 4, failed: 0 });
}

#[tokio::test]
async fn failed_refresh_keeps_the_old_value_and_logs_only_name_and_ref() {
    let fixture = Fixture::start().await;
    let registry = registry_for(&fixture, no_ttl());
    for kind in KINDS {
        let key = format!("fail-{}", kind.replace('-', ""));
        let old = format!("old-secret-{kind}");
        fixture.set(&key, &old);
        let reference = fixture.reference(kind, &key);
        let name = format!("VAR_{}", kind.replace('-', "_").to_uppercase());
        let secret = registry.register(&name, &reference).await.unwrap();

        fixture.break_key(&key);
        let logs = capture_logs();
        let report = registry.reload().await;
        let text = logs.contents();
        drop(logs);
        assert!(report.failed >= 1, "{kind}: {report:?}");
        assert_eq!(&*secret.get(), old, "{kind}: a failed refresh keeps the old value");
        assert!(text.contains(&name), "{kind}: log names the variable: {text}");
        let shown = SecretRef::parse(&reference).unwrap().to_string();
        assert!(text.contains(&shown), "{kind}: log names the ref: {text}");
        assert!(!text.contains(&old), "{kind}: log leaks the value: {text}");
        fixture.set(&key, &format!("new-secret-{kind}"));
        registry.reload().await;
        assert_eq!(&*secret.get(), format!("new-secret-{kind}"), "{kind}: recovers");
    }
}

#[tokio::test]
async fn rotation_logs_never_carry_values() {
    let fixture = Fixture::start().await;
    let registry = registry_for(&fixture, no_ttl());
    fixture.set("logged", "first-value");
    let reference = fixture.reference("k8s", "logged");
    registry.register("CCTUI_VAULT_KEY_LIKE", &reference).await.unwrap();
    fixture.set("logged", "second-value");
    let logs = capture_logs();
    registry.reload().await;
    let text = logs.contents();
    assert!(text.contains("CCTUI_VAULT_KEY_LIKE"), "{text}");
    assert!(!text.contains("first-value") && !text.contains("second-value"), "{text}");
}

#[tokio::test]
async fn register_failure_names_the_variable_and_ref_not_a_value() {
    let fixture = Fixture::start().await;
    let registry = registry_for(&fixture, no_ttl());
    for kind in KINDS {
        let reference = fixture.reference(kind, "never-set");
        let msg = registry.register("CCTUI_LANGFUSE_SECRET_KEY", &reference).await.unwrap_err();
        let msg = format!("{msg:#}");
        assert!(msg.contains("CCTUI_LANGFUSE_SECRET_KEY"), "{kind}: {msg}");
        let shown = SecretRef::parse(&reference).unwrap().to_string();
        assert!(msg.contains(&shown), "{kind}: {msg}");
    }
    let msg = registry.register("DATABASE_URL", "k8s:no-key").await.unwrap_err().to_string();
    assert!(msg.starts_with("DATABASE_URL: "), "{msg}");
}

#[tokio::test]
async fn ttl_refresh_rotates_only_refs_with_a_ttl() {
    let fixture = Fixture::start().await;
    let ttl = ttl_env(&[("CCTUI_SECRETS_TTL_DATABASE_URL", "1")]);
    let registry = registry_for(&fixture, ttl);
    let mut with_ttl = Vec::new();
    for kind in KINDS {
        let key = format!("ttl-{}", kind.replace('-', ""));
        fixture.set(&key, "v1");
        let secret = registry.register("DATABASE_URL", &fixture.reference(kind, &key)).await;
        with_ttl.push((kind, secret.unwrap()));
        fixture.set(&key, "v2");
    }
    fixture.set("ttl-off", "v1");
    let reference = fixture.reference("file", "ttl-off");
    let without = registry.register("CCTUI_EMOJI_TOKEN", &reference).await.unwrap();
    fixture.set("ttl-off", "v2");

    assert_eq!(registry.reload_due().await, ReloadReport::default(), "TTL not elapsed yet");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(registry.reload_due().await, ReloadReport { changed: 4, unchanged: 0, failed: 0 });
    for (kind, secret) in &with_ttl {
        assert_eq!(&*secret.get(), "v2", "{kind}");
    }
    assert_eq!(&*without.get(), "v1", "no TTL configured: only SIGHUP refreshes it");
}

#[tokio::test]
async fn reloader_follows_the_ttl_without_a_signal() {
    let fixture = Fixture::start().await;
    let registry = Arc::new(registry_for(&fixture, ttl_env(&[("CCTUI_SECRETS_TTL", "1")])));
    fixture.set("loop", "v1");
    let reference = fixture.reference("aws-sm", "loop");
    let secret = registry.register("CCTUI_GITHUB_TOKEN", &reference).await.unwrap();
    let task = registry.spawn_reloader();
    fixture.set("loop", "v2");
    assert!(eventually(Duration::from_secs(5), || &*secret.get() == "v2").await);
    task.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn sighup_reloads_every_kind() {
    let fixture = Fixture::start().await;
    let registry = Arc::new(registry_for(&fixture, no_ttl()));
    let mut secrets = Vec::new();
    for kind in KINDS {
        let key = format!("hup-{}", kind.replace('-', ""));
        fixture.set(&key, "v1");
        secrets.push(registry.register(kind, &fixture.reference(kind, &key)).await.unwrap());
        fixture.set(&key, "v2");
    }
    let task = registry.spawn_reloader();
    let status = std::process::Command::new("kill")
        .args(["-HUP", &std::process::id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(eventually(Duration::from_secs(5), || secrets.iter().all(|s| &*s.get() == "v2")).await);
    task.abort();
}

#[test]
fn ttl_config_per_name_wins_zero_is_off_and_garbage_names_the_var() {
    let registry = SecretRegistry::from_env().with_ttl_lookup(ttl_env(&[
        ("CCTUI_SECRETS_TTL", "300"),
        ("CCTUI_SECRETS_TTL_DATABASE_URL", "30"),
        ("CCTUI_SECRETS_TTL_DISPATCHER_KEY", "0"),
        ("CCTUI_SECRETS_TTL_CCTUI_NTFY_TOKEN", "soon"),
    ]));
    assert_eq!(registry.ttl_for("DATABASE_URL").unwrap(), Some(Duration::from_secs(30)));
    assert_eq!(registry.ttl_for("CCTUI_GITHUB_TOKEN").unwrap(), Some(Duration::from_mins(5)));
    assert_eq!(registry.ttl_for("dispatcher_key").unwrap(), None);
    let msg = registry.ttl_for("CCTUI_NTFY_TOKEN").unwrap_err().to_string();
    assert!(msg.contains("CCTUI_SECRETS_TTL_CCTUI_NTFY_TOKEN"), "{msg}");
    let off = SecretRegistry::from_env().with_ttl_lookup(no_ttl());
    assert_eq!(off.ttl_for("DATABASE_URL").unwrap(), None, "TTL is off by default");
}

#[test]
fn secret_debug_redacts_the_value() {
    let shown = format!("{:?}", Secret::fixed("CCTUI_VAULT_KEY", "deadbeef"));
    assert!(!shown.contains("deadbeef"), "{shown}");
    assert!(shown.contains("CCTUI_VAULT_KEY"), "{shown}");
}
