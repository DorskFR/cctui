use super::action::{Action, Effect, HeartbeatUsage};
use super::conversation::{self, ConversationAction};
use super::send;
use super::state::{App, View};
use super::toast::Level;

/// The single place app state changes. Pure: no clock, no IO — anything that
/// needs either comes back as an [`Effect`].
pub fn reduce(app: &mut App, action: Action) -> Vec<Effect> {
    let mut effects = reduce_action(app, action);
    effects.extend(super::drafts::sync_composer(app));
    effects
}

#[allow(clippy::too_many_lines)]
fn reduce_action(app: &mut App, action: Action) -> Vec<Effect> {
    match action {
        Action::Auth(auth) => super::identity::reduce_auth(app, auth),
        Action::Attention(attention) => super::attention::reduce_attention(app, attention),
        Action::Drafts(drafts) => super::drafts::reduce_drafts(app, drafts),
        Action::Send(action) => send::reduce_send(app, action),
        Action::Tick => send::tick(app),

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
            app.router.push(View::Help);
            Vec::new()
        }
        // Help dismisses to the session list, never to the view it was opened
        // over, so it collapses the stack exactly as leaving a conversation does.
        Action::CloseHelp => {
            app.router.reset(View::SessionList);
            conversation::leave(app)
        }
        // Line-select is a mode inside the conversation: the same key leaves it
        // first and only closes the conversation on a second press.
        Action::LeaveConversation => {
            if conversation::line_select_active(app) {
                return conversation::reduce(app, ConversationAction::ToggleLineCursor);
            }
            app.router.reset(View::SessionList);
            conversation::leave(app)
        }
        Action::OpenSelectedConversation => {
            let Some(session_id) = app.selected_session_id() else { return Vec::new() };
            conversation::open(app, session_id)
        }

        // Line-wise keys move the focused line instead of the viewport while
        // line-select is on; paging keys keep scrolling either way.
        Action::Scroll { lines, .. }
            if conversation::line_select_active(app) && lines.abs() == 1 =>
        {
            conversation::reduce(app, ConversationAction::MoveCursor { delta: lines })
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
        Action::CancelInput => {
            app.input_active = false;
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
        Action::SubmitInput => {
            let content = app.message_input.lines().join("\n");
            let target = app.selected_session_id();
            app.reset_input();
            app.input_active = false;
            match target {
                Some(session_id) if !content.trim().is_empty() => {
                    send::submit(app, session_id, content, None)
                }
                // Nothing to send, but the emptied composer is still a draft
                // change the store has to hear about.
                Some(session_id) => super::drafts::on_send(app, &session_id, &content),
                None => Vec::new(),
            }
        }

        Action::InterruptSelected => app
            .selected_session_id()
            .map(|session_id| vec![Effect::Interrupt { session_id }])
            .unwrap_or_default(),
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

        Action::RefreshSessions => vec![Effect::RefreshSessions],
        Action::SessionsLoaded(sessions) => {
            app.sessions = sessions;
            app.update_aggregates();
            Vec::new()
        }
        Action::Conversation(action) => conversation::reduce(app, action),
        Action::Prompt(action) => super::prompt::reduce_prompt(app, action),

        Action::StreamLine { session_id, seq, line, usage } => {
            if let Some(usage) = usage {
                apply_heartbeat_usage(app, &session_id, &usage);
            }
            if let Some(line) = line {
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
            Vec::new()
        }
        Action::SessionDeregistered(session_id) => {
            deregister_session(app, &session_id);
            Vec::new()
        }

        Action::Reconnected => {
            app.toast(Level::Info, "reconnected");
            let mut effects = conversation::reconnect(app);
            effects.extend(send::redispatch_parked(app));
            effects.push(Effect::RefreshSessions);
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
                Effect::MarkSeen { .. },
            ] => assert_eq!(session_id, "s-a"),
            _ => panic!("expected a load, a subscribe and a seen mark"),
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
