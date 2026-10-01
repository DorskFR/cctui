use super::action::{Action, Effect, HeartbeatUsage};
use super::conversation::{self, ConversationAction};
use super::state::{App, View};
use super::toast::Level;
use super::{row_actions, send, terminal};

/// The single place app state changes. Pure: no clock, no IO — anything that
/// needs either comes back as an [`Effect`].
pub fn reduce(app: &mut App, action: Action) -> Vec<Effect> {
    let mut effects = reduce_action(app, action);
    effects.extend(super::drafts::sync_composer(app));
    // Every action can move a session in or out of waiting, so the diff runs
    // once per pass rather than being hooked onto the handful that obviously do.
    super::attention::reconcile(app);
    effects
}

#[allow(clippy::too_many_lines)]
fn reduce_action(app: &mut App, action: Action) -> Vec<Effect> {
    match action {
        Action::Auth(auth) => super::identity::reduce_auth(app, auth),
        Action::RowAction(action) => row_actions::reduce_row_actions(app, action),
        Action::Attach(action) => super::attach::reduce_attach(app, action),
        Action::Labels(action) => super::labels::reduce_labels(app, action),
        Action::Terminal(action) => terminal::reduce_terminal(app, action),
        Action::PendingChord(chord) => {
            app.pending_chord = Some(chord);
            Vec::new()
        }
        Action::PasteText(text) => {
            app.input_active = true;
            app.message_input.insert_str(&text);
            super::drafts::on_input(app)
        }
        Action::Attention(attention) => super::attention::reduce_attention(app, attention),
        Action::FileView(action) => super::fileview::reduce_fileview(app, action),
        Action::Drafts(drafts) => super::drafts::reduce_drafts(app, drafts),
        Action::Pins(pins) => super::pins::reduce_pins(app, pins),
        Action::Bookmarks(action) => super::bookmarks::reduce_bookmarks(app, action),
        Action::SpawnDrafts(action) => super::spawn_drafts::reduce_drafts(app, action),
        Action::Macros(action) => super::macros::reduce_macros(app, action),
        Action::AcceptMention(key) => super::mentions::accept(app).unwrap_or_else(|| {
            app.message_input.input(key);
            super::drafts::on_input(app)
        }),
        Action::Send(action) => send::reduce_send(app, action),
        Action::SessionLive(action) => super::session_live::reduce_session_live(app, action),
        // One clock for the whole app: delivery deadlines move, and the session
        // list polls only when its own period has elapsed.
        Action::Tick => {
            row_actions::prune(app);
            let mut effects = send::tick(app);
            effects.extend(super::session_live::poll_if_due(app));
            effects.extend(super::list_search::on_tick(app));
            effects.extend(super::unread::tick(app));
            effects
        }

        Action::Quit => {
            app.should_quit = true;
            Vec::new()
        }

        Action::SelectNext => {
            app.select_next();
            Vec::new()
        }
        Action::SelectPrev => {
            app.select_prev();
            Vec::new()
        }
        Action::SelectFirst => {
            app.select_first();
            Vec::new()
        }
        Action::SelectLast => {
            app.select_last();
            Vec::new()
        }
        Action::SelectIndex(index) => {
            if index < app.flattened_sessions().len() {
                app.selected_index = index;
                app.follow_tail = true;
            }
            Vec::new()
        }

        Action::ToggleTimestamps => {
            app.show_timestamps = !app.show_timestamps;
            Vec::new()
        }

        Action::OpenHelp => {
            app.help_scroll = 0;
            app.router.push(View::Help);
            Vec::new()
        }
        // Help dismisses to the session list, never to the view it was opened
        // over, so it collapses the stack exactly as leaving a conversation does.
        Action::CloseHelp => {
            let mut effects = terminal::close(app);
            app.router.reset(View::SessionList);
            effects.extend(conversation::leave(app));
            effects
        }
        // Line-select is a mode inside the conversation: the same key leaves it
        // first and only closes the conversation on a second press.
        Action::LeaveConversation => {
            if conversation::line_select_active(app) {
                return conversation::reduce(app, ConversationAction::ToggleLineCursor);
            }
            let mut effects = terminal::close(app);
            app.router.reset(View::SessionList);
            effects.extend(conversation::leave(app));
            effects
        }
        Action::OpenSelectedConversation => {
            let Some(session_id) = app.selected_session_id() else { return Vec::new() };
            conversation::open(app, session_id)
        }

        // Line-wise keys move the focused line instead of the viewport while
        // line-select is on; paging keys keep scrolling either way.
        Action::Scroll { lines, .. } if app.view() == View::Help => {
            app.help_scroll = app.help_scroll.saturating_add_signed(lines as isize);
            Vec::new()
        }
        Action::Scroll { lines, .. }
            if conversation::line_select_active(app) && lines.abs() == 1 =>
        {
            conversation::reduce(app, ConversationAction::MoveCursor { delta: lines })
        }
        // The pager borrows the conversation's scroll keys, so the same actions
        // have to land on whichever is on top.
        Action::Scroll { lines, .. } if app.view() == View::FileViewer => {
            super::fileview::reduce_fileview(app, super::fileview::FileViewAction::Scroll(lines))
        }
        Action::ScrollToTop if app.view() == View::FileViewer => {
            if let Some(view) = app.file_view.as_mut() {
                view.scroll = 0;
            }
            Vec::new()
        }
        Action::Scroll { lines, release_follow } => {
            snap_scroll_if_following(app);
            app.scroll_offset = if lines < 0 {
                app.scroll_offset.saturating_sub(lines.unsigned_abs() as usize)
            } else {
                app.scroll_offset.saturating_add(lines as usize)
            };
            if release_follow {
                app.follow_tail = false;
            }
            conversation::load_older(app)
        }
        Action::ScrollToTop => {
            app.scroll_offset = 0;
            app.follow_tail = false;
            conversation::load_older(app)
        }
        Action::ScrollToBottom => {
            app.follow_tail = true;
            Vec::new()
        }

        Action::ActivateInputWith(key) => {
            if app.selected_session_ended() {
                app.toast(Level::Info, "this session has ended");
                return Vec::new();
            }
            app.input_active = true;
            app.message_input.input(key);
            super::drafts::on_input(app)
        }
        // The first escape dismisses an open completion, the next one the
        // composer itself.
        Action::CancelInput => {
            if !super::mentions::close(app) {
                app.input_active = false;
            }
            Vec::new()
        }
        Action::InputKey(key) => {
            app.message_input.input(key);
            super::drafts::on_input(app)
        }
        Action::InputNewline => {
            app.message_input.insert_newline();
            super::drafts::on_input(app)
        }
        // Enter takes an open completion instead of sending: the message is
        // not finished if the user is still naming a session.
        Action::SubmitInput => {
            if let Some(effects) = super::mentions::accept(app) {
                return effects;
            }
            let content = app.message_input.lines().join("\n");
            let target = app.selected_session_id();
            app.reset_input();
            app.input_active = false;
            match target {
                // Staged files have to reach the working dir before the prompt
                // that references them does, so the upload goes first and its
                // reply carries the send.
                Some(session_id) if !content.trim().is_empty() => {
                    super::attach::upload_effect(app, &session_id, &content).map_or_else(
                        || send::submit(app, session_id.clone(), content.clone(), None),
                        |effect| vec![effect],
                    )
                }
                // Nothing to send, but the emptied composer is still a draft
                // change the store has to hear about.
                Some(session_id) => super::drafts::on_send(app, &session_id, &content),
                None => Vec::new(),
            }
        }

        Action::Controls(action) => super::controls::reduce_controls(app, action),
        Action::Sidebar(action) => super::sidebar::reduce_sidebar(app, action),
        Action::Unread(action) => super::unread::reduce_unread(app, action),
        Action::ToggleAutoApproveSelected => app
            .selected_session()
            .map(|s| (s.id.clone(), !s.auto_approve))
            .map(|(session_id, enabled)| vec![Effect::SetAutoApprove { session_id, enabled }])
            .unwrap_or_default(),
        Action::AutoApproveSet { session_id, enabled } => {
            if let Some(s) = app.sessions.iter_mut().find(|s| s.id == session_id) {
                s.auto_approve = enabled;
            }
            Vec::new()
        }

        Action::RefreshSessions => super::session_live::refresh(app),
        Action::SessionsLoaded(sessions) => {
            app.sessions = sessions;
            app.update_aggregates();
            row_actions::prune(app);
            super::controls::take_pending_jump(app)
        }
        Action::Conversation(action) => conversation::reduce(app, action),
        Action::ListShape(action) => super::list_shape_reduce::reduce(app, action),
        Action::ListSearch(action) => super::list_search::reduce(app, action),
        // Decision 7: one key, scoped to whatever view is in front.
        Action::SearchCurrentView => match app.view() {
            View::SessionList => {
                super::list_search::reduce(app, super::list_search::ListSearchAction::Open)
            }
            _ => super::cmdline::reduce(
                app,
                super::cmdline::CmdAction::Open(super::cmdline::Mode::Search),
            ),
        },
        Action::SearchHitNext => match app.view() {
            View::SessionList => {
                super::list_search::reduce(app, super::list_search::ListSearchAction::Next)
            }
            _ => super::cmdline::reduce(app, super::cmdline::CmdAction::NextHit),
        },
        Action::SearchHitPrev => match app.view() {
            View::SessionList => {
                super::list_search::reduce(app, super::list_search::ListSearchAction::Prev)
            }
            _ => super::cmdline::reduce(app, super::cmdline::CmdAction::PrevHit),
        },
        Action::CmdLine(action) => super::cmdline::reduce(app, action),
        Action::Copy(what) => copy(app, what),
        Action::Prompt(action) => super::prompt::reduce_prompt(app, action),
        Action::Diagnose(action) => super::diagnose::reduce_diagnose(app, action),
        Action::Slice(action) => super::slice::reduce_slice(app, action),
        Action::DeepLink(action) => super::deeplink::reduce_deeplink(app, action),

        Action::StreamLine { session_id, seq, line, usage } => {
            if let Some(usage) = usage {
                apply_heartbeat_usage(app, &session_id, &usage);
            }
            if let Some(line) = line {
                super::unread::stream(app, &session_id, line.kind);
                conversation::stream(app, &session_id, seq, *line);
            }
            Vec::new()
        }
        Action::SessionStatusChanged { session_id, status } => {
            if let Some(session) = app.sessions.iter_mut().find(|s| s.id == session_id) {
                session.status = status;
                app.update_aggregates();
            }
            Vec::new()
        }
        Action::SessionRegistered(session) => {
            register_session(app, *session);
            // A fork normally registers over the socket before the refresh
            // that was asked for on its behalf lands.
            super::controls::take_pending_jump(app)
        }
        Action::SessionDeregistered(session_id) => {
            deregister_session(app, &session_id);
            Vec::new()
        }

        Action::Reconnected => {
            app.toast(Level::Info, "reconnected");
            terminal::reconnect(app);
            let mut effects = conversation::reconnect(app);
            effects.extend(send::redispatch_parked(app));
            effects.extend(super::session_live::refresh(app));
            effects.push(Effect::FetchPendingPermissions);
            effects
        }
        Action::Toast(level, text) => {
            app.toast(level, text);
            Vec::new()
        }
        Action::UndecodableWsMessage(reason) => {
            app.status.undecodable_ws_messages += 1;
            tracing::warn!(%reason, "dropping an undecodable websocket message");
            app.toast(Level::Warn, "dropped an undecodable server message");
            Vec::new()
        }
        Action::UndecodableAgentEvents(count) => {
            app.status.undecodable_agent_events += count as u64;
            tracing::warn!(count, "dropping undecodable agent events");
            app.toast(Level::Warn, format!("dropped {count} unreadable conversation events"));
            Vec::new()
        }
    }
}

