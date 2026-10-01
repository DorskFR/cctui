//! The Access slice: the user list, its three detail tabs and their dialogs.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};

use crate::app::App;
use crate::app::admin::{Access, Mode, Tab, mintable_scopes};
use crate::theme;

pub fn draw(frame: &mut Frame, app: &App) {
    let [tabs_area, body_area, hotkeys_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1), Constraint::Length(1)])
            .areas(frame.area());

    frame.render_widget(Paragraph::new(tab_line(&app.access)), tabs_area);
    match app.access.mode() {
        Mode::Form { kind, text } => {
            frame.render_widget(form_lines(kind.prompt(), &text), body_area);
        }
        Mode::Confirm(pending) => frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {}", pending.question()),
                theme::error(),
            ))),
            body_area,
        ),
        Mode::TypedConfirm { name, typed, .. } => {
            frame.render_widget(purge_lines(&name, &typed), body_area);
        }
        Mode::KeyScopes { key_id, label, cursor, granted, .. } => {
            frame.render_widget(
                scope_lines(app, key_id.is_some(), &label, cursor, &granted),
                body_area,
            );
        }
        Mode::Secret { what, secret } => {
            frame.render_widget(secret_lines(what, &secret), body_area);
        }
        Mode::Browse => draw_rows(frame, app, body_area),
    }
    frame.render_widget(Paragraph::new(hotkeys(&app.access)), hotkeys_area);
}

fn tab_line(access: &Access) -> Line<'static> {
    let mut spans = vec![Span::styled(" Access ", theme::section_title())];
    for tab in Tab::ORDER {
        let current = tab == access.tab;
        let label =
            if current { format!("[{}] ", tab.label()) } else { format!(" {} ", tab.label()) };
        spans.push(Span::styled(label, if current { theme::bold() } else { theme::dim() }));
    }
    if let Some(user) = access.user()
        && access.tab != Tab::Users
    {
        spans.push(Span::styled(format!(" — {}", user.name), theme::branch()));
    }
    Line::from(spans)
}

fn draw_rows(frame: &mut Frame, app: &App, area: Rect) {
    let access = &app.access;
    if let Some(error) = &access.error {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(format!(" {error}"), theme::error()))),
            area,
        );
        return;
    }
    let rows = access.rows();
    if rows.is_empty() {
        let text = if access.loading { " loading…" } else { " nothing here" };
        frame.render_widget(Paragraph::new(Line::from(Span::styled(text, theme::dim()))), area);
        return;
    }

    let [header_area, rows_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
    frame.render_widget(Paragraph::new(header(access.tab)), header_area);

    let items: Vec<ListItem> = rows.iter().map(|index| row_item(access, *index)).collect();
    let list = List::new(items).highlight_style(theme::selected()).highlight_symbol("▸ ");
    let mut state = ListState::default().with_selected(Some(access.row));
    frame.render_stateful_widget(list, rows_area, &mut state);
}

fn header(tab: Tab) -> Line<'static> {
    let text = match tab {
        Tab::Users => format!("   {:<20}{:<11}{}", "NAME", "STATE", "DISPATCH"),
        Tab::Tokens => format!("   {:<20}{:<11}{}", "LABEL", "STATE", "PREVIEW"),
        Tab::Machines => format!("   {:<20}{:<11}{}", "NAME", "STATE", "PREVIEW"),
        Tab::Keys => format!("   {:<20}{:<11}{}", "LABEL", "STATE", "SCOPES"),
    };
    Line::from(Span::styled(text, theme::dim()))
}

/// Never the secret itself: the server only ever hands back a non-secret
/// fragment, and a row with none shows a dash rather than inventing one.
fn preview(fragment: Option<&String>) -> String {
    fragment.filter(|p| !p.is_empty()).cloned().unwrap_or_else(|| "—".to_owned())
}

const NAME_WIDTH: usize = 20;

fn row_item(access: &Access, index: usize) -> ListItem<'static> {
    let name = |text: &str| {
        Span::styled(
            format!(" {:<NAME_WIDTH$}", crate::app::session_status::truncate(text, NAME_WIDTH - 1)),
            theme::bold(),
        )
    };
    let state = |text: &str, revoked: bool| {
        Span::styled(format!("{text:<11}"), if revoked { theme::error() } else { theme::active() })
    };
    let spans = match access.tab {
        Tab::Users => {
            let Some(user) = access.users.get(index) else { return ListItem::new("") };
            vec![
                name(&user.name),
                state(user.state(), user.state() != "active"),
                Span::styled(if user.can_dispatch { "yes" } else { "no" }.to_owned(), theme::dim()),
            ]
        }
        Tab::Tokens => {
            let Some(token) = access.tokens.get(index) else { return ListItem::new("") };
            let revoked = token.revoked_at.is_some();
            vec![
                name(token.label.as_deref().unwrap_or("(unlabelled)")),
                state(if revoked { "revoked" } else { "active" }, revoked),
                Span::styled(preview(token.token_preview.as_ref()), theme::dim()),
            ]
        }
        Tab::Machines => {
            let Some(machine) = access.machines.get(index) else { return ListItem::new("") };
            let revoked = machine.revoked_at.is_some();
            vec![
                name(machine.label()),
                state(if revoked { "revoked" } else { liveness(machine) }, revoked),
                Span::styled(preview(machine.key_preview.as_ref()), theme::dim()),
            ]
        }
        Tab::Keys => {
            let Some(key) = access.keys.get(index) else { return ListItem::new("") };
            let revoked = key.revoked_at.is_some();
            vec![
                name(key.label.as_deref().unwrap_or("(unlabelled)")),
                state(if revoked { "revoked" } else { "active" }, revoked),
                Span::styled(key.scopes.join(","), theme::branch()),
            ]
        }
    };
    ListItem::new(Line::from(spans))
}

