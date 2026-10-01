mod app;
mod auth;
mod clipboard;
mod config;
mod editor;
mod install;
#[cfg(test)]
mod journeys;
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
    /// Re-download the release the server runs and re-apply settings.
    Update {
        /// Install it even if it is older than the running version.
        #[arg(long)]
        force: bool,
    },
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

/// How long quit waits for the flushed draft writes before giving up on them.
const QUIT_FLUSH: std::time::Duration = std::time::Duration::from_millis(1500);

#[tokio::main]
async fn main() -> Result<()> {
    use clap::Parser;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cli = Cli::parse();
    let filter = cli.filter;
    match cli.command {
        Some(Command::Update { force }) => {
            let (base_url, _) = resolve_identity();
            selfupdate::force_update(&base_url, force).await
        }
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
    let age = app::diagnose::fmt_age_opt(fact.age_ms);
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

fn fmt_age_since(at_ms: i64) -> String {
    app::diagnose::fmt_age((chrono::Utc::now().timestamp_millis() - at_ms).max(0))
}

/// Hands the terminal back to the shell: cooked mode, no mouse reporting, off
/// the alternate screen, cursor visible.
///
/// Written against any `Write` and ignoring errors, because the two callers that
/// matter are the normal exit and the panic hook, and a hook that gives up
/// halfway leaves the terminal unusable.
fn restore_terminal(out: &mut impl io::Write) {
    let _ = disable_raw_mode();
    let _ = execute!(
        out,
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen,
        crossterm::cursor::Show
    );
    let _ = out.flush();
}

/// Restores the terminal before the default hook prints, so the panic lands on
/// the shell's screen instead of the alternate one that is about to disappear.
///
/// Only the thread that draws gets that treatment. The hook is process-wide, and
/// a panic in a background task does not end the UI — tokio catches it and the
/// main loop keeps drawing — so tearing the terminal down there would leave the
/// TUI painting over a cooked-mode shell. Those are logged instead; the effect
/// runner turns the ones it owns into a toast.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    let ui_thread = std::thread::current().id();
    std::panic::set_hook(Box::new(move |info| {
        if on_panic(ui_thread, &mut io::stdout()) {
            previous(info);
            return;
        }
        tracing::error!(%info, "a background task panicked");
    }));
}

/// Hands the terminal back only when the panicking thread is the one that draws.
/// Returns whether it did, which is also whether the default hook should print:
/// its message would otherwise land on a screen the TUI is still painting.
fn on_panic(ui_thread: std::thread::ThreadId, out: &mut impl io::Write) -> bool {
    if std::thread::current().id() != ui_thread {
        return false;
    }
    restore_terminal(out);
    true
}

async fn run_tui(startup: Startup) -> Result<()> {
    let (base_url, token) = resolve_identity();

    install_panic_hook();
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal, base_url, token, startup).await;

    restore_terminal(terminal.backend_mut());

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
    // The graphics query writes to and reads from the tty, so it runs before
    // the first frame is drawn over it.
    app.images.detect();
    app.images.inline_pref = app.config.prefs.inline_images;
    apply_server_settings(&server, &mut app).await;
    theme::init(app.config.theme);

    init_sessions(&server, &mut app).await;
    let startup_effects = app::deeplink::apply(&mut app, startup);
    let (ws, mut event_rx) = server.connect_ws();
    let (effects, mut action_rx) = Effects::start(Arc::clone(&server), Arc::new(ws));
    effects.dispatch_all(startup_effects);
    effects.dispatch(app::action::Effect::FetchSessionStats);
    effects.dispatch(app::action::Effect::FetchIdentity);
    app.permissions.fetch_started();
    effects.dispatch(app::action::Effect::FetchPendingPermissions);
    // Drafts a previous exit could not hand over. They go back before the server
    // index lands; the index only fills gaps, so the newer local copy wins.
    effects.dispatch_all(app::drafts::restore_recovered(&mut app, config::recovery::load()));
    // The texts are in hand now; a quit that still cannot reach the server writes
    // them again, so the file must not outlive this restore.
    config::recovery::clear();
    effects.dispatch(app::action::Effect::LoadDraftIndex);
    effects.dispatch(app::action::Effect::FetchDispatchers);
    // The status line counts machines, so it cannot wait for the slice's visit.
    effects.dispatch(app::action::Effect::FetchMachines);
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
            let actions = editor_handoff(terminal, &gate, &request);
            for action in actions {
                effects.dispatch_all(reduce(&mut app, action));
            }
        }

        if app.should_quit {
            // The draft writes `Quit` just queued are the last thing the user
            // typed; a debounced or still-queued save dies with the process.
            effects.drain(QUIT_FLUSH).await;
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
        app.settings_blob = Some(payload.data);
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
/// What the loop does with a pending editor request: the text that came back,
/// or a toast saying why it did not.
fn editor_handoff(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    gate: &editor::InputGate,
    request: &editor::EditorRequest,
) -> Vec<Action> {
    match hand_over_to_editor(terminal, gate, &request.text) {
        Ok(text) => vec![Action::EditorFinished { target: request.target, text }],
        Err(e) => {
            tracing::warn!(%e, "the editor handoff failed");
            vec![Action::Toast(Level::Error, format!("editor: {e}"))]
        }
    }
}

/// Re-enters the TUI's terminal: raw mode, mouse reporting, alternate screen.
fn reenter_terminal(out: &mut impl io::Write) {
    let _ = enable_raw_mode();
    let _ = execute!(out, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste);
    let _ = out.flush();
}

/// Takes the terminal back and lets the input thread read again, however the
/// handoff ended. A `?` that skipped either would leave the TUI drawn but deaf.
struct EditorHandoff<'a> {
    gate: &'a editor::InputGate,
    /// Tests set this false: `enable_raw_mode` would act on the real terminal
    /// running the test, and the gate is the half worth asserting on.
    reenter: bool,
}

