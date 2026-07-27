//! Self-describing API surface for agents.
//!
//! Two artifacts are generated AT RUNTIME from the single source of truth — the
//! [`authz::RouteDescriptor`] table that [`crate::build_api_routes`] already
//! builds — so they can never drift from the real routes:
//!
//!   * [`openapi_json`] — an OpenAPI 3.1 document, paths/methods/auth/scopes
//!     emitted from the descriptor list. Schemas are deliberately minimal (the
//!     descriptor table carries no DTO type info); the document's job is to let
//!     an agent enumerate the surface and its auth, not to validate bodies.
//!   * [`llms_txt`] — a short Markdown capability index, the agent-first
//!     entrypoint: what cctui is, the auth model, the grouped endpoint list, and
//!     the WS protocol, with a pointer to `openapi.json`.
//!
//! Neither annotates handlers (`utoipa` macros on 87 handlers is exactly what
//! this avoids). New routes inherit documentation for free; the CI guard
//! `every_route_has_a_summary` (in `authz.rs`) keeps every route's `summary`
//! non-empty.

// "OpenAPI" is a proper noun that trips clippy's camel-case doc heuristic all
// over this module's docs; it is not a code item, so silence it file-wide.
#![allow(clippy::doc_markdown)]

use std::collections::BTreeMap;
use std::fmt::Write as _;

use axum::http::{Method, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use crate::authz::RouteDescriptor;

/// Public base prefix every descriptor path nests under.
const API_PREFIX: &str = "/api/v1";

/// Build the descriptor table without standing up any runtime state. The router
/// half is discarded; only the descriptors drive the documents.
fn descriptors() -> Vec<RouteDescriptor> {
    crate::build_api_routes().into_parts().1
}

/// Lower-case HTTP method name as OpenAPI expects it as the operation key.
const fn method_key(m: &Method) -> &'static str {
    match *m {
        Method::POST => "post",
        Method::PUT => "put",
        Method::PATCH => "patch",
        Method::DELETE => "delete",
        Method::HEAD => "head",
        Method::OPTIONS => "options",
        _ => "get",
    }
}

/// A coarse group name for a path, used to `tag` operations and to bucket the
/// `llms.txt` endpoint list. Derived from the first non-`{}` path segment, with
/// a couple of merges so related routes read together.
fn group_of(path: &str) -> &'static str {
    let seg = path.trim_start_matches('/').split('/').next().unwrap_or("");
    match seg {
        "sessions" => "sessions",
        "dispatchers" | "dispatcher" => "dispatchers",
        "accounts" => "accounts",
        "keys" => "credentials",
        "labels" => "labels",
        "prompts" => "prompts",
        "settings" | "me" | "capabilities" | "version" => "meta",
        "skills" => "skills",
        "admin" | "users" => "admin",
        "machines" | "manifest" | "daemon" | "enroll" | "deenroll" => "machines",
        "permissions" => "permissions",
        _ => "other",
    }
}

/// Convert an axum path (`/sessions/{id}`) — already OpenAPI-shaped (`{id}`) —
/// into the full public path under the API prefix.
fn full_path(path: &str) -> String {
    format!("{API_PREFIX}{path}")
}

/// Human label for a route's required scope (matches `auth::Scope::as_str`).
const fn scope_label(d: &RouteDescriptor) -> &'static str {
    d.authz.doc_scope().as_str()
}

/// Emit the OpenAPI 3.1 document as a `serde_json::Value`.
#[must_use]
pub fn build_openapi() -> Value {
    let descs = descriptors();

    // Collect operations per path so one path with several methods becomes one
    // Path Item Object with multiple operation keys.
    let mut paths: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    for d in &descs {
        let item = paths.entry(full_path(d.path)).or_default();
        let scope = scope_label(d);
        // bearerAuth always applies; the per-route scope is recorded as a
        // `x-required-scope` extension so an agent can see the gate without a
        // bespoke per-scope security scheme.
        let op = json!({
            "summary": d.summary,
            "tags": [group_of(d.path)],
            "security": [{ "bearerAuth": [] }],
            "x-required-scope": scope,
            "responses": {
                "200": { "description": "OK" },
                "401": { "description": "Unauthenticated" },
                "403": { "description": "Forbidden (insufficient scope or not owner)" }
            }
        });
        item.insert(method_key(&d.method).to_string(), op);
    }

    let paths_obj: Map<String, Value> =
        paths.into_iter().map(|(k, v)| (k, Value::Object(v))).collect();

    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "cctui API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Self-describing surface for cctui (CCT-464). Generated at \
                runtime from the route descriptor table. Auth: `Authorization: Bearer \
                <token>` (or HttpOnly cookie). Scopes: Read < Dispatch / Enroll < Admin; \
                effective scope = key ACL ∩ user ACL. See /llms.txt for the agent index."
        },
        "servers": [{ "url": "/" }],
        "components": {
            "securitySchemes": {
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "description": "Machine key, agent token, or admin token. Also \
                        accepted as an HttpOnly auth cookie for the web UI."
                }
            }
        },
        "security": [{ "bearerAuth": [] }],
        "paths": Value::Object(paths_obj)
    })
}

/// `GET /api/v1/openapi.json` — the runtime-generated OpenAPI 3.1 document.
///
/// Unauthenticated meta route (mounted on the outer app beside `/health` and
/// `/llms.txt`): it exposes ONLY the public shape of the API — paths, methods,
/// the auth model, and per-route scope — never any data, so it sits outside the
/// `auth_middleware` group as the agent-first discovery surface.
pub async fn openapi_json() -> Response {
    axum::Json(build_openapi()).into_response()
}

