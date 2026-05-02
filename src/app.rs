use std::collections::HashMap;
use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{DateTime, Utc};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table, TableState, Tabs, Wrap};
use ratatui::{Frame, Terminal};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tracing::warn;

use crate::config::Config;
use crate::dirs::Paths;
use crate::github::{fetch_issue_comments, refresh_dashboard};
use crate::model::{
    CommentPreview, ItemKind, SectionKind, SectionSnapshot, WorkItem, configured_sections,
    merge_cached_sections, merge_refreshed_sections, section_counts,
};
use crate::snapshot::SnapshotStore;

enum AppMsg {
    RefreshStarted,
    RefreshFinished {
        sections: Vec<SectionSnapshot>,
        save_error: Option<String>,
    },
    DetailsLoaded {
        item_id: String,
        result: std::result::Result<Vec<CommentPreview>, String>,
    },
}

#[derive(Debug, Clone)]
enum DetailState {
    Loading,
    Loaded(Vec<CommentPreview>),
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusTarget {
    List,
    Details,
}

struct AppState {
    active_view: SectionKind,
    sections: Vec<SectionSnapshot>,
    section_index: [usize; 3],
    selected_index: [usize; 3],
    focus: FocusTarget,
    details_scroll: u16,
    search_active: bool,
    search_query: String,
    status: String,
    refreshing: bool,
    last_refresh_request: Instant,
    details: HashMap<String, DetailState>,
}

pub async fn run(config: Config, paths: Paths, store: SnapshotStore) -> Result<()> {
    let cached = store.load_all()?;
    let sections = merge_cached_sections(configured_sections(&config), cached);
    let mut app = AppState::new(config.defaults.view, sections);

    let (tx, mut rx) = mpsc::unbounded_channel();
    start_refresh(config.clone(), store.clone(), tx.clone());

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(
        &mut terminal,
        &mut app,
        &config,
        &paths,
        &store,
        &tx,
        &mut rx,
    )
    .await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut AppState,
    config: &Config,
    paths: &Paths,
    store: &SnapshotStore,
    tx: &UnboundedSender<AppMsg>,
    rx: &mut UnboundedReceiver<AppMsg>,
) -> Result<()> {
    loop {
        while let Ok(message) = rx.try_recv() {
            app.handle_msg(message);
        }
        app.ensure_current_details_loading(tx);

        if !app.refreshing
            && config.defaults.refetch_interval_seconds > 0
            && app.last_refresh_request.elapsed().as_secs()
                >= config.defaults.refetch_interval_seconds
        {
            start_refresh(config.clone(), store.clone(), tx.clone());
        }

        terminal.draw(|frame| draw(frame, app, paths))?;

        if event::poll(Duration::from_millis(120))? {
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }

            if handle_key(app, key, config, store, tx) {
                break;
            }
        }
    }

    Ok(())
}

fn start_refresh(config: Config, store: SnapshotStore, tx: UnboundedSender<AppMsg>) {
    let _ = tx.send(AppMsg::RefreshStarted);
    tokio::spawn(async move {
        let sections = refresh_dashboard(&config).await;
        let mut save_error = None;
        for section in &sections {
            if section.error.is_some() {
                continue;
            }
            if let Err(error) = store.save_section(section) {
                let message = error.to_string();
                warn!(error = %message, "failed to save refreshed snapshot");
                save_error = Some(message);
                break;
            }
        }
        let _ = tx.send(AppMsg::RefreshFinished {
            sections,
            save_error,
        });
    });
}

fn start_details_load(item: WorkItem, tx: UnboundedSender<AppMsg>) {
    tokio::spawn(async move {
        let result = match item.number {
            Some(number) => fetch_issue_comments(&item.repo, number, 5)
                .await
                .map_err(|error| error.to_string()),
            None => Ok(Vec::new()),
        };
        let _ = tx.send(AppMsg::DetailsLoaded {
            item_id: item.id,
            result,
        });
    });
}

