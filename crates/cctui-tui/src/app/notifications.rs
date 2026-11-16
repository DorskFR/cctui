use cctui_client::{Client, ClientError};
use cctui_proto::api::notifications::{
    Notification, NotificationActRequest, NotificationList, NotificationReadAll,
    NotificationTransition,
};

use super::action::{Action, Effect};
use super::state::{App, View};
use super::toast::Level;

const PAGE: i64 = 50;

#[must_use]
pub fn is_unread(n: &Notification) -> bool {
    matches!(n.state.as_str(), "pending" | "delivered")
}

#[must_use]
pub fn is_terminal(n: &Notification) -> bool {
    matches!(n.state.as_str(), "resolved" | "expired")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticeAction {
    pub id: String,
    pub label: String,
}

#[must_use]
pub fn actions(n: &Notification) -> Vec<NoticeAction> {
    if n.state == "acted" || is_terminal(n) {
        return Vec::new();
    }
    n.actions
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| {
            let id = a.get("id")?.as_str()?.to_owned();
            let label = a.get("label").and_then(|l| l.as_str()).unwrap_or(&id).to_owned();
            Some(NoticeAction { id, label })
        })
        .collect()
}

#[must_use]
pub fn outcome(n: &Notification) -> Option<String> {
    let decision = n.decision.as_deref()?;
    Some(
        n.acted_channel
            .as_deref()
            .map_or_else(|| decision.to_owned(), |channel| format!("{decision} via {channel}")),
    )
}

#[derive(Debug, Default)]
pub struct Panel {
    pub selected: usize,
    pub loading: bool,
    pub busy: bool,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub struct NotificationState {
    pub unread: i64,
    pub items: Vec<Notification>,
    pub panel: Option<Panel>,
}

impl NotificationState {
    pub fn upsert(&mut self, n: Notification) {
        let focused = self.focused().map(|f| f.id);
        self.items.retain(|item| item.id != n.id);
        if !is_terminal(&n) {
            let at = self.items.partition_point(|item| item.created_at > n.created_at);
            self.items.insert(at, n);
        }
        self.refocus(focused);
    }

    fn replace(&mut self, list: NotificationList) {
        let focused = self.focused().map(|f| f.id);
        self.unread = list.unread;
        self.items = list.notifications.into_iter().filter(|n| !is_terminal(n)).collect();
        self.refocus(focused);
    }

    fn refocus(&mut self, focused: Option<uuid::Uuid>) {
        let Some(panel) = self.panel.as_mut() else { return };
        let last = self.items.len().saturating_sub(1);
        panel.selected = focused
            .and_then(|id| self.items.iter().position(|n| n.id == id))
            .unwrap_or(panel.selected)
            .min(last);
    }

