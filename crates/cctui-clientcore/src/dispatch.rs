//! The dispatch request body, shared with the web UI's `dispatchBody.ts`.
//!
//! The worker reads its task out of `payload`, which the server treats as
//! opaque; an empty field is omitted so the worker's own default applies. Both
//! clients build it here so the same choices cannot produce two bodies.

use serde_json::{Map, Value};

// The account/pool sentinels and the provider rule are spawn-wide, so `spawn`
// owns them and this module uses them rather than keeping a second copy: two
// copies of a parity-governed rule is what the fixtures exist to prevent.
pub use super::spawn::{NO_ACCOUNT, POOL_PREFIX, is_compatible_provider, pool_name};

/// Context-pack field → the env var the worker entrypoint reads. Fixed keys, in
/// the order the web UI writes them.
pub const CONTEXT_PACK_ENV: [(&str, &str); 4] = [
    ("context_pack_url", "CONTEXT_PACK_URL"),
    ("context_pack_ref", "CONTEXT_PACK_REF"),
    ("context_pack_subdir", "CONTEXT_PACK_SUBDIR"),
    ("context_pack_token", "CONTEXT_PACK_TOKEN"),
];

/// The four context-pack fields of the spawn form.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextPack {
    pub url: String,
    pub r#ref: String,
    pub subdir: String,
    /// Never persisted and never logged: it is a git credential.
    pub token: String,
}

impl ContextPack {
    fn field(&self, name: &str) -> &str {
        match name {
            "context_pack_url" => &self.url,
            "context_pack_ref" => &self.r#ref,
            "context_pack_subdir" => &self.subdir,
            "context_pack_token" => &self.token,
            _ => "",
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        context_pack_env(self).is_empty()
    }
}

/// The `CONTEXT_PACK_*` entries for the filled-in fields.
#[must_use]
pub fn context_pack_env(pack: &ContextPack) -> Vec<(String, String)> {
    CONTEXT_PACK_ENV
        .iter()
        .filter_map(|(field, key)| {
            let value = pack.field(field).trim();
            (!value.is_empty()).then(|| ((*key).to_owned(), value.to_owned()))
        })
        .collect()
}

/// Everything the dispatch body reads off the form.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DispatchForm {
    pub dispatcher: String,
    /// `claude-code` when empty; any other harness names its adapter in the payload.
    pub dispatch_adapter: String,
    pub name: String,
    pub identity: String,
    pub repo: String,
    pub ticket: String,
    pub prompt: String,
    pub prompt_file: String,
    pub model_claude: String,
    pub model_codex: String,
    /// The model id a `-compatible` provider was configured with.
    pub model_account: String,
    pub effort_claude: String,
    pub effort_codex: String,
    /// Minutes, as typed; anything unparseable means "the runtime default".
    pub timeout: String,
    /// The account picker's value, sentinels included.
    pub account: String,
}

impl DispatchForm {
    fn adapter(&self) -> &str {
        if self.dispatch_adapter.is_empty() { "claude-code" } else { &self.dispatch_adapter }
    }

    fn is_codex(&self) -> bool {
        self.adapter() == "codex"
    }

    /// Which model field applies: a `-compatible` provider overrides the
    /// harness's own, else the harness picks its family's field.
    fn model(&self, provider: Option<&str>) -> &str {
        if provider.is_some_and(is_compatible_provider) {
            return self.model_account.trim();
        }
        if self.is_codex() { self.model_codex.trim() } else { self.model_claude.trim() }
    }

    fn effort(&self) -> &str {
        if self.is_codex() { self.effort_codex.trim() } else { self.effort_claude.trim() }
    }

    /// `None` for Auto, the no-account sentinel and a pool pick: a pool is
    /// resolved to one of its members server-side.
    fn account(&self) -> Option<String> {
        if self.account == NO_ACCOUNT || pool_name(&self.account).is_some() {
            return None;
        }
        let trimmed = self.account.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    }

    fn timeout(&self) -> Option<u32> {
        self.timeout.trim().parse::<u32>().ok()
    }
}

/// An omitted field is a field the worker defaults, so a blank never lands.
fn put(payload: &mut Map<String, Value>, key: &str, value: &str) {
    let value = value.trim();
    if !value.is_empty() {
        payload.insert(key.to_owned(), Value::String(value.to_owned()));
    }
}