fn handle_key(
    app: &mut AppState,
    key: KeyEvent,
    config: &Config,
    store: &SnapshotStore,
    tx: &UnboundedSender<AppMsg>,
) -> bool {
    if app.search_active {
        match key.code {
            KeyCode::Esc => app.clear_search(),
            KeyCode::Enter => app.search_active = false,
            KeyCode::Backspace => app.pop_search_char(),
            KeyCode::Char(value) => app.push_search_char(value),
            _ => {}
        }
        return false;
    }

    if app.focus == FocusTarget::Details {
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Esc => app.focus_list(),
            KeyCode::Char('4') => app.focus_primary_list(),
            KeyCode::Char('o') => app.open_selected(),
            KeyCode::Down | KeyCode::Char('j') => app.scroll_details(1),
            KeyCode::Up | KeyCode::Char('k') => app.scroll_details(-1),
            KeyCode::PageDown | KeyCode::Char('d') => app.scroll_details(8),
            KeyCode::PageUp | KeyCode::Char('u') => app.scroll_details(-8),
            KeyCode::Char('g') => app.details_scroll = 0,
            _ => {}
        }
        return false;
    }

    match key.code {
        KeyCode::Char('q') => return true,
        KeyCode::Esc if !app.search_query.is_empty() => app.clear_search(),
        KeyCode::Esc => {}
        KeyCode::Char('/') => app.start_search(),
        KeyCode::Char('5') => app.focus_details(),
        KeyCode::Tab => app.switch_view(app.active_view.next()),
        KeyCode::BackTab => app.switch_view(app.active_view.previous()),
        KeyCode::Char('1') => app.switch_view(SectionKind::PullRequests),
        KeyCode::Char('2') => app.switch_view(SectionKind::Issues),
        KeyCode::Char('3') => app.switch_view(SectionKind::Notifications),
        KeyCode::Char('4') => app.focus_primary_list(),
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(']') => app.move_section(1),
        KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('[') => app.move_section(-1),
        KeyCode::Down | KeyCode::Char('j') => app.move_selection(1),
        KeyCode::Up | KeyCode::Char('k') => app.move_selection(-1),
        KeyCode::Char('g') => app.set_selection(0),
        KeyCode::Char('G') => app.select_last(),
        KeyCode::Char('r') => {
            if app.refreshing {
                app.status = "refresh already running".to_string();
            } else {
                start_refresh(config.clone(), store.clone(), tx.clone());
            }
        }
        KeyCode::Char('o') => app.open_selected(),
        KeyCode::Enter => app.focus_details(),
        _ => {}
    }

    false
}

fn draw(frame: &mut Frame<'_>, app: &AppState, paths: &Paths) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(2),
        ])
        .split(area);

    draw_view_tabs(frame, app, chunks[0]);
    draw_section_tabs(frame, app, chunks[1]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(64), Constraint::Percentage(36)])
        .split(chunks[2]);
    draw_table(frame, app, body[0]);
    draw_details(frame, app, body[1]);
    draw_footer(frame, app, paths, chunks[3]);
}

fn draw_view_tabs(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let views = [
        SectionKind::PullRequests,
        SectionKind::Issues,
        SectionKind::Notifications,
    ];
    let titles = views
        .iter()
        .map(|kind| Line::from(kind.label()))
        .collect::<Vec<_>>();
    let active = views
        .iter()
        .position(|kind| *kind == app.active_view)
        .unwrap_or(0);

    let tabs = Tabs::new(titles)
        .select(active)
        .block(Block::default().borders(Borders::ALL).title("ghr"))
        .style(Style::default().fg(Color::Gray))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_widget(tabs, area);
}

fn draw_section_tabs(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let sections = app.visible_sections();
    if sections.is_empty() {
        frame.render_widget(
            Paragraph::new("No sections configured").block(Block::default().borders(Borders::ALL)),
            area,
        );
        return;
    }

    let titles = sections
        .iter()
        .map(|section| {
            let (total, unread) = section_counts(section);
            let label = if !app.search_query.is_empty() {
                format!(
                    "{} ({}/{total})",
                    section.title,
                    app.filtered_indices(section).len()
                )
            } else if unread > 0 {
                format!("{} ({total}/{unread})", section.title)
            } else {
                format!("{} ({total})", section.title)
            };
            Line::from(label)
        })
        .collect::<Vec<_>>();

    let tabs = Tabs::new(titles)
        .select(app.current_section_position())
        .block(Block::default().borders(Borders::ALL).title("sections"))
        .style(Style::default().fg(Color::Gray))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_widget(tabs, area);
}

