#![allow(dead_code)]

mod app;

use std::io;
use std::time::Duration;

use anyhow::Result;
use app::{App, DetailMode, Pane, View};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;
use ratatui::{Frame, Terminal};

fn main() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    let result = run_event_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    loop {
        terminal.draw(|f| render(f, app))?;

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            handle_key(app, key.code);
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, code: KeyCode) {
    // Message input mode captures characters
    if app.message_input.is_some() {
        match code {
            KeyCode::Esc | KeyCode::Enter => {
                app.message_input = None;
            }
            KeyCode::Backspace => {
                if let Some(ref mut input) = app.message_input {
                    input.pop();
                }
            }
            KeyCode::Char(c) => {
                if let Some(ref mut input) = app.message_input {
                    input.push(c);
                }
            }
            _ => {}
        }
        return;
    }

    match code {
        KeyCode::Char('q') => {
            app.should_quit = true;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.select_next();
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.select_prev();
        }
        KeyCode::Tab => {
            app.active_pane = match app.active_pane {
                Pane::Tree => Pane::Detail,
                Pane::Detail => Pane::Tree,
            };
        }
        KeyCode::Char('l') => {
            app.detail_mode = DetailMode::Log;
        }
        KeyCode::Char('c') => {
            app.detail_mode = DetailMode::Conversation;
        }
        KeyCode::Char('m') => {
            app.message_input = Some(String::new());
        }
        KeyCode::Char('?') => {
            app.view = View::Help;
        }
        KeyCode::Enter => {
            app.view = View::Conversation;
        }
        KeyCode::Esc => {
            app.view = View::Sessions;
        }
        _ => {}
    }
}

fn render(frame: &mut Frame, _app: &App) {
    let text = Text::raw("Loading sessions...");
    let paragraph = Paragraph::new(text);
    frame.render_widget(paragraph, frame.area());
}