/// Resolves a copy key against the focused line. Without one there is nothing
/// to copy, so it says so rather than copying something arbitrary.
fn copy(app: &mut App, what: super::action::CopyWhat) -> Vec<Effect> {
    use super::action::CopyWhat;

    if what == CopyWhat::SessionLink {
        let Some(session_id) = app.selected_session_id() else { return Vec::new() };
        let text = super::copy::session_link(&app.server_url, &session_id);
        return vec![Effect::Copy { text, label: "session link" }];
    }
    let Some(line) = app.focused_line() else {
        app.toast(Level::Info, "press v to pick a line first");
        return Vec::new();
    };
    match what {
        CopyWhat::Line => {
            vec![Effect::Copy { text: super::copy::line_markdown(line), label: "line" }]
        }
        CopyWhat::CodeBlock => {
            let Some(text) = super::copy::code_block(line) else {
                app.toast(Level::Info, "no code block on this line");
                return Vec::new();
            };
            vec![Effect::Copy { text, label: "code block" }]
        }
        CopyWhat::SessionLink => Vec::new(),
    }
}

/// When `follow_tail` is active, resolve `scroll_offset` to the actual bottom
/// position so that relative scroll operations work immediately without a dead zone.
const fn snap_scroll_if_following(app: &mut App) {
    if app.follow_tail {
        app.scroll_offset = app.total_display_lines.saturating_sub(app.viewport_height);
    }
}