fn draw_table(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let Some(section) = app.current_section() else {
        frame.render_widget(
            Paragraph::new("No data").block(Block::default().borders(Borders::ALL)),
            area,
        );
        return;
    };

    let filtered_indices = app.filtered_indices(section);
    let rows = filtered_indices
        .iter()
        .filter_map(|index| section.items.get(*index))
        .map(|item| {
            Row::new(vec![
                relative_time(item.updated_at),
                item.repo.clone(),
                item.number
                    .map(|number| format!("#{number}"))
                    .unwrap_or_default(),
                item.title.clone(),
                item_meta(item),
            ])
        })
        .collect::<Vec<_>>();

    let header = Row::new(vec!["Updated", "Repo", "#", "Title", "Meta"])
        .style(
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        )
        .bottom_margin(1);

    let mut title = if app.search_query.is_empty() {
        section.title.clone()
    } else {
        format!(
            "{} - {} / {} matching '{}'",
            section.title,
            filtered_indices.len(),
            section.items.len(),
            app.search_query
        )
    };
    if let Some(error) = &section.error {
        title.push_str(&format!(" - error: {error}"));
    };

    let border_style = if app.focus == FocusTarget::List {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Length(24),
            Constraint::Length(8),
            Constraint::Min(20),
            Constraint::Length(18),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(title),
    )
    .row_highlight_style(
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("> ");

    let mut table_state = TableState::default();
    if !filtered_indices.is_empty() {
        table_state.select(Some(app.current_selected_position()));
    }
    frame.render_stateful_widget(table, area, &mut table_state);
}

fn draw_details(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let title = app
        .current_section()
        .map(|section| format!("details: {}", section.kind.label()))
        .unwrap_or_else(|| "details".to_string());
    let border_style = if app.focus == FocusTarget::Details {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let lines = match app.current_item() {
        Some(item) => {
            let mut lines = vec![
                Line::from(vec![Span::styled(
                    item.title.clone(),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )]),
                Line::from(""),
                Line::from(format!("repo: {}", item.repo)),
                Line::from(format!(
                    "number: {}",
                    item.number
                        .map(|number| format!("#{number}"))
                        .unwrap_or_else(|| "-".to_string())
                )),
                Line::from(format!("updated: {}", relative_time(item.updated_at))),
                Line::from(format!(
                    "author: {}",
                    item.author.clone().unwrap_or_else(|| "-".to_string())
                )),
                Line::from(format!(
                    "state: {}",
                    item.state.clone().unwrap_or_else(|| "-".to_string())
                )),
                Line::from(format!(
                    "reason: {}",
                    item.reason.clone().unwrap_or_else(|| "-".to_string())
                )),
                Line::from(format!(
                    "comments: {}",
                    item.comments
                        .map(|comments| comments.to_string())
                        .unwrap_or_else(|| "-".to_string())
                )),
                Line::from(format!("url: {}", item.url)),
            ];

            if !item.labels.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(format!("labels: {}", item.labels.join(", "))));
            }

            if let Some(extra) = &item.extra {
                lines.push(Line::from(""));
                lines.push(Line::from(format!("extra: {extra}")));
            }

            lines.push(Line::from(""));
            push_heading(&mut lines, "Description");
            push_text_block(
                &mut lines,
                item.body.as_deref().unwrap_or(""),
                "No description.",
                22,
                2_400,
            );

            if matches!(item.kind, ItemKind::Issue | ItemKind::PullRequest) {
                lines.push(Line::from(""));
                push_heading(&mut lines, "Recent Comments");
                match app.details.get(&item.id) {
                    Some(DetailState::Loading) => {
                        lines.push(Line::from("loading comments..."));
                    }
                    Some(DetailState::Loaded(comments)) if comments.is_empty() => {
                        lines.push(Line::from("No comments."));
                    }
                    Some(DetailState::Loaded(comments)) => {
                        for comment in comments {
                            push_comment(&mut lines, comment);
                        }
                    }
                    Some(DetailState::Error(error)) => {
                        lines.push(Line::from(format!("Failed to load comments: {error}")));
                    }
                    None => {
                        lines.push(Line::from("loading comments..."));
                    }
                }
            }

            lines
        }
        None => vec![Line::from("No item selected")],
    };

    let details = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(title),
        )
        .scroll((app.details_scroll, 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(details, area);
}

fn draw_footer(frame: &mut Frame<'_>, app: &AppState, paths: &Paths, area: Rect) {
    let refresh = if app.refreshing { "refreshing" } else { "idle" };
    let focus = match app.focus {
        FocusTarget::List => "list",
        FocusTarget::Details => "details",
    };
    let search = if app.search_active {
        format!("search: /{}_", app.search_query)
    } else if app.search_query.is_empty() {
        "/ search".to_string()
    } else {
        format!("filter: /{}", app.search_query)
    };
    let text = format!(
        "tab/1-3 view  4 list  h/l section  j/k move  enter/5 details  esc list  / search  o open  q quit | focus {focus} | {search} | {refresh} | {} | db {}",
        app.status,
        paths.db_path.display()
    );
    let footer = Paragraph::new(text).style(Style::default().fg(Color::Gray));
    frame.render_widget(footer, area);
}

fn push_heading(lines: &mut Vec<Line<'static>>, text: &str) {
    lines.push(Line::from(Span::styled(
        text.to_string(),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )));
}

fn push_text_block(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    empty_message: &str,
    max_lines: usize,
    max_chars: usize,
) {
    let text = truncate_text(&normalize_text(text), max_chars);
    if text.trim().is_empty() {
        lines.push(Line::from(empty_message.to_string()));
        return;
    }

    for (emitted, line) in text.lines().enumerate() {
        if emitted >= max_lines {
            lines.push(Line::from("..."));
            return;
        }
        lines.push(Line::from(line.to_string()));
    }
}

fn push_comment(lines: &mut Vec<Line<'static>>, comment: &CommentPreview) {
    let timestamp = comment
        .updated_at
        .as_ref()
        .or(comment.created_at.as_ref())
        .cloned();
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            comment.author.clone(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" - {}", relative_time(timestamp))),
    ]));
    push_text_block(lines, &comment.body, "No comment body.", 6, 900);
}

