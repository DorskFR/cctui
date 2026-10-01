//! Replays the webui's journey IR against the TUI.
//!
//! `docs/journeys/<id>/ir.json` is the shared spec and `journeys.toml` says
//! which of it the TUI must express. A step's target resolves through the
//! widget registry below: a `required` step whose target is unmapped fails.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crossterm::event::KeyEvent;
use serde::Deserialize;

use crate::app::{App, View, reduce};
use crate::config::chord::Chord;
use crate::keys::{self, InputEvent};
use crate::testsupport::{CLOCK_MS, app_with_sessions, conversation_store, diagnosable_session};

#[derive(Debug, Deserialize)]
struct Ir {
    id: String,
    version: u32,
    route: Option<String>,
    steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
struct Step {
    id: String,
    #[serde(rename = "do")]
    act: Do,
    route: Option<String>,
    target: Option<Target>,
    #[serde(default)]
    expect: Vec<Expect>,
    when: Option<When>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Do {
    None,
    Click,
    Fill { value: serde_json::Value },
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Target {
    Named(String),
    Query(serde_json::Map<String, serde_json::Value>),
}

#[derive(Debug, Deserialize)]
struct Expect {
    visible: Option<Target>,
    hidden: Option<Target>,
    count: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct When {
    viewport: Option<String>,
}

impl Target {
    /// The registry key. A query object collapses to one line so the registry
    /// can name a css/label target as flatly as a `data-journey` path.
    fn key(&self) -> String {
        match self {
            Self::Named(name) => name.clone(),
            Self::Query(map) => {
                let part = |k: &str| map.get(k).and_then(serde_json::Value::as_str).unwrap_or("");
                let nth = map.get("nth").and_then(serde_json::Value::as_i64);
                let mut key = String::new();
                if !part("css").is_empty() {
                    let _ = write!(key, "css:{}", part("css"));
                } else if !part("label").is_empty() {
                    let _ = write!(key, "label:{}", part("label"));
                } else if !part("role").is_empty() {
                    let _ = write!(key, "role:{}/{}", part("role"), part("name"));
                }
                if !part("within").is_empty() {
                    let _ = write!(key, "@{}", part("within"));
                }
                if let Some(nth) = nth {
                    let _ = write!(key, "#{nth}");
                }
                key
            }
        }
    }
}

/// What makes a widget's TUI counterpart observable.
enum Probe {
    /// A substring of the rendered screen.
    Screen(&'static str),
    State(fn(&App) -> bool),
    /// State the screen alone cannot show, plus what it must still draw.
    Both(fn(&App) -> bool, &'static str),
}

/// One journey target's TUI counterpart: the keys that bring it on screen, what
/// proves it is there, how to put the screen back, and what acting on it means.
struct Widget {
    reveal: &'static [&'static str],
    dismiss: &'static [&'static str],
    probe: Probe,
    click: Option<&'static [&'static str]>,
    fill: Option<&'static [&'static str]>,
}

impl Widget {
    const fn new(probe: Probe) -> Self {
        Self { reveal: &[], dismiss: &[], probe, click: None, fill: None }
    }

    const fn behind(
        mut self,
        reveal: &'static [&'static str],
        dismiss: &'static [&'static str],
    ) -> Self {
        self.reveal = reveal;
        self.dismiss = dismiss;
        self
    }

    const fn clicked_by(mut self, keys: &'static [&'static str]) -> Self {
        self.click = Some(keys);
        self
    }

    const fn filled_by(mut self, keys: &'static [&'static str]) -> Self {
        self.fill = Some(keys);
        self
    }
}

