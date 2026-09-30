//! `parity.toml` must stay in step with the route table and `ServerEvent`.
//!
//! One entry per route and per variant, saying whether the TUI handles it,
//! which epic ticket will, or why it never will. Any drift fails here.

use std::collections::BTreeMap;
use std::path::PathBuf;

use cctui_proto::api::routes::ROUTES;
use serde::Deserialize;

const MANIFEST: &str = include_str!("../parity.toml");
const SERVER_EVENT_RS: &str = include_str!("app/server_event.rs");

/// Leaf tickets of the TUI epic; a `planned` entry must name one.
const TICKET_RANGE: std::ops::RangeInclusive<u32> = 1200..=1278;

#[derive(Debug, Deserialize)]
struct Manifest {
    #[serde(default)]
    route: Vec<Entry>,
    #[serde(default)]
    ws_event: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
struct Entry {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    tui: Status,
    #[serde(default)]
    module: Option<String>,
    #[serde(default)]
    ticket: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Handled,
    Planned,
    Waived,
}

impl Entry {
    fn key(&self) -> &str {
        if self.id.is_empty() { &self.name } else { &self.id }
    }
}

fn manifest() -> Manifest {
    toml::from_str(MANIFEST).expect("crates/cctui-tui/parity.toml does not parse")
}

fn src_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Entries by key, rejecting duplicates.
fn index<'a>(kind: &str, entries: &'a [Entry]) -> BTreeMap<&'a str, &'a Entry> {
    let mut out = BTreeMap::new();
    for entry in entries {
        assert!(!entry.key().is_empty(), "a [[{kind}]] entry in parity.toml has no id/name");
        assert!(
            out.insert(entry.key(), entry).is_none(),
            "parity.toml lists [[{kind}]] {} twice",
            entry.key()
        );
    }
    out
}

/// Shape rules every entry obeys, whatever it describes.
fn check_shape(kind: &str, entry: &Entry) {
    let key = entry.key();
    match entry.tui {
        Status::Handled => {
            let module = entry
                .module
                .as_ref()
                .unwrap_or_else(|| panic!("{kind} {key} is handled but names no module"));
            assert!(
                src_dir().join(module).is_file(),
                "{kind} {key} names module {module}, which is not a file under crates/cctui-tui/src"
            );
        }
        Status::Planned => {
            let ticket = entry
                .ticket
                .as_ref()
                .unwrap_or_else(|| panic!("{kind} {key} is planned but names no ticket"));
            let number = ticket
                .strip_prefix("CCT-")
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or_else(|| panic!("{kind} {key}: {ticket} is not a CCT-<number> ticket"));
            assert!(
                TICKET_RANGE.contains(&number),
                "{kind} {key}: {ticket} is outside the TUI epic (CCT-{}..{})",
                TICKET_RANGE.start(),
                TICKET_RANGE.end()
            );
        }
        Status::Waived => {
            let reason = entry
                .reason
                .as_ref()
                .unwrap_or_else(|| panic!("{kind} {key} is waived but gives no reason"));
            assert!(reason.len() > 10, "{kind} {key}: the waiver reason is too terse: {reason}");
        }
    }
}

#[test]
fn every_entry_is_well_formed() {
    let manifest = manifest();
    for entry in &manifest.route {
        check_shape("route", entry);
    }
    for entry in &manifest.ws_event {
        check_shape("ws_event", entry);
    }
}

#[test]
fn every_route_has_an_entry() {
    let manifest = manifest();
    let entries = index("route", &manifest.route);
    for route in ROUTES {
        assert!(
            entries.contains_key(route.id),
            "{} {} is not in crates/cctui-tui/parity.toml: add a [[route]] entry marking it \
             handled, planned (with a ticket) or waived (with a reason)",
            route.method,
            route.path
        );
    }
}