const fn liveness(machine: &cctui_client::UserMachine) -> &'static str {
    match machine.liveness {
        cctui_proto::models::MachineLiveness::Online => "online",
        cctui_proto::models::MachineLiveness::Stale => "stale",
        cctui_proto::models::MachineLiveness::Offline => "offline",
    }
}

fn form_lines(prompt: &str, text: &str) -> Paragraph<'static> {
    Paragraph::new(vec![
        Line::from(Span::styled(format!(" {prompt}"), theme::section_title())),
        Line::raw(""),
        Line::from(vec![
            Span::styled("  ❯ ", theme::border_focused()),
            Span::raw(text.to_owned()),
            Span::styled("▏", theme::dim()),
        ]),
    ])
}

fn purge_lines(name: &str, typed: &str) -> Paragraph<'static> {
    let matches = typed.trim() == name;
    Paragraph::new(vec![
        Line::from(Span::styled(format!(" purge {name}"), theme::error())),
        Line::raw(""),
        Line::from(Span::styled(
            "  a purge is a hard delete: nothing about it comes back.".to_owned(),
            theme::dim(),
        )),
        Line::from(Span::styled(format!("  type {name} to confirm:"), theme::dim())),
        Line::raw(""),
        Line::from(vec![
            Span::styled("  ❯ ", theme::border_focused()),
            Span::styled(typed.to_owned(), if matches { theme::active() } else { theme::error() }),
            Span::styled("▏", theme::dim()),
        ]),
    ])
}

fn scope_lines(
    app: &App,
    editing: bool,
    label: &str,
    cursor: usize,
    granted: &[bool],
) -> Paragraph<'static> {
    let title = if editing { " re-grant this key" } else { " mint an api key" };
    let mut lines = vec![
        Line::from(Span::styled(title, theme::section_title())),
        Line::raw(""),
        Line::from(vec![
            Span::styled("  label  ", theme::dim()),
            Span::raw(label.to_owned()),
            Span::styled("▏", theme::dim()),
        ]),
        Line::raw(""),
    ];
    for (index, (scope, allowed)) in mintable_scopes(&app.access.ceiling).into_iter().enumerate() {
        let on = granted.get(index).copied().unwrap_or(false);
        let marker = if index == cursor { "❯ " } else { "  " };
        let box_text = if on { "[x]" } else { "[ ]" };
        let style = if allowed { theme::bold() } else { theme::dim() };
        let mut spans = vec![
            Span::styled(format!("  {marker}"), theme::border_focused()),
            Span::styled(format!("{box_text} {scope}"), style),
        ];
        if !allowed {
            spans.push(Span::styled("  (outside this user's ceiling)", theme::dim()));
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::raw(""));
    let footer = if editing {
        "  Space toggles, Enter re-grants (the key keeps working), Esc backs out"
    } else {
        "  Space toggles, Enter mints, Esc backs out"
    };
    lines.push(Line::from(Span::styled(footer.to_owned(), theme::dim())));
    Paragraph::new(lines)
}

/// The one time a secret is on screen. It is in no log, toast or draft, and the
/// buffer holding it is overwritten the moment this dialog closes.
fn secret_lines(what: &str, secret: &str) -> Paragraph<'static> {
    Paragraph::new(vec![
        Line::from(Span::styled(format!(" new {what}"), theme::section_title())),
        Line::raw(""),
        Line::from(Span::styled(
            "  shown once — the server kept only a hash:".to_owned(),
            theme::dim(),
        )),
        Line::raw(""),
        Line::from(Span::styled(format!("  {secret}"), theme::bold())),
        Line::raw(""),
        Line::from(Span::styled(
            "  y copies it, Enter closes. Take it now.".to_owned(),
            theme::error(),
        )),
    ])
}

fn hotkeys(access: &Access) -> Line<'static> {
    let pair = |key: &'static str, desc: &'static str| {
        [Span::styled(key, theme::hotkey()), Span::styled(desc, theme::hotkey_desc())]
    };
    let mut spans: Vec<Span<'static>> = Vec::new();
    match access.mode() {
        Mode::Browse => {
            spans.extend(pair(" j/k", ":move  "));
            spans.extend(pair("Tab", ":tab  "));
            match access.tab {
                Tab::Users => {
                    spans.extend(pair("n", ":new  "));
                    spans.extend(pair("e", ":rename  "));
                    spans.extend(pair("d", ":disable  "));
                }
                Tab::Tokens => {
                    spans.extend(pair("n", ":mint  "));
                    spans.extend(pair("e", ":relabel  "));
                }
                Tab::Machines => spans.extend(pair("R", ":rotate  ")),
                Tab::Keys => {
                    spans.extend(pair("n", ":mint  "));
                    spans.extend(pair("s", ":scopes  "));
                }
            }
            spans.extend(pair("x", ":revoke  "));
            spans.extend(pair("P", ":purge  "));
            spans.extend(pair("G", ":disp"));
        }
        Mode::Form { .. } | Mode::KeyScopes { .. } | Mode::TypedConfirm { .. } => {
            spans.extend(pair(" Enter", ":confirm  "));
            spans.extend(pair("Esc", ":back"));
        }
        Mode::Confirm(_) => {
            spans.extend(pair(" Enter", ":do it  "));
            spans.extend(pair("Esc", ":keep"));
        }
        Mode::Secret { .. } => {
            spans.extend(pair(" y", ":copy  "));
            spans.extend(pair("Enter", ":close"));
        }
    }
    Line::from(spans)
}
