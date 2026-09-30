//! The `/api/v1` route table, shared by the server, the TUI and the webui.
//!
//! The server asserts its router against [`ROUTES`] (every registered route is
//! in the table and every table entry is registered), so a renamed path cannot
//! strand a client. The same table is emitted to TypeScript as `routes.ts`.

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

/// Prefix every [`Route::path`] hangs under.
pub const API_PREFIX: &str = "/api/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export, rename = "ApiMethod"))]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl Method {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }

    /// `None` for a method the API does not use.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "GET" => Some(Self::Get),
            "POST" => Some(Self::Post),
            "PUT" => Some(Self::Put),
            "PATCH" => Some(Self::Patch),
            "DELETE" => Some(Self::Delete),
            _ => None,
        }
    }
}

impl std::fmt::Display for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One `/api/v1` endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export, rename = "ApiRoute"))]
pub struct Route {
    /// Stable identifier, `{method}_{path}` slugged. What a client names.
    pub id: &'static str,
    pub method: Method,
    /// Path template under [`API_PREFIX`], with `{param}` placeholders.
    pub path: &'static str,
    pub summary: &'static str,
    /// Exported TS name of the JSON request body, when the route reads one.
    pub request: Option<&'static str>,
    /// Exported TS name of the JSON response body.
    pub response: Option<&'static str>,
}

impl Route {
    /// [`Self::path`] with `{param}` placeholders substituted from `params`,
    /// without [`API_PREFIX`]. An unknown placeholder is left in place rather
    /// than silently dropped, so a wrong call is visible in the request log
    /// instead of hitting the wrong route.
    #[must_use]
    pub fn path_with(&self, params: &[(&str, &str)]) -> String {
        let mut out = String::with_capacity(self.path.len());
        let mut rest = self.path;
        while let Some(open) = rest.find('{') {
            let Some(close) = rest[open..].find('}').map(|i| open + i) else { break };
            out.push_str(&rest[..open]);
            let name = &rest[open + 1..close];
            match params.iter().find(|(k, _)| *k == name) {
                Some((_, v)) => out.push_str(v),
                None => out.push_str(&rest[open..=close]),
            }
            rest = &rest[close + 1..];
        }
        out.push_str(rest);
        out
    }

    /// [`Self::path_with`] under [`API_PREFIX`]: what a client requests.
    #[must_use]
    pub fn url(&self, params: &[(&str, &str)]) -> String {
        format!("{API_PREFIX}{}", self.path_with(params))
    }

    /// Placeholder names in the order they appear.
    #[must_use]
    pub fn params(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        let mut rest = self.path;
        while let Some(open) = rest.find('{') {
            let Some(close) = rest[open..].find('}').map(|i| open + i) else { break };
            out.push(&rest[open + 1..close]);
            rest = &rest[close + 1..];
        }
        out
    }
}

/// The route with this id.
#[must_use]
pub fn by_id(id: &str) -> Option<&'static Route> {
    ROUTES.iter().find(|r| r.id == id)
}

/// The route registered for this exact method and path template.
#[must_use]
pub fn find(method: Method, path: &str) -> Option<&'static Route> {
    ROUTES.iter().find(|r| r.method == method && r.path == path)
}

