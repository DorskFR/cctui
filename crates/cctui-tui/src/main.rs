mod app;
mod auth;
mod clipboard;
mod config;
mod editor;
mod install;
mod keys;
#[cfg(test)]
mod parity;
mod selfupdate;
#[cfg(test)]
mod server_event_contract;
mod termnotify;
#[cfg(test)]
mod testsupport;
mod theme;
mod ui;
#[cfg(test)]
mod view_snapshots;
mod views;
mod widgets;

use std::io;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use app::deeplink::Startup;
use app::effects::Effects;
use app::session_live::{SessionLiveAction, TICK_MS};
use app::toast::Level;
use app::{Action, App, reduce, server_event};
use cctui_client::{Client, Incoming};
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyEventKind, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use keys::InputEvent;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;
use tokio::time;

/// Prefer `~/.config/cctui/user.json`; env vars still override so local dev
/// (e.g. `CCTUI_TOKEN=dev-admin`) keeps working.
fn resolve_identity() -> (String, String) {
    let identity = cctui_proto::identity::load_user();
    let default_url = identity
        .as_ref()
        .map_or_else(|| "http://localhost:8700".to_string(), |i| i.server_url.clone());
    let default_token = identity.map(|i| i.user_key).unwrap_or_default();
    let base_url = std::env::var("CCTUI_URL").unwrap_or(default_url);
    let token = std::env::var("CCTUI_TOKEN").unwrap_or(default_token);
    (base_url, token)
}

#[derive(clap::Parser)]
#[command(name = "cctui", version, about = "Claude Code Control TUI")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Open the list with this search already applied, e.g. `tag:wave-5`.
    #[arg(long, value_name = "QUERY", global = true)]
    filter: Option<String>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Log in and store the credential in `~/.config/cctui/user.json`.
    Login {
        /// Server to log in to. Defaults to the stored one, then `CCTUI_URL`.
        #[arg(long)]
        server: Option<String>,
        /// Use an existing key instead of the browser approval flow. Without a
        /// value the key is read from stdin, keeping it out of the shell history.
        #[arg(long, num_args = 0..=1, default_missing_value = "")]
        key: Option<String>,
    },
    /// Forget the stored credential.
    Logout {
        /// Also revoke the key server-side, so it cannot be used again.
        #[arg(long)]
        revoke: bool,
    },
    /// Force re-download of the latest cctui release and re-apply settings.
    Update,
    /// Start on one session's conversation. The session need not be in the
    /// live list: an archived one is fetched by id.
    Open {
        /// The session id (as shown in the session list / URL).
        session_id: String,
        /// Transcript position to land on.
        #[arg(long)]
        seq: Option<i64>,
    },
    /// One-call session diagnose: print everything the daemon knows
    /// about a session — each fact dated + sourced — plus the server-side
    /// gateway/account binding facts.
    Diagnose {
        /// The session id (as shown in the session list / URL).
        session_id: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    use clap::Parser;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cli = Cli::parse();
    let filter = cli.filter;
    match cli.command {
        Some(Command::Update) => selfupdate::force_update().await,
        Some(Command::Login { server, key }) => auth::login(server, key).await,
        Some(Command::Logout { revoke }) => auth::logout(revoke).await,
        Some(Command::Diagnose { session_id }) => run_diagnose(&session_id).await,
        Some(Command::Open { session_id, seq }) => {
            let (base_url, _) = resolve_identity();
            selfupdate::maybe_update(&base_url).await;
            run_tui(Startup { open: Some(session_id), seq, filter }).await
        }
        None => {
            let (base_url, _) = resolve_identity();
            selfupdate::maybe_update(&base_url).await;
            run_tui(Startup { filter, ..Startup::default() }).await
        }
    }
}

/// `cctui diagnose <session-id>`: fetch the one-call diagnose blob
/// and render it as one dated, sourced line per fact.
async fn run_diagnose(session_id: &str) -> Result<()> {
    let (base_url, token) = resolve_identity();
    let server = Client::new(&base_url, &token);
    let resp = server.diagnose(session_id).await?;

    println!("session {}", resp.session_id);
    let s = &resp.server;
    println!(
        "server: status={} adapter={} account_bound={} accounts=[{}] machine={} last_seen={}",
        s.status.as_deref().unwrap_or("?"),
        s.adapter_id.as_deref().unwrap_or("?"),
        s.account_bound,
        s.accounts.join(", "),
        s.machine_id.as_deref().unwrap_or("?"),
        s.machine_last_seen_ms.map_or_else(|| "?".to_owned(), fmt_age_since),
    );
    if let Some(err) = &resp.daemon_error {
        println!("daemon: UNAVAILABLE — {err}");
    }
    let Some(d) = &resp.daemon else { return Ok(()) };
    println!(
        "daemon report: adapter={} short={} generated_at={}",
        d.adapter,
        d.short.as_deref().unwrap_or("?"),
        d.generated_at_ms,
    );
    print_fact("effective_state", &d.effective_state);
    print_fact("last_hook_event", &d.last_hook_event);
    print_fact("attach", &d.attach);
    print_fact("pty_output", &d.pty_output);
    print_fact("claude_socket", &d.claude_socket);
    print_fact("transcript", &d.transcript);
    print_fact("prompts", &d.prompts);
    print_fact("permission_mode", &d.permission_mode);
    print_fact("dispatch", &d.dispatch);
    print_fact("gateway", &d.gateway);
    Ok(())
}