/// Build the `llms.txt` Markdown capability index from the descriptor table.
#[must_use]
pub fn build_llms_txt() -> String {
    let descs = descriptors();

    // Group → sorted, de-duplicated (method+path, summary) rows.
    let mut groups: BTreeMap<&'static str, Vec<(String, &'static str)>> = BTreeMap::new();
    for d in &descs {
        let line = format!("{} {}", d.method.as_str(), full_path(d.path));
        groups.entry(group_of(d.path)).or_default().push((line, d.summary));
    }
    for rows in groups.values_mut() {
        rows.sort();
        rows.dedup();
    }

    let mut out = String::new();
    out.push_str("# cctui\n\n");
    out.push_str(
        "cctui is a server that spawns, observes, and controls Claude (and Codex) \
         coding sessions across many machines and ephemeral workers. Agents and humans \
         drive it over a REST API plus three WebSocket protocols: create/list/control \
         sessions, dispatch work to enrolled executors, manage OAuth accounts, provider \
         keys, prompts, skills, labels, and read usage stats. This file is the machine \
         entrypoint; the full schema is at `/api/v1/openapi.json`.\n\n",
    );

    out.push_str("## Auth\n\n");
    out.push_str(
        "- Send `Authorization: Bearer <token>` on every request (the web UI may instead \
         present an HttpOnly auth cookie).\n\
         - Tokens are one of: a machine key, an agent/user token, or the admin token.\n\
         - Scopes form four tiers: `Read` (list/get), `Dispatch` (dispatch sessions), \
         `Enroll` (enroll/manage machines + dispatchers), `Admin` (user/machine/token \
         management, ACLs).\n\
         - Effective scope = key ACL ∩ user ACL: a key can only ever exercise scopes its \
         owning user also holds.\n\
         - Per-object routes (e.g. `/sessions/{id}`) additionally require you to own (or \
         be granted/admin over) that object — otherwise 404 (unknown) or 403.\n\n",
    );

    out.push_str("## Endpoints\n\n");
    out.push_str("Each operation's required scope is in `x-required-scope` in openapi.json.\n\n");
    for (group, rows) in &groups {
        let _ = writeln!(out, "### {group}\n");
        for (line, summary) in rows {
            let _ = writeln!(out, "- `{line}` — {summary}");
        }
        out.push('\n');
    }

    out.push_str("## WebSocket protocols\n\n");
    out.push_str(
        "All three carry tagged-JSON frames defined in the `cctui-proto` crate; auth is \
         the same Bearer token (or cookie), presented on the upgrade.\n\n\
         - `GET /api/v1/ws` — TUI/web client stream: subscribe to sessions, receive \
         normalized conversation/status events, send messages and control ops.\n\
         - `GET /api/v1/daemon/ws` — per-machine daemon link: the daemon registers, \
         streams session lifecycle/heartbeats, and receives spawn/control commands.\n\
         - `GET /api/v1/dispatcher/ws` — enrolled executor link: receives dispatch \
         requests and reports results, so the server can place work on remote runners.\n\n",
    );

    out.push_str("## More\n\n- OpenAPI 3.1 schema: `/api/v1/openapi.json`\n");
    out
}

/// `GET /llms.txt` — the agent-first Markdown capability index. Served at the
/// app root (outside the `/api/v1` auth group) so an agent handed only a base
/// URL can discover the surface before it even has a token. It contains no
/// secrets — only the public shape of the API.
pub async fn llms_txt() -> Response {
    ([(header::CONTENT_TYPE, "text/markdown; charset=utf-8")], build_llms_txt()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_is_3_1_and_lists_session_control_routes() {
        let doc = build_openapi();
        assert_eq!(doc["openapi"], "3.1.0");
        assert!(doc["components"]["securitySchemes"]["bearerAuth"].is_object());
        let paths = doc["paths"].as_object().unwrap();
        // A session control route appears with its method and scope.
        let kill = &paths["/api/v1/sessions/{id}/kill"]["post"];
        assert_eq!(kill["x-required-scope"], "read");
        assert!(!kill["summary"].as_str().unwrap().is_empty());
        // The dispatch route is gated by the `dispatch` scope.
        let dispatch = &paths["/api/v1/sessions/dispatch"]["post"];
        assert_eq!(dispatch["x-required-scope"], "dispatch");
        // The list route is present.
        assert!(paths["/api/v1/sessions"]["get"].is_object());
        // The Langfuse read proxy is session-read scoped.
        assert_eq!(paths["/api/v1/sessions/{id}/langfuse"]["get"]["x-required-scope"], "read");
    }

    #[test]
    fn llms_txt_has_index_and_ws_section() {
        let txt = build_llms_txt();
        assert!(txt.contains("# cctui"));
        assert!(txt.contains("## Auth"));
        assert!(txt.contains("/api/v1/openapi.json"));
        assert!(txt.contains("/api/v1/daemon/ws"));
        assert!(txt.contains("/api/v1/dispatcher/ws"));
        // The descriptor-driven endpoint list is present.
        assert!(txt.contains("GET /api/v1/sessions"));
    }

    /// Sanity: every emitted operation carries a non-empty summary (the document
    /// half of the guard).
    #[test]
    fn every_emitted_operation_has_a_summary() {
        let doc = build_openapi();
        for (path, item) in doc["paths"].as_object().unwrap() {
            for (method, op) in item.as_object().unwrap() {
                let s = op["summary"].as_str().unwrap_or("");
                assert!(!s.trim().is_empty(), "{method} {path} has empty summary");
            }
        }
    }
}