fn normalize_text(text: &str) -> String {
    text.replace('\r', "")
        .replace('\t', "    ")
        .trim()
        .to_string()
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    let mut truncated = text.chars().take(max_chars).collect::<String>();
    truncated.push_str("\n...");
    truncated
}

impl AppState {
    fn new(active_view: SectionKind, sections: Vec<SectionSnapshot>) -> Self {
        Self {
            active_view,
            sections,
            section_index: [0; 3],
            selected_index: [0; 3],
            focus: FocusTarget::List,
            details_scroll: 0,
            search_active: false,
            search_query: String::new(),
            status: "loading snapshot; background refresh started".to_string(),
            refreshing: false,
            last_refresh_request: Instant::now(),
            details: HashMap::new(),
        }
    }

    fn handle_msg(&mut self, message: AppMsg) {
        match message {
            AppMsg::RefreshStarted => {
                self.refreshing = true;
                self.last_refresh_request = Instant::now();
                self.status = "refreshing from GitHub".to_string();
            }
            AppMsg::RefreshFinished {
                sections,
                save_error,
            } => {
                let errors = sections
                    .iter()
                    .filter(|section| section.error.is_some())
                    .count();
                let current = std::mem::take(&mut self.sections);
                self.sections = merge_refreshed_sections(current, sections);
                self.details_scroll = 0;
                self.clamp_positions();
                self.refreshing = false;
                self.status = match (errors, save_error) {
                    (0, None) => "refresh complete".to_string(),
                    (count, None) => format!("refresh complete with {count} failed section(s)"),
                    (_, Some(error)) => format!("snapshot save failed: {error}"),
                };
            }
            AppMsg::DetailsLoaded { item_id, result } => match result {
                Ok(comments) => {
                    self.details.insert(item_id, DetailState::Loaded(comments));
                }
                Err(error) => {
                    self.details.insert(item_id, DetailState::Error(error));
                }
            },
        }
    }