fn apply_heartbeat_usage(app: &mut App, session_id: &str, usage: &HeartbeatUsage) {
    if let Some(session) = app.sessions.iter_mut().find(|s| s.id == session_id) {
        session.token_usage.tokens_in = usage.tokens_in;
        session.token_usage.tokens_out = usage.tokens_out;
        session.token_usage.cost_usd = usage.cost_usd;
    }
}

fn register_session(app: &mut App, session: cctui_proto::models::Session) {
    if app.sessions.iter().any(|s| s.id == session.id) {
        return;
    }
    app.sessions.push(cctui_proto::api::SessionListItem {
        id: session.id,
        parent_id: session.parent_id,
        machine_id: session.machine_id,
        working_dir: session.working_dir,
        status: session.status,
        liveness: cctui_proto::models::Liveness::Active,
        attention: None,
        // Classifier signals arrive on the next REST refresh; Working until then.
        bucket: cctui_proto::classifier::Bucket::Working,
        token_usage: cctui_proto::models::TokenUsage::default(),
        metadata: session.metadata,
        adapter_id: session.adapter_id,
        machine_name: None,
        machine_hue: None,
        machine_kind: None,
        account_name: None,
        unread_count: 0,
        activity_detail: None,
        last_tool_at: None,
        last_tool_name: None,
        tool_use_count: 0,
        todos: Vec::new(),
        user_actions: None,
        has_token_credentials: false,
        account_traffic_observed: false,
        last_message_text: None,
        last_message_at: None,
        registered_at: Some(session.registered_at),
        name: None,
        model: None,
        effort: None,
        permission_mode: None,
        auto_approve: false,
        match_snippet: None,
        match_seq: None,
        last_activity_at: None,
        cache_cold: false,
        estimated_burst_tokens: None,
        hibernated: false,
        pinned: false,
        labels: Vec::new(),
        room_id: None,
        room_name: None,
        last_heartbeat: None,
        pr_links: Vec::new(),
        end_reason: None,
        end_detail: None,
        ended_at: None,
        auto_archive_at: None,
        archived_by: None,
        keepalive: None,
        last_keepalive_at: None,
        launch_at: None,
        launch_error: None,
    });
    app.update_aggregates();
}