/// Every `/api/v1` route, as the server registers them.
pub const ROUTES: &[Route] = &[
    Route {
        id: "post_accounts_by_id_providers",
        method: Method::Post,
        path: "/accounts/{id}/providers",
        summary: "Attach a provider credential to an account.",
        request: None,
        response: None,
    },
    Route {
        id: "patch_accounts_by_id_providers_by_provider",
        method: Method::Patch,
        path: "/accounts/{id}/providers/{provider_id}",
        summary: "Edit or remove one of an account's provider credentials.",
        request: None,
        response: None,
    },
    Route {
        id: "delete_accounts_by_id_providers_by_provider",
        method: Method::Delete,
        path: "/accounts/{id}/providers/{provider_id}",
        summary: "Edit or remove one of an account's provider credentials.",
        request: None,
        response: None,
    },
    Route {
        id: "post_accounts_by_id_providers_by_provider_move",
        method: Method::Post,
        path: "/accounts/{id}/providers/{provider_id}/move",
        summary: "Move a provider credential to another account of the same owner.",
        request: None,
        response: None,
    },
    Route {
        id: "get_accounts_usage",
        method: Method::Get,
        path: "/accounts/usage",
        summary: "Usage windows of every provider credential the caller owns, in one call.",
        request: None,
        response: Some("AccountUsageEntry[]"),
    },
    Route {
        id: "get_accounts_by_id_usage",
        method: Method::Get,
        path: "/accounts/{id}/usage",
        summary: "Get an account's usage/limits.",
        request: None,
        response: None,
    },
    Route {
        id: "get_accounts_by_id_usage_history",
        method: Method::Get,
        path: "/accounts/{id}/usage/history",
        summary: "Sampled usage of one provider credential over time.",
        request: None,
        response: None,
    },
    Route {
        id: "get_accounts_by_id_usage_closes",
        method: Method::Get,
        path: "/accounts/{id}/usage/closes",
        summary: "Closed usage windows of one provider credential, with unused share.",
        request: None,
        response: None,
    },
    Route {
        id: "get_accounts_usage_closes",
        method: Method::Get,
        path: "/accounts/usage/closes",
        summary: "Closed usage windows of every owned credential, with unused share.",
        request: None,
        response: None,
    },
    Route {
        id: "post_accounts_by_id_limit_reset",
        method: Method::Post,
        path: "/accounts/{id}/limit-reset",
        summary: "Claim a usage-limit reset on a provider credential.",
        request: None,
        response: None,
    },
    Route {
        id: "get_accounts_settings_catalog",
        method: Method::Get,
        path: "/accounts/settings-catalog",
        summary: "The per-account settings catalog (exposable keys, env allowlist, preset).",
        request: None,
        response: Some("SettingsCatalogResponse"),
    },
    Route {
        id: "get_accounts",
        method: Method::Get,
        path: "/accounts",
        summary: "List your accounts (identities + provider credentials), or create one.",
        request: None,
        response: Some("OAuthAccount[]"),
    },
    Route {
        id: "post_accounts",
        method: Method::Post,
        path: "/accounts",
        summary: "List your accounts (identities + provider credentials), or create one.",
        request: Some("CreateAccount"),
        response: Some("OAuthAccount"),
    },
    Route {
        id: "post_accounts_oauth_start",
        method: Method::Post,
        path: "/accounts/oauth/start",
        summary: "Begin an OAuth account authorization flow.",
        request: None,
        response: None,
    },
    Route {
        id: "post_accounts_oauth_finish",
        method: Method::Post,
        path: "/accounts/oauth/finish",
        summary: "Complete an OAuth account authorization flow.",
        request: None,
        response: None,
    },
    Route {
        id: "get_accounts_by_id",
        method: Method::Get,
        path: "/accounts/{id}",
        summary: "Get, rename/re-env, or delete an account identity.",
        request: None,
        response: Some("OAuthAccount"),
    },
    Route {
        id: "patch_accounts_by_id",
        method: Method::Patch,
        path: "/accounts/{id}",
        summary: "Get, rename/re-env, or delete an account identity.",
        request: Some("UpdateAccount"),
        response: Some("OAuthAccount"),
    },
    Route {
        id: "delete_accounts_by_id",
        method: Method::Delete,
        path: "/accounts/{id}",
        summary: "Get, rename/re-env, or delete an account identity.",
        request: None,
        response: None,
    },
    Route {
        id: "put_accounts_by_id_redirect",
        method: Method::Put,
        path: "/accounts/{id}/redirect",
        summary: "Create/overwrite a launch-time redirect rule for this account.",
        request: None,
        response: None,
    },
    Route {
        id: "get_accounts_by_id_tool_policy",
        method: Method::Get,
        path: "/accounts/{id}/tool-policy",
        summary: "Get or replace the account's gateway tool-call policy.",
        request: None,
        response: None,
    },
    Route {
        id: "put_accounts_by_id_tool_policy",
        method: Method::Put,
        path: "/accounts/{id}/tool-policy",
        summary: "Get or replace the account's gateway tool-call policy.",
        request: None,
        response: None,
    },
    Route {
        id: "get_redirects",
        method: Method::Get,
        path: "/redirects",
        summary: "The caller's live account/model redirect rules.",
        request: None,
        response: None,
    },
    Route {
        id: "delete_redirects_by_id",
        method: Method::Delete,
        path: "/redirects/{id}",
        summary: "Delete a redirect rule.",
        request: None,
        response: None,
    },
    Route {
        id: "post_admin_users",
        method: Method::Post,
        path: "/admin/users",
        summary: "List all users, or create a user (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "get_admin_users",
        method: Method::Get,
        path: "/admin/users",
        summary: "List all users, or create a user (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "put_admin_instance",
        method: Method::Put,
        path: "/admin/instance",
        summary: "Set or clear the server-wide deployment name shown in the webui header (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "get_admin_instance_self_update",
        method: Method::Get,
        path: "/admin/instance/self-update",
        summary: "Read or set the machine + directory the self-update agent runs on (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "put_admin_instance_self_update",
        method: Method::Put,
        path: "/admin/instance/self-update",
        summary: "Read or set the machine + directory the self-update agent runs on (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "get_admin_instance_spawn_defaults",
        method: Method::Get,
        path: "/admin/instance/spawn-defaults",
        summary: "Read or set the default CctuiAgent limits for sessions that declare none (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "put_admin_instance_spawn_defaults",
        method: Method::Put,
        path: "/admin/instance/spawn-defaults",
        summary: "Read or set the default CctuiAgent limits for sessions that declare none (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "get_admin_instance_upstream_hosts",
        method: Method::Get,
        path: "/admin/instance/upstream-hosts",
        summary: "Read or set the hosts per-account upstreams may reach despite the SSRF guard (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "put_admin_instance_upstream_hosts",
        method: Method::Put,
        path: "/admin/instance/upstream-hosts",
        summary: "Read or set the hosts per-account upstreams may reach despite the SSRF guard (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "get_admin_instance_upload_caps",
        method: Method::Get,
        path: "/admin/instance/upload-caps",
        summary: "Read or set the per-upload file count and size caps (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "put_admin_instance_upload_caps",
        method: Method::Put,
        path: "/admin/instance/upload-caps",
        summary: "Read or set the per-upload file count and size caps (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "get_admin_harness_autoupdate",
        method: Method::Get,
        path: "/admin/harness-autoupdate",
        summary: "Read the harness auto-update settings of every machine, or set the instance default (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "put_admin_harness_autoupdate",
        method: Method::Put,
        path: "/admin/harness-autoupdate",
        summary: "Read the harness auto-update settings of every machine, or set the instance default (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "put_admin_harness_autoupdate_by_machine",
        method: Method::Put,
        path: "/admin/harness-autoupdate/{machine_id}",
        summary: "Set or clear one machine's harness auto-update override (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "delete_admin_users_by_id",
        method: Method::Delete,
        path: "/admin/users/{id}",
        summary: "Revoke or update a user (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "patch_admin_users_by_id",
        method: Method::Patch,
        path: "/admin/users/{id}",
        summary: "Revoke or update a user (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "delete_admin_users_by_id_purge",
        method: Method::Delete,
        path: "/admin/users/{id}/purge",
        summary: "Hard-delete a user and all their data (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "post_admin_users_by_id_rotate",
        method: Method::Post,
        path: "/admin/users/{id}/rotate",
        summary: "Rotate a user's tokens (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "get_admin_users_by_id_machines",
        method: Method::Get,
        path: "/admin/users/{id}/machines",
        summary: "List a user's machines (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "get_admin_users_by_id_tokens",
        method: Method::Get,
        path: "/admin/users/{id}/tokens",
        summary: "List a user's tokens (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "patch_admin_users_by_id_tokens_by_token",
        method: Method::Patch,
        path: "/admin/users/{id}/tokens/{token_id}",
        summary: "Relabel or revoke a user's token (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "delete_admin_users_by_id_tokens_by_token",
        method: Method::Delete,
        path: "/admin/users/{id}/tokens/{token_id}",
        summary: "Relabel or revoke a user's token (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "delete_admin_users_by_id_tokens_by_token_purge",
        method: Method::Delete,
        path: "/admin/users/{id}/tokens/{token_id}/purge",
        summary: "Hard-delete a user's token (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "delete_admin_machines_by_id",
        method: Method::Delete,
        path: "/admin/machines/{id}",
        summary: "Revoke or rename a machine (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "patch_admin_machines_by_id",
        method: Method::Patch,
        path: "/admin/machines/{id}",
        summary: "Revoke or rename a machine (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "post_admin_machines_by_id_rotate",
        method: Method::Post,
        path: "/admin/machines/{id}/rotate",
        summary: "Rotate a machine's key (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "delete_admin_machines_by_id_purge",
        method: Method::Delete,
        path: "/admin/machines/{id}/purge",
        summary: "Hard-delete a machine (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "get_context",
        method: Method::Get,
        path: "/context",
        summary: "List the caller's context items, or create one.",
        request: None,
        response: Some("ContextItem[]"),
    },
    Route {
        id: "post_context",
        method: Method::Post,
        path: "/context",
        summary: "List the caller's context items, or create one.",
        request: Some("ContextItemSpec"),
        response: Some("ContextItem"),
    },
    Route {
        id: "get_context_resolve",
        method: Method::Get,
        path: "/context/resolve",
        summary: "The context a spawn into these coordinates would attach on its own.",
        request: None,
        response: None,
    },
    Route {
        id: "patch_context_by_id",
        method: Method::Patch,
        path: "/context/{id}",
        summary: "Edit or delete a context item.",
        request: Some("UpdateContextItemRequest"),
        response: Some("ContextItem"),
    },
    Route {
        id: "delete_context_by_id",
        method: Method::Delete,
        path: "/context/{id}",
        summary: "Edit or delete a context item.",
        request: None,
        response: None,
    },
    Route {
        id: "get_manifest_daemon",
        method: Method::Get,
        path: "/manifest/daemon",
        summary: "Daemon update manifest (latest version + download URLs).",
        request: None,
        response: None,
    },
    Route {
        id: "get_daemon_binary_by_target",
        method: Method::Get,
        path: "/daemon/binary/{target}",
        summary: "Download a daemon binary for a target (self-update proxy).",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_dispatch",
        method: Method::Post,
        path: "/sessions/dispatch",
        summary: "Dispatch a session to an enrolled executor (remote runner).",
        request: None,
        response: None,
    },
    Route {
        id: "get_sessions_dispatchers",
        method: Method::Get,
        path: "/sessions/dispatchers",
        summary: "List dispatch targets available for a spawn.",
        request: None,
        response: None,
    },
    Route {
        id: "get_dispatchers",
        method: Method::Get,
        path: "/dispatchers",
        summary: "List enrolled dispatchers with liveness.",
        request: None,
        response: None,
    },
    Route {
        id: "patch_dispatchers_by_id",
        method: Method::Patch,
        path: "/dispatchers/{id}",
        summary: "Rename or remove an enrolled dispatcher.",
        request: None,
        response: None,
    },
    Route {
        id: "delete_dispatchers_by_id",
        method: Method::Delete,
        path: "/dispatchers/{id}",
        summary: "Rename or remove an enrolled dispatcher.",
        request: None,
        response: None,
    },
    Route {
        id: "post_dispatcher_enroll",
        method: Method::Post,
        path: "/dispatcher/enroll",
        summary: "Enroll a new dispatcher (executor) and mint its key.",
        request: None,
        response: None,
    },
    Route {
        id: "get_keys",
        method: Method::Get,
        path: "/keys",
        summary: "List your provider API keys, or store a new one.",
        request: None,
        response: None,
    },
    Route {
        id: "post_keys",
        method: Method::Post,
        path: "/keys",
        summary: "List your provider API keys, or store a new one.",
        request: None,
        response: None,
    },
    Route {
        id: "delete_keys_by_id",
        method: Method::Delete,
        path: "/keys/{id}",
        summary: "Delete a stored provider API key.",
        request: None,
        response: None,
    },
    Route {
        id: "get_keys_by_id_value",
        method: Method::Get,
        path: "/keys/{id}/value",
        summary: "Reveal a stored provider API key's value.",
        request: None,
        response: None,
    },
    Route {
        id: "get_labels",
        method: Method::Get,
        path: "/labels",
        summary: "List label definitions, or create one.",
        request: None,
        response: None,
    },
    Route {
        id: "post_labels",
        method: Method::Post,
        path: "/labels",
        summary: "List label definitions, or create one.",
        request: None,
        response: None,
    },
    Route {
        id: "patch_labels_by_id",
        method: Method::Patch,
        path: "/labels/{id}",
        summary: "Rename or delete a label definition.",
        request: None,
        response: None,
    },
    Route {
        id: "delete_labels_by_id",
        method: Method::Delete,
        path: "/labels/{id}",
        summary: "Rename or delete a label definition.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_by_id_labels",
        method: Method::Post,
        path: "/sessions/{id}/labels",
        summary: "Attach a label to a session.",
        request: None,
        response: None,
    },
    Route {
        id: "delete_sessions_by_id_labels_by_label",
        method: Method::Delete,
        path: "/sessions/{id}/labels/{label_id}",
        summary: "Detach a label from a session.",
        request: None,
        response: None,
    },
    Route {
        id: "get_machines_by_machine_commands_pending",
        method: Method::Get,
        path: "/machines/{machine_id}/commands/pending",
        summary: "Poll a machine's pending spawn/control commands.",
        request: None,
        response: None,
    },
    Route {
        id: "get_machines_by_machine_fs_dirs",
        method: Method::Get,
        path: "/machines/{machine_id}/fs/dirs",
        summary: "List directories on a machine (spawn dir picker).",
        request: None,
        response: None,
    },
    Route {
        id: "get_machines_by_machine_fs_gitinfo",
        method: Method::Get,
        path: "/machines/{machine_id}/fs/gitinfo",
        summary: "Git branch / detached HEAD of a directory on a machine (spawn dir badge).",
        request: None,
        response: None,
    },
    Route {
        id: "get_machines_by_machine_fs_file",
        method: Method::Get,
        path: "/machines/{machine_id}/fs/file",
        summary: "Read one file on a machine (agent-linked path): inline or blob redirect.",
        request: None,
        response: None,
    },
    Route {
        id: "get_machines_by_machine_codex_models",
        method: Method::Get,
        path: "/machines/{machine_id}/codex-models",
        summary: "Machine/account-scoped codex model catalog.",
        request: None,
        response: None,
    },
    Route {
        id: "post_machines_by_machine_codex_models_refresh",
        method: Method::Post,
        path: "/machines/{machine_id}/codex-models/refresh",
        summary: "Re-read every OpenAI account's codex model catalog from upstream.",
        request: None,
        response: None,
    },
    Route {
        id: "get_models_codex_catalog",
        method: Method::Get,
        path: "/models/codex/catalog",
        summary: "Codex model catalog merged across every machine (newest report wins).",
        request: None,
        response: None,
    },
    Route {
        id: "get_models_by_harness",
        method: Method::Get,
        path: "/models/{harness}",
        summary: "Model and effort options for a harness picker (optional machine_id).",
        request: None,
        response: Some("HarnessModels"),
    },
    Route {
        id: "get_meta_domain",
        method: Method::Get,
        path: "/meta/domain",
        summary: "Provider metadata, quota probes, end-reason tones, permission modes.",
        request: None,
        response: Some("DomainMeta"),
    },
    Route {
        id: "get_me",
        method: Method::Get,
        path: "/me",
        summary: "Get the current principal (user, scopes, machine).",
        request: None,
        response: Some("MeResponse"),
    },
    Route {
        id: "delete_me_key",
        method: Method::Delete,
        path: "/me/key",
        summary: "Revoke the credential this request authenticated with.",
        request: None,
        response: None,
    },
    Route {
        id: "get_auth_device_by_user_code",
        method: Method::Get,
        path: "/auth/device/{user_code}",
        summary: "What a pending device login is asking for.",
        request: None,
        response: Some("DeviceAuthRequestInfo"),
    },
    Route {
        id: "post_auth_device_by_user_code_decision",
        method: Method::Post,
        path: "/auth/device/{user_code}/decision",
        summary: "Approve or deny a device login, granting it your scopes.",
        request: Some("DeviceAuthDecision"),
        response: None,
    },
    Route {
        id: "get_settings",
        method: Method::Get,
        path: "/settings",
        summary: "Get your user settings.",
        request: None,
        response: Some("SettingsPayload"),
    },
    Route {
        id: "put_settings",
        method: Method::Put,
        path: "/settings",
        summary: "Replace your user settings.",
        request: Some("SettingsPayload"),
        response: Some("SettingsPayload"),
    },
    Route {
        id: "post_settings_rescrub",
        method: Method::Post,
        path: "/settings/rescrub",
        summary: "Start a privacy scan over your stored events; returns its job.",
        request: None,
        response: None,
    },
    Route {
        id: "get_settings_rescrub",
        method: Method::Get,
        path: "/settings/rescrub",
        summary: "Your most recent privacy scan job, running or finished.",
        request: None,
        response: None,
    },
    Route {
        id: "post_settings_rescrub_cancel",
        method: Method::Post,
        path: "/settings/rescrub/cancel",
        summary: "Cancel your running privacy scan.",
        request: None,
        response: None,
    },
    Route {
        id: "get_capabilities",
        method: Method::Get,
        path: "/capabilities",
        summary: "List server capabilities/feature flags.",
        request: None,
        response: Some("CapabilitiesResponse"),
    },
    Route {
        id: "post_enroll",
        method: Method::Post,
        path: "/enroll",
        summary: "Enroll this machine and mint its machine key.",
        request: None,
        response: None,
    },
    Route {
        id: "get_machines_resources",
        method: Method::Get,
        path: "/machines/resources",
        summary: "The caller's daemon machines with their last host CPU/memory/disk snapshot.",
        request: None,
        response: None,
    },
    Route {
        id: "get_machines_by_machine_status",
        method: Method::Get,
        path: "/machines/{machine_id}/status",
        summary: "Machine connectivity/liveness snapshot (remote-enroll verification).",
        request: None,
        response: None,
    },
    Route {
        id: "post_deenroll",
        method: Method::Post,
        path: "/deenroll",
        summary: "Deenroll the current machine.",
        request: None,
        response: None,
    },
    Route {
        id: "get_version",
        method: Method::Get,
        path: "/version",
        summary: "Server version and build info. Public; deployment fields need a token.",
        request: None,
        response: Some("VersionInfo"),
    },
    Route {
        id: "get_passkeys",
        method: Method::Get,
        path: "/passkeys",
        summary: "List the passkeys enrolled on your account.",
        request: None,
        response: None,
    },
    Route {
        id: "post_passkeys_register_start",
        method: Method::Post,
        path: "/passkeys/register/start",
        summary: "Begin enrolling a passkey on your account.",
        request: None,
        response: None,
    },
    Route {
        id: "post_passkeys_register_finish",
        method: Method::Post,
        path: "/passkeys/register/finish",
        summary: "Finish enrolling a passkey and store the credential.",
        request: None,
        response: None,
    },
    Route {
        id: "post_passkeys_test_start",
        method: Method::Post,
        path: "/passkeys/test/start",
        summary: "Begin a test of an enrolled passkey without signing out.",
        request: None,
        response: None,
    },
    Route {
        id: "post_passkeys_test_finish",
        method: Method::Post,
        path: "/passkeys/test/finish",
        summary: "Finish a passkey test and report which key answered.",
        request: None,
        response: None,
    },
    Route {
        id: "patch_passkeys_by_id",
        method: Method::Patch,
        path: "/passkeys/{id}",
        summary: "Rename or revoke one of your passkeys.",
        request: None,
        response: None,
    },
    Route {
        id: "delete_passkeys_by_id",
        method: Method::Delete,
        path: "/passkeys/{id}",
        summary: "Rename or revoke one of your passkeys.",
        request: None,
        response: None,
    },
    Route {
        id: "put_admin_passkeys_auto_prompt",
        method: Method::Put,
        path: "/admin/passkeys/auto-prompt",
        summary: "Server-wide: try the passkey as soon as the login screen opens (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "get_plugins",
        method: Method::Get,
        path: "/plugins",
        summary: "List installed plugins with the caller's enabled flags.",
        request: None,
        response: None,
    },
    Route {
        id: "get_plugins_by_id_backend_by_*path",
        method: Method::Get,
        path: "/plugins/{id}/backend/{*path}",
        summary: "Proxy a request to the plugin's own backend, authenticated as the caller with signed identity headers; the plugin must be instance-enabled and enabled by the caller.",
        request: None,
        response: None,
    },
    Route {
        id: "post_plugins_by_id_backend_by_*path",
        method: Method::Post,
        path: "/plugins/{id}/backend/{*path}",
        summary: "Proxy a request to the plugin's own backend, authenticated as the caller with signed identity headers; the plugin must be instance-enabled and enabled by the caller.",
        request: None,
        response: None,
    },
    Route {
        id: "put_plugins_by_id_backend_by_*path",
        method: Method::Put,
        path: "/plugins/{id}/backend/{*path}",
        summary: "Proxy a request to the plugin's own backend, authenticated as the caller with signed identity headers; the plugin must be instance-enabled and enabled by the caller.",
        request: None,
        response: None,
    },
    Route {
        id: "patch_plugins_by_id_backend_by_*path",
        method: Method::Patch,
        path: "/plugins/{id}/backend/{*path}",
        summary: "Proxy a request to the plugin's own backend, authenticated as the caller with signed identity headers; the plugin must be instance-enabled and enabled by the caller.",
        request: None,
        response: None,
    },
    Route {
        id: "delete_plugins_by_id_backend_by_*path",
        method: Method::Delete,
        path: "/plugins/{id}/backend/{*path}",
        summary: "Proxy a request to the plugin's own backend, authenticated as the caller with signed identity headers; the plugin must be instance-enabled and enabled by the caller.",
        request: None,
        response: None,
    },
    Route {
        id: "post_plugins_rescan",
        method: Method::Post,
        path: "/plugins/rescan",
        summary: "Re-read the plugins directory (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "get_admin_plugins",
        method: Method::Get,
        path: "/admin/plugins",
        summary: "List every plugin with its source and instance toggle, or install one from a published catalog id (JSON `{catalog}`), an https URL (JSON `{url}`) or a multipart `file` upload (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "post_admin_plugins",
        method: Method::Post,
        path: "/admin/plugins",
        summary: "List every plugin with its source and instance toggle, or install one from a published catalog id (JSON `{catalog}`), an https URL (JSON `{url}`) or a multipart `file` upload (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "get_admin_plugins_catalog",
        method: Method::Get,
        path: "/admin/plugins/catalog",
        summary: "List the published plugin catalog, annotated with what this instance has installed (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "patch_admin_plugins_by_id",
        method: Method::Patch,
        path: "/admin/plugins/{id}",
        summary: "Enable or disable an installed plugin instance-wide, or uninstall it (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "delete_admin_plugins_by_id",
        method: Method::Delete,
        path: "/admin/plugins/{id}",
        summary: "Enable or disable an installed plugin instance-wide, or uninstall it (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "get_admin_plugins_by_id_settings",
        method: Method::Get,
        path: "/admin/plugins/{id}/settings",
        summary: "Read or write a plugin's instance-level settings; secret values are never returned, only whether each is set (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "put_admin_plugins_by_id_settings",
        method: Method::Put,
        path: "/admin/plugins/{id}/settings",
        summary: "Read or write a plugin's instance-level settings; secret values are never returned, only whether each is set (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "post_admin_plugins_by_id_proxy_secret",
        method: Method::Post,
        path: "/admin/plugins/{id}/proxy-secret",
        summary: "Rotate the plugin's backend-proxy signing secret and return the new value once (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "get_sessions_by_id_previews",
        method: Method::Get,
        path: "/sessions/{id}/previews",
        summary: "Dev-server previews open on a session (owner only).",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_by_id_previews_by_pid_ticket",
        method: Method::Post,
        path: "/sessions/{id}/previews/{pid}/ticket",
        summary: "Mint a 60 s single-use ticket that signs the owner into a preview host.",
        request: None,
        response: None,
    },
    Route {
        id: "get_profiles",
        method: Method::Get,
        path: "/profiles",
        summary: "List the caller's spawn profiles, or create one.",
        request: None,
        response: Some("SessionProfile[]"),
    },
    Route {
        id: "post_profiles",
        method: Method::Post,
        path: "/profiles",
        summary: "List the caller's spawn profiles, or create one.",
        request: Some("CreateProfileRequest"),
        response: Some("SessionProfile"),
    },
    Route {
        id: "put_profiles_order",
        method: Method::Put,
        path: "/profiles/order",
        summary: "Persist the caller's profile order.",
        request: Some("ReorderProfilesRequest"),
        response: Some("SessionProfile[]"),
    },
    Route {
        id: "patch_profiles_by_id",
        method: Method::Patch,
        path: "/profiles/{id}",
        summary: "Rename, adjust or delete a spawn profile.",
        request: Some("UpdateProfileRequest"),
        response: Some("SessionProfile"),
    },
    Route {
        id: "delete_profiles_by_id",
        method: Method::Delete,
        path: "/profiles/{id}",
        summary: "Rename, adjust or delete a spawn profile.",
        request: None,
        response: None,
    },
    Route {
        id: "get_account_pools",
        method: Method::Get,
        path: "/account-pools",
        summary: "List the caller's account pools, or create one.",
        request: None,
        response: Some("AccountPoolView[]"),
    },
    Route {
        id: "post_account_pools",
        method: Method::Post,
        path: "/account-pools",
        summary: "List the caller's account pools, or create one.",
        request: Some("CreatePoolRequest"),
        response: Some("AccountPoolView"),
    },
    Route {
        id: "get_account_pools_usage",
        method: Method::Get,
        path: "/account-pools/usage",
        summary: "Every pool's quota windows aggregated per provider family: level, pace, projection.",
        request: None,
        response: Some("PoolUsageView[]"),
    },
    Route {
        id: "patch_account_pools_by_id",
        method: Method::Patch,
        path: "/account-pools/{id}",
        summary: "Edit a pool (name, strategy, failover, membership) or delete it.",
        request: Some("UpdatePoolRequest"),
        response: Some("AccountPoolView"),
    },
    Route {
        id: "delete_account_pools_by_id",
        method: Method::Delete,
        path: "/account-pools/{id}",
        summary: "Edit a pool (name, strategy, failover, membership) or delete it.",
        request: None,
        response: None,
    },
    Route {
        id: "get_sessions_by_id_rebinds",
        method: Method::Get,
        path: "/sessions/{id}/rebinds",
        summary: "Every mid-run account move this session made, newest first.",
        request: None,
        response: None,
    },
    Route {
        id: "get_bookmarks",
        method: Method::Get,
        path: "/bookmarks",
        summary: "List your saved messages, or save one.",
        request: None,
        response: Some("Bookmark[]"),
    },
    Route {
        id: "post_bookmarks",
        method: Method::Post,
        path: "/bookmarks",
        summary: "List your saved messages, or save one.",
        request: Some("CreateBookmark"),
        response: Some("Bookmark"),
    },
    Route {
        id: "patch_bookmarks_by_id",
        method: Method::Patch,
        path: "/bookmarks/{id}",
        summary: "Edit a bookmark's title/note, or delete it.",
        request: Some("UpdateBookmark"),
        response: Some("Bookmark"),
    },
    Route {
        id: "delete_bookmarks_by_id",
        method: Method::Delete,
        path: "/bookmarks/{id}",
        summary: "Edit a bookmark's title/note, or delete it.",
        request: None,
        response: None,
    },
    Route {
        id: "get_prompts",
        method: Method::Get,
        path: "/prompts",
        summary: "List your saved prompts, or create one.",
        request: None,
        response: Some("Prompt[]"),
    },
    Route {
        id: "post_prompts",
        method: Method::Post,
        path: "/prompts",
        summary: "List your saved prompts, or create one.",
        request: None,
        response: None,
    },
    Route {
        id: "get_prompts_resolve",
        method: Method::Get,
        path: "/prompts/resolve",
        summary: "Resolve a prompt by name/reference.",
        request: None,
        response: None,
    },
    Route {
        id: "get_prompts_by_id",
        method: Method::Get,
        path: "/prompts/{id}",
        summary: "Get or delete a saved prompt.",
        request: None,
        response: None,
    },
    Route {
        id: "delete_prompts_by_id",
        method: Method::Delete,
        path: "/prompts/{id}",
        summary: "Get or delete a saved prompt.",
        request: None,
        response: None,
    },
    Route {
        id: "get_provider_status",
        method: Method::Get,
        path: "/provider-status",
        summary: "What each upstream provider family reports on its status page.",
        request: None,
        response: None,
    },
    Route {
        id: "get_rooms",
        method: Method::Get,
        path: "/rooms",
        summary: "List rooms, for the picker.",
        request: None,
        response: None,
    },
    Route {
        id: "patch_rooms_by_id",
        method: Method::Patch,
        path: "/rooms/{id}",
        summary: "Rename/archive, or delete a room.",
        request: None,
        response: None,
    },
    Route {
        id: "delete_rooms_by_id",
        method: Method::Delete,
        path: "/rooms/{id}",
        summary: "Rename/archive, or delete a room.",
        request: None,
        response: None,
    },
    Route {
        id: "put_sessions_by_id_room",
        method: Method::Put,
        path: "/sessions/{id}/room",
        summary: "Put this session in a room (by id or name), or take it out of one.",
        request: None,
        response: None,
    },
    Route {
        id: "delete_sessions_by_id_room",
        method: Method::Delete,
        path: "/sessions/{id}/room",
        summary: "Put this session in a room (by id or name), or take it out of one.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_archive",
        method: Method::Post,
        path: "/sessions/archive",
        summary: "Archive a batch of sessions by id.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_unarchive",
        method: Method::Post,
        path: "/sessions/unarchive",
        summary: "Unarchive a batch of sessions by id.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_pin",
        method: Method::Post,
        path: "/sessions/pin",
        summary: "Pin a batch of sessions by id.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_unpin",
        method: Method::Post,
        path: "/sessions/unpin",
        summary: "Unpin a batch of sessions by id.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_by_id_launch",
        method: Method::Post,
        path: "/sessions/{id}/launch",
        summary: "Launch a draft session into a live spawn.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_by_id_discard",
        method: Method::Post,
        path: "/sessions/{id}/discard",
        summary: "Discard a draft session.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_by_id_schedule_launch",
        method: Method::Post,
        path: "/sessions/{id}/schedule-launch",
        summary: "Queue a draft session to launch later.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_by_id_cancel_launch",
        method: Method::Post,
        path: "/sessions/{id}/cancel-launch",
        summary: "Cancel a draft session's queued launch.",
        request: None,
        response: None,
    },
    Route {
        id: "put_sessions_by_id_draft",
        method: Method::Put,
        path: "/sessions/{id}/draft",
        summary: "Replace a draft session's stored spawn payload in place.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_by_id_interrupt",
        method: Method::Post,
        path: "/sessions/{id}/interrupt",
        summary: "Interrupt a session's current turn.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_by_id_resume",
        method: Method::Post,
        path: "/sessions/{id}/resume",
        summary: "Resume an exited session.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_by_id_set_model",
        method: Method::Post,
        path: "/sessions/{id}/set-model",
        summary: "Change a session's model.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_by_id_switch_account",
        method: Method::Post,
        path: "/sessions/{id}/switch-account",
        summary: "Switch the account backing a session.",
        request: None,
        response: None,
    },
    Route {
        id: "get_sessions_by_id_bindings",
        method: Method::Get,
        path: "/sessions/{id}/bindings",
        summary: "List a session's per-family account bindings.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_register",
        method: Method::Post,
        path: "/sessions/register",
        summary: "Register a session the daemon just launched.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_by_id_deregister",
        method: Method::Post,
        path: "/sessions/{id}/deregister",
        summary: "Deregister a session (mark it gone).",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_spawn",
        method: Method::Post,
        path: "/sessions/spawn",
        summary: "Spawn a new session on a machine, with optional file uploads.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_by_id_files",
        method: Method::Post,
        path: "/sessions/{id}/files",
        summary: "Attach files to a live session mid-conversation.",
        request: None,
        response: None,
    },
    Route {
        id: "get_sessions",
        method: Method::Get,
        path: "/sessions",
        summary: "List your sessions (admin: all).",
        request: None,
        response: None,
    },
    Route {
        id: "get_sessions_stats",
        method: Method::Get,
        path: "/sessions/stats",
        summary: "Aggregate session counts/status stats.",
        request: None,
        response: None,
    },
    Route {
        id: "get_sessions_stats_tokens",
        method: Method::Get,
        path: "/sessions/stats/tokens",
        summary: "Token-usage stats across sessions.",
        request: None,
        response: None,
    },
    Route {
        id: "get_sessions_stats_usage",
        method: Method::Get,
        path: "/sessions/stats/usage",
        summary: "Overview usage analytics: tokens over time, per-model, heatmap.",
        request: None,
        response: None,
    },
    Route {
        id: "get_sessions_stats_cache_busts",
        method: Method::Get,
        path: "/sessions/stats/cache-busts",
        summary: "Dollars lost to prompt-cache busts per day, by reason.",
        request: None,
        response: None,
    },
    Route {
        id: "get_sessions_search",
        method: Method::Get,
        path: "/sessions/search",
        summary: "Full-text search across your sessions.",
        request: None,
        response: None,
    },
    Route {
        id: "get_sessions_search_values",
        method: Method::Get,
        path: "/sessions/search/values",
        summary: "Autocomplete values for a search field.",
        request: None,
        response: None,
    },
    Route {
        id: "get_sessions_recent_dirs",
        method: Method::Get,
        path: "/sessions/recent-dirs",
        summary: "List recently used working directories.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_by_id_message",
        method: Method::Post,
        path: "/sessions/{id}/message",
        summary: "Send a message to a live session.",
        request: None,
        response: None,
    },
    Route {
        id: "get_sessions_by_id_messages_scheduled",
        method: Method::Get,
        path: "/sessions/{id}/messages/scheduled",
        summary: "List a session's scheduled messages.",
        request: None,
        response: None,
    },
    Route {
        id: "patch_sessions_by_id_messages_scheduled_by_queue",
        method: Method::Patch,
        path: "/sessions/{id}/messages/scheduled/{queue_id}",
        summary: "Edit/reschedule or cancel a scheduled message.",
        request: None,
        response: None,
    },
    Route {
        id: "delete_sessions_by_id_messages_scheduled_by_queue",
        method: Method::Delete,
        path: "/sessions/{id}/messages/scheduled/{queue_id}",
        summary: "Edit/reschedule or cancel a scheduled message.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_by_id_messages_scheduled_by_queue_send_now",
        method: Method::Post,
        path: "/sessions/{id}/messages/scheduled/{queue_id}/send-now",
        summary: "Deliver a scheduled message immediately.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_by_id_kill",
        method: Method::Post,
        path: "/sessions/{id}/kill",
        summary: "Kill a session's underlying process.",
        request: None,
        response: None,
    },
    Route {
        id: "get_sessions_by_id_pins",
        method: Method::Get,
        path: "/sessions/{id}/pins",
        summary: "List the caller's pinned messages in a session.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_by_id_pins",
        method: Method::Post,
        path: "/sessions/{id}/pins",
        summary: "Pin a message (by stream seq) in a session.",
        request: None,
        response: None,
    },
    Route {
        id: "delete_sessions_by_id_pins_by_seq",
        method: Method::Delete,
        path: "/sessions/{id}/pins/{seq}",
        summary: "Unpin a message in a session.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_by_id_seen",
        method: Method::Post,
        path: "/sessions/{id}/seen",
        summary: "Mark this session's messages seen for the caller.",
        request: None,
        response: None,
    },
    Route {
        id: "get_sessions_by_id_brief",
        method: Method::Get,
        path: "/sessions/{id}/brief",
        summary: "Render a session's user/assistant transcript as a capped markdown brief.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_by_id_fork",
        method: Method::Post,
        path: "/sessions/{id}/fork",
        summary: "Fork a session into a new one.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_by_id_auto_approve",
        method: Method::Post,
        path: "/sessions/{id}/auto-approve",
        summary: "Toggle auto-approval of tool-use for a session.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_by_id_archive",
        method: Method::Post,
        path: "/sessions/{id}/archive",
        summary: "Archive a single session.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_by_id_unarchive",
        method: Method::Post,
        path: "/sessions/{id}/unarchive",
        summary: "Unarchive a single session.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_by_id_pin",
        method: Method::Post,
        path: "/sessions/{id}/pin",
        summary: "Pin a single session.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_by_id_unpin",
        method: Method::Post,
        path: "/sessions/{id}/unpin",
        summary: "Unpin a single session.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_by_id_keepalive",
        method: Method::Post,
        path: "/sessions/{id}/keepalive",
        summary: "Set or clear the session's prompt-cache keep-alive schedule.",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_by_id_policy",
        method: Method::Post,
        path: "/sessions/{id}/policy",
        summary: "Set a session's permission policy.",
        request: None,
        response: None,
    },
    Route {
        id: "patch_sessions_by_id_plugins_by_plugin",
        method: Method::Patch,
        path: "/sessions/{id}/plugins/{plugin_id}",
        summary: "Set or clear a session's per-plugin data slot.",
        request: None,
        response: None,
    },
    Route {
        id: "get_sessions_by_id",
        method: Method::Get,
        path: "/sessions/{id}",
        summary: "Get one session's details.",
        request: None,
        response: None,
    },
    Route {
        id: "patch_sessions_by_id",
        method: Method::Patch,
        path: "/sessions/{id}",
        summary: "Rename a session.",
        request: None,
        response: None,
    },
    Route {
        id: "get_sessions_by_id_conversation",
        method: Method::Get,
        path: "/sessions/{id}/conversation",
        summary: "Fetch a session's normalized conversation transcript.",
        request: None,
        response: None,
    },
    Route {
        id: "get_sessions_by_id_search",
        method: Method::Get,
        path: "/sessions/{id}/search",
        summary: "Find-in-conversation hit list for one session's transcript.",
        request: None,
        response: None,
    },
    Route {
        id: "get_sessions_by_id_images_by_image",
        method: Method::Get,
        path: "/sessions/{id}/images/{image_id}",
        summary: "Fetch an agent-posted image blob.",
        request: None,
        response: None,
    },
    Route {
        id: "get_sessions_by_id_blobs_by_hash",
        method: Method::Get,
        path: "/sessions/{id}/blobs/{hash}",
        summary: "Resolve a content-addressed embedded-attachment blob.",
        request: None,
        response: None,
    },
    Route {
        id: "get_sessions_by_id_attachments",
        method: Method::Get,
        path: "/sessions/{id}/attachments",
        summary: "List the files the user uploaded into a session (served via blobs).",
        request: None,
        response: None,
    },
    Route {
        id: "get_sessions_by_id_linked_file_owner",
        method: Method::Get,
        path: "/sessions/{id}/linked-file-owner",
        summary: "Which session and machine linked a path, when this session did not.",
        request: None,
        response: None,
    },
    Route {
        id: "get_sessions_by_id_diagnose",
        method: Method::Get,
        path: "/sessions/{id}/diagnose",
        summary: "Snapshot everything the daemon knows about a session, dated.",
        request: None,
        response: None,
    },
    Route {
        id: "get_sessions_by_id_langfuse",
        method: Method::Get,
        path: "/sessions/{id}/langfuse",
        summary: "Langfuse cost/usage rollup for a session.",
        request: None,
        response: None,
    },
    Route {
        id: "get_accounts_by_id_shares",
        method: Method::Get,
        path: "/accounts/{id}/shares",
        summary: "List or grant shares of an account to other users.",
        request: None,
        response: None,
    },
    Route {
        id: "post_accounts_by_id_shares",
        method: Method::Post,
        path: "/accounts/{id}/shares",
        summary: "List or grant shares of an account to other users.",
        request: None,
        response: None,
    },
    Route {
        id: "delete_accounts_by_id_shares_by_user",
        method: Method::Delete,
        path: "/accounts/{id}/shares/{user_id}",
        summary: "Revoke a user's share of an account.",
        request: None,
        response: None,
    },
    Route {
        id: "get_by_resource_type_by_id_shares",
        method: Method::Get,
        path: "/{resource_type}/{id}/shares",
        summary: "List or grant shares of a resource to other users.",
        request: None,
        response: None,
    },
    Route {
        id: "post_by_resource_type_by_id_shares",
        method: Method::Post,
        path: "/{resource_type}/{id}/shares",
        summary: "List or grant shares of a resource to other users.",
        request: None,
        response: None,
    },
    Route {
        id: "delete_by_resource_type_by_id_shares_by_user",
        method: Method::Delete,
        path: "/{resource_type}/{id}/shares/{user_id}",
        summary: "Revoke a user's share of a resource.",
        request: None,
        response: None,
    },
    Route {
        id: "get_permissions_pending",
        method: Method::Get,
        path: "/permissions/pending",
        summary: "List pending tool-use permission requests.",
        request: None,
        response: None,
    },
    Route {
        id: "get_skills_index",
        method: Method::Get,
        path: "/skills/index",
        summary: "List available skills.",
        request: None,
        response: None,
    },
    Route {
        id: "put_skills_by_name",
        method: Method::Put,
        path: "/skills/{name}",
        summary: "Upload or fetch a skill bundle by name.",
        request: None,
        response: None,
    },
    Route {
        id: "get_skills_by_name",
        method: Method::Get,
        path: "/skills/{name}",
        summary: "Upload or fetch a skill bundle by name.",
        request: None,
        response: None,
    },
    Route {
        id: "get_sessions_by_id_user_actions",
        method: Method::Get,
        path: "/sessions/{id}/user-actions",
        summary: "What the agent is waiting on from the user (owner only).",
        request: None,
        response: None,
    },
    Route {
        id: "post_sessions_by_id_user_actions_by_aid_tick",
        method: Method::Post,
        path: "/sessions/{id}/user-actions/{aid}/tick",
        summary: "Resolve one user action as the user (done or dropped).",
        request: None,
        response: None,
    },
    Route {
        id: "get_drafts",
        method: Method::Get,
        path: "/drafts",
        summary: "List your unsent drafts.",
        request: None,
        response: Some("DraftList"),
    },
    Route {
        id: "get_drafts_by_*key",
        method: Method::Get,
        path: "/drafts/{*key}",
        summary: "Read, save or discard one draft.",
        request: None,
        response: Some("Draft"),
    },
    Route {
        id: "put_drafts_by_*key",
        method: Method::Put,
        path: "/drafts/{*key}",
        summary: "Read, save or discard one draft.",
        request: Some("PutDraftRequest"),
        response: None,
    },
    Route {
        id: "delete_drafts_by_*key",
        method: Method::Delete,
        path: "/drafts/{*key}",
        summary: "Read, save or discard one draft.",
        request: None,
        response: None,
    },
    Route {
        id: "get_spawn_memory",
        method: Method::Get,
        path: "/spawn-memory",
        summary: "Get or replace your remembered spawn configuration per target.",
        request: None,
        response: Some("SpawnMemoryPayload"),
    },
    Route {
        id: "put_spawn_memory",
        method: Method::Put,
        path: "/spawn-memory",
        summary: "Get or replace your remembered spawn configuration per target.",
        request: Some("SpawnMemoryPayload"),
        response: Some("SpawnMemoryPayload"),
    },
    Route {
        id: "post_users_by_id_tokens",
        method: Method::Post,
        path: "/users/{id}/tokens",
        summary: "Mint a token for a user (self or admin).",
        request: None,
        response: None,
    },
    Route {
        id: "get_users_by_id_acls",
        method: Method::Get,
        path: "/users/{id}/acls",
        summary: "Get or set a user's scope ceiling (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "patch_users_by_id_acls",
        method: Method::Patch,
        path: "/users/{id}/acls",
        summary: "Get or set a user's scope ceiling (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "get_users_by_id_keys",
        method: Method::Get,
        path: "/users/{id}/keys",
        summary: "List or mint a user's API keys (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "post_users_by_id_keys",
        method: Method::Post,
        path: "/users/{id}/keys",
        summary: "List or mint a user's API keys (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "delete_users_by_id_keys_by_kid",
        method: Method::Delete,
        path: "/users/{id}/keys/{kid}",
        summary: "Revoke a user's API key (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "patch_users_by_id_keys_by_kid_acls",
        method: Method::Patch,
        path: "/users/{id}/keys/{kid}/acls",
        summary: "Set a key's scope grant (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "post_version_refresh",
        method: Method::Post,
        path: "/version/refresh",
        summary: "Probe upstream for a newer release now instead of waiting out the background interval.",
        request: None,
        response: None,
    },
    Route {
        id: "get_version_changelog",
        method: Method::Get,
        path: "/version/changelog",
        summary: "Release notes of every upstream release newer than this server.",
        request: None,
        response: Some("ChangelogResponse"),
    },
    Route {
        id: "post_version_self_update",
        method: Method::Post,
        path: "/version/self-update",
        summary: "Deploy the newer release: the machine's own update hook when it has one, a YOLO agent otherwise (admin).",
        request: None,
        response: None,
    },
    Route {
        id: "get_version_self_update",
        method: Method::Get,
        path: "/version/self-update",
        summary: "The most recent update-hook run and where it got to.",
        request: None,
        response: None,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        let mut seen: Vec<&str> = ROUTES.iter().map(|r| r.id).collect();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(before, seen.len(), "duplicate route id");
    }

    #[test]
    fn method_and_path_pairs_are_unique() {
        let mut seen: Vec<(Method, &str)> = ROUTES.iter().map(|r| (r.method, r.path)).collect();
        seen.sort_unstable_by_key(|(m, p)| (m.as_str(), *p));
        let before = seen.len();
        seen.dedup();
        assert_eq!(before, seen.len(), "duplicate method+path");
    }

    #[test]
    fn paths_are_rooted_and_templated() {
        for r in ROUTES {
            assert!(r.path.starts_with('/'), "{}: path must start with /", r.id);
            assert!(!r.path.contains(API_PREFIX), "{}: path must not repeat the prefix", r.id);
            assert!(!r.summary.is_empty(), "{}: every route documents itself", r.id);
            assert_eq!(
                r.path.matches('{').count(),
                r.path.matches('}').count(),
                "{}: unbalanced placeholder",
                r.id
            );
        }
    }

    #[test]
    fn url_substitutes_named_params() {
        let r = find(Method::Patch, "/profiles/{id}").expect("route");
        assert_eq!(r.path_with(&[("id", "abc")]), "/profiles/abc");
        assert_eq!(r.url(&[("id", "abc")]), "/api/v1/profiles/abc");
        assert_eq!(r.params(), vec!["id"]);
        assert_eq!(r.url(&[]), "/api/v1/profiles/{id}");
        let get = find(Method::Get, "/me").expect("route");
        assert_eq!(get.url(&[]), "/api/v1/me");
        assert_eq!(get.path_with(&[]), "/me");
        assert!(get.params().is_empty());
        let two = find(Method::Delete, "/sessions/{id}/labels/{label_id}").expect("route");
        assert_eq!(two.params(), vec!["id", "label_id"]);
        assert_eq!(two.url(&[("id", "s1"), ("label_id", "l2")]), "/api/v1/sessions/s1/labels/l2");
    }

    #[test]
    fn lookup_by_id_round_trips() {
        for r in ROUTES {
            assert_eq!(by_id(r.id).map(|f| (f.method, f.path)), Some((r.method, r.path)));
        }
    }
}