/// The payload the dispatcher unpacks into `TASK_*` env.
///
/// `env` is the form's own rows; the context pack is spread over them, so a
/// filled-in field wins over a hand-typed duplicate of the same key.
#[must_use]
pub fn dispatch_payload(
    form: &DispatchForm,
    env: &[(String, String)],
    pack: &ContextPack,
    provider: Option<&str>,
) -> Value {
    let mut payload = Map::new();
    put(&mut payload, "name", &form.name);
    put(&mut payload, "identity", &form.identity);
    put(&mut payload, "repo", &form.repo);
    if !form.ticket.trim().is_empty() {
        let mut context = Map::new();
        context.insert("issue_id".to_owned(), Value::String(form.ticket.trim().to_owned()));
        payload.insert("context".to_owned(), Value::Object(context));
    }
    put(&mut payload, "prompt", &form.prompt);
    put(&mut payload, "prompt_file", &form.prompt_file);
    if form.adapter() != "claude-code" {
        payload.insert("adapter".to_owned(), Value::String(form.adapter().to_owned()));
    }
    put(&mut payload, "model", form.model(provider));
    put(&mut payload, "effort", form.effort());

    let mut full_env: Map<String, Value> =
        env.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
    for (key, value) in context_pack_env(pack) {
        full_env.insert(key, Value::String(value));
    }
    if !full_env.is_empty() {
        payload.insert("env".to_owned(), Value::Object(full_env));
    }
    Value::Object(payload)
}

/// The whole `POST /dispatch` body. `session_id` doubles as the idempotency
/// key, so resubmitting the same one returns the session already running.
#[must_use]
pub fn build_dispatch_body(
    form: &DispatchForm,
    env: &[(String, String)],
    pack: &ContextPack,
    provider: Option<&str>,
    session_id: &str,
) -> Value {
    let mut body = Map::new();
    body.insert("dispatcher".to_owned(), Value::String(form.dispatcher.clone()));
    body.insert("session_id".to_owned(), Value::String(session_id.to_owned()));
    body.insert(
        "timeout".to_owned(),
        form.timeout().map_or(Value::Null, |t| Value::Number(t.into())),
    );
    body.insert("reply_url".to_owned(), Value::Null);
    body.insert("notify_url".to_owned(), Value::Null);
    body.insert("notify_secret".to_owned(), Value::Null);
    body.insert("account".to_owned(), form.account().map_or(Value::Null, Value::String));
    body.insert(
        "provider".to_owned(),
        provider.map_or(Value::Null, |p| Value::String(p.to_owned())),
    );
    body.insert("accounts".to_owned(), Value::Array(Vec::new()));
    body.insert("payload".to_owned(), dispatch_payload(form, env, pack, provider));
    Value::Object(body)
}

/// Where a remembered dispatch form is keyed: per dispatcher and repo, as the
/// web UI remembers it.
#[must_use]
pub fn memory_key(dispatcher: &str, repo: &str) -> String {
    format!("{}/{}", dispatcher.trim(), repo.trim())
}

#[cfg(test)]
mod tests {
    use super::{
        ContextPack, DispatchForm, NO_ACCOUNT, POOL_PREFIX, build_dispatch_body, context_pack_env,
        is_compatible_provider, memory_key, pool_name,
    };

    fn pack() -> ContextPack {
        ContextPack {
            url: "  https://git/packs.git ".to_owned(),
            r#ref: "main".to_owned(),
            subdir: String::new(),
            token: "secret".to_owned(),
        }
    }

    #[test]
    fn the_context_pack_env_skips_the_blank_fields_and_trims() {
        assert_eq!(
            context_pack_env(&pack()),
            vec![
                ("CONTEXT_PACK_URL".to_owned(), "https://git/packs.git".to_owned()),
                ("CONTEXT_PACK_REF".to_owned(), "main".to_owned()),
                ("CONTEXT_PACK_TOKEN".to_owned(), "secret".to_owned()),
            ]
        );
        assert!(ContextPack::default().is_empty());
        assert!(!pack().is_empty());
    }

    /// An explicit field beats a hand-typed row with the same key.
    #[test]
    fn a_context_pack_field_wins_over_a_duplicate_env_row() {
        let form = DispatchForm { dispatcher: "k8s".to_owned(), ..DispatchForm::default() };
        let env = vec![("CONTEXT_PACK_REF".to_owned(), "typed".to_owned())];
        let body = build_dispatch_body(&form, &env, &pack(), None, "s-1");
        assert_eq!(body["payload"]["env"]["CONTEXT_PACK_REF"], "main");
    }

