mod app;
mod client;
mod config;
mod install;
mod keys;
mod selfupdate;
#[cfg(test)]
mod server_event_contract;
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
use app::effects::Effects;
use app::{Action, App, reduce, server_event};
use cctui_proto::ws::TuiCommand;
use client::{Incoming, ServerClient};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, MouseEventKind,
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
}

#[derive(clap::Subcommand)]
enum Command {
    /// Force re-download of the latest cctui release and re-apply settings.
    Update,
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
    match Cli::parse().command {
        Some(Command::Update) => {
            let (base_url, _) = resolve_identity();
            selfupdate::force_update(&base_url).await
        }
        Some(Command::Diagnose { session_id }) => run_diagnose(&session_id).await,
        None => {
            let (base_url, _) = resolve_identity();
            selfupdate::maybe_update(&base_url).await;
            run_tui().await
        }
    }
}

/// `cctui diagnose <session-id>`: fetch the one-call diagnose blob
/// and render it as one dated, sourced line per fact.
async fn run_diagnose(session_id: &str) -> Result<()> {
    let (base_url, token) = resolve_identity();
    let server = ServerClient::new(&base_url, &token);
    let resp = server.diagnose_session(session_id).await?;

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

async fn run_tui() -> Result<()> {
    let (base_url, token) = resolve_identity();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal, base_url, token).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), DisableMouseCapture, LeaveAlternateScreen)?;
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
) -> Result<()> {
    let server = Arc::new(ServerClient::new(&base_url, &token));
    let mut app = App::new();
    apply_config(&mut app);
    apply_server_settings(&server, &mut app).await;
    theme::init(app.config.theme);

    init_sessions(&server, &mut app).await;
    let (cmd_tx, mut event_rx) = connect_ws_or_dummy(&server).await;
    let (effects, mut action_rx) = Effects::start(Arc::clone(&server), cmd_tx);
    let mut refresh_interval = time::interval(Duration::from_secs(5));
    refresh_interval.tick().await;
    let mut input_rx = spawn_input_task();
    // Backoff for WS reconnect attempts after the stream drops.
    let mut reconnect_backoff_secs: u64 = 1;
    let mut reconnect_timer: Option<std::pin::Pin<Box<time::Sleep>>> = None;

    loop {
        app.clock_ms = now_ms();
        app.toasts.prune(app.clock_ms);
        update_scroll_metrics(&mut app);
        terminal.draw(|f| views::render(f, &mut app))?;

        let actions: Vec<Action> = tokio::select! {
            biased;

            maybe_input = input_rx.recv() => {
                maybe_input
                    .and_then(|input| {
                        keys::map_input(&app.config.keys, app.view(), app.input_active, input)
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
            maybe_event = event_rx.recv() => {
                match maybe_event {
                    Some(event) => {
                        let mut actions = incoming_actions(event);
                        while let Ok(ev) = event_rx.try_recv() {
                            actions.extend(incoming_actions(ev));
                        }
                        actions
                    }
                    None if reconnect_timer.is_none() => {
                        // Stream dropped — schedule a reconnect attempt.
                        reconnect_timer = Some(Box::pin(time::sleep(Duration::from_secs(reconnect_backoff_secs))));
                        Vec::new()
                    }
                    None => {
                        // Already waiting; yield so the timer branch can fire.
                        tokio::task::yield_now().await;
                        Vec::new()
                    }
                }
            }
            () = async { reconnect_timer.as_mut().unwrap().await }, if reconnect_timer.is_some() => {
                reconnect_timer = None;
                if let Ok((new_tx, new_rx)) = server.connect_ws().await {
                    effects.set_commands(new_tx);
                    event_rx = new_rx;
                    reconnect_backoff_secs = 1;
                    vec![Action::Reconnected]
                } else {
                    reconnect_backoff_secs = (reconnect_backoff_secs * 2).min(30);
                    reconnect_timer = Some(Box::pin(time::sleep(Duration::from_secs(reconnect_backoff_secs))));
                    Vec::new()
                }
            }
            _ = refresh_interval.tick() => vec![Action::RefreshSessions],
        };

        for action in actions {
            effects.dispatch_all(reduce(&mut app, action));
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
    for problem in loaded.problems {
        app.toast(app::toast::Level::Warn, format!("tui.toml: {problem}"));
    }
}

/// The server's user settings are defaults under the local file. An
/// unreachable or unreadable server simply leaves the local config in force.
async fn apply_server_settings(server: &ServerClient, app: &mut App) {
    if let Ok(payload) = server.get_settings().await {
        app.config.apply_server(&config::server::ServerPrefs::from_settings(&payload.data));
    }
    app.show_timestamps = app.config.prefs.timestamps;
}

async fn init_sessions(server: &ServerClient, app: &mut App) {
    if let Ok(resp) = server.list_sessions().await {
        app.sessions = resp.sessions;
        app.update_aggregates();
    }
}

async fn connect_ws_or_dummy(
    server: &ServerClient,
) -> (mpsc::Sender<TuiCommand>, mpsc::Receiver<Incoming>) {
    (server.connect_ws().await).unwrap_or_else(|_| {
        let (tx, _) = mpsc::channel::<TuiCommand>(1);
        let (_, rx) = mpsc::channel::<Incoming>(1);
        (tx, rx)
    })
}

fn incoming_actions(incoming: Incoming) -> Vec<Action> {
    match incoming {
        Incoming::Event(event) => server_event::to_actions(*event),
        Incoming::Undecodable(reason) => vec![Action::UndecodableWsMessage(reason)],
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
fn spawn_input_task() -> mpsc::Receiver<InputEvent> {
    let (tx, rx) = mpsc::channel::<InputEvent>(64);
    std::thread::spawn(move || {
        loop {
            match event::poll(Duration::from_millis(100)) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(_) => return,
            }
            let Ok(ev) = event::read() else { return };
            let mapped = match ev {
                Event::Key(key) if key.kind == KeyEventKind::Press => Some(InputEvent::Key(key)),
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
