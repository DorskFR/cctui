//! The instance panel: status rows, the last hook run, and the update confirm.

use cctui_clientcore::instance::{PhaseTone, can_launch};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::App;
use crate::app::instance::{Mode, is_admin};
use crate::theme;

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area().inner(Margin { horizontal: 4, vertical: 2 });
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border_focused())
        .title(" Instance ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [body, hotkeys_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);

    match app.instance.mode() {
        Mode::Confirm => draw_confirm(frame, app, body),
        Mode::Browse => draw_status(frame, app, body),
    }
    frame.render_widget(Paragraph::new(hotkeys(app)), hotkeys_area);
}

fn tone_style(tone: PhaseTone) -> Style {
    match tone {
        PhaseTone::Success => theme::active(),
        PhaseTone::Faint => theme::dim(),
        PhaseTone::Danger => theme::error(),
    }
}

fn row(label: &'static str, value: String, style: Style) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!(" {label:<18}"), theme::dim()),
        Span::styled(value, style),
    ])
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    if let Some(error) = &app.instance.error {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(format!(" {error}"), theme::error()))),
            area,
        );
        return;
    }
    if app.instance.info.is_none() {
        let text = if app.instance.loading { " loading…" } else { " no status" };
        frame.render_widget(Paragraph::new(Line::from(Span::styled(text, theme::dim()))), area);
        return;
    }

    let instance = &app.instance;
    let latest =
        instance.latest().map_or_else(|| "up to date".to_owned(), |latest| format!("v{latest}"));
    let latest_style = if instance.update_available() { theme::stale() } else { theme::dim() };

    let mut lines = vec![
        row("name", instance.name(), theme::bold()),
        row("version", format!("v{}", instance.version()), theme::bold()),
        row("channel", instance.channel().to_owned(), theme::dim()),
        row("latest available", latest, latest_style),
        row(
            "self-update",
            if instance.self_update_hook() {
                "update hook".to_owned()
            } else if instance.self_update_ready() {
                "agent fallback".to_owned()
            } else {
                "not configured".to_owned()
            },
            theme::dim(),
        ),
        Line::raw(""),
    ];

    match instance.run_readout().zip(instance.run.as_ref()) {
        Some(((phase, tone), run)) => {
            lines.push(Line::from(Span::styled(" last update run", theme::section_title())));
            lines.push(row(
                "phase",
                format!("{phase}  (v{} → v{})", run.from_version, run.version),
                tone_style(tone),
            ));
            if !run.detail.is_empty() {
                lines.push(row("detail", run.detail.clone(), theme::dim()));
            }
            if let Some(code) = run.exit_code {
                lines.push(row("exit", code.to_string(), theme::dim()));
            }
            if let Some(tail) = run.output_tail.as_deref().filter(|t| !t.is_empty()) {
                lines.push(Line::raw(""));
                lines.push(Line::from(Span::styled(" command output", theme::dim())));
                for line in tail.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev() {
                    lines.push(Line::from(Span::styled(format!("   {line}"), theme::dim())));
                }
            }
        }
        None => lines.push(Line::from(Span::styled(" no update run yet", theme::dim()))),
    }

    if !instance.releases.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(" release notes", theme::section_title())));
        for release in instance.releases.iter().take(3) {
            lines.push(Line::from(Span::styled(format!("  v{}", release.version), theme::bold())));
            for note in release.body.lines().filter(|l| !l.trim().is_empty()).take(4) {
                lines.push(Line::from(Span::styled(format!("    {note}"), theme::dim())));
            }
        }
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        format!("  {}", instance.hint(is_admin(app))),
        theme::dim(),
    )));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn draw_confirm(frame: &mut Frame, app: &App, area: Rect) {
    let latest = app.instance.latest().unwrap_or("-");
    let lines = vec![
        Line::from(Span::styled(format!(" update this server to v{latest}?"), theme::error())),
        Line::raw(""),
        Line::from(Span::styled(
            format!("  {}", app.instance.badge_text()),
            theme::section_title(),
        )),
        Line::raw(""),
        Line::from(Span::styled(format!("  {}", app.instance.confirm_text()), theme::dim())),
        Line::raw(""),
        Line::from(Span::styled("  Enter starts it, Esc keeps this version.", theme::bold())),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn hotkeys(app: &App) -> Line<'static> {
    let pair = |key: &'static str, desc: &'static str| {
        [Span::styled(key, theme::hotkey()), Span::styled(desc, theme::hotkey_desc())]
    };
    let mut spans: Vec<Span<'static>> = Vec::new();
    match app.instance.mode() {
        Mode::Browse => {
            spans.extend(pair(" p", ":check upstream  "));
            if app.instance.launching {
                spans.push(Span::styled("updating…  ", theme::stale()));
            } else if can_launch(
                is_admin(app),
                app.instance.self_update_ready(),
                app.instance.update_available(),
            ) {
                spans.extend(pair("U", ":update  "));
            }
            spans.extend(pair("Ctrl+r", ":refresh  "));
            spans.extend(pair("Esc", ":close"));
        }
        Mode::Confirm => {
            spans.extend(pair(" Enter", ":update  "));
            spans.extend(pair("Esc", ":keep this version"));
        }
    }
    Line::from(spans)
}
