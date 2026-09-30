use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::PendingPermission;
use crate::theme;

pub fn draw(frame: &mut Frame, req: &PendingPermission) {
    let area = frame.area().inner(Margin { horizontal: 8, vertical: 5 });
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border_focused())
        .title(" Tool Approval Required ");

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [tool_area, desc_area, preview_area, _, hotkeys_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    // Tool name
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  Tool:  ", theme::dim()),
            Span::styled(req.tool_name.clone(), theme::bold()),
        ])),
        tool_area,
    );

    // Description
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  Desc:  ", theme::dim()),
            Span::raw(req.description.clone()),
        ]))
        .wrap(Wrap { trim: true }),
        desc_area,
    );

    // Input preview
    let preview_block = Block::default()
        .borders(Borders::TOP)
        .border_style(theme::border_dim())
        .title(" Input Preview ");
    let preview_inner = preview_block.inner(preview_area);
    frame.render_widget(preview_block, preview_area);
    frame.render_widget(
        Paragraph::new(req.input_preview.clone()).style(theme::dim()).wrap(Wrap { trim: true }),
        preview_inner,
    );

    // Hotkeys
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  y ", theme::hotkey()),
            Span::raw("Allow  "),
            Span::styled("n ", theme::hotkey()),
            Span::raw("Deny"),
        ])),
        hotkeys_area,
    );
}