impl Drop for EditorHandoff<'_> {
    fn drop(&mut self) {
        // Unwinding past here means the hook has already handed the terminal
        // back; re-entering would undo exactly that, and nothing restores after.
        if self.reenter && !std::thread::panicking() {
            reenter_terminal(&mut io::stdout());
        }
        self.gate.unpark();
    }
}

/// Gives the terminal up, runs the editor, and takes it back.
///
/// The input thread is parked first: it and the editor would otherwise both be
/// reading stdin. The guard restores both on every path out, so neither a broken
/// `$EDITOR` nor a write error on the way down can leave the terminal unusable.
fn hand_over_to_editor(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    gate: &editor::InputGate,
    text: &str,
) -> io::Result<String> {
    gate.park();
    let handoff = EditorHandoff { gate, reenter: true };

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

    drop(handoff);
    terminal.clear()?;
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

#[cfg(test)]
mod terminal_tests {
    use super::{EditorHandoff, install_panic_hook, on_panic, reenter_terminal, restore_terminal};
    use crate::editor::InputGate;

    /// What the shell needs back: cooked mode is a syscall, the rest are
    /// sequences we can read.
    #[test]
    fn restoring_leaves_the_alternate_screen_and_shows_the_cursor() {
        let mut out: Vec<u8> = Vec::new();
        restore_terminal(&mut out);
        let written = String::from_utf8_lossy(&out);
        assert!(written.contains("\x1b[?1049l"), "it must leave the alternate screen: {written:?}");
        assert!(written.contains("\x1b[?1000l"), "it must stop mouse reporting: {written:?}");
        assert!(written.contains("\x1b[?2004l"), "it must stop bracketed paste: {written:?}");
        assert!(written.contains("\x1b[?25h"), "it must show the cursor: {written:?}");
    }

    #[test]
    fn re_entering_is_the_mirror_of_restoring() {
        let mut out: Vec<u8> = Vec::new();
        reenter_terminal(&mut out);
        let written = String::from_utf8_lossy(&out);
        assert!(written.contains("\x1b[?1049h"), "it must enter the alternate screen: {written:?}");
        assert!(written.contains("\x1b[?1000h"), "it must resume mouse reporting: {written:?}");
        assert!(written.contains("\x1b[?2004h"), "it must resume bracketed paste: {written:?}");
    }

    /// F7: a panic used to unwind straight past the teardown, leaving the shell
    /// in raw mode on the alternate screen with the message written to it.
    #[test]
    fn a_panic_restores_the_terminal_and_still_reports() {
        use std::sync::atomic::{AtomicBool, Ordering};
        static REPORTED: AtomicBool = AtomicBool::new(false);

        let original = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| REPORTED.store(true, Ordering::SeqCst)));
        install_panic_hook();

        let outcome = std::panic::catch_unwind(|| panic!("a reducer indexed off the end"));

        let _ = std::panic::take_hook();
        std::panic::set_hook(original);

        assert!(outcome.is_err(), "the panic still happens");
        assert!(
            REPORTED.load(Ordering::SeqCst),
            "the hook must chain to the previous one, or the panic is swallowed"
        );
    }

    /// F13: the early `?` on the way down used to return with the gate still
    /// parked, and the input thread then slept forever.
    #[test]
    fn the_handoff_guard_always_lets_input_resume() {
        let gate = InputGate::new();

        // Mirrors the input thread, so `park` does not wait out its full timeout.
        let thread_gate = gate.clone();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_stop = std::sync::Arc::clone(&stop);
        let input = std::thread::spawn(move || {
            while !thread_stop.load(std::sync::atomic::Ordering::SeqCst) {
                let _ = thread_gate.should_park();
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        });

        let handed_off = |fail: bool| -> std::io::Result<()> {
            gate.park();
            let _handoff = EditorHandoff { gate: &gate, reenter: false };
            if fail {
                // Stands in for the `?` on disable_raw_mode / execute!.
                return Err(std::io::Error::other("stdout went away"));
            }
            Ok(())
        };

        assert!(handed_off(true).is_err(), "the write error still surfaces");
        assert!(!gate.should_park(), "a failed handoff left the input thread parked");

        handed_off(false).expect("the clean path");
        assert!(!gate.should_park(), "the clean path must unpark too");

        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        input.join().expect("the stand-in input thread");
    }

    /// N2: the hook is process-wide, and a panicking effect task does not end the
    /// UI — tokio catches it and the main loop keeps drawing. Tearing the terminal
    /// down there left the TUI painting over a cooked-mode shell.
    #[test]
    fn a_background_panic_leaves_the_terminal_alone() {
        let ui_thread = std::thread::current().id();

        let elsewhere = std::thread::spawn(move || {
            let mut out: Vec<u8> = Vec::new();
            let tore_down = on_panic(ui_thread, &mut out);
            (tore_down, out)
        });
        let (tore_down, out) = elsewhere.join().expect("the background thread");

        assert!(!tore_down, "a background panic must not hand the terminal back");
        assert!(out.is_empty(), "it wrote {} bytes at the running TUI", out.len());
    }

    /// The other half: a panic on the drawing thread does end the UI, so it still
    /// restores and still lets the default hook print.
    #[test]
    fn a_panic_on_the_drawing_thread_still_restores() {
        let mut out: Vec<u8> = Vec::new();
        let tore_down = on_panic(std::thread::current().id(), &mut out);

        assert!(tore_down, "the drawing thread's panic ends the UI");
        let written = String::from_utf8_lossy(&out);
        assert!(written.contains("\x1b[?1049l"), "it must leave the alternate screen: {written:?}");
        assert!(written.contains("\x1b[?25h"), "it must show the cursor: {written:?}");
    }

    /// N3: unwinding through the handoff re-entered raw mode on the alternate
    /// screen after the hook had just handed the terminal back.
    #[test]
    fn a_panic_during_the_handoff_does_not_re_enter_the_alternate_screen() {
        use crate::editor::InputGate;
        let gate = InputGate::new();

        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _handoff = EditorHandoff { gate: &gate, reenter: true };
            panic!("the editor blew up");
        }));

        assert!(outcome.is_err(), "the panic still happens");
        assert!(!gate.should_park(), "the gate is released either way");
    }
}
