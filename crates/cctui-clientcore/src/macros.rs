//! Running a saved macro as a new session, shared with the web UI's
//! `macros.logic.ts`.

use std::collections::BTreeMap;

use cctui_proto::api::SpawnRequest;

/// A macro as the settings blob stores it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MacroSpec {
    pub id: String,
    pub title: String,
    pub prompt: String,
    pub adapter: String,
    pub machine_id: Option<String>,
    pub working_dir: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub pool_id: Option<String>,
    pub permission_mode: Option<String>,
    /// Ask before launching; the stored default is `true`.
    pub confirm: bool,
}

/// What a macro is missing before it can run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacroProblem {
    Title,
    Prompt,
    Machine,
    Cwd,
}

impl MacroProblem {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Prompt => "prompt",
            Self::Machine => "machine",
            Self::Cwd => "cwd",
        }
    }
}

#[must_use]
pub fn macro_problems(mac: &MacroSpec) -> Vec<MacroProblem> {
    let mut out = Vec::new();
    if mac.title.trim().is_empty() {
        out.push(MacroProblem::Title);
    }
    if mac.prompt.trim().is_empty() {
        out.push(MacroProblem::Prompt);
    }
    if mac.machine_id.is_none() {
        out.push(MacroProblem::Machine);
    }
    if mac.working_dir.as_deref().map(str::trim).unwrap_or_default().is_empty() {
        out.push(MacroProblem::Cwd);
    }
    out
}

fn trimmed(value: Option<&str>) -> Option<String> {
    value.map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned)
}

/// The spawn a macro launches: its own knobs, plus `auto_archive` so the
/// server files the session away once its turn ends cleanly.
#[must_use]
pub fn macro_spawn_body(mac: &MacroSpec) -> SpawnRequest {
    let pool = trimmed(mac.pool_id.as_deref());
    SpawnRequest {
        machine_id: mac.machine_id.clone().unwrap_or_default(),
        working_dir: mac.working_dir.as_deref().unwrap_or_default().trim().to_owned(),
        prompt: trimmed(Some(&mac.prompt)),
        prompt_name: None,
        name: trimmed(Some(&mac.title)),
        adapter_id: Some(if mac.adapter.is_empty() {
            "claude-code".to_owned()
        } else {
            mac.adapter.clone()
        }),
        permission_mode: trimmed(mac.permission_mode.as_deref())
            .and_then(|mode| serde_json::from_value(serde_json::Value::String(mode)).ok()),
        effort: trimmed(mac.effort.as_deref()),
        model: trimmed(mac.model.as_deref()),
        service_tier: None,
        env: BTreeMap::new(),
        account: None,
        provider: None,
        no_account: false,
        auto_account: pool.is_none(),
        pool,
        save_draft: false,
        auto_archive: true,
        env_keys: Vec::new(),
        attachment_names: Vec::new(),
        label_ids: Vec::new(),
        spawn_capability: None,
        relation: None,
        parent_session_id: None,
        context: None,
        profile_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::{MacroProblem, MacroSpec, macro_problems, macro_spawn_body};

    fn mac() -> MacroSpec {
        MacroSpec {
            id: "m1".to_owned(),
            title: " Triage ".to_owned(),
            prompt: " triage the inbox ".to_owned(),
            adapter: "codex".to_owned(),
            machine_id: Some("m-1".to_owned()),
            working_dir: Some(" /w ".to_owned()),
            model: Some("gpt-5.6".to_owned()),
            effort: Some("high".to_owned()),
            pool_id: None,
            permission_mode: Some("yolo".to_owned()),
            confirm: true,
        }
    }

    #[test]
    fn a_macro_spawns_its_own_knobs_and_archives_itself() {
        let body = macro_spawn_body(&mac());
        assert_eq!(body.machine_id, "m-1");
        assert_eq!(body.working_dir, "/w");
        assert_eq!(body.name.as_deref(), Some("Triage"));
        assert_eq!(body.prompt.as_deref(), Some("triage the inbox"));
        assert_eq!(body.adapter_id.as_deref(), Some("codex"));
        assert_eq!(body.model.as_deref(), Some("gpt-5.6"));
        assert_eq!(body.effort.as_deref(), Some("high"));
        assert!(body.auto_archive, "a macro session files itself away");
        assert!(body.auto_account, "no pool means the server elects an account");
        assert!(!body.save_draft);
        assert!(body.env.is_empty());
    }

    #[test]
    fn a_pool_takes_over_from_auto_account() {
        let pooled = MacroSpec { pool_id: Some(" work ".to_owned()), ..mac() };
        let body = macro_spawn_body(&pooled);
        assert_eq!(body.pool.as_deref(), Some("work"));
        assert!(!body.auto_account, "the pool is the choice");
    }

    #[test]
    fn a_blank_harness_falls_back_to_claude_code() {
        let bare = MacroSpec { adapter: String::new(), ..mac() };
        assert_eq!(macro_spawn_body(&bare).adapter_id.as_deref(), Some("claude-code"));
    }

    #[test]
    fn an_incomplete_macro_names_what_it_is_missing() {
        assert!(macro_problems(&mac()).is_empty());
        let empty = MacroSpec::default();
        assert_eq!(
            macro_problems(&empty),
            vec![
                MacroProblem::Title,
                MacroProblem::Prompt,
                MacroProblem::Machine,
                MacroProblem::Cwd
            ]
        );
        let blank_cwd = MacroSpec { working_dir: Some("  ".to_owned()), ..mac() };
        assert_eq!(macro_problems(&blank_cwd), vec![MacroProblem::Cwd]);
    }
}