    fn ensure_current_details_loading(&mut self, tx: &UnboundedSender<AppMsg>) {
        let Some(item) = self.current_item().cloned() else {
            return;
        };
        if !matches!(item.kind, ItemKind::Issue | ItemKind::PullRequest) || item.number.is_none() {
            return;
        }
        if self.details.contains_key(&item.id) {
            return;
        }

        self.details.insert(item.id.clone(), DetailState::Loading);
        start_details_load(item, tx.clone());
    }

    fn switch_view(&mut self, view: SectionKind) {
        self.active_view = view;
        self.focus = FocusTarget::List;
        self.details_scroll = 0;
        self.clamp_positions();
    }

    fn focus_primary_list(&mut self) {
        self.focus = FocusTarget::List;
        self.details_scroll = 0;
        self.search_active = false;
        self.status = "list focused".to_string();
        self.clamp_positions();
    }

    fn move_section(&mut self, delta: isize) {
        let len = self.visible_sections().len();
        if len == 0 {
            return;
        }
        let slot = kind_slot(self.active_view);
        let current = self.section_index[slot].min(len - 1);
        self.section_index[slot] = move_bounded(current, len, delta);
        self.selected_index[slot] = 0;
        self.focus = FocusTarget::List;
        self.details_scroll = 0;
    }

    fn move_selection(&mut self, delta: isize) {
        let Some(section) = self.current_section() else {
            return;
        };
        let len = self.filtered_indices(section).len();
        if len == 0 {
            return;
        }
        let slot = kind_slot(self.active_view);
        let current = self.selected_index[slot].min(len - 1);
        self.selected_index[slot] = move_bounded(current, len, delta);
        self.details_scroll = 0;
    }

    fn set_selection(&mut self, index: usize) {
        let slot = kind_slot(self.active_view);
        self.selected_index[slot] = index;
        self.details_scroll = 0;
        self.clamp_positions();
    }

    fn select_last(&mut self) {
        let Some(section) = self.current_section() else {
            return;
        };
        let len = self.filtered_indices(section).len();
        if len > 0 {
            let slot = kind_slot(self.active_view);
            self.selected_index[slot] = len - 1;
            self.details_scroll = 0;
        }
    }

    fn focus_details(&mut self) {
        if self.current_item().is_some() {
            self.focus = FocusTarget::Details;
            self.search_active = false;
            self.status = "details focused".to_string();
        } else {
            self.status = "nothing to focus".to_string();
        }
    }

    fn focus_list(&mut self) {
        self.focus = FocusTarget::List;
        self.status = "list focused".to_string();
    }

    fn scroll_details(&mut self, delta: i16) {
        if delta < 0 {
            self.details_scroll = self.details_scroll.saturating_sub(delta.unsigned_abs());
        } else {
            self.details_scroll = self.details_scroll.saturating_add(delta as u16);
        }
    }

    fn start_search(&mut self) {
        self.focus = FocusTarget::List;
        self.search_active = true;
        self.status = "search mode".to_string();
        self.clamp_positions();
    }

    fn clear_search(&mut self) {
        self.search_active = false;
        self.search_query.clear();
        self.focus = FocusTarget::List;
        self.details_scroll = 0;
        self.status = "search cleared".to_string();
        self.clamp_positions();
    }

    fn push_search_char(&mut self, value: char) {
        self.search_query.push(value);
        self.selected_index[kind_slot(self.active_view)] = 0;
        self.details_scroll = 0;
        self.clamp_positions();
    }