fn deregister_session(app: &mut App, session_id: &str) {
    app.sessions.retain(|s| s.id != session_id);
    app.conversations.remove(session_id);
    app.permissions.drop_session(session_id);
    app.drafts.forget(session_id);
    if app.subscribed.as_deref() == Some(session_id) {
        app.subscribed = None;
    }
    let len = app.flattened_sessions().len();
    if len > 0 && app.selected_index >= len {
        app.selected_index = len - 1;
    }
    app.update_aggregates();
}

#[cfg(test)]
mod tests {
    use super::{Action, App, Effect, Level, View, reduce};
    use crate::app::state::LineKind;
    use crate::testsupport::session;

    fn conversation_len(app: &App, session_id: &str) -> usize {
        app.conversation(session_id).map_or(0, crate::app::ConversationStore::len)
    }

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![
            session("s-a", "alpha", "active", "working"),
            session("s-b", "beta", "active", "working"),
        ];
        app.update_aggregates();
        app
    }

    fn line(text: &str) -> Box<crate::app::state::ConversationLine> {
        Box::new(crate::app::state::ConversationLine::new(LineKind::Assistant, text, 0))
    }

    #[test]
    fn quit_sets_the_flag_and_needs_no_effect() {
        let mut app = app();
        assert!(reduce(&mut app, Action::Quit).is_empty());
        assert!(app.should_quit);
    }

    #[test]
    fn selection_is_clamped_at_both_ends() {
        let mut app = app();
        reduce(&mut app, Action::SelectPrev);
        assert_eq!(app.selected_index, 0);
        reduce(&mut app, Action::SelectLast);
        assert_eq!(app.selected_index, 1);
        reduce(&mut app, Action::SelectNext);
        assert_eq!(app.selected_index, 1);
        reduce(&mut app, Action::SelectFirst);
        assert_eq!(app.selected_index, 0);
    }

    #[test]
    fn select_index_out_of_range_is_ignored() {
        let mut app = app();
        reduce(&mut app, Action::SelectIndex(9));
        assert_eq!(app.selected_index, 0);
        reduce(&mut app, Action::SelectIndex(1));
        assert_eq!(app.selected_index, 1);
        assert!(app.follow_tail);
    }

    #[test]
    fn opening_a_conversation_loads_subscribes_and_marks_seen() {
        let mut app = app();
        let effects = reduce(&mut app, Action::OpenSelectedConversation);
        assert_eq!(app.view(), View::Conversation);
        match effects.as_slice() {
            [
                Effect::LoadConversationPage { session_id, .. },
                Effect::Subscribe { .. },
                Effect::LoadPins { .. },
                Effect::MarkSeen { .. },
            ] => assert_eq!(session_id, "s-a"),
            _ => panic!("expected a load, a subscribe, a pin read and a seen mark"),
        }
    }

    #[test]
    fn leaving_a_conversation_returns_to_the_list_and_unsubscribes() {
        let mut app = app();
        reduce(&mut app, Action::OpenSelectedConversation);
        let effects = reduce(&mut app, Action::LeaveConversation);
        assert_eq!(app.view(), View::SessionList);
        assert_eq!(app.router.depth(), 1);
        assert!(matches!(effects.as_slice(), [Effect::Unsubscribe { .. }]));
    }

    #[test]
    fn help_scrolls_and_reopens_at_the_top() {
        let mut app = app();
        reduce(&mut app, Action::OpenHelp);
        reduce(&mut app, Action::Scroll { lines: 15, release_follow: false });
        assert_eq!(app.help_scroll, 15);
        reduce(&mut app, Action::Scroll { lines: -20, release_follow: true });
        assert_eq!(app.help_scroll, 0);
        reduce(&mut app, Action::Scroll { lines: 3, release_follow: false });
        reduce(&mut app, Action::CloseHelp);
        reduce(&mut app, Action::OpenHelp);
        assert_eq!(app.help_scroll, 0);
    }

    #[test]
    fn help_opens_as_an_overlay_and_dismisses_to_the_list() {
        let mut app = app();
        reduce(&mut app, Action::OpenSelectedConversation);
        reduce(&mut app, Action::OpenHelp);
        assert_eq!(app.view(), View::Help);
        assert_eq!(app.router.below(), Some(View::Conversation));
        reduce(&mut app, Action::CloseHelp);
        assert_eq!(app.view(), View::SessionList);
        assert_eq!(app.router.depth(), 1);
    }

    #[test]
    fn scrolling_up_detaches_from_the_tail_and_down_by_page_does_not() {
        let mut app = app();
        app.total_display_lines = 200;
        app.viewport_height = 20;

        reduce(&mut app, Action::Scroll { lines: -3, release_follow: true });
        assert_eq!(app.scroll_offset, 177);
        assert!(!app.follow_tail);

        app.follow_tail = true;
        reduce(&mut app, Action::Scroll { lines: 15, release_follow: false });
        assert_eq!(app.scroll_offset, 195);
        assert!(app.follow_tail);
    }

    #[test]
    fn scroll_offset_never_underflows() {
        let mut app = app();
        app.follow_tail = false;
        app.scroll_offset = 1;
        reduce(&mut app, Action::Scroll { lines: -50, release_follow: true });
        assert_eq!(app.scroll_offset, 0);
    }

    #[test]
    fn scroll_to_top_and_bottom_set_follow_tail() {
        let mut app = app();
        reduce(&mut app, Action::ScrollToTop);
        assert_eq!(app.scroll_offset, 0);
        assert!(!app.follow_tail);
        reduce(&mut app, Action::ScrollToBottom);
        assert!(app.follow_tail);
    }

    #[test]
    fn submitting_sends_the_composer_text_and_clears_it() {
        let mut app = app();
        app.input_active = true;
        app.message_input.insert_str("hello there");
        let effects = reduce(&mut app, Action::SubmitInput);
        let sent = effects
            .iter()
            .find(|e| matches!(e, Effect::SendMessage { .. }))
            .expect("expected a send effect");
        match sent {
            Effect::SendMessage { session_id, content, ask_picks: None, .. } => {
                assert_eq!(session_id, "s-a");
                assert_eq!(content, "hello there");
            }
            _ => unreachable!("filtered above"),
        }
        assert!(!app.input_active);
        assert_eq!(app.message_input.lines().join("\n"), "");
    }

    #[test]
    fn submitting_blank_text_sends_nothing() {
        let mut app = app();
        app.message_input.insert_str("   ");
        assert!(reduce(&mut app, Action::SubmitInput).is_empty());
        assert!(!app.input_active);
    }

    #[test]
    fn auto_approve_is_applied_only_once_the_server_confirms() {
        let mut app = app();
        let effects = reduce(&mut app, Action::ToggleAutoApproveSelected);
        match effects.as_slice() {
            [Effect::SetAutoApprove { session_id, enabled: true }] => {
                assert_eq!(session_id, "s-a");
            }
            _ => panic!("expected an auto-approve effect"),
        }
        assert!(!app.sessions[0].auto_approve);
        reduce(&mut app, Action::AutoApproveSet { session_id: "s-a".to_owned(), enabled: true });
        assert!(app.sessions[0].auto_approve);
    }

    #[test]
    fn an_ended_session_refuses_the_composer() {
        let mut app = app();
        app.sessions[0].end_reason = Some(cctui_proto::models::SessionEndReason::Completed);
        let key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('x'),
            crossterm::event::KeyModifiers::NONE,
        );
        assert!(reduce(&mut app, Action::ActivateInputWith(key)).is_empty());
        assert!(!app.input_active);
        assert!(app.toasts.latest().is_some());
    }

    #[test]
    fn a_re_delivered_stream_line_is_deduped_by_seq() {
        let mut app = app();
        for _ in 0..3 {
            reduce(
                &mut app,
                Action::StreamLine {
                    session_id: "s-a".to_owned(),
                    seq: Some(1),
                    line: Some(line("same")),
                    usage: None,
                },
            );
        }
        assert_eq!(conversation_len(&app, "s-a"), 1);
        reduce(
            &mut app,
            Action::StreamLine {
                session_id: "s-a".to_owned(),
                seq: Some(2),
                line: Some(line("different")),
                usage: None,
            },
        );
        assert_eq!(conversation_len(&app, "s-a"), 2);
    }

    #[test]
    fn a_heartbeat_updates_the_session_usage() {
        let mut app = app();
        reduce(
            &mut app,
            Action::StreamLine {
                session_id: "s-a".to_owned(),
                seq: Some(1),
                line: None,
                usage: Some(super::HeartbeatUsage { tokens_in: 7, tokens_out: 8, cost_usd: 9.5 }),
            },
        );
        assert_eq!(conversation_len(&app, "s-a"), 0, "a heartbeat adds no row");
        assert_eq!(app.sessions[0].token_usage.tokens_in, 7);
        assert_eq!(app.sessions[0].token_usage.tokens_out, 8);
        assert!((app.sessions[0].token_usage.cost_usd - 9.5).abs() < f64::EPSILON);
    }

    #[test]
    fn deregistering_clamps_the_selection_and_drops_the_buffer() {
        let mut app = app();
        reduce(&mut app, Action::SelectLast);
        reduce(
            &mut app,
            Action::StreamLine {
                session_id: "s-b".to_owned(),
                seq: Some(1),
                line: Some(line("bye")),
                usage: None,
            },
        );
        reduce(&mut app, Action::SessionDeregistered("s-b".to_owned()));
        assert_eq!(app.sessions.len(), 1);
        assert_eq!(app.selected_index, 0);
        assert!(!app.conversations.contains_key("s-b"));
    }

    /// A reconnect always resyncs the whole-state pieces — the session list and
    /// the server's pending permissions — and the transcript gap only when a
    /// conversation is open.
    #[test]
    fn reconnecting_resubscribes_and_refetches_only_from_the_conversation() {
        let mut app = app();
        assert!(matches!(
            reduce(&mut app, Action::Reconnected).as_slice(),
            [Effect::RefreshSessions, Effect::FetchPendingPermissions]
        ));

        reduce(&mut app, Action::OpenSelectedConversation);
        app.clock_ms += crate::app::session_live::REFRESH_DEBOUNCE_MS;
        assert!(matches!(
            reduce(&mut app, Action::Reconnected).as_slice(),
            [
                Effect::Subscribe { .. },
                Effect::LoadConversationPage { .. },
                Effect::RefreshSessions,
                Effect::FetchPendingPermissions
            ]
        ));
    }

    #[test]
    fn undecodable_messages_are_counted_and_surfaced() {
        let mut app = app();
        reduce(&mut app, Action::UndecodableWsMessage("unknown variant".to_owned()));
        reduce(&mut app, Action::UndecodableAgentEvents(4));
        assert_eq!(app.status.undecodable_ws_messages, 1);
        assert_eq!(app.status.undecodable_agent_events, 4);
        assert_eq!(app.status.total(), 5);
        assert!(!app.status.is_clean());
        assert!(app.toasts.latest().is_some());
    }

    #[test]
    fn toasts_carry_the_reducer_clock() {
        let mut app = app();
        app.clock_ms = 1_000;
        reduce(&mut app, Action::Toast(Level::Error, "boom".to_owned()));
        let toast = app.toasts.latest().expect("a toast");
        assert_eq!(toast.text, "boom");
        assert_eq!(toast.expires_ms, 1_000 + crate::app::toast::Toasts::TTL_MS);
    }
}