#[test]
fn no_entry_names_a_route_that_is_gone() {
    let manifest = manifest();
    for entry in &manifest.route {
        assert!(
            ROUTES.iter().any(|r| r.id == entry.key()),
            "crates/cctui-tui/parity.toml lists route {}, which is no longer in \
             cctui_proto::api::routes::ROUTES — drop the entry or fix the id",
            entry.key()
        );
    }
}

#[test]
fn every_handled_route_is_called_by_its_module() {
    let manifest = manifest();
    for entry in manifest.route.iter().filter(|e| e.tui == Status::Handled) {
        let module = entry.module.as_ref().expect("checked by every_entry_is_well_formed");
        let route = ROUTES.iter().find(|r| r.id == entry.key()).expect("checked above");
        let source = std::fs::read_to_string(src_dir().join(module))
            .unwrap_or_else(|e| panic!("cannot read {module}: {e}"));
        let template = placeholders_blanked(route.path);
        let methods = client_methods_for(route.id);
        assert!(
            source.contains(route.id)
                || source.contains(&template)
                || methods.iter().any(|m| source.contains(&format!(".{m}("))),
            "route {} is marked handled by {module}, but {module} mentions neither {}, {template} \
             nor a cctui-client method typed on it ({methods:?})",
            entry.key(),
            route.id
        );
    }
}

/// The `cctui_client::Client` methods whose body names `route_id`.
fn client_methods_for(route_id: &str) -> Vec<String> {
    let rest = src_dir().join("../../cctui-client/src/rest.rs");
    let source = std::fs::read_to_string(&rest)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", rest.display()));
    let quoted = format!("\"{route_id}\"");
    source
        .split("pub async fn ")
        .skip(1)
        .filter(|body| body.split("\n    }").next().is_some_and(|b| b.contains(&quoted)))
        .filter_map(|body| body.split(['(', '<']).next().map(str::to_owned))
        .collect()
}

/// `/sessions/{id}/pins` -> `/sessions/{}/pins`, the shape a `format!` call has.
fn placeholders_blanked(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}').map(|i| open + i) else { break };
        out.push_str(&rest[..open]);
        out.push_str("{}");
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

/// One `(wire name, is waived)` per arm of `server_event::to_actions`.
///
/// The match is exhaustive over `ServerEvent`, so this is the variant list.
fn arms() -> Vec<(String, bool)> {
    let body = SERVER_EVENT_RS
        .split_once("pub fn to_actions")
        .expect("to_actions moved out of app/server_event.rs")
        .1;
    let body = body.split_once("\n}\n").expect("to_actions has no end").0;
    body.split("\n        ServerEvent::")
        .skip(1)
        .map(|arm| {
            let name: String =
                arm.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            let waived = arm.contains("waived(");
            (snake_case(&name), waived)
        })
        .collect()
}

fn snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, ch) in name.char_indices() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

#[test]
fn every_server_event_variant_has_an_entry() {
    let variants = arms();
    assert!(variants.len() > 20, "the arm parser found only {} arms", variants.len());
    let manifest = manifest();
    let entries = index("ws_event", &manifest.ws_event);
    for (name, _) in &variants {
        assert!(
            entries.contains_key(name.as_str()),
            "ServerEvent::{name} is not in crates/cctui-tui/parity.toml: add a [[ws_event]] entry"
        );
    }
    for entry in &manifest.ws_event {
        assert!(
            variants.iter().any(|(name, _)| name == entry.key()),
            "crates/cctui-tui/parity.toml lists ws_event {}, which is not a ServerEvent variant",
            entry.key()
        );
    }
}

#[test]
fn handled_ws_events_have_a_real_handler() {
    let manifest = manifest();
    let entries = index("ws_event", &manifest.ws_event);
    for (name, waived_in_source) in arms() {
        let entry = entries[name.as_str()];
        let handled = entry.tui == Status::Handled;
        assert_eq!(
            handled,
            !waived_in_source,
            "parity.toml says {name} is {:?}, but its arm in app/server_event.rs {} — \
             update whichever is wrong",
            entry.tui,
            if waived_in_source { "calls waived(...)" } else { "handles it" }
        );
    }
}