    fn pop_search_char(&mut self) {
        self.search_query.pop();
        self.selected_index[kind_slot(self.active_view)] = 0;
        self.details_scroll = 0;
        self.clamp_positions();
    }

    fn open_selected(&mut self) {
        let Some(item) = self.current_item() else {
            self.status = "nothing to open".to_string();
            return;
        };

        match open::that(&item.url) {
            Ok(_) => {
                self.status = format!("opened {}", item.url);
            }
            Err(error) => {
                self.status = format!("open failed: {error}");
            }
        }
    }

    fn visible_sections(&self) -> Vec<&SectionSnapshot> {
        self.sections
            .iter()
            .filter(|section| section.kind == self.active_view)
            .collect()
    }

    fn current_section(&self) -> Option<&SectionSnapshot> {
        let sections = self.visible_sections();
        if sections.is_empty() {
            return None;
        }
        sections
            .get(self.current_section_position().min(sections.len() - 1))
            .copied()
    }

    fn current_item(&self) -> Option<&WorkItem> {
        let section = self.current_section()?;
        let filtered_indices = self.filtered_indices(section);
        let item_index = filtered_indices.get(self.current_selected_position())?;
        section.items.get(*item_index)
    }

    fn current_section_position(&self) -> usize {
        self.section_index[kind_slot(self.active_view)]
    }

    fn current_selected_position(&self) -> usize {
        self.selected_index[kind_slot(self.active_view)]
    }

    fn clamp_positions(&mut self) {
        for kind in [
            SectionKind::PullRequests,
            SectionKind::Issues,
            SectionKind::Notifications,
        ] {
            let slot = kind_slot(kind);
            let section_count = self
                .sections
                .iter()
                .filter(|section| section.kind == kind)
                .count();
            if section_count == 0 {
                self.section_index[slot] = 0;
                self.selected_index[slot] = 0;
                continue;
            }

            self.section_index[slot] = self.section_index[slot].min(section_count - 1);
            let item_count = self
                .sections
                .iter()
                .filter(|section| section.kind == kind)
                .nth(self.section_index[slot])
                .map(|section| self.filtered_indices(section).len())
                .unwrap_or(0);
            if item_count == 0 {
                self.selected_index[slot] = 0;
            } else {
                self.selected_index[slot] = self.selected_index[slot].min(item_count - 1);
            }
        }
    }

    fn filtered_indices(&self, section: &SectionSnapshot) -> Vec<usize> {
        filtered_indices(section, &self.search_query)
    }
}

fn kind_slot(kind: SectionKind) -> usize {
    match kind {
        SectionKind::PullRequests => 0,
        SectionKind::Issues => 1,
        SectionKind::Notifications => 2,
    }
}

fn filtered_indices(section: &SectionSnapshot, query: &str) -> Vec<usize> {
    let query = query.trim();
    if query.is_empty() {
        return (0..section.items.len()).collect();
    }

    let mut scored = section
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| fuzzy_score_item(item, query).map(|score| (index, score)))
        .collect::<Vec<_>>();
    scored.sort_by(|(left_index, left_score), (right_index, right_score)| {
        right_score
            .cmp(left_score)
            .then_with(|| left_index.cmp(right_index))
    });
    scored.into_iter().map(|(index, _)| index).collect()
}

fn fuzzy_score_item(item: &WorkItem, query: &str) -> Option<i64> {
    let haystack = searchable_text(item);
    let mut total = 0;
    for token in query.split_whitespace() {
        total += fuzzy_score(token, &haystack)?;
    }
    Some(total)
}

fn searchable_text(item: &WorkItem) -> String {
    let mut parts = vec![item.repo.clone(), item.title.clone(), item.url.clone()];
    if let Some(number) = item.number {
        parts.push(format!("#{number}"));
        parts.push(number.to_string());
    }
    if let Some(author) = &item.author {
        parts.push(author.clone());
    }
    if let Some(state) = &item.state {
        parts.push(state.clone());
    }
    if let Some(reason) = &item.reason {
        parts.push(reason.clone());
    }
    if let Some(extra) = &item.extra {
        parts.push(extra.clone());
    }
    if let Some(body) = &item.body {
        parts.push(body.clone());
    }
    parts.extend(item.labels.iter().cloned());
    parts.join(" ").to_lowercase()
}

