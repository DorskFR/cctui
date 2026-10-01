//! The Bookmarks slice: status line, tab bar and hotkeys, so the switcher has
//! somewhere to land. The bookmark list itself replaces [`draw_body`].

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::theme;

pub fn draw(frame: &mut Frame, app: &App) {
    let [status_area, tabs_area, body_area, hotkeys_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let mut status = vec![
        Span::styled(" cctui ", theme::status_bar_bg()),
        Span::raw(" "),
        Span::styled(format!("v{}", app.version), theme::dim()),
        Span::raw("  "),
    ];
    status.extend(crate::widgets::tabs::summary_spans(app, usize::from(status_area.width)));
    status.extend(crate::widgets::status::status_spans(app));
    frame.render_widget(Paragraph::new(Line::from(status)), status_area);

    frame.render_widget(
        Paragraph::new(crate::widgets::tabs::tab_line(app, usize::from(tabs_area.width))),
        tabs_area,
    );

    draw_body(frame, app, body_area);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" 1-9 ", theme::hotkey()),
            Span::raw("Switch view  "),
            Span::styled("? ", theme::hotkey()),
            Span::raw("Help  "),
            Span::styled("q ", theme::hotkey()),
            Span::raw("Quit"),
        ])),
        hotkeys_area,
    );
}

fn draw_body(frame: &mut Frame, _app: &App, area: ratatui::layout::Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(" no bookmarks yet", theme::dim()))),
        area,
    );
}
