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
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use sqlx::SqlitePool;
use std::io;

use crate::db;
use crate::models::Exchange;

pub struct App {
    pool: SqlitePool,
    session: String,
    exchanges: Vec<Exchange>,
    filtered_exchanges: Vec<Exchange>,
    list_state: ListState,
    should_quit: bool,
    detail_scroll: u16,
    detail_mode: DetailMode,
    // Search/filter state
    search_mode: SearchMode,
    search_query: String,
    search_cursor: usize,
    // Grouping state
    group_by_host: bool,
    collapsed_groups: std::collections::HashSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum DetailMode {
    Request,
    Response,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SearchMode {
    None,
    Filtering,
}

impl App {
    pub fn new(pool: SqlitePool, session: String) -> Self {
        Self {
            pool,
            session,
            exchanges: Vec::new(),
            filtered_exchanges: Vec::new(),
            list_state: ListState::default(),
            should_quit: false,
            detail_scroll: 0,
            detail_mode: DetailMode::Response,
            search_mode: SearchMode::None,
            search_query: String::new(),
            search_cursor: 0,
            group_by_host: true,
            collapsed_groups: std::collections::HashSet::new(),
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        // Load initial data
        self.exchanges = db::list_exchanges(&self.pool, &self.session, 500).await?;
        self.apply_filter();

        enable_raw_mode()?;
        let mut stdout = io::stdout();
        crossterm::execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        self.list_state.select(Some(0));

        let mut last_refresh = std::time::Instant::now();
        let refresh_interval = std::time::Duration::from_secs(2);

        loop {
            terminal.draw(|f| self.draw(f))?;
            self.handle_events()?;

            // Auto-refresh: poll DB every 2 seconds for new exchanges
            if last_refresh.elapsed() >= refresh_interval {
                if let Ok(new_exchanges) = db::list_exchanges(&self.pool, &self.session, 500).await
                    && new_exchanges.len() != self.exchanges.len()
                {
                    self.exchanges = new_exchanges;
                    self.apply_filter();
                    // Keep selection valid
                    let len = self.filtered_exchanges.len() as i32;
                    if len > 0 {
                        let current = self.list_state.selected().map_or(0, |i| i as i32);
                        let next = current.clamp(0, len - 1) as usize;
                        self.list_state.select(Some(next));
                    }
                }
                last_refresh = std::time::Instant::now();
            }

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
            Span::raw(" │ "),
            Span::styled(
                format!("{} requests", self.filtered_exchanges.len()),
                Style::default().fg(Color::Yellow),
            ),
            if self.search_mode == SearchMode::Filtering {
                Span::raw(" │ ")
            } else {
                Span::raw("")
            },
            if self.search_mode == SearchMode::Filtering {
                Span::styled(
                    "FILTERING",
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                )
            } else if !self.search_query.is_empty() {
                Span::styled(
                    format!("filter: '{}'", self.search_query),
                    Style::default().fg(Color::Magenta),
                )
            } else {
                Span::raw("")
            },
        ]))
        .block(title_block);
        frame.render_widget(title, outer[0]);

        let main = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(outer[1]);

        let request_list = self.build_request_list();
        frame.render_stateful_widget(request_list, main[0], &mut self.list_state.clone());

        let detail = self.build_detail_view();
        frame.render_widget(detail, main[1]);

        // Search overlay
        if self.search_mode == SearchMode::Filtering {
            let search_area = self.centered_rect(60, 3, size);
            frame.render_widget(Clear, search_area);
            let search_block = Block::default()
                .borders(Borders::ALL)
                .title(" Filter ")
                .style(Style::default().fg(Color::Yellow));
            let search_text = Paragraph::new(self.search_query.as_str()).block(search_block);
            frame.render_widget(search_text, search_area);
            // Position cursor in search box
            let cursor_x = search_area.x + self.search_cursor as u16 + 1;
            let cursor_y = search_area.y + 1;
            frame.set_cursor_position((cursor_x, cursor_y));
        }

        let status_bar = Block::default()
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::DarkGray));
        let status_text = if self.search_mode == SearchMode::Filtering {
            Line::from(vec![
                Span::styled(
                    " Enter:Apply ",
                    Style::default().fg(Color::White).bg(Color::DarkGray),
                ),
                Span::raw("  "),
                Span::styled(
                    " Esc:Cancel ",
                    Style::default().fg(Color::White).bg(Color::DarkGray),
                ),
                Span::raw("  "),
                Span::styled(
                    " Backspace:Delete ",
                    Style::default().fg(Color::White).bg(Color::DarkGray),
                ),
            ])
        } else {
            Line::from(vec![
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
                    " Tab:Toggle Detail ",
                    Style::default().fg(Color::White).bg(Color::DarkGray),
                ),
                Span::raw("  "),
                Span::styled(
                    " /:Filter ",
                    Style::default().fg(Color::White).bg(Color::DarkGray),
                ),
                Span::raw("  "),
                Span::styled(
                    " g:Toggle Group ",
                    Style::default().fg(Color::White).bg(Color::DarkGray),
                ),
                Span::raw("  "),
                Span::styled(
                    " e:Expand/Collapse ",
                    Style::default().fg(Color::White).bg(Color::DarkGray),
                ),
                Span::raw("  "),
                Span::styled(
                    format!(" {} requests ", self.filtered_exchanges.len()),
                    Style::default().fg(Color::Cyan),
                ),
            ])
        };
        let status = Paragraph::new(status_text).block(status_bar);
        frame.render_widget(status, outer[2]);
    }

    fn centered_rect(
        &self,
        percent_x: u16,
        height: u16,
        r: ratatui::layout::Rect,
    ) -> ratatui::layout::Rect {
        let popup_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage((100 - height * 100 / r.height) / 2),
                Constraint::Length(height),
                Constraint::Percentage((100 - height * 100 / r.height) / 2),
            ])
            .split(r);

        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage((100 - percent_x) / 2),
                Constraint::Percentage(percent_x),
                Constraint::Percentage((100 - percent_x) / 2),
            ])
            .split(popup_layout[1])[1]
    }

    fn build_request_list(&self) -> List<'_> {
        let mut items: Vec<ListItem> = Vec::new();

        if self.group_by_host {
            // Group by host
            use std::collections::BTreeMap;
            let mut groups: BTreeMap<&str, Vec<&Exchange>> = BTreeMap::new();
            for ex in &self.filtered_exchanges {
                groups.entry(&ex.request.host).or_default().push(ex);
            }

            for (host, group_exchanges) in &groups {
                let is_collapsed = self.collapsed_groups.contains(*host);

                // Calculate group stats
                let count = group_exchanges.len();
                let avg_latency = group_exchanges
                    .iter()
                    .filter_map(|ex| ex.response.as_ref().map(|r| r.latency_ms))
                    .sum::<u64>()
                    / count.max(1) as u64;
                let error_count = group_exchanges
                    .iter()
                    .filter(|ex| matches!(ex.response, Some(ref r) if r.status >= 400))
                    .count();
                let error_rate = (error_count * 100) / count.max(1);

                let group_style = if is_collapsed {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default().fg(Color::Cyan)
                };

                let expand_icon = if is_collapsed { "▶" } else { "▼" };

                items.push(ListItem::new(Line::from(vec![
                    Span::styled(format!("{expand_icon} "), group_style),
                    Span::styled(format!("{host} "), group_style.add_modifier(Modifier::BOLD)),
                    Span::styled(
                        format!(
                            "({} req, {}ms avg, {}% err)",
                            count, avg_latency, error_rate
                        ),
                        Style::default().fg(Color::DarkGray),
                    ),
                ])));

                if !is_collapsed {
                    for exchange in group_exchanges.iter() {
                        items.push(self.exchange_list_item(exchange));
                    }
                }
            }
        } else {
            // Flat list
            for exchange in &self.filtered_exchanges {
                items.push(self.exchange_list_item(exchange));
            }
        }

        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Requests ")
                .style(Style::default().fg(Color::White)),
        )
    }

    fn exchange_list_item(&self, exchange: &Exchange) -> ListItem<'_> {
        let method = &exchange.request.method;
        let status = exchange.status_label();
        let path = &exchange.request.path;
        let host = &exchange.request.host;
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
            "3xx" => Style::default().fg(Color::Blue),
            "4xx" | "5xx" => Style::default().fg(Color::Red),
            _ => Style::default().fg(Color::DarkGray),
        };
        ListItem::new(Line::from(vec![
            Span::raw("  "), // indent under group header
            Span::styled(format!("{method:>6} "), method_style),
            Span::styled(format!("{status:>3} "), status_style),
            Span::raw(format!("{host}{path}")),
        ]))
    }

    fn group_for_selection(&self) -> Option<String> {
        if !self.group_by_host {
            return None;
        }

        let selected = self.list_state.selected()?;

        // Walk through grouped items to find which group the selection is in
        use std::collections::BTreeMap;
        let mut groups: BTreeMap<&str, Vec<&Exchange>> = BTreeMap::new();
        for ex in &self.filtered_exchanges {
            groups.entry(&ex.request.host).or_default().push(ex);
        }

        let mut idx = 0usize;
        for (host, group_exchanges) in &groups {
            if idx == selected {
                // Selection is on the group header itself
                return Some(host.to_string());
            }
            idx += 1;

            if !self.collapsed_groups.contains(*host) {
                let group_size = group_exchanges.len();
                if selected >= idx && selected < idx + group_size {
                    return Some(host.to_string());
                }
                idx += group_size;
            }
        }

        None
    }

    fn build_detail_view(&self) -> Paragraph<'_> {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(
                " Detail ({}) ",
                match self.detail_mode {
                    DetailMode::Request => "Request",
                    DetailMode::Response => "Response",
                }
            ))
            .style(Style::default().fg(Color::Yellow));

        let text = match self
            .list_state
            .selected()
            .and_then(|i| self.filtered_exchanges.get(i))
        {
            Some(exchange) => match self.detail_mode {
                DetailMode::Request => self.format_request_detail(exchange),
                DetailMode::Response => self.format_response_detail(exchange),
            },
            None => Text::from("Select a request to view details"),
        };

        Paragraph::new(text)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((self.detail_scroll, 0))
    }

    fn format_request_detail(&self, exchange: &Exchange) -> Text<'static> {
        let req = &exchange.request;
        let mut lines: Vec<Line> = vec![
            Line::from(vec![
                Span::styled("Method: ", Style::default().fg(Color::Cyan)),
                Span::raw(req.method.clone()),
            ]),
            Line::from(vec![
                Span::styled("URL: ", Style::default().fg(Color::Cyan)),
                Span::raw(req.url.clone()),
            ]),
            Line::from(vec![
                Span::styled("Host: ", Style::default().fg(Color::Cyan)),
                Span::raw(req.host.clone()),
            ]),
            Line::from(vec![
                Span::styled("Time: ", Style::default().fg(Color::Cyan)),
                Span::raw(req.timestamp.to_rfc3339()),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "Headers:",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
        ];

        for (k, v) in &req.headers {
            lines.push(Line::from(vec![
                Span::styled(format!("{}: ", k), Style::default().fg(Color::Yellow)),
                Span::raw(v.clone()),
            ]));
        }

        if let Some(ref body) = req.body {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Body:",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.extend(self.highlight_json_body(body));
        }

        Text::from(lines)
    }

    fn format_response_detail(&self, exchange: &Exchange) -> Text<'static> {
        let Some(ref resp) = exchange.response else {
            return Text::from("No response captured");
        };

        let mut lines: Vec<Line> = vec![
            Line::from(vec![
                Span::styled("Status: ", Style::default().fg(Color::Cyan)),
                Span::styled(
                    format!("{} {}", resp.status, resp.status_text),
                    if resp.status >= 400 {
                        Style::default().fg(Color::Red)
                    } else {
                        Style::default().fg(Color::Green)
                    },
                ),
            ]),
            Line::from(vec![
                Span::styled("Latency: ", Style::default().fg(Color::Cyan)),
                Span::raw(format!("{} ms", resp.latency_ms)),
            ]),
            Line::from(vec![
                Span::styled("Time: ", Style::default().fg(Color::Cyan)),
                Span::raw(resp.timestamp.to_rfc3339()),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "Headers:",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
        ];

        for (k, v) in &resp.headers {
            lines.push(Line::from(vec![
                Span::styled(format!("{}: ", k), Style::default().fg(Color::Yellow)),
                Span::raw(v.clone()),
            ]));
        }

        if let Some(ref body) = resp.body {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Body:",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.extend(self.highlight_json_body(body));
        }

        Text::from(lines)
    }

    /// Highlight JSON body with colorized keys, strings, numbers, booleans.
    fn highlight_json_body(&self, body: &[u8]) -> Vec<Line<'static>> {
        let text = String::from_utf8_lossy(body);
        let trimmed = text.trim();

        // Quick check: does it look like JSON?
        if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
            // Not JSON, return as plain text
            return text.lines().map(|l| Line::from(l.to_string())).collect();
        }

        // Try to pretty-print and highlight
        match serde_json::from_str::<serde_json::Value>(trimmed) {
            Ok(val) => {
                let pretty = serde_json::to_string_pretty(&val).unwrap_or_default();
                colorize_json_owned(pretty)
            }
            Err(_) => {
                // Invalid JSON, return raw
                text.lines().map(|l| Line::from(l.to_string())).collect()
            }
        }
    }

    fn handle_events(&mut self) -> Result<()> {
        if event::poll(std::time::Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
        {
            match self.search_mode {
                SearchMode::Filtering => self.handle_search_input(key.code),
                SearchMode::None => self.handle_normal_input(key.code),
            }
        }
        Ok(())
    }

    fn handle_normal_input(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Up => {
                self.move_selection(-1);
                self.detail_scroll = 0;
            }
            KeyCode::Down => {
                self.move_selection(1);
                self.detail_scroll = 0;
            }
            KeyCode::Tab => {
                self.detail_mode = match self.detail_mode {
                    DetailMode::Request => DetailMode::Response,
                    DetailMode::Response => DetailMode::Request,
                };
                self.detail_scroll = 0;
            }
            KeyCode::PageUp => {
                if self.detail_scroll >= 10 {
                    self.detail_scroll -= 10;
                } else {
                    self.detail_scroll = 0;
                }
            }
            KeyCode::PageDown => {
                self.detail_scroll += 10;
            }
            KeyCode::Char('/') => {
                self.search_mode = SearchMode::Filtering;
                self.search_query.clear();
                self.search_cursor = 0;
            }
            KeyCode::Char('g') => {
                self.group_by_host = !self.group_by_host;
                self.list_state.select(Some(0));
                self.detail_scroll = 0;
            }
            KeyCode::Char('e') => {
                // Toggle expand/collapse for the group containing the selected item
                if self.group_by_host
                    && let Some(group) = self.group_for_selection()
                {
                    if self.collapsed_groups.contains(&group) {
                        self.collapsed_groups.remove(&group);
                    } else {
                        self.collapsed_groups.insert(group);
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_search_input(&mut self, code: KeyCode) {
        match code {
            KeyCode::Enter => {
                self.search_mode = SearchMode::None;
                self.apply_filter();
                self.list_state.select(Some(0));
                self.detail_scroll = 0;
            }
            KeyCode::Esc => {
                self.search_mode = SearchMode::None;
                self.search_query.clear();
                self.apply_filter();
                self.list_state.select(Some(0));
                self.detail_scroll = 0;
            }
            KeyCode::Backspace => {
                if self.search_cursor > 0 {
                    self.search_query.remove(self.search_cursor - 1);
                    self.search_cursor -= 1;
                    self.apply_filter();
                }
            }
            KeyCode::Left => {
                if self.search_cursor > 0 {
                    self.search_cursor -= 1;
                }
            }
            KeyCode::Right => {
                if self.search_cursor < self.search_query.len() {
                    self.search_cursor += 1;
                }
            }
            KeyCode::Char(c) => {
                self.search_query.insert(self.search_cursor, c);
                self.search_cursor += 1;
                self.apply_filter();
            }
            _ => {}
        }
    }

    fn apply_filter(&mut self) {
        if self.search_query.is_empty() {
            self.filtered_exchanges = self.exchanges.clone();
            return;
        }

        let query = self.search_query.to_lowercase();
        self.filtered_exchanges = self
            .exchanges
            .iter()
            .filter(|ex| {
                let method = ex.request.method.to_lowercase();
                let path = ex.request.path.to_lowercase();
                let host = ex.request.host.to_lowercase();
                let status = ex.status_label().to_lowercase();

                // Support key:value syntax for targeted filtering
                if query.contains(':') {
                    let parts: Vec<&str> = query.split(':').collect();
                    if parts.len() == 2 {
                        let key = parts[0].trim();
                        let val = parts[1].trim();
                        return match key {
                            "method" => method.contains(val),
                            "path" => path.contains(val),
                            "host" => host.contains(val),
                            "status" => status.contains(val),
                            _ => {
                                method.contains(&query)
                                    || path.contains(&query)
                                    || host.contains(&query)
                                    || status.contains(&query)
                            }
                        };
                    }
                }

                method.contains(&query)
                    || path.contains(&query)
                    || host.contains(&query)
                    || status.contains(&query)
            })
            .cloned()
            .collect();
    }

    fn move_selection(&mut self, delta: i32) {
        let len = self.filtered_exchanges.len() as i32;
        if len == 0 {
            return;
        }
        let current = self.list_state.selected().map_or(0, |i| i as i32);
        let next = ((current + delta).clamp(0, len - 1)) as usize;
        self.list_state.select(Some(next));
    }
}

/// Colorize a pretty-printed JSON string into ratatui Lines with styled spans.
/// Takes ownership of the string to avoid lifetime issues.
fn colorize_json_owned(json: String) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    for line in json.lines() {
        let mut spans = Vec::new();
        let mut chars = line.char_indices().peekable();

        while let Some((start, ch)) = chars.next() {
            match ch {
                '"' => {
                    // String literal
                    let mut end = start + 1;
                    while let Some((i, c)) = chars.next() {
                        end = i + c.len_utf8();
                        if c == '\\' {
                            chars.next(); // skip escaped char
                        } else if c == '"' {
                            break;
                        }
                    }
                    let s = &line[start..end];

                    // Determine if this is a key (followed by colon) or a string value
                    let is_key = line[end..].trim_start().starts_with(':');

                    if is_key {
                        spans.push(Span::styled(
                            s.to_string(),
                            Style::default().fg(Color::Cyan),
                        ));
                    } else {
                        spans.push(Span::styled(
                            s.to_string(),
                            Style::default().fg(Color::Green),
                        ));
                    }
                }
                '0'..='9' | '-' => {
                    // Number
                    let mut end = start + ch.len_utf8();
                    while let Some(&(_, c)) = chars.peek() {
                        if c.is_ascii_digit()
                            || c == '.'
                            || c == 'e'
                            || c == 'E'
                            || c == '+'
                            || c == '-'
                        {
                            end += c.len_utf8();
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    spans.push(Span::styled(
                        line[start..end].to_string(),
                        Style::default().fg(Color::Yellow),
                    ));
                }
                't' | 'f' | 'n' => {
                    // true, false, null
                    let keywords = [("true", 4), ("false", 5), ("null", 4)];
                    let mut matched = false;
                    for &(kw, len) in &keywords {
                        if line[start..].starts_with(kw) {
                            spans.push(Span::styled(
                                line[start..start + len].to_string(),
                                if kw == "null" {
                                    Style::default().fg(Color::DarkGray)
                                } else {
                                    Style::default().fg(Color::Magenta)
                                },
                            ));
                            for _ in 1..len {
                                chars.next();
                            }
                            matched = true;
                            break;
                        }
                    }
                    if !matched {
                        spans.push(Span::raw(ch.to_string()));
                    }
                }
                '{' | '}' | '[' | ']' | ':' | ',' => {
                    spans.push(Span::styled(
                        ch.to_string(),
                        Style::default().fg(Color::DarkGray),
                    ));
                }
                c if c.is_whitespace() => {
                    spans.push(Span::raw(c.to_string()));
                }
                _ => {
                    spans.push(Span::raw(ch.to_string()));
                }
            }
        }

        lines.push(Line::from(spans));
    }

    lines
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_colorize_json_simple() {
        let json = r#"{"name": "Alice", "age": 30, "active": true, "score": null}"#.to_string();
        let lines = colorize_json_owned(json);
        assert!(!lines.is_empty());
        let text = lines[0].to_string();
        assert!(text.contains("name"));
        assert!(text.contains("Alice"));
    }

    #[test]
    fn test_colorize_json_nested() {
        let json = r#"{"user": {"name": "Bob"}, "tags": ["a", "b"]}"#.to_string();
        let lines = colorize_json_owned(json);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_highlight_json_body_non_json() {
        // SqlitePool::connect_lazy requires tokio context, so test the standalone fn directly
        let body = b"plain text response";
        let lines = colorize_json_owned(String::from_utf8_lossy(body).to_string());
        assert_eq!(lines.len(), 1);
        assert!(lines[0].to_string().contains("plain text"));
    }

    #[test]
    fn test_highlight_json_body_valid_json() {
        let body = br#"{"key":"value","num":42}"#;
        let text = String::from_utf8_lossy(body);
        let trimmed = text.trim();
        let pretty = serde_json::to_string_pretty(
            &serde_json::from_str::<serde_json::Value>(trimmed).unwrap(),
        )
        .unwrap();
        let lines = colorize_json_owned(pretty);
        assert!(lines.len() > 1); // pretty-printed should be multi-line
    }
}
