//! The read-only terminal pane, drawn over the whole screen.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Widget};

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
    frame.render_widget(Screen(pane.screen()), screen_area);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " Esc close · j/k scroll back · read-only",
            theme::dim(),
        ))),
        footer_area,
    );
}

struct Screen<'a>(&'a vt100::Screen);

impl Widget for Screen<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        for row in 0..area.height {
            for col in 0..area.width {
                if let Some(cell) = self.0.cell(row, col) {
                    paint(cell, &mut buf[(area.x + col, area.y + row)]);
                }
            }
        }
    }
}

fn paint(cell: &vt100::Cell, out: &mut ratatui::buffer::Cell) {
    if cell.has_contents() {
        out.set_symbol(cell.contents());
    }
    let mut modifier = Modifier::empty();
    for (on, m) in [
        (cell.bold(), Modifier::BOLD),
        (cell.italic(), Modifier::ITALIC),
        (cell.underline(), Modifier::UNDERLINED),
        (cell.inverse(), Modifier::REVERSED),
        (cell.dim(), Modifier::DIM),
    ] {
        if on {
            modifier |= m;
        }
    }
    out.set_style(
        Style::reset().fg(color(cell.fgcolor())).bg(color(cell.bgcolor())).add_modifier(modifier),
    );
}

const fn color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

fn short_id(session_id: &str) -> &str {
    session_id.get(..8).unwrap_or(session_id)
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU16;

    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier};
    use ratatui::widgets::Widget;

    use super::Screen;

    #[test]
    fn renders_cells_with_style_and_clips_to_area() {
        let mut parser =
            vt100::Parser::new(NonZeroU16::new(3).unwrap(), NonZeroU16::new(10).unwrap(), 0);
        parser.process(b"\x1b[1;31mhi\x1b[0m there\r\n\x1b[38;2;1;2;3mrgb");
        let area = Rect::new(1, 1, 5, 2);
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 4));
        Screen(parser.screen()).render(area, &mut buf);

        let h = &buf[(1, 1)];
        assert_eq!(h.symbol(), "h");
        assert_eq!(h.fg, Color::Indexed(1));
        assert!(h.modifier.contains(Modifier::BOLD));
        assert_eq!(buf[(4, 1)].symbol(), "t");
        assert_eq!(buf[(4, 1)].fg, Color::Reset);
        assert_eq!(buf[(1, 2)].fg, Color::Rgb(1, 2, 3));
        assert_eq!(buf[(6, 1)].symbol(), " ", "column past the area stays untouched");
    }
}