fn registry(key: &str) -> Option<Widget> {
    Some(match key {
        "session-list" => Widget::new(Probe::Both(
            |app| app.view() == View::SessionList && !app.flattened_sessions().is_empty(),
            "[claude-code]",
        )),
        // The section switches are a menu in the web and an overlay on `f` here.
        "sections" => {
            Widget::new(Probe::Screen("Sections")).behind(&["f"], &["esc"]).clicked_by(&["f"])
        }
        // The ⋯ menu's dimension pickers are a permanent strip in the TUI.
        "options" => Widget::new(Probe::Screen("group:")),
        "search" | "label:Search sessions@search" => {
            Widget::new(Probe::State(|app| app.list_search.open))
                .behind(&["/"], &["esc"])
                .clicked_by(&["/"])
                .filled_by(&["/"])
        }
        "css:[data-journey=\"session\"] [data-journey=\"title\"]#0" => {
            Widget::new(Probe::State(|app| app.selected_session_id().is_some()))
                .clicked_by(&["enter"])
        }
        "conversation" => Widget::new(Probe::State(|app| app.view() == View::Conversation)),
        "composer" | "composer/message" => Widget::new(Probe::Screen("Type a message")),
        "conversation/header" => Widget::new(Probe::Screen(" on orion")),
        "conversation/head-meta" => Widget::new(Probe::Screen("opus ── $")),
        "conversation/head-details" => {
            Widget::new(Probe::Screen("Session info")).behind(&["i"], &["esc"])
        }
        "activity" => Widget::new(Probe::Screen("waiting for input")),
        "css:[data-journey=\"conversation\"] [data-journey=\"line\"]#0" => {
            Widget::new(Probe::State(|app| {
                app.selected_session_id()
                    .and_then(|id| app.conversation(&id))
                    .is_some_and(|store| store.lines().next().is_some())
            }))
        }
        "filters" => Widget::new(Probe::State(|app| app.view() == View::Conversation)),
        // A chip per line kind in the web; in the TUI the same per-category
        // switches are the `F` menu, which opens on the assistant row.
        "filters/quick[assistant]" => Widget::new(Probe::Screen("Assistant prose"))
            .behind(&["F"], &["esc"])
            .clicked_by(&["F", "space", "esc"]),
        "filters/filter-menu" => {
            Widget::new(Probe::Screen("show which lines")).behind(&["F"], &["esc"])
        }
        "conversation/line[assistant]" => Widget::new(Probe::Screen("● Assistant")),
        _ => return None,
    })
}

/// The TUI's stand-ins for the `{param}` names a journey fills in from the live
/// instance.
fn param(name: &str) -> Option<&'static str> {
    Some(match name {
        "var.query" => "infra",
        "var.blank" | "var.label" | "var.prompt" | "var.facet" => "",
        "fixture.session" => "s-working",
        "fixture.me" => "dorsk",
        _ => return None,
    })
}

#[derive(Debug, Deserialize)]
struct Manifest {
    journey: Vec<JourneySpec>,
}

#[derive(Debug, Deserialize)]
struct JourneySpec {
    id: String,
    tui: Verdict,
    reason: Option<String>,
    #[serde(default)]
    step: Vec<StepSpec>,
}

#[derive(Debug, Deserialize)]
struct StepSpec {
    id: String,
    tui: Verdict,
    reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Verdict {
    Required,
    Waived,
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("the repo root")
}

fn manifest() -> Manifest {
    let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("journeys.toml"))
        .expect("crates/cctui-tui/journeys.toml");
    toml::from_str(&text).expect("crates/cctui-tui/journeys.toml does not parse")
}

fn exported() -> Vec<Ir> {
    let dir = repo_root().join("docs/journeys");
    let mut out = Vec::new();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("docs/journeys")
        .filter_map(|e| Some(e.ok()?.path()))
        .collect();
    entries.sort();
    for entry in entries {
        let file = entry.join("ir.json");
        if !file.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&file).expect("an exported journey");
        out.push(
            serde_json::from_str(&text).unwrap_or_else(|e| {
                panic!("{} does not parse as a journey IR: {e}", file.display())
            }),
        );
    }
    assert!(!out.is_empty(), "no docs/journeys/*/ir.json: run `npm run journey:ir` in webui");
    out
}

fn ir(id: &str) -> Ir {
    exported()
        .into_iter()
        .find(|j| j.id == id)
        .unwrap_or_else(|| panic!("journey {id} is not exported"))
}

fn spec_for(id: &str) -> JourneySpec {
    manifest()
        .journey
        .into_iter()
        .find(|j| j.id == id)
        .unwrap_or_else(|| panic!("no spec for {id}"))
}

fn key_event(key: &str) -> KeyEvent {
    Chord::parse(key).unwrap_or_else(|e| panic!("journey runner key {key:?}: {e}")).event()
}

fn press(app: &mut App, key: &str) {
    let pending = app.pending_chord.take();
    let action = keys::map_input(
        &app.config.keys,
        app.view(),
        app.input_active,
        app.prompt_focus(),
        app.key_overlay(),
        pending,
        InputEvent::Key(key_event(key)),
    );
    if let Some(action) = action {
        reduce(app, action);
    }
}

