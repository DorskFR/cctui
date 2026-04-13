#![allow(dead_code)]
#![feature(let_chains)]

mod app;
mod client;
mod theme;
mod ui;
mod views;
mod widgets;

use std::io;
use std::time::Duration;

use anyhow::{Context, Result};
use app::{App, ConversationLine, LineKind, PendingPermission, View};
use cctui_proto::ws::{AgentEvent, ServerEvent, TuiCommand};
use client::ServerClient;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::{Frame, Terminal};
use tokio::sync::mpsc;
use tokio::time;

///// Input event from the terminal: either a key press or mouse scroll.
#[derive(Debug, Clone)]
enum InputEvent {
    Key(KeyEvent),
    ScrollUp,
    ScrollDown,
}

#[tokio::main]
async fn main() -> Result<()> {
    let base_url = std::env::var("CCTUI_URL").unwrap_or_else(|_| "http://localhost:8700".into());
    let token = std::env::var("CCTUI_TOKEN").unwrap_or_default();

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
        // Update scroll metrics before drawing so scroll input works immediately.
        update_scroll_metrics(&mut app);
        terminal.draw(|f| render(f, &mut app))?;

        tokio::select! {
            biased;

            input_result = tokio::task::spawn_blocking(poll_input) => {
                let maybe_input = input_result.context("input task panicked")??;
                if let Some(input) = maybe_input {
                    handle_input(&mut app, input, &cmd_tx, &server).await;
                }
            }
            maybe_event = event_rx.recv(), if ws_open => {
                match maybe_event {
                    Some(event) => {
                        handle_server_event(&mut app, event);
                        // Drain any additional queued server events before redrawing
                        while let Ok(ev) = event_rx.try_recv() {
                            handle_server_event(&mut app, ev);
                        }
                        // follow_tail is checked during render — no offset update needed here
                    }
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

/// Bootstrap `viewport_height` from terminal size if not yet set by a render pass.
fn update_scroll_metrics(app: &mut App) {
    if app.viewport_height == 0
        && let Ok((_, rows)) = crossterm::terminal::size()
    {
        app.viewport_height = (rows as usize).saturating_sub(5);
    }
}

fn poll_input() -> Result<Option<InputEvent>> {
    if !event::poll(Duration::from_millis(16))? {
        return Ok(None);
    }
    match event::read()? {
        Event::Key(key) if key.kind == KeyEventKind::Press => Ok(Some(InputEvent::Key(key))),
        Event::Mouse(mouse) => match mouse.kind {
            MouseEventKind::ScrollUp => Ok(Some(InputEvent::ScrollUp)),
            MouseEventKind::ScrollDown => Ok(Some(InputEvent::ScrollDown)),
            _ => Ok(None),
        },
        _ => Ok(None),
    }
}

// --- Input handling ---

async fn handle_input(
    app: &mut App,
    input: InputEvent,
    cmd_tx: &mpsc::Sender<TuiCommand>,
    server: &ServerClient,
) {
    match input {
        InputEvent::Key(key) => {
            if app.input_active {
                handle_input_mode(app, key, cmd_tx).await;
                return;
            }

            match app.view {
                View::SessionList => handle_session_list_keys(app, key.code, cmd_tx, server).await,
                View::Conversation => {
                    if !handle_conversation_keys(app, key) {
                        // Key not consumed by navigation — auto-activate input mode.
                        app.input_active = true;
                        handle_input_mode(app, key, cmd_tx).await;
                    }
                }
                View::Help => {
                    if matches!(key.code, KeyCode::Esc | KeyCode::Char('?' | 'q')) {
                        app.view = View::SessionList;
                    }
                }
                View::PermissionDialog => {
                    handle_permission_dialog_keys(app, key.code, cmd_tx).await;
                }
            }
        }
        InputEvent::ScrollUp => match app.view {
            View::Conversation => {
                snap_scroll_if_following(app);
                app.scroll_offset = app.scroll_offset.saturating_sub(3);
                app.follow_tail = false;
            }
            View::SessionList => app.select_prev(),
            View::Help | View::PermissionDialog => {}
        },
        InputEvent::ScrollDown => match app.view {
            View::Conversation => {
                snap_scroll_if_following(app);
                app.scroll_offset = app.scroll_offset.saturating_add(3);
            }
            View::SessionList => app.select_next(),
            View::Help | View::PermissionDialog => {}
        },
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
        KeyCode::Char('a') => app.show_all_sessions = !app.show_all_sessions,
        KeyCode::Char('?') => app.view = View::Help,
        KeyCode::Enter => {
            load_conversation(app, cmd_tx, server).await;
            app.follow_tail = true;
            app.view = View::Conversation;
        }
        _ => {}
    }
}

///// When `follow_tail` is active, resolve `scroll_offset` to the actual bottom
/// position so that relative scroll operations work immediately without a dead zone.
const fn snap_scroll_if_following(app: &mut App) {
    if app.follow_tail {
        app.scroll_offset = app.total_display_lines.saturating_sub(app.viewport_height);
    }
}

/// Returns `true` if the key was consumed as a navigation command, `false` if not.
/// Unhandled keys in conversation view trigger auto-activation of input mode.
fn handle_conversation_keys(app: &mut App, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.view = View::SessionList;
            true
        }
        KeyCode::Char('j') | KeyCode::Down => {
            snap_scroll_if_following(app);
            app.scroll_offset = app.scroll_offset.saturating_add(1);
            app.follow_tail = false;
            true
        }
        KeyCode::Char('k') | KeyCode::Up => {
            snap_scroll_if_following(app);
            app.scroll_offset = app.scroll_offset.saturating_sub(1);
            app.follow_tail = false;
            true
        }
        KeyCode::PageUp => {
            snap_scroll_if_following(app);
            app.scroll_offset = app.scroll_offset.saturating_sub(15);
            app.follow_tail = false;
            true
        }
        KeyCode::PageDown => {
            snap_scroll_if_following(app);
            app.scroll_offset = app.scroll_offset.saturating_add(15);
            true
        }
        KeyCode::Char('g') => {
            app.scroll_offset = 0;
            app.follow_tail = false;
            true
        }
        KeyCode::Char('G') => {
            app.follow_tail = true;
            true
        }
        KeyCode::Char('?') => {
            app.view = View::Help;
            true
        }
        KeyCode::Char('t') => {
            app.show_timestamps = !app.show_timestamps;
            true
        }
        KeyCode::Char(c @ '1'..='9') => {
            let idx = (c as usize) - ('1' as usize);
            let flat = app.flattened_sessions();
            if idx < flat.len() {
                app.selected_index = idx;
                app.follow_tail = true;
            }
            true
        }
        _ => false,
    }
}

async fn handle_permission_dialog_keys(
    app: &mut App,
    code: KeyCode,
    cmd_tx: &mpsc::Sender<TuiCommand>,
) {
    let behavior = match code {
        KeyCode::Char('y') | KeyCode::Enter => "allow",
        KeyCode::Char('n') | KeyCode::Esc => "deny",
        _ => return,
    };

    if let Some(req) = app.permission_queue.pop_front() {
        let _ = cmd_tx
            .send(TuiCommand::PermissionResponse {
                session_id: req.session_id,
                request_id: req.request_id,
                behavior: behavior.to_string(),
            })
            .await;
    }

    // Advance to next queued request or restore previous view
    if app.permission_queue.is_empty() {
        app.view = app.pre_permission_view.clone();
    }
    // else: stay in PermissionDialog, front() now points to the next request
}

async fn handle_input_mode(app: &mut App, key: KeyEvent, cmd_tx: &mpsc::Sender<TuiCommand>) {
    match key.code {
        KeyCode::Esc => {
            app.input_active = false;
        }
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
            app.message_input.insert_newline();
        }
        KeyCode::Enter => {
            let content = app.message_input.lines().join("\n");
            if !content.trim().is_empty()
                && let Some(id) = app.selected_session().map(|s| s.id.clone())
            {
                let _ = cmd_tx.send(TuiCommand::Message { session_id: id, content }).await;
            }
            app.reset_input();
            app.input_active = false;
        }
        _ => {
            app.message_input.input(key);
        }
    }
}

// --- Data loading ---

async fn load_conversation(
    app: &mut App,
    cmd_tx: &mpsc::Sender<TuiCommand>,
    server: &ServerClient,
) {
    let Some(id) = app.selected_session().map(|s| s.id.clone()) else { return };

    if let std::collections::hash_map::Entry::Vacant(entry) = app.stream_buffer.entry(id.clone())
        && let Ok(events) = server.get_conversation(&id).await
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

    let _ = cmd_tx.send(TuiCommand::Subscribe { session_id: id.clone() }).await;
}

// --- Server events ---

fn handle_server_event(app: &mut App, event: ServerEvent) {
    match event {
        ServerEvent::PermissionRequest {
            session_id,
            request_id,
            tool_name,
            description,
            input_preview,
        } => {
            let req =
                PendingPermission { session_id, request_id, tool_name, description, input_preview };
            let was_empty = app.permission_queue.is_empty();
            app.permission_queue.push_back(req);
            if was_empty {
                app.pre_permission_view = app.view.clone();
                app.view = View::PermissionDialog;
            }
        }
        ServerEvent::Stream { session_id, data } => {
            if let AgentEvent::Heartbeat { tokens_in, tokens_out, cost_usd, .. } = &data
                && let Some(session) = app.sessions.iter_mut().find(|s| s.id == session_id)
            {
                session.token_usage.tokens_in = *tokens_in;
                session.token_usage.tokens_out = *tokens_out;
                session.token_usage.cost_usd = *cost_usd;
            }
            let line = agent_event_to_line(&data);
            // Dedup: skip if the last line has identical text and kind
            let buf = app.stream_buffer.entry(session_id).or_default();
            let is_dup =
                buf.last().is_some_and(|last| last.kind == line.kind && last.text == line.text);
            if !is_dup {
                buf.push(line);
            }
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

fn extract_tag_content(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    if let Some(start) = text.find(&open)
        && let Some(end) = text[start..].find(&close)
    {
        let content_start = start + open.len();
        return Some(text[content_start..content_start + end].to_string());
    }
    None
}

fn remove_tag_pair(text: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    if let Some(start) = text.find(&open)
        && let Some(end) = text[start..].find(&close)
    {
        let end_pos = start + end + close.len();
        let mut result = text[..start].to_string();
        result.push_str(&text[end_pos..]);
        return remove_tag_pair(&result, tag);
    }
    text.to_string()
}

fn strip_all_tags(text: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    for ch in text.chars() {
        if ch == '<' {
            in_tag = true;
        } else if ch == '>' {
            in_tag = false;
        } else if !in_tag {
            result.push(ch);
        }
    }
    result
}

fn strip_ansi_codes(text: &str) -> String {
    let mut result = String::new();
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            while let Some(&c) = chars.peek() {
                chars.next();
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else if ch == '[' {
            let mut temp = chars.clone();
            let is_ansi = temp.peek().is_some_and(|&c| c.is_ascii_digit() || c == ';');
            if is_ansi {
                while let Some(&c) = chars.peek() {
                    chars.next();
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                result.push(ch);
            }
        } else {
            result.push(ch);
        }
    }
    result
}

/// Strip XML tags, ANSI codes, and system noise from user message text.
/// Returns None if the result is empty or only whitespace.
fn clean_user_message(text: &str) -> Option<String> {
    let mut result = remove_tag_pair(text, "system-reminder");
    result = remove_tag_pair(&result, "local-command-caveat");

    let cmd_name = extract_tag_content(&result, "command-name");
    let cmd_args = extract_tag_content(&result, "command-args");
    let cmd_stdout = extract_tag_content(&result, "local-command-stdout");

    result = remove_tag_pair(&result, "command-name");
    result = remove_tag_pair(&result, "command-args");
    result = remove_tag_pair(&result, "local-command-stdout");
    result = strip_all_tags(&result);

    if let Some(ref name) = cmd_name {
        let mut cmd_line = format!("/{name}");
        if let Some(ref args) = cmd_args
            && !args.is_empty()
        {
            cmd_line.push(' ');
            cmd_line.push_str(args);
        }
        if let Some(ref stdout) = cmd_stdout {
            cmd_line.push_str(" → ");
            cmd_line.push_str(stdout);
        }
        result = if result.trim().is_empty() { cmd_line } else { format!("{cmd_line} {result}") };
    }

    result = strip_ansi_codes(&result);
    let trimmed = result.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
}

fn agent_event_to_line(event: &AgentEvent) -> ConversationLine {
    match event {
        AgentEvent::Text { content, ts } => {
            let (kind, text) = if content.starts_with("▷ User:") {
                let user_text = content.trim_start_matches("▷ User: ");
                clean_user_message(user_text).map_or_else(
                    || (LineKind::System, String::new()),
                    |cleaned| (LineKind::User, cleaned),
                )
            } else {
                (LineKind::Assistant, content.clone())
            };
            ConversationLine { timestamp: *ts, kind, text, tool: None, tool_input: None }
        }
        AgentEvent::ToolCall { tool, input, ts } => {
            let detail = views::sessions::format_tool_input(tool, input);
            // Keep raw input for Edit/Write so we can generate diffs during render
            let keep_input = matches!(tool.as_str(), "Edit" | "Write");
            ConversationLine {
                timestamp: *ts,
                kind: LineKind::ToolCall,
                text: format!("[{tool}] {detail}"),
                tool: Some(tool.clone()),
                tool_input: if keep_input { Some(input.clone()) } else { None },
            }
        }
        AgentEvent::ToolResult { tool, output_summary, ts } => ConversationLine {
            timestamp: *ts,
            kind: LineKind::ToolResult,
            text: format!("  → {output_summary}"),
            tool: Some(tool.clone()),
            tool_input: None,
        },
        AgentEvent::Heartbeat { ts, .. } | AgentEvent::TurnEnd { ts } => ConversationLine {
            timestamp: *ts,
            kind: LineKind::System,
            text: String::new(),
            tool: None,
            tool_input: None,
        },
        AgentEvent::Reply { content, ts } => ConversationLine {
            timestamp: *ts,
            kind: LineKind::Reply,
            text: content.clone(),
            tool: None,
            tool_input: None,
        },
    }
}

// --- Rendering ---

fn render(frame: &mut Frame, app: &mut App) {
    match app.view {
        View::SessionList => views::sessions::draw(frame, app),
        View::Conversation => views::conversation::draw(frame, app),
        View::Help => {
            // Show help on top of whatever view was active
            views::sessions::draw(frame, app);
            views::help::draw(frame);
        }
        View::PermissionDialog => {
            // Draw underlying view, then overlay the dialog
            match app.pre_permission_view {
                View::Conversation => views::conversation::draw(frame, app),
                _ => views::sessions::draw(frame, app),
            }
            if let Some(req) = app.permission_queue.front() {
                views::permission::draw(frame, req);
            }
        }
    }
}
