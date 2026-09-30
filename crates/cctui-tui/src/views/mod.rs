pub mod banner;
pub mod cards;
pub mod conversation;
pub mod help;
pub mod history;
pub mod model_picker;
pub mod prompt;
pub mod sessions;
pub mod sidebar;

use ratatui::Frame;

use crate::app::{App, View};

pub fn render(frame: &mut Frame, app: &mut App) {
    match app.view() {
        View::SessionList => sessions::draw(frame, app),
        // The sidebar is focus only: the conversation draws the panel itself,
        // so taking the keyboard never redraws the transcript differently.
        View::Conversation | View::Sidebar => conversation::draw(frame, app),
        // Help always renders over the session list, whatever it was opened from.
        View::Help => {
            sessions::draw(frame, app);
            help::draw(frame, &app.config.keys, &mut app.help_scroll);
        }
        View::ModelPicker => {
            conversation::draw(frame, app);
            if let Some(picker) = app.controls.picker.as_ref() {
                model_picker::draw(frame, picker);
            }
        }
        View::HistoryPicker => {
            match app.router.below() {
                Some(View::Conversation) => conversation::draw(frame, app),
                _ => sessions::draw(frame, app),
            }
            if let Some(picker) = app.drafts.picker.as_ref() {
                history::draw(frame, picker);
            }
        }
    }
}
