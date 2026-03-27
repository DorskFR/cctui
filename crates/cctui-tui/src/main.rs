#![allow(dead_code)]

mod app;
mod client;
mod theme;
mod views;
mod widgets;

use std::io;
use std::time::Duration;

use anyhow::{Context, Result};
use app::{App, ConversationLine, LineKind, View};
use cctui_proto::ws::{AgentEvent, ServerEvent, TuiCommand};
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
    refresh_interval.tick().await;

    loop {
        terminal.draw(|f| render(f, &app))?;

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
                    None => ws_open = false,
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

async fn init_sessions(server: &ServerClient, app: &mut App) {
    if let Ok(resp) = server.list_sessions().await {
        app.sessions = resp.sessions;
        app.update_aggregates();
    }
}

async fn connect_ws_or_dummy(
    server: &ServerClient,
) -> (mpsc::Sender<TuiCommand>, mpsc::Receiver<ServerEvent>) {
    (server.connect_ws().await).unwrap_or_else(|_| {
        let (tx, _) = mpsc::channel::<TuiCommand>(1);
        let (_, rx) = mpsc::channel::<ServerEvent>(1);
        (tx, rx)
    })
}

async fn refresh_sessions(server: &ServerClient, app: &mut App) {
    if let Ok(resp) = server.list_sessions().await {
        app.sessions = resp.sessions;
        app.update_aggregates();
    }
}

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

// --- Key handling ---

async fn handle_key(
    app: &mut App,
    code: KeyCode,
    cmd_tx: &mpsc::Sender<TuiCommand>,
    server: &ServerClient,
) {
    if app.input_active {
        handle_input_mode(app, code, cmd_tx).await;
        return;
    }

    match app.view {
        View::SessionList => handle_session_list_keys(app, code, cmd_tx, server).await,
        View::Conversation => handle_conversation_keys(app, code),
        View::Help => {
            if matches!(code, KeyCode::Esc | KeyCode::Char('?' | 'q')) {
                app.view = View::SessionList;
            }
        }
    }
}

async fn handle_session_list_keys(
    app: &mut App,
    code: KeyCode,
    cmd_tx: &mpsc::Sender<TuiCommand>,
    server: &ServerClient,
) {
    match code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('j') | KeyCode::Down => app.select_next(),
        KeyCode::Char('k') | KeyCode::Up => app.select_prev(),
        KeyCode::Char('g') => app.select_first(),
        KeyCode::Char('G') => app.select_last(),
        KeyCode::Char('?') => app.view = View::Help,
        KeyCode::Enter => {
            load_conversation(app, cmd_tx, server).await;
            app.scroll_offset = usize::MAX; // auto-scroll to bottom
            app.view = View::Conversation;
        }
        _ => {}
    }
}

#[allow(clippy::missing_const_for_fn)]
fn handle_conversation_keys(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc | KeyCode::Char('q') => app.view = View::SessionList,
        KeyCode::Char('j') | KeyCode::Down => {
            app.scroll_offset = app.scroll_offset.saturating_add(1);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.scroll_offset = app.scroll_offset.saturating_sub(1);
        }
        KeyCode::Char('g') => app.scroll_offset = 0,
        KeyCode::Char('G') => app.scroll_offset = usize::MAX,
        KeyCode::Char('i') => app.input_active = true,
        KeyCode::Char('?') => app.view = View::Help,
        _ => {}
    }
}

async fn handle_input_mode(app: &mut App, code: KeyCode, cmd_tx: &mpsc::Sender<TuiCommand>) {
    match code {
        KeyCode::Esc => {
            app.input_active = false;
        }
        KeyCode::Enter => {
            let content = std::mem::take(&mut app.message_input);
            if !content.is_empty()
                && let Some(id) = app.selected_session().map(|s| s.id)
            {
                let _ = cmd_tx.send(TuiCommand::Message { session_id: id, content }).await;
            }
            app.input_active = false;
        }
        KeyCode::Backspace => {
            app.message_input.pop();
        }
        KeyCode::Char(c) => {
            app.message_input.push(c);
        }
        _ => {}
    }
}

// --- Data loading ---

async fn load_conversation(
    app: &mut App,
    cmd_tx: &mpsc::Sender<TuiCommand>,
    server: &ServerClient,
) {
    let Some(id) = app.selected_session().map(|s| s.id) else { return };

    if let std::collections::hash_map::Entry::Vacant(entry) = app.stream_buffer.entry(id)
        && let Ok(events) = server.get_conversation(id).await
    {
        let lines: Vec<ConversationLine> = events
            .iter()
            .filter_map(|v| serde_json::from_value::<AgentEvent>(v.clone()).ok())
            .map(|e| agent_event_to_line(&e))
            .collect();
        if !lines.is_empty() {
            entry.insert(lines);
        }
    }

    let _ = cmd_tx.send(TuiCommand::Subscribe { session_id: id }).await;
}

// --- Server events ---

fn handle_server_event(app: &mut App, event: ServerEvent) {
    match event {
        ServerEvent::Stream { session_id, data } => {
            let line = agent_event_to_line(&data);
            app.stream_buffer.entry(session_id).or_default().push(line);
        }
        ServerEvent::Status { session_id, status } => {
            if let Some(session) = app.sessions.iter_mut().find(|s| s.id == session_id) {
                session.status = status;
                app.update_aggregates();
            }
        }
        ServerEvent::SessionRegistered { session } => {
            if !app.sessions.iter().any(|s| s.id == session.id) {
                app.sessions.push(cctui_proto::api::SessionListItem {
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
        ServerEvent::SessionDeregistered { session_id } => {
            app.sessions.retain(|s| s.id != session_id);
            app.stream_buffer.remove(&session_id);
            let len = app.flattened_sessions().len();
            if len > 0 && app.selected_index >= len {
                app.selected_index = len - 1;
            }
            app.update_aggregates();
        }
    }
}

fn agent_event_to_line(event: &AgentEvent) -> ConversationLine {
    match event {
        AgentEvent::Text { content, ts } => {
            let (kind, text) = if content.starts_with("▷ User:") {
                (LineKind::User, content.trim_start_matches("▷ User: ").to_string())
            } else {
                (LineKind::Assistant, content.clone())
            };
            ConversationLine { timestamp: *ts, kind, text }
        }
        AgentEvent::ToolCall { tool, input, ts } => {
            let detail = views::sessions::format_tool_input(tool, input);
            ConversationLine {
                timestamp: *ts,
                kind: LineKind::ToolCall,
                text: format!("[{tool}] {detail}"),
            }
        }
        AgentEvent::ToolResult { output_summary, ts, .. } => ConversationLine {
            timestamp: *ts,
            kind: LineKind::ToolResult,
            text: format!("  → {output_summary}"),
        },
        AgentEvent::Heartbeat { ts, .. } => {
            ConversationLine { timestamp: *ts, kind: LineKind::System, text: String::new() }
        }
    }
}

// --- Rendering ---

fn render(frame: &mut Frame, app: &App) {
    match app.view {
        View::SessionList => views::sessions::draw(frame, app),
        View::Conversation => views::conversation::draw(frame, app),
        View::Help => {
            // Show help on top of whatever view was active
            views::sessions::draw(frame, app);
            views::help::draw(frame);
        }
    }
}
