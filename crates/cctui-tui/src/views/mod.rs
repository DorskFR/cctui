pub mod banner;
pub mod cards;
pub mod conversation;
pub mod help;
pub mod history;
pub mod macros;
pub mod mentions;
pub mod pins;
pub mod prompt;
pub mod sessions;

use ratatui::Frame;

use crate::app::{App, View};

/// What a modal overlay floats over: the conversation it was opened from, else
/// the session list.
fn draw_below(frame: &mut Frame, app: &mut App) {
    match app.router.below() {
        Some(View::Conversation) => conversation::draw(frame, app),
        _ => sessions::draw(frame, app),
    }
}

pub fn render(frame: &mut Frame, app: &mut App) {
    match app.view() {
        View::SessionList => sessions::draw(frame, app),
        View::Conversation => conversation::draw(frame, app),
        // Help always renders over the session list, whatever it was opened from.
        View::Help => {
            sessions::draw(frame, app);
            help::draw(frame, &app.config.keys, &mut app.help_scroll);
        }
        View::HistoryPicker => {
            draw_below(frame, app);
            if let Some(picker) = app.drafts.picker.as_ref() {
                history::draw(frame, picker);
            }
        }
        View::Pins => {
            draw_below(frame, app);
            if let Some(list) = app.pins.list.as_ref() {
                pins::draw(frame, list);
            }
        }
        View::Macros => {
            draw_below(frame, app);
            macros::draw(frame, &app.macros);
        }
    }
}
