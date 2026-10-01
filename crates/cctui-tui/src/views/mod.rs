pub mod attach;
pub mod banner;
pub mod cards;
pub mod conversation;
pub mod diagnose;
pub mod fileview;
pub mod filters;
pub mod help;
pub mod history;
pub mod macros;
pub mod mentions;
pub mod model_picker;
pub mod pins;
pub mod prompt;
pub mod sections;
pub mod sessions;
pub mod sidebar;
pub mod terminal;

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
        View::FileViewer => {
            conversation::draw(frame, app);
            if let Some(view) = app.file_view.as_ref() {
                // Full width: a pager showing source wants every column, and a
                // margin would leave the conversation showing at the edges.
                fileview::draw(frame, view, frame.area());
            }
        }
        View::Diagnose => {
            match app.router.below() {
                Some(View::Conversation) => conversation::draw(frame, app),
                _ => sessions::draw(frame, app),
            }
            diagnose::draw(frame, app);
        }
        View::Terminal => terminal::draw(frame, app),
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