fn fuzzy_score(query: &str, haystack: &str) -> Option<i64> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    if let Some(index) = haystack.find(&query) {
        return Some(10_000 - index as i64);
    }

    let mut score = 0;
    let mut search_start = 0;
    let mut previous_match = None;
    for needle in query.chars() {
        let mut matched = None;
        for (offset, candidate) in haystack[search_start..].char_indices() {
            if candidate == needle {
                matched = Some(search_start + offset);
                break;
            }
        }

        let index = matched?;
        score += 100;
        if let Some(previous) = previous_match {
            let gap = index.saturating_sub(previous + 1);
            if gap > 32 {
                return None;
            }
            if gap == 0 {
                score += 30;
            } else {
                score -= gap.min(30) as i64;
            }
        } else {
            score -= index.min(50) as i64;
        }
        if index == 0 || haystack[..index].ends_with([' ', '/', '#', '-', '_']) {
            score += 20;
        }

        previous_match = Some(index);
        search_start = index + needle.len_utf8();
    }

    Some(score)
}

fn move_bounded(current: usize, len: usize, delta: isize) -> usize {
    let next = current as isize + delta;
    next.clamp(0, len.saturating_sub(1) as isize) as usize
}

fn relative_time(value: Option<DateTime<Utc>>) -> String {
    let Some(value) = value else {
        return "-".to_string();
    };
    let delta = Utc::now().signed_duration_since(value);
    if delta.num_minutes() < 1 {
        "now".to_string()
    } else if delta.num_hours() < 1 {
        format!("{}m", delta.num_minutes())
    } else if delta.num_days() < 1 {
        format!("{}h", delta.num_hours())
    } else if delta.num_days() < 30 {
        format!("{}d", delta.num_days())
    } else {
        format!("{}mo", delta.num_days() / 30)
    }
}

