#![allow(dead_code)]

mod app;
mod client;
mod views;
mod widgets;

use std::io;
use std::time::Duration;

use anyhow::{Context, Result};
use app::{App, DetailMode, Pane, View};
use cctui_proto::ws::{ServerEvent, TuiCommand};
use client::ServerClient;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::{Frame, Terminal};
use tokio::sync::mpsc;
use tokio::time;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    let base_url = std::env::var("CCTUI_URL").unwrap_or_else(|_| "http://localhost:8700".into());
    let token = std::env::var("CCTUI_TOKEN").unwrap_or_default();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal, base_url, token).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

async fn init_sessions(server: &ServerClient, app: &mut App) {
    match server.list_sessions().await {
        Ok(resp) => {
            app.sessions = resp.sessions;
            app.update_aggregates();
        }
        Err(e) => tracing::warn!("Failed to fetch initial sessions: {e}"),
    }
}

async fn connect_ws_or_dummy(
    server: &ServerClient,
) -> (mpsc::Sender<TuiCommand>, mpsc::Receiver<ServerEvent>) {
    match server.connect_ws().await {
        Ok(pair) => pair,
        Err(e) => {
            tracing::warn!("Failed to connect WebSocket: {e}");
            let (tx, _) = mpsc::channel::<TuiCommand>(1);
            let (_, rx) = mpsc::channel::<ServerEvent>(1);
            (tx, rx)
        }
    }
}

async fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    base_url: String,
    token: String,
) -> Result<()> {
    let server = ServerClient::new(&base_url, &token);
    let mut app = App::new();

    init_sessions(&server, &mut app).await;
    let (cmd_tx, mut event_rx) = connect_ws_or_dummy(&server).await;
    let mut ws_open = true;
    let mut refresh_interval = time::interval(Duration::from_secs(5));
    // Skip the first immediate tick so it doesn't starve input on startup
    refresh_interval.tick().await;

    loop {
        terminal.draw(|f| render(f, &app))?;

        // biased ensures key input is checked first every iteration
        tokio::select! {
            biased;

            key_result = tokio::task::spawn_blocking(poll_key) => {
                let maybe_key = key_result.context("input task panicked")??;
                if let Some(code) = maybe_key {
                    handle_key(&mut app, code, &cmd_tx, &server).await;
                }
            }
            maybe_event = event_rx.recv(), if ws_open => {
                match maybe_event {
                    Some(event) => handle_server_event(&mut app, event),
                    None => ws_open = false, // channel closed, stop polling it
                }
            }
            _ = refresh_interval.tick() => {
                refresh_sessions(&server, &mut app).await;
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

async fn refresh_sessions(server: &ServerClient, app: &mut App) {
    match server.list_sessions().await {
        Ok(resp) => {
            app.sessions = resp.sessions;
            app.update_aggregates();
        }
        Err(e) => tracing::warn!("Refresh failed: {e}"),
    }
}

/// Poll for a single key event with a short timeout. Returns None on timeout.
fn poll_key() -> Result<Option<KeyCode>> {
    if !event::poll(Duration::from_millis(50))? {
        return Ok(None);
    }
    if let Event::Key(key) = event::read()?
        && key.kind == KeyEventKind::Press
    {
        return Ok(Some(key.code));
    }
    Ok(None)
}

async fn handle_key(
    app: &mut App,
    code: KeyCode,
    cmd_tx: &mpsc::Sender<TuiCommand>,
    server: &ServerClient,
) {
    // Message input mode captures characters
    if app.message_input.is_some() {
        handle_key_input_mode(app, code, cmd_tx).await;
        return;
    }

    handle_key_normal_mode(app, code, cmd_tx, server).await;
}

async fn handle_key_input_mode(app: &mut App, code: KeyCode, cmd_tx: &mpsc::Sender<TuiCommand>) {
    match code {
        KeyCode::Esc => {
            app.message_input = None;
        }
        KeyCode::Enter => {
            let content = app.message_input.take().unwrap_or_default();
            if !content.is_empty() {
                let session_id: Option<Uuid> = app.selected_session().map(|s| s.id);
                if let Some(id) = session_id {
                    let _ = cmd_tx.send(TuiCommand::Message { session_id: id, content }).await;
                }
            }
        }
        KeyCode::Backspace => {
            if let Some(ref mut input) = app.message_input {
                input.pop();
            }
        }
        KeyCode::Char(c) => {
            if let Some(ref mut input) = app.message_input {
                input.push(c);
            }
        }
        _ => {}
    }
}

async fn handle_key_normal_mode(
    app: &mut App,
    code: KeyCode,
    cmd_tx: &mpsc::Sender<TuiCommand>,
    server: &ServerClient,
) {
    match code {
        KeyCode::Char('q') => {
            app.should_quit = true;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.select_next();
            select_session(app, cmd_tx, server).await;
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.select_prev();
            select_session(app, cmd_tx, server).await;
        }
        KeyCode::Tab => {
            app.active_pane = match app.active_pane {
                Pane::Tree => Pane::Detail,
                Pane::Detail => Pane::Tree,
            };
        }
        KeyCode::Char('l') => {
            app.detail_mode = DetailMode::Log;
        }
        KeyCode::Char('c') => {
            app.detail_mode = DetailMode::Conversation;
        }
        KeyCode::Char('m') => {
            app.message_input = Some(String::new());
        }
        KeyCode::Char('?') => {
            app.view = View::Help;
        }
        KeyCode::Enter => {
            app.view = View::Conversation;
            select_session(app, cmd_tx, server).await;
        }
        KeyCode::Esc => {
            app.view = View::Sessions;
        }
        _ => {}
    }
}

async fn select_session(app: &mut App, cmd_tx: &mpsc::Sender<TuiCommand>, server: &ServerClient) {
    let session_id: Option<Uuid> = app.selected_session().map(|s| s.id);
    let Some(id) = session_id else { return };

    // Fetch conversation history from server if we don't have it yet
    if let std::collections::hash_map::Entry::Vacant(entry) = app.stream_buffer.entry(id)
        && let Ok(events) = server.get_conversation(id).await
    {
        let lines: Vec<String> = events
            .iter()
            .filter_map(|v| serde_json::from_value::<cctui_proto::ws::AgentEvent>(v.clone()).ok())
            .map(|e| views::sessions::agent_event_to_string(&e))
            .collect();
        if !lines.is_empty() {
            entry.insert(lines);
        }
    }

    // Subscribe to live stream for new events
    let _ = cmd_tx.send(TuiCommand::Subscribe { session_id: id }).await;
}

fn handle_server_event(app: &mut App, event: ServerEvent) {
    match event {
        ServerEvent::Stream { session_id, data } => {
            let line = views::sessions::agent_event_to_string(&data);
            app.stream_buffer.entry(session_id).or_default().push(line);
        }
        ServerEvent::Status { session_id, status } => {
            if let Some(session) = app.sessions.iter_mut().find(|s| s.id == session_id) {
                session.status = status;
                app.update_aggregates();
            }
        }
        ServerEvent::SessionRegistered { session } => {
            register_session(app, session);
        }
        ServerEvent::SessionDeregistered { session_id } => {
            deregister_session(app, session_id);
        }
    }
}

fn register_session(app: &mut App, session: cctui_proto::models::Session) {
    use cctui_proto::api::SessionListItem;
    if !app.sessions.iter().any(|s| s.id == session.id) {
        app.sessions.push(SessionListItem {
            id: session.id,
            parent_id: session.parent_id,
            machine_id: session.machine_id,
            working_dir: session.working_dir,
            status: session.status,
            uptime_secs: 0,
            token_usage: cctui_proto::models::TokenUsage::default(),
            metadata: session.metadata,
        });
        app.update_aggregates();
    }
}

fn deregister_session(app: &mut App, session_id: Uuid) {
    app.sessions.retain(|s| s.id != session_id);
    app.stream_buffer.remove(&session_id);
    let len = app.flattened_sessions().len();
    if len > 0 && app.selected_index >= len {
        app.selected_index = len - 1;
    }
    app.update_aggregates();
}

fn render(frame: &mut Frame, app: &App) {
    match app.view {
        View::Sessions | View::Conversation => views::sessions::draw(frame, app),
        View::Help => {
            views::sessions::draw(frame, app);
            views::help::draw(frame);
        }
    }
}