fn press_all(app: &mut App, keys: &[&str]) {
    for key in keys {
        press(app, key);
    }
}

fn holds(app: &mut App, w: &Widget) -> bool {
    match w.probe {
        Probe::Screen(anchor) => crate::testsupport::render_screen(app).contains(anchor),
        Probe::State(f) => f(app),
        Probe::Both(f, anchor) => f(app) && crate::testsupport::render_screen(app).contains(anchor),
    }
}

/// A probe may need the widget on screen first; whatever it took is undone, so
/// one expectation cannot change what the next step sees.
fn observe(app: &mut App, w: &Widget) -> bool {
    if holds(app, w) {
        return true;
    }
    if w.reveal.is_empty() {
        return false;
    }
    press_all(app, w.reveal);
    let seen = holds(app, w);
    press_all(app, w.dismiss);
    seen
}

struct Run {
    app: App,
    log: Vec<String>,
}

fn resolve(journey: &str, step: &str, target: &Target, what: &str) -> Widget {
    let key = target.key();
    registry(&key).unwrap_or_else(|| {
        panic!(
            "{journey}/{step}: journeys.toml requires this step but its {what} target {key:?} \
             has no widget in the TUI registry — map it in src/journeys.rs or waive the step"
        )
    })
}

impl Run {
    fn new() -> Self {
        let mut app = app_with_sessions();
        app.clock_ms = CLOCK_MS;
        let diagnosable = diagnosable_session();
        if let Some(row) = app.sessions.iter_mut().find(|s| s.id == diagnosable.id) {
            *row = diagnosable;
        }
        app.update_aggregates();
        for id in ["s-working", "s-blocked"] {
            app.conversations.insert(id.to_owned(), conversation_store());
        }
        Self { app, log: Vec::new() }
    }

    fn route(&mut self, route: &str) {
        let view = match route {
            "/sessions" => View::SessionList,
            "/" => View::Overview,
            other => panic!("journey route {other} has no TUI view"),
        };
        self.app.router.reset(view);
        let line = format!("route {route} -> {view:?}");
        if self.log.last() != Some(&line) {
            self.log.push(line);
        }
    }

    fn step(&mut self, journey: &str, step: &Step, spec: &StepSpec) {
        if spec.tui == Verdict::Waived {
            let reason = spec.reason.as_deref().expect("a waived step states a reason");
            self.log.push(format!("step {} WAIVED: {reason}", step.id));
            return;
        }
        if let Some(route) = &step.route {
            self.route(route);
        }
        self.log.push(format!("step {}", step.id));
        self.perform(journey, step);
        for expect in &step.expect {
            self.expect(journey, &step.id, expect);
        }
    }

    fn expect(&mut self, journey: &str, step: &str, expect: &Expect) {
        assert!(
            expect.count.is_none(),
            "{journey}/{step}: a `count` expectation has no TUI form yet — waive the step"
        );
        if let Some(target) = &expect.visible {
            let widget = resolve(journey, step, target, "visible");
            assert!(
                observe(&mut self.app, &widget),
                "{journey}/{step}: expected {} to be visible",
                target.key()
            );
            self.log.push(format!("  visible {}", target.key()));
        }
        if let Some(target) = &expect.hidden {
            let widget = resolve(journey, step, target, "hidden");
            assert!(
                !holds(&mut self.app, &widget),
                "{journey}/{step}: expected {} to be hidden",
                target.key()
            );
            self.log.push(format!("  hidden {}", target.key()));
        }
    }

    fn perform(&mut self, journey: &str, step: &Step) {
        let target = match (&step.act, &step.target) {
            (Do::None, _) => return,
            (_, None) => panic!("{journey}/{} acts on nothing", step.id),
            (_, Some(target)) => target,
        };
        let widget = resolve(journey, &step.id, target, "action");
        match &step.act {
            Do::None => unreachable!(),
            Do::Click => {
                let keys = widget.click.unwrap_or_else(|| {
                    panic!("{journey}/{}: {} is not clickable in the TUI", step.id, target.key())
                });
                press_all(&mut self.app, keys);
                self.log.push(format!("  click {} via {keys:?}", target.key()));
            }
            Do::Fill { value } => {
                let keys = widget.fill.unwrap_or_else(|| {
                    panic!("{journey}/{}: {} takes no text in the TUI", step.id, target.key())
                });
                let text = fill_text(value);
                press_all(&mut self.app, keys);
                // `fill` replaces the field's value, so an empty one clears it.
                for _ in 0..64 {
                    press(&mut self.app, "backspace");
                }
                for ch in text.chars() {
                    press(&mut self.app, &ch.to_string());
                }
                // The TUI's search is a modal overlay: emptying the box is how
                // the web leaves search, and here leaving it is what restores
                // the unfiltered screen behind it.
                if text.is_empty() {
                    press_all(&mut self.app, widget.dismiss);
                }
                self.log.push(format!("  fill {} with {text:?}", target.key()));
            }
        }
    }
}