fn item_meta(item: &WorkItem) -> String {
    let mut parts = Vec::new();
    if item.unread.unwrap_or(false) {
        parts.push("unread".to_string());
    }
    if let Some(reason) = &item.reason {
        parts.push(reason.clone());
    }
    if let Some(state) = &item.state {
        parts.push(state.clone());
    }
    if let Some(comments) = item.comments {
        parts.push(format!("{comments}c"));
    }
    if let Some(extra) = &item.extra {
        parts.push(extra.clone());
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_score_matches_ordered_subsequence() {
        assert!(fuzzy_score("frc", "fix rust closure").is_some());
        assert!(fuzzy_score("zz", "fix rust closure").is_none());
    }

    #[test]
    fn filtered_indices_match_multiple_fields_and_rank_substrings() {
        let section = SectionSnapshot {
            key: "pull_requests:test".to_string(),
            kind: SectionKind::PullRequests,
            title: "Test".to_string(),
            filters: String::new(),
            items: vec![
                work_item("1", "rust-lang/rust", 1, "Fix closure span", Some("alice")),
                work_item(
                    "2",
                    "nervosnetwork/fiber",
                    2,
                    "Channel persistence",
                    Some("bob"),
                ),
                work_item("3", "nervosnetwork/ckb", 3, "RPC docs", Some("carol")),
            ],
            refreshed_at: None,
            error: None,
        };

        assert_eq!(filtered_indices(&section, "fiber"), vec![1]);
        assert_eq!(filtered_indices(&section, "alice closure"), vec![0]);
        assert!(filtered_indices(&section, "not-present").is_empty());
    }

    #[test]
    fn current_item_uses_filtered_selection() {
        let section = SectionSnapshot {
            key: "issues:test".to_string(),
            kind: SectionKind::Issues,
            title: "Test".to_string(),
            filters: String::new(),
            items: vec![
                work_item("1", "rust-lang/rust", 1, "Compiler diagnostics", None),
                work_item("2", "nervosnetwork/fiber", 2, "Funding state", None),
            ],
            refreshed_at: None,
            error: None,
        };
        let mut app = AppState::new(SectionKind::Issues, vec![section]);

        app.search_query = "fiber".to_string();
        app.clamp_positions();

        assert_eq!(app.current_item().map(|item| item.id.as_str()), Some("2"));
    }

    #[test]
    fn enter_and_five_focus_details_without_quitting() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Enter),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.focus, FocusTarget::Details);

        app.focus_list();
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('5')),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.focus, FocusTarget::Details);
    }

    #[test]
    fn escape_returns_from_details_to_list() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        app.focus_details();
        app.scroll_details(5);
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Esc),
            &config,
            &store,
            &tx
        ));

        assert_eq!(app.focus, FocusTarget::List);
        assert_eq!(app.details_scroll, 5);
    }

    #[test]
    fn details_scroll_does_not_move_list_selection() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        app.focus_details();
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('j')),
            &config,
            &store,
            &tx
        ));

        assert_eq!(app.current_selected_position(), 0);
        assert_eq!(app.details_scroll, 1);
    }

    #[test]
    fn four_focuses_primary_list_without_changing_page() {
        let sections = vec![
            SectionSnapshot {
                key: "pull_requests:My Pull Requests".to_string(),
                kind: SectionKind::PullRequests,
                title: "My Pull Requests".to_string(),
                filters: String::new(),
                items: vec![work_item(
                    "1",
                    "rust-lang/rust",
                    1,
                    "Compiler diagnostics",
                    None,
                )],
                refreshed_at: None,
                error: None,
            },
            SectionSnapshot {
                key: "pull_requests:Assigned to Me".to_string(),
                kind: SectionKind::PullRequests,
                title: "Assigned to Me".to_string(),
                filters: String::new(),
                items: vec![work_item(
                    "2",
                    "nervosnetwork/fiber",
                    2,
                    "Funding state",
                    None,
                )],
                refreshed_at: None,
                error: None,
            },
            SectionSnapshot {
                key: "issues:Test".to_string(),
                kind: SectionKind::Issues,
                title: "Test".to_string(),
                filters: String::new(),
                items: vec![work_item("3", "nervosnetwork/ckb", 3, "Issue", None)],
                refreshed_at: None,
                error: None,
            },
        ];
        let mut app = AppState::new(SectionKind::Issues, sections);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        app.focus_details();
        app.scroll_details(3);
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('4')),
            &config,
            &store,
            &tx
        ));

        assert_eq!(app.active_view, SectionKind::Issues);
        assert_eq!(app.current_section_position(), 0);
        assert_eq!(app.current_selected_position(), 0);
        assert_eq!(app.focus, FocusTarget::List);
        assert_eq!(app.details_scroll, 0);
        assert_eq!(
            app.current_section().map(|section| section.title.as_str()),
            Some("Test")
        );
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    fn test_section() -> SectionSnapshot {
        SectionSnapshot {
            key: "pull_requests:test".to_string(),
            kind: SectionKind::PullRequests,
            title: "Test".to_string(),
            filters: String::new(),
            items: vec![
                work_item("1", "rust-lang/rust", 1, "Compiler diagnostics", None),
                work_item("2", "nervosnetwork/fiber", 2, "Funding state", None),
            ],
            refreshed_at: None,
            error: None,
        }
    }

    fn work_item(id: &str, repo: &str, number: u64, title: &str, author: Option<&str>) -> WorkItem {
        WorkItem {
            id: id.to_string(),
            kind: ItemKind::PullRequest,
            repo: repo.to_string(),
            number: Some(number),
            title: title.to_string(),
            body: Some("A body with useful context".to_string()),
            author: author.map(str::to_string),
            state: Some("open".to_string()),
            url: format!("https://github.com/{repo}/pull/{number}"),
            updated_at: None,
            labels: vec!["T-compiler".to_string()],
            comments: Some(0),
            unread: None,
            reason: None,
            extra: None,
        }
    }
}
