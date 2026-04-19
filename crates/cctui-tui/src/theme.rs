use ratatui::style::{Color, Modifier, Style};

// Status colors
pub const ACTIVE: Style = Style::new().fg(Color::Green);
pub const NEW: Style = Style::new().fg(Color::Cyan);
pub const INACTIVE: Style = Style::new().fg(Color::DarkGray);

// Message roles
pub const USER_MSG: Style = Style::new().fg(Color::Cyan);
pub const ASSISTANT_MSG: Style = Style::new().fg(Color::White);
pub const TOOL_CALL: Style = Style::new().fg(Color::Yellow);
pub const TOOL_RESULT: Style = Style::new().fg(Color::DarkGray);
pub const TIMESTAMP: Style = Style::new().fg(Color::DarkGray);

// Tool call styles
pub const TOOL_BADGE_FG: Style = Style::new().fg(Color::Black).bg(Color::Yellow);
pub const TOOL_BADGE_BASH: Style = Style::new().fg(Color::Black).bg(Color::Cyan);
pub const TOOL_BADGE_FILE: Style = Style::new().fg(Color::Black).bg(Color::Blue);
pub const TOOL_PATH: Style = Style::new().fg(Color::Blue);
pub const TOOL_COMMAND: Style = Style::new().fg(Color::Cyan);
pub const TOOL_RESULT_ARROW: Style = Style::new().fg(Color::DarkGray);

// UI chrome
pub const BORDER_FOCUSED: Style = Style::new().fg(Color::Blue);
pub const BORDER_DIM: Style = Style::new().fg(Color::DarkGray);
pub const MACHINE_HEADER: Style = Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD);
pub const SELECTED: Style = Style::new().bg(Color::DarkGray).add_modifier(Modifier::BOLD);
pub const HOTKEY: Style = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
pub const HOTKEY_DESC: Style = Style::new().fg(Color::DarkGray);
pub const STATUS_BAR_BG: Style = Style::new().fg(Color::White).bg(Color::DarkGray);
pub const DIM: Style = Style::new().fg(Color::DarkGray);
pub const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);

// Session list details
pub const MODEL: Style = Style::new().fg(Color::DarkGray);
pub const COST: Style = Style::new().fg(Color::Yellow);
pub const BRANCH: Style = Style::new().fg(Color::DarkGray);

// Borderless layout
pub const HEADER_BG: Style = Style::new().fg(Color::White).bg(Color::DarkGray);
pub const SECTION_TITLE: Style = Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD);

// Markdown styles
pub const MD_CODE: Style = Style::new().fg(Color::Green);
pub const MD_BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
pub const MD_ITALIC: Style = Style::new().add_modifier(Modifier::ITALIC);

pub const fn status_style(status: cctui_proto::models::SessionStatus) -> Style {
    match status {
        cctui_proto::models::SessionStatus::Active => ACTIVE,
        cctui_proto::models::SessionStatus::New => NEW,
        cctui_proto::models::SessionStatus::Inactive => INACTIVE,
    }
}

pub const fn status_icon(status: cctui_proto::models::SessionStatus) -> &'static str {
    match status {
        cctui_proto::models::SessionStatus::Active => "●",
        cctui_proto::models::SessionStatus::New => "◎",
        cctui_proto::models::SessionStatus::Inactive => "○",
    }
}
