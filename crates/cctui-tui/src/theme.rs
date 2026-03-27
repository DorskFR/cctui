use ratatui::style::{Color, Modifier, Style};

// Status colors
pub const ACTIVE: Style = Style::new().fg(Color::Green);
pub const IDLE: Style = Style::new().fg(Color::Yellow);
pub const TERMINATED: Style = Style::new().fg(Color::DarkGray);
pub const DISCONNECTED: Style = Style::new().fg(Color::Red);
pub const REGISTERING: Style = Style::new().fg(Color::Cyan);

// Message roles
pub const USER_MSG: Style = Style::new().fg(Color::Cyan);
pub const ASSISTANT_MSG: Style = Style::new().fg(Color::White);
pub const TOOL_CALL: Style = Style::new().fg(Color::Yellow);
pub const TOOL_RESULT: Style = Style::new().fg(Color::DarkGray);
pub const TIMESTAMP: Style = Style::new().fg(Color::DarkGray);

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

pub const fn status_style(status: &cctui_proto::models::SessionStatus) -> Style {
    match status {
        cctui_proto::models::SessionStatus::Active => ACTIVE,
        cctui_proto::models::SessionStatus::Idle => IDLE,
        cctui_proto::models::SessionStatus::Terminated => TERMINATED,
        cctui_proto::models::SessionStatus::Disconnected => DISCONNECTED,
        cctui_proto::models::SessionStatus::Registering => REGISTERING,
    }
}

pub const fn status_icon(status: &cctui_proto::models::SessionStatus) -> &'static str {
    match status {
        cctui_proto::models::SessionStatus::Active => "●",
        cctui_proto::models::SessionStatus::Idle => "○",
        cctui_proto::models::SessionStatus::Terminated => "✕",
        cctui_proto::models::SessionStatus::Disconnected => "◌",
        cctui_proto::models::SessionStatus::Registering => "◎",
    }
}
