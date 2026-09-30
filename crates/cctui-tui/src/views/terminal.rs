//! The read-only terminal pane, drawn over the whole screen.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use tui_term::widget::{Cursor, PseudoTerminal};

use crate::app::App;
use crate::theme;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let [header_area, screen_area, footer_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1), Constraint::Length(1)])
            .areas(area);

    let Some(pane) = app.terminal.as_mut() else {
        frame.render_widget(Paragraph::new(Span::styled("No terminal", theme::dim())), area);
        return;
    };
    pane.view = (screen_area.width, screen_area.height);

    let title = format!(" terminal · {} ", short_id(&pane.session_id));
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(title, theme::header_bg()),
            Span::raw(" "),
            Span::styled(pane.header(), theme::dim()),
        ])),
        header_area,
    );

    // The emulator is the remote's size, not the pane's: a narrow pane shows
    // the left of the real screen rather than a reflowed guess at it.
    frame.render_widget(
        PseudoTerminal::new(pane.screen()).cursor(Cursor::default().visibility(false)),
        screen_area,
    );

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " Esc close · j/k scroll back · read-only",
            theme::dim(),
        ))),
        footer_area,
    );
}

fn short_id(session_id: &str) -> &str {
    session_id.get(..8).unwrap_or(session_id)
}