    #[must_use]
    pub fn focused(&self) -> Option<&Notification> {
        self.items.get(self.panel.as_ref()?.selected)
    }
}

pub enum NotificationEffect {
    Fetch,
    Read { id: String },
    Act { id: String, action: String },
    Dismiss { id: String },
    ReadAll,
}

pub enum NotificationAction {
    Open,
    Close,
    SelectNext,
    SelectPrev,
    Act(usize),
    Dismiss,
    ReadAll,
    Loaded(Box<NotificationList>),
    Failed(String),
    Upsert(Box<Notification>),
    Unread(i64),
    Acted(Box<NotificationTransition>),
    Dismissed(Box<Notification>),
    ReadAllDone(i64),
    WriteFailed(String),
}

pub fn reduce_notifications(app: &mut App, action: NotificationAction) -> Vec<Effect> {
    match action {
        NotificationAction::Open => open(app),
        NotificationAction::Close => {
            close(app);
            Vec::new()
        }
        NotificationAction::SelectNext => select(app, 1),
        NotificationAction::SelectPrev => select(app, -1),
        NotificationAction::Act(index) => act(app, index),
        NotificationAction::Dismiss => dismiss(app),
        NotificationAction::ReadAll => read_all(app),
        NotificationAction::Loaded(list) => {
            let state = &mut app.notifications;
            state.replace(*list);
            if let Some(panel) = state.panel.as_mut() {
                panel.loading = false;
                panel.error = None;
            }
            mark_focused_read(app)
        }
        NotificationAction::Failed(message) => {
            if let Some(panel) = app.notifications.panel.as_mut() {
                panel.loading = false;
                panel.error = Some(message);
            }
            Vec::new()
        }
        NotificationAction::Upsert(n) => {
            app.notifications.upsert(*n);
            Vec::new()
        }
        NotificationAction::Unread(count) => {
            app.notifications.unread = count;
            Vec::new()
        }
        NotificationAction::Acted(t) => acted(app, *t),
        NotificationAction::Dismissed(n) => {
            settle(app);
            app.notifications.upsert(*n);
            Vec::new()
        }
        NotificationAction::ReadAllDone(unread) => {
            app.notifications.unread = unread;
            settle(app);
            vec![fetch()]
        }
        NotificationAction::WriteFailed(message) => {
            settle(app);
            app.toast(Level::Warn, message);
            Vec::new()
        }
    }
}

const fn fetch() -> Effect {
    Effect::Notifications(NotificationEffect::Fetch)
}

#[must_use]
pub const fn startup() -> Effect {
    fetch()
}

fn open(app: &mut App) -> Vec<Effect> {
    if app.notifications.panel.is_none() {
        app.notifications.panel = Some(Panel { loading: true, ..Panel::default() });
        app.router.push(View::Notifications);
    }
    vec![fetch()]
}

fn close(app: &mut App) {
    app.notifications.panel = None;
    if app.view() == View::Notifications {
        app.router.pop();
    }
}

fn settle(app: &mut App) {
    if let Some(panel) = app.notifications.panel.as_mut() {
        panel.busy = false;
    }
}

fn select(app: &mut App, step: isize) -> Vec<Effect> {
    let len = app.notifications.items.len();
    let Some(panel) = app.notifications.panel.as_mut() else { return Vec::new() };
    if len == 0 {
        return Vec::new();
    }
    panel.selected = panel.selected.saturating_add_signed(step).min(len - 1);
    mark_focused_read(app)
}

/// Focus is the TUI's "seen": the list shows one notice at a time in full.
fn mark_focused_read(app: &mut App) -> Vec<Effect> {
    let Some(panel) = app.notifications.panel.as_ref() else { return Vec::new() };
    let Some(n) = app.notifications.items.get_mut(panel.selected) else { return Vec::new() };
    if !is_unread(n) {
        return Vec::new();
    }
    n.state = "read".to_owned();
    vec![Effect::Notifications(NotificationEffect::Read { id: n.id.to_string() })]
}

fn idle_focus(app: &mut App) -> Option<Notification> {
    let state = &mut app.notifications;
    let focused = state.focused().cloned();
    let panel = state.panel.as_mut()?;
    if panel.busy || panel.loading {
        return None;
    }
    if focused.is_none() {
        panel.error = Some("nothing selected".to_owned());
    }
    focused
}

fn act(app: &mut App, index: usize) -> Vec<Effect> {
    let Some(n) = idle_focus(app) else { return Vec::new() };
    let offered = actions(&n);
    let Some(choice) = offered.get(index) else {
        let message = if offered.is_empty() {
            "this notification has nothing to answer".to_owned()
        } else {
            format!("pick 1-{}", offered.len())
        };
        app.toast(Level::Info, message);
        return Vec::new();
    };
    if let Some(panel) = app.notifications.panel.as_mut() {
        panel.busy = true;
    }
    vec![Effect::Notifications(NotificationEffect::Act {
        id: n.id.to_string(),
        action: choice.id.clone(),
    })]
}

fn dismiss(app: &mut App) -> Vec<Effect> {
    let Some(n) = idle_focus(app) else { return Vec::new() };
    if let Some(panel) = app.notifications.panel.as_mut() {
        panel.busy = true;
    }
    vec![Effect::Notifications(NotificationEffect::Dismiss { id: n.id.to_string() })]
}

fn read_all(app: &mut App) -> Vec<Effect> {
    if let Some(panel) = app.notifications.panel.as_mut() {
        if panel.busy {
            return Vec::new();
        }
        panel.busy = true;
    }
    vec![Effect::Notifications(NotificationEffect::ReadAll)]
}

/// `applied: false` is someone else's answer winning, not an error.
fn acted(app: &mut App, t: NotificationTransition) -> Vec<Effect> {
    settle(app);
    let message = if t.applied {
        format!("answered: {}", t.notification.title)
    } else {
        let by = outcome(&t.notification).unwrap_or_else(|| t.notification.state.clone());
        format!("already answered ({by})")
    };
    app.toast(Level::Info, message);
    app.notifications.upsert(t.notification);
    Vec::new()
}

fn decode<T: serde::de::DeserializeOwned>(
    value: serde_json::Value,
    route: &'static str,
) -> Result<T, ClientError> {
    serde_json::from_value(value).map_err(|source| ClientError::Decode { route, source })
}

async fn transition(
    server: &Client,
    route: &'static str,
    id: &str,
) -> Result<NotificationTransition, ClientError> {
    decode(server.post(route, &[("id", id)], None).await?, route)
}

fn dismiss_refused(e: &ClientError) -> Option<String> {
    matches!(e, ClientError::Status { status: 409, .. })
        .then(|| "a blocking approval cannot be dismissed: answer it instead".to_owned())
}

pub async fn run(server: &Client, effect: NotificationEffect) -> Vec<Action> {
    let wrap = |a| vec![Action::Notifications(a)];
    match effect {
        NotificationEffect::Fetch => {
            let query = [("state", "open".to_owned()), ("limit", PAGE.to_string())];
            match server
                .get("get_notifications", &[], &query)
                .await
                .and_then(|v| decode::<NotificationList>(v, "get_notifications"))
            {
                Ok(list) => wrap(NotificationAction::Loaded(Box::new(list))),
                Err(e) => {
                    tracing::warn!(%e, "notification list fetch failed");
                    wrap(NotificationAction::Failed(e.to_string()))
                }
            }
        }
        NotificationEffect::Read { id } => {
            match transition(server, "post_notifications_by_id_read", &id).await {
                Ok(t) => wrap(NotificationAction::Upsert(Box::new(t.notification))),
                Err(e) => {
                    tracing::warn!(%e, id, "marking the notification read failed");
                    Vec::new()
                }
            }
        }
        NotificationEffect::Act { id, action } => {
            let body = NotificationActRequest { action, note: None, channel: Some("tui".into()) };
            match server
                .post_as::<_, NotificationTransition>(
                    "post_notifications_by_id_act",
                    &[("id", id.as_str())],
                    &body,
                )
                .await
            {
                Ok(t) => wrap(NotificationAction::Acted(Box::new(t))),
                Err(e) => wrap(NotificationAction::WriteFailed(format!("answer failed: {e}"))),
            }
        }
        NotificationEffect::Dismiss { id } => {
            match transition(server, "post_notifications_by_id_dismiss", &id).await {
                Ok(t) => wrap(NotificationAction::Dismissed(Box::new(t.notification))),
                Err(e) => wrap(NotificationAction::WriteFailed(
                    dismiss_refused(&e).unwrap_or_else(|| format!("dismiss failed: {e}")),
                )),
            }
        }
        NotificationEffect::ReadAll => match server
            .post("post_notifications_read_all", &[], None)
            .await
            .and_then(|v| decode::<NotificationReadAll>(v, "post_notifications_read_all"))
        {
            Ok(done) => wrap(NotificationAction::ReadAllDone(done.unread)),
            Err(e) => wrap(NotificationAction::WriteFailed(format!("mark all read failed: {e}"))),
        },
    }
}

#[cfg(test)]
#[path = "notifications_tests.rs"]
mod tests;