fn fill_text(value: &serde_json::Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_owned();
    }
    let name = value
        .get("$param")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("a fill value the runner cannot read: {value}"));
    param(name).unwrap_or_else(|| panic!("journey param {name} has no TUI stand-in")).to_owned()
}

fn replay(id: &str) -> String {
    let ir = ir(id);
    let spec = spec_for(id);
    assert_eq!(spec.tui, Verdict::Required, "{id} is waived, not replayable");
    let mut run = Run::new();
    run.log.push(format!("journey {} v{}", ir.id, ir.version));
    if let Some(route) = &ir.route {
        run.route(route);
    }
    for step in &ir.steps {
        let step_spec = spec
            .step
            .iter()
            .find(|s| s.id == step.id)
            .unwrap_or_else(|| panic!("journeys.toml has no entry for {id}/{}", step.id));
        if step.when.as_ref().and_then(|w| w.viewport.as_deref()) == Some("mobile") {
            run.log.push(format!("step {} SKIPPED: mobile variant", step.id));
            continue;
        }
        run.step(id, step, step_spec);
    }
    run.log.join("\n")
}

#[test]
fn sessions_list_journey() {
    insta::assert_snapshot!(replay("sessions-list"));
}

#[test]
fn follow_session_journey() {
    insta::assert_snapshot!(replay("follow-session"));
}

#[test]
fn every_exported_journey_has_a_verdict() {
    let manifest = manifest();
    let specs: BTreeSet<&str> = manifest.journey.iter().map(|j| j.id.as_str()).collect();
    assert_eq!(specs.len(), manifest.journey.len(), "journeys.toml lists a journey twice");
    for ir in exported() {
        let spec = manifest.journey.iter().find(|j| j.id == ir.id).unwrap_or_else(|| {
            panic!(
                "journey {} is exported under docs/journeys but missing from \
                 crates/cctui-tui/journeys.toml",
                ir.id
            )
        });
        match spec.tui {
            Verdict::Waived => {
                assert!(
                    spec.reason.as_ref().is_some_and(|r| r.len() > 20),
                    "journeys.toml waives {} without saying why",
                    ir.id
                );
                assert!(
                    spec.step.is_empty(),
                    "journeys.toml waives {} as a whole, so it must not list steps",
                    ir.id
                );
            }
            Verdict::Required => {
                let steps: Vec<&str> = ir.steps.iter().map(|s| s.id.as_str()).collect();
                let listed: Vec<&str> = spec.step.iter().map(|s| s.id.as_str()).collect();
                assert_eq!(
                    steps, listed,
                    "journeys.toml's steps for {} drifted from docs/journeys/{}/ir.json",
                    ir.id, ir.id
                );
                for step in &spec.step {
                    assert!(
                        step.tui == Verdict::Required
                            || step.reason.as_ref().is_some_and(|r| r.len() > 20),
                        "journeys.toml waives {}/{} without saying why",
                        ir.id,
                        step.id
                    );
                }
            }
        }
    }
    let exported: BTreeSet<String> = exported().into_iter().map(|j| j.id).collect();
    for spec in &manifest.journey {
        assert!(
            exported.contains(&spec.id),
            "journeys.toml lists {}, which no docs/journeys/*/ir.json exports",
            spec.id
        );
    }
}

/// The acceptance criterion: a required step whose target is unmapped must
/// fail rather than pass silently.
#[test]
fn an_unmapped_required_target_is_a_failure() {
    let target = Target::Named("accounts".to_owned());
    assert!(registry(&target.key()).is_none(), "pick a target the TUI really has no widget for");
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        resolve("made-up", "step", &target, "visible");
    }));
    assert!(failure.is_err(), "an unmapped required target must panic");
}