/// The TypeScript form of [`ROUTES`], emitted next to the ts-rs bindings so
/// the webui endpoint helpers read the same table the server is asserted
/// against.
#[cfg(feature = "ts")]
#[must_use]
pub fn typescript() -> String {
    use std::fmt::Write as _;

    let quote = |v: Option<&str>| v.map_or_else(|| "null".to_owned(), |s| format!("\"{s}\""));
    let mut out = String::new();
    out.push_str(
        "// AUTO-GENERATED by cctui-proto (api::routes::typescript) — do not edit.\n\
         import type { ApiMethod } from './ApiMethod';\n\
         import type { ApiRoute } from './ApiRoute';\n\n\
         export type { ApiMethod, ApiRoute };\n\n",
    );
    let _ = writeln!(out, "export const API_PREFIX = \"{API_PREFIX}\";\n");
    out.push_str("export const ROUTES = [\n");
    for r in ROUTES {
        let _ = writeln!(
            out,
            "  {{ id: \"{}\", method: \"{}\", path: \"{}\", summary: {}, request: {}, \
             response: {} }},",
            r.id,
            r.method,
            r.path,
            serde_json::to_string(r.summary).unwrap_or_else(|_| "\"\"".to_owned()),
            quote(r.request),
            quote(r.response),
        );
    }
    out.push_str("] as const satisfies readonly ApiRoute[];\n\n");
    out.push_str(
        "export type RouteId = (typeof ROUTES)[number]['id'];\n\n\
         const BY_ID = new Map(ROUTES.map((r) => [r.id, r]));\n\n\
         /** The route with this id. Throws on an unknown id: the table is the contract. */\n\
         export function route(id: RouteId): ApiRoute {\n\
         \tconst r = BY_ID.get(id);\n\
         \tif (!r) throw new Error(`unknown route id: ${id}`);\n\
         \treturn r;\n\
         }\n\n\
         /** Route path with `{param}` placeholders substituted, WITHOUT the api prefix. */\n\
         export function path(id: RouteId, params: Record<string, string | number> = {}): string {\n\
         \treturn route(id).path.replace(/\\{(\\w+)\\}/g, (whole, name: string) =>\n\
         \t\tname in params ? encodeURIComponent(String(params[name])) : whole,\n\
         \t);\n\
         }\n\n\
         /** `path()` under the api prefix: an absolute URL path. */\n\
         export function url(id: RouteId, params: Record<string, string | number> = {}): string {\n\
         \treturn `${API_PREFIX}${path(id, params)}`;\n\
         }\n",
    );
    out
}

#[cfg(all(test, feature = "ts"))]
mod ts_export {
    /// Named to match the `export_bindings` filter `webui/scripts/gen-bindings.sh`
    /// runs, so the table lands beside the ts-rs output.
    #[test]
    fn export_bindings_routes() {
        let dir = std::env::var("TS_RS_EXPORT_DIR").unwrap_or_else(|_| "./bindings".to_owned());
        std::fs::create_dir_all(&dir).expect("bindings dir");
        std::fs::write(format!("{dir}/routes.ts"), super::typescript()).expect("write routes.ts");
    }
}
