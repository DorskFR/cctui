pub mod conversation;
pub mod help;
pub mod permission;
pub mod prompt;
pub mod sessions;

use ratatui::Frame;

use crate::app::{App, View};

pub fn render(frame: &mut Frame, app: &mut App) {
    match app.view() {
        View::SessionList => sessions::draw(frame, app),
        View::Conversation => conversation::draw(frame, app),
        // Help always renders over the session list, whatever it was opened from.
        View::Help => {
            sessions::draw(frame, app);
            help::draw(frame, &app.config.keys);
        }
        View::PermissionDialog => {
            match app.router.below() {
                Some(View::Conversation) => conversation::draw(frame, app),
                _ => sessions::draw(frame, app),
            }
            if let Some(req) = app.permission_queue.front() {
                permission::draw(frame, req);
            }
        }
    }
}
