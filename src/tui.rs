//! Interactive terminal UI built with ratatui.

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};
use sqlx::SqlitePool;
use std::io;

use crate::models::Exchange;

pub struct App {
    pool: SqlitePool,
    session: String,
    exchanges: Vec<Exchange>,
    list_state: ListState,
    should_quit: bool,
}

impl App {
    pub fn new(pool: SqlitePool, session: String) -> Self {
        Self {
            pool,
            session,
            exchanges: Vec::new(),
            list_state: ListState::default(),
            should_quit: false,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        crossterm::execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        self.list_state.select(Some(0));

        loop {
            terminal.draw(|f| self.draw(f))?;
            self.handle_events()?;

            if self.should_quit {
                break;
            }
        }

        disable_raw_mode()?;
        crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;
        Ok(())
    }

    fn draw(&self, frame: &mut ratatui::Frame) {
        let size = frame.area();

        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(3),
            ])
            .split(size);

        let title_block = Block::default()
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::Cyan));
        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                " LEDGER ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" │ "),
            Span::styled(
                format!("session: {}", self.session),
                Style::default().fg(Color::DarkGray),
            ),
        ]))
        .block(title_block);
        frame.render_widget(title, outer[0]);

        let main = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(outer[1]);

        let request_list = self.build_request_list();
        frame.render_stateful_widget(request_list, main[0], &mut self.list_state.clone());

        let detail_block = Block::default()
            .borders(Borders::ALL)
            .title(" Detail ")
            .style(Style::default().fg(Color::Yellow));
        let detail = Paragraph::new("Select a request to view details").block(detail_block);
        frame.render_widget(detail, main[1]);

        let status_bar = Block::default()
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::DarkGray));
        let status = Paragraph::new(Line::from(vec![
            Span::styled(
                " q:Quit ",
                Style::default().fg(Color::White).bg(Color::DarkGray),
            ),
            Span::raw("  "),
            Span::styled(
                " ↑↓:Navigate ",
                Style::default().fg(Color::White).bg(Color::DarkGray),
            ),
            Span::raw("  "),
            Span::styled(
                format!(" {} requests ", self.exchanges.len()),
                Style::default().fg(Color::Cyan),
            ),
        ]))
        .block(status_bar);
        frame.render_widget(status, outer[2]);
    }

    fn build_request_list(&self) -> List<'_> {
        let items: Vec<ListItem> = self
            .exchanges
            .iter()
            .map(|exchange| {
                let method = &exchange.request.method;
                let status = exchange.status_label();
                let path = &exchange.request.path;
                let method_style = match method.as_str() {
                    "GET" => Style::default().fg(Color::Green),
                    "POST" => Style::default().fg(Color::Yellow),
                    "PUT" => Style::default().fg(Color::Blue),
                    "DELETE" => Style::default().fg(Color::Red),
                    "PATCH" => Style::default().fg(Color::Magenta),
                    _ => Style::default().fg(Color::White),
                };
                let status_style = match status {
                    "2xx" => Style::default().fg(Color::Green),
                    "4xx" | "5xx" => Style::default().fg(Color::Red),
                    _ => Style::default().fg(Color::DarkGray),
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{method:>6} "), method_style),
                    Span::styled(format!("{status:>3} "), status_style),
                    Span::raw(path.clone()),
                ]))
            })
            .collect();

        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Requests ")
                .style(Style::default().fg(Color::White)),
        )
    }

    fn handle_events(&mut self) -> Result<()> {
        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => self.should_quit = true,
                    KeyCode::Up => self.move_selection(-1),
                    KeyCode::Down => self.move_selection(1),
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn move_selection(&mut self, delta: i32) {
        let len = self.exchanges.len() as i32;
        if len == 0 {
            return;
        }
        let current = self.list_state.selected().map_or(0, |i| i as i32);
        let next = ((current + delta).clamp(0, len - 1)) as usize;
        self.list_state.select(Some(next));
    }
}