/// One `name [source, age]: value-or-reason` line per fact.
fn print_fact<T: serde::Serialize>(name: &str, fact: &cctui_proto::diagnose::DiagnoseFact<T>) {
    let age = fact.age_ms.map_or_else(|| "undated".to_owned(), fmt_age);
    match &fact.value {
        Some(v) => {
            let rendered = serde_json::to_string(v).unwrap_or_else(|_| "<unserializable>".into());
            println!("  {name} [{}, {age}]: {rendered}", fact.source);
        }
        None => println!(
            "  {name} [{}, {age}]: — ({})",
            fact.source,
            fact.missing_reason.as_deref().unwrap_or("missing"),
        ),
    }
}

fn fmt_age(ms: i64) -> String {
    match ms {
        ms if ms < 1_000 => format!("{ms}ms ago"),
        ms if ms < 60_000 => format!("{}s ago", ms / 1_000),
        ms if ms < 3_600_000 => format!("{}m ago", ms / 60_000),
        ms => format!("{}h ago", ms / 3_600_000),
    }
}

fn fmt_age_since(at_ms: i64) -> String {
    fmt_age((chrono::Utc::now().timestamp_millis() - at_ms).max(0))
}

async fn run_tui(startup: Startup) -> Result<()> {
    let (base_url, token) = resolve_identity();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal, base_url, token, startup).await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;

    result
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

async fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    base_url: String,
    token: String,
    startup: Startup,
) -> Result<()> {
    let server = Arc::new(Client::new(&base_url, &token));
    let mut app = App::new();
    app.server_url = base_url.clone();
    apply_config(&mut app);
    apply_server_settings(&server, &mut app).await;
    theme::init(app.config.theme);

    init_sessions(&server, &mut app).await;
    let startup_effects = app::deeplink::apply(&mut app, startup);
    let (ws, mut event_rx) = server.connect_ws();
    let (effects, mut action_rx) = Effects::start(Arc::clone(&server), Arc::new(ws));
    effects.dispatch_all(startup_effects);
    effects.dispatch(app::action::Effect::FetchSessionStats);
    effects.dispatch(app::action::Effect::FetchIdentity);
    effects.dispatch(app::action::Effect::FetchPendingPermissions);
    effects.dispatch(app::action::Effect::LoadDraftIndex);
    // The one clock in the app. Delivery deadlines are the reducer's and the
    // reducer only moves when it is called, so this has to be far tighter than
    // the session-list poll, which the reducer gates on its own elapsed period.
    let mut tick = time::interval(Duration::from_millis(TICK_MS));
    tick.tick().await;
    let gate = editor::InputGate::new();
    let mut input_rx = spawn_input_task(gate.clone());
    let mut ws_closed = false;
    let mut ws_ever_connected = false;

    loop {
        app.clock_ms = now_ms();
        app.toasts.prune(app.clock_ms);
        update_scroll_metrics(&mut app);
        terminal.draw(|f| views::render(f, &mut app))?;
        // The one safe point for an escape sequence: the frame is on screen and
        // nothing else is mid-write.
        if let Err(e) = termnotify::emit(&app.watch.take_pending()) {
            tracing::warn!(%e, "cannot write the terminal attention sequences");
        }

        let actions: Vec<Action> = tokio::select! {
            biased;

            maybe_input = input_rx.recv() => {
                // A held lead chord lives exactly one key long, whatever that
                // key turns out to mean.
                let pending = app.pending_chord.take();
                maybe_input
                    .and_then(|input| {
                        keys::map_input(
                            &app.config.keys,
                            app.view(),
                            app.input_active,
                            app.prompt_focus(),
                            app.key_overlay(),
                            pending,
                            input,
                        )
                    })
                    .map_or_else(Vec::new, |action| vec![action])
            }
            maybe_action = action_rx.recv() => {
                let mut actions = maybe_action.map_or_else(Vec::new, |a| vec![a]);
                while let Ok(a) = action_rx.try_recv() {
                    actions.push(a);
                }
                actions
            }
            maybe_event = event_rx.recv(), if !ws_closed => {
                maybe_event.map_or_else(
                    || {
                        ws_closed = true;
                        vec![
                            Action::SessionLive(SessionLiveAction::WsHealth(false)),
                            Action::Toast(Level::Error, "event stream closed".to_owned()),
                        ]
                    },
                    |event| {
                        let mut actions = incoming_actions(event, &mut ws_ever_connected);
                        while let Ok(ev) = event_rx.try_recv() {
                            actions.extend(incoming_actions(ev, &mut ws_ever_connected));
                        }
                        actions
                    },
                )
            }
            _ = tick.tick() => vec![Action::Tick],
        };

        for action in actions {
            effects.dispatch_all(reduce(&mut app, action));
        }

        if let Some(request) = app.editor.take() {
            match hand_over_to_editor(terminal, &gate, &request.text) {
                Ok(text) => effects.dispatch_all(reduce(
                    &mut app,
                    Action::EditorFinished { target: request.target, text },
                )),
                Err(e) => {
                    tracing::warn!(%e, "the editor handoff failed");
                    effects.dispatch_all(reduce(
                        &mut app,
                        Action::Toast(Level::Error, format!("editor: {e}")),
                    ));
                }
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

/// Config problems are toasts, never a startup failure: a typo in one binding
/// must not keep the TUI from opening.
fn apply_config(app: &mut App) {
    app.clock_ms = now_ms();
    let loaded = config::load();
    app.config = loaded.config;
    app.ui = config::uistate::load();
    app.filter = app::cmdline::restore(&app.ui);
    for problem in loaded.problems {
        app.toast(Level::Warn, format!("tui.toml: {problem}"));
    }
}

/// The server's user settings are defaults under the local file. An
/// unreachable or unreadable server simply leaves the local config in force.
async fn apply_server_settings(server: &Client, app: &mut App) {
    if let Ok(payload) = server.settings().await {
        app.config.apply_server(config::server::ServerPrefs::from_settings(&payload.data));
        app.macros = app::macros::from_settings(&payload.data);
        app.list_shape = app::list_view::ListShape::from_settings(&payload.data);
        // Kept whole: a settings write is a replace, so a patch needs the rest.
        app.settings_blob = payload.data;
        app.reshape();
    }
    app.show_timestamps = app.config.prefs.timestamps;
}

async fn init_sessions(server: &Client, app: &mut App) {
    if let Ok(resp) = server.list_sessions().await {
        app.sessions = resp.sessions;
        app.update_aggregates();
        app.refresh.sent += 1;
        app.last_refresh_ms = app.clock_ms;
    }
}

/// `ever_connected` keeps the first connect silent: only a genuine reconnect
/// refreshes state and toasts.
fn incoming_actions(incoming: Incoming, ever_connected: &mut bool) -> Vec<Action> {
    match incoming {
        Incoming::Event(event) => server_event::to_actions(*event),
        Incoming::Undecodable(reason) => vec![Action::UndecodableWsMessage(reason)],
        Incoming::Connected => {
            let healthy = Action::SessionLive(SessionLiveAction::WsHealth(true));
            if std::mem::replace(ever_connected, true) {
                vec![healthy, Action::Reconnected]
            } else {
                vec![healthy]
            }
        }
        Incoming::Disconnected(reason) => {
            tracing::warn!(%reason, "websocket dropped; reconnecting");
            vec![
                Action::SessionLive(SessionLiveAction::WsHealth(false)),
                Action::Toast(Level::Warn, "connection lost — reconnecting".to_owned()),
            ]
        }
    }
}

/// Bootstrap `viewport_height` from terminal size if not yet set by a render pass.
fn update_scroll_metrics(app: &mut App) {
    if app.viewport_height == 0
        && let Ok((_, rows)) = crossterm::terminal::size()
    {
        app.viewport_height = (rows as usize).saturating_sub(5);
    }
}

/// Spawns a dedicated blocking thread that reads terminal events and forwards
/// them to the main loop via a channel. Using a persistent task (rather than
/// `spawn_blocking` per iteration inside `tokio::select!`) prevents input
/// starvation when the WS event stream keeps the select loop busy — the
/// channel retains pending keypresses across iterations.
/// Gives the terminal up, runs the editor, and takes it back.
///
/// The input thread is parked first: it and the editor would otherwise both be
/// reading stdin. Everything is restored on the way out, including after a
/// failure, so a broken `$EDITOR` cannot leave the terminal unusable.
fn hand_over_to_editor(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    gate: &editor::InputGate,
    text: &str,
) -> io::Result<String> {
    gate.park();
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;

    let argv = editor::editor_from_env();
    let edited = editor::edit_via_file(text, |path| editor::run_editor(&argv, path));

    enable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    terminal.clear()?;
    gate.unpark();
    edited
}

fn spawn_input_task(gate: editor::InputGate) -> mpsc::Receiver<InputEvent> {
    let (tx, rx) = mpsc::channel::<InputEvent>(64);
    std::thread::spawn(move || {
        loop {
            if gate.should_park() {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            match event::poll(Duration::from_millis(100)) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(_) => return,
            }
            let Ok(ev) = event::read() else { return };
            let mapped = match ev {
                Event::Key(key) if key.kind == KeyEventKind::Press => Some(InputEvent::Key(key)),
                Event::Paste(text) => Some(InputEvent::Paste(text)),
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => Some(InputEvent::ScrollUp),
                    MouseEventKind::ScrollDown => Some(InputEvent::ScrollDown),
                    _ => None,
                },
                _ => None,
            };
            if let Some(input) = mapped
                && tx.blocking_send(input).is_err()
            {
                return;
            }
        }
    });
    rx
}
