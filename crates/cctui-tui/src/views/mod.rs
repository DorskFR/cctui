pub mod cards;
pub mod conversation;
pub mod help;
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
    }
}
