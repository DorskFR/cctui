pub mod attach;
pub mod banner;
pub mod cards;
pub mod conversation;
pub mod fileview;
pub mod help;
pub mod history;
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
            help::draw(frame, &app.config.keys, &mut app.help_scroll);
        }
        View::FileViewer => {
            conversation::draw(frame, app);
            if let Some(view) = app.file_view.as_ref() {
                // Full width: a pager showing source wants every column, and a
                // margin would leave the conversation showing at the edges.
                fileview::draw(frame, view, frame.area());
            }
        }
        View::AttachPrompt => {
            conversation::draw(frame, app);
            if let Some(prompt) = app.attach_prompt.as_ref() {
                attach::draw_prompt(frame, prompt, frame.area());
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