    #[test]
    fn an_empty_form_omits_everything_the_worker_can_default() {
        let form = DispatchForm { dispatcher: "k8s".to_owned(), ..DispatchForm::default() };
        let body = build_dispatch_body(&form, &[], &ContextPack::default(), None, "s-1");
        assert_eq!(body["payload"], serde_json::json!({}));
        assert_eq!(body["timeout"], serde_json::Value::Null);
        assert_eq!(body["account"], serde_json::Value::Null);
        assert_eq!(body["accounts"], serde_json::json!([]));
    }

    #[test]
    fn codex_names_its_adapter_and_takes_its_own_model_and_effort() {
        let form = DispatchForm {
            dispatcher: "k8s".to_owned(),
            dispatch_adapter: "codex".to_owned(),
            model_claude: "opus".to_owned(),
            model_codex: "gpt-5.6-sol".to_owned(),
            effort_claude: "high".to_owned(),
            effort_codex: "low".to_owned(),
            ..DispatchForm::default()
        };
        let body = build_dispatch_body(&form, &[], &ContextPack::default(), None, "s-1");
        assert_eq!(body["payload"]["adapter"], "codex");
        assert_eq!(body["payload"]["model"], "gpt-5.6-sol");
        assert_eq!(body["payload"]["effort"], "low");
    }

    #[test]
    fn claude_code_is_the_default_adapter_and_is_not_named() {
        let form = DispatchForm {
            dispatcher: "k8s".to_owned(),
            model_claude: "opus".to_owned(),
            ..DispatchForm::default()
        };
        let body = build_dispatch_body(&form, &[], &ContextPack::default(), None, "s-1");
        assert!(body["payload"].get("adapter").is_none());
        assert_eq!(body["payload"]["model"], "opus");
    }

    #[test]
    fn a_compatible_provider_overrides_the_harness_model() {
        let form = DispatchForm {
            dispatcher: "k8s".to_owned(),
            model_claude: "opus".to_owned(),
            model_account: "llama-70b".to_owned(),
            ..DispatchForm::default()
        };
        let body = build_dispatch_body(
            &form,
            &[],
            &ContextPack::default(),
            Some("anthropic-compatible"),
            "s-1",
        );
        assert_eq!(body["payload"]["model"], "llama-70b");
        assert_eq!(body["provider"], "anthropic-compatible");
        assert!(is_compatible_provider("openai-compatible"));
        assert!(!is_compatible_provider("anthropic"));
    }

    #[test]
    fn the_sentinel_and_a_pool_pick_both_mean_no_account_on_the_wire() {
        let base = DispatchForm { dispatcher: "k8s".to_owned(), ..DispatchForm::default() };
        for value in [NO_ACCOUNT.to_owned(), format!("{POOL_PREFIX}fleet"), String::new()] {
            let form = DispatchForm { account: value.clone(), ..base.clone() };
            let body = build_dispatch_body(&form, &[], &ContextPack::default(), None, "s-1");
            assert_eq!(body["account"], serde_json::Value::Null, "{value:?}");
        }
        let form = DispatchForm { account: " main ".to_owned(), ..base };
        let body = build_dispatch_body(&form, &[], &ContextPack::default(), None, "s-1");
        assert_eq!(body["account"], "main");
        assert_eq!(pool_name(&format!("{POOL_PREFIX}fleet")), Some("fleet"));
        assert_eq!(pool_name("main"), None);
    }

    #[test]
    fn an_unparseable_timeout_is_the_runtime_default() {
        let base = DispatchForm { dispatcher: "k8s".to_owned(), ..DispatchForm::default() };
        for value in ["", "   ", "soon", "-5", "9.5"] {
            let form = DispatchForm { timeout: value.to_owned(), ..base.clone() };
            let body = build_dispatch_body(&form, &[], &ContextPack::default(), None, "s-1");
            assert_eq!(body["timeout"], serde_json::Value::Null, "{value:?}");
        }
        let form = DispatchForm { timeout: " 60 ".to_owned(), ..base };
        let body = build_dispatch_body(&form, &[], &ContextPack::default(), None, "s-1");
        assert_eq!(body["timeout"], 60);
    }

    #[test]
    fn a_ticket_becomes_the_context_object_the_worker_reads() {
        let form = DispatchForm {
            dispatcher: "k8s".to_owned(),
            ticket: " CCT-1102 ".to_owned(),
            ..DispatchForm::default()
        };
        let body = build_dispatch_body(&form, &[], &ContextPack::default(), None, "s-1");
        assert_eq!(body["payload"]["context"], serde_json::json!({"issue_id": "CCT-1102"}));
    }

    #[test]
    fn the_memory_is_keyed_by_dispatcher_and_repo() {
        assert_eq!(memory_key(" k8s ", " cctui "), "k8s/cctui");
    }
}
