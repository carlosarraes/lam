use super::draw::{key, ACCENT, BOLD, DIM, META, RULE, SELECTION};
use super::{Action, App, Mode};
use crate::client::{Article, ArticlePage};
use chrono::{DateTime, NaiveDate, Utc};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

pub(super) struct ArticlesPane {
    pub items: Vec<Article>,
    pub selected: usize,
    pub cursor: Option<String>,
    pub loaded: bool,
    pub loading: bool,
    pub query: String,
    pub read: usize,
    pub day: Option<NaiveDate>,
    applied_query: String,
    applied_read: usize,
    requested_query: String,
    requested_read: usize,
    pub invalidated: bool,
    pub generation: u64,
    pub error: Option<String>,
}

fn today_at(now: DateTime<Utc>) -> NaiveDate {
    now.with_timezone(&chrono_tz::America::Sao_Paulo)
        .date_naive()
}

fn today() -> NaiveDate {
    today_at(Utc::now())
}

fn day_label(day: Option<NaiveDate>, today: NaiveDate) -> String {
    match day {
        None => "All dates".into(),
        Some(day) if day == today => format!("Today · {day}"),
        Some(day) if Some(day) == today.pred_opt() => format!("Yesterday · {day}"),
        Some(day) => day.format("%A · %Y-%m-%d").to_string(),
    }
}

fn creation_time(value: &str) -> String {
    DateTime::parse_from_rfc3339(value)
        .map(|time| {
            time.with_timezone(&chrono_tz::America::Sao_Paulo)
                .format("%Y-%m-%d %H:%M %:z")
                .to_string()
        })
        .unwrap_or_else(|_| format!("{value} (UTC)"))
}

impl Default for ArticlesPane {
    fn default() -> Self {
        Self {
            items: vec![],
            selected: 0,
            cursor: None,
            loaded: false,
            loading: false,
            query: String::new(),
            read: 0,
            day: Some(today()),
            applied_query: String::new(),
            applied_read: 0,
            requested_query: String::new(),
            requested_read: 0,
            invalidated: false,
            generation: 0,
            error: None,
        }
    }
}

impl ArticlesPane {
    pub fn read_filter(&self) -> &'static str {
        ["all", "unread", "read"][self.read]
    }

    pub fn load(&mut self, reset: bool) -> Option<Action> {
        if reset {
            self.generation += 1;
            self.cursor = None;
            self.loaded = false;
            self.loading = false;
        }
        if self.loading || (self.loaded && self.cursor.is_none()) {
            return None;
        }
        self.loading = true;
        self.error = None;
        self.requested_query = self.query.clone();
        self.requested_read = self.read;
        Some(Action::LoadArticles {
            query: self.query.clone(),
            read: self.read_filter().into(),
            day: self.day.map(|date| date.to_string()),
            cursor: self.cursor.clone(),
            generation: self.generation,
        })
    }

    fn set_day(&mut self, day: Option<NaiveDate>) -> Option<Action> {
        if self.day != day {
            self.items.clear();
            self.selected = 0;
            self.day = day;
        }
        self.load(true)
    }

    pub fn reload(&mut self) -> Option<Action> {
        // Reconnect must not submit text that is still being edited.
        let query = std::mem::replace(&mut self.query, self.requested_query.clone());
        let read = std::mem::replace(&mut self.read, self.requested_read);
        let action = self.load(true);
        self.query = query;
        self.read = read;
        action
    }

    pub fn add(&mut self, page: ArticlePage, generation: u64, first: bool) {
        if generation != self.generation {
            return;
        }
        self.applied_query = self.requested_query.clone();
        self.applied_read = self.requested_read;
        let selected = self.items.get(self.selected).map(|row| row.id.clone());
        if first {
            // A delayed list cannot roll back an explicit read/unread result.
            let mut items = page.items;
            for row in &mut items {
                if let Some(newer) = self
                    .items
                    .iter()
                    .find(|old| old.id == row.id && old.version > row.version)
                {
                    *row = newer.clone();
                }
            }
            self.items = items;
        } else {
            for article in page.items {
                if let Some(old) = self.items.iter_mut().find(|row| row.id == article.id) {
                    if old.version <= article.version {
                        *old = article;
                    }
                } else {
                    self.items.push(article);
                }
            }
        }
        self.cursor = page.next_cursor;
        self.items.retain(|row| {
            self.applied_read == 0 || (self.applied_read == 1) == row.read_at.is_none()
        });
        self.loaded = true;
        self.loading = false;
        self.error = None;
        self.selected = selected
            .and_then(|id| self.items.iter().position(|row| row.id == id))
            .unwrap_or(self.selected)
            .min(self.items.len().saturating_sub(1));
    }

    pub fn reconcile(&mut self, article: Article) {
        if let Some(row) = self
            .items
            .iter_mut()
            .find(|row| row.id == article.id && row.version <= article.version)
        {
            *row = article;
        }
        self.items.retain(|row| {
            self.applied_read == 0 || (self.applied_read == 1) == row.read_at.is_none()
        });
        self.selected = self.selected.min(self.items.len().saturating_sub(1));
    }
}

impl App {
    pub(super) fn handle_articles(&mut self, key: KeyEvent) -> Option<Action> {
        if let Mode::ArticleDate(input) = &mut self.mode {
            match key.code {
                KeyCode::Esc => {
                    self.mode = Mode::Normal;
                    self.articles.error = None;
                }
                KeyCode::Enter => {
                    match crate::articles::parse_day(input)
                        .ok()
                        .filter(|date| *date <= today())
                    {
                        Some(date) => {
                            self.mode = Mode::Normal;
                            return self.articles.set_day(Some(date));
                        }
                        None => {
                            self.articles.error = Some("Use YYYY-MM-DD, today or earlier.".into())
                        }
                    }
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) if c.is_ascii_digit() || c == '-' => input.push(c),
                _ => {}
            }
            return None;
        }
        if matches!(self.mode, Mode::Filter) {
            match key.code {
                KeyCode::Esc => {
                    self.articles.query.clear();
                    self.mode = Mode::Normal;
                    return self.articles.load(true);
                }
                KeyCode::Enter => {
                    self.mode = Mode::Normal;
                    return self.articles.load(true);
                }
                KeyCode::Backspace => {
                    self.articles.query.pop();
                }
                KeyCode::Char(c) => self.articles.query.push(c),
                _ => {}
            }
            return None;
        }
        match key.code {
            KeyCode::Char('[') => self
                .articles
                .day
                .unwrap_or_else(today)
                .pred_opt()
                .and_then(|date| self.articles.set_day(Some(date))),
            KeyCode::Char(']') => self
                .articles
                .day
                .and_then(|date| date.succ_opt())
                .filter(|date| *date <= today())
                .and_then(|date| self.articles.set_day(Some(date))),
            KeyCode::Char('t') => self.articles.set_day(Some(today())),
            KeyCode::Char('D') => {
                self.mode = Mode::ArticleDate(String::new());
                None
            }
            KeyCode::Char('U') => {
                self.articles.read = 1;
                self.articles.set_day(None)
            }
            KeyCode::Char('/') => {
                self.mode = Mode::Filter;
                None
            }
            KeyCode::Char('R') => self.articles.load(true),
            KeyCode::Char('f') => {
                self.articles.read = (self.articles.read + 1) % 3;
                self.articles.load(true)
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.articles.selected =
                    (self.articles.selected + 1).min(self.articles.items.len().saturating_sub(1));
                if self.articles.selected + 3 >= self.articles.items.len() {
                    self.articles.load(false)
                } else {
                    None
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.articles.selected = self.articles.selected.saturating_sub(1);
                None
            }
            KeyCode::Char('m' | 'o') | KeyCode::Enter => self
                .articles
                .items
                .get(self.articles.selected)
                .map(|row| Action::OpenArticle(row.id.clone())),
            KeyCode::Char('u') => {
                self.articles
                    .items
                    .get(self.articles.selected)
                    .map(|row| Action::ArticleUnread {
                        id: row.id.clone(),
                        version: row.version,
                    })
            }
            _ => None,
        }
    }

    pub(super) fn draw_articles(&self, frame: &mut Frame) {
        let [header, days, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(3),
        ])
        .areas(frame.area());
        let list_h =
            (self.articles.items.len() as u16 + 1).clamp(3, (frame.area().height / 3).max(3));
        let [list, detail] =
            Layout::vertical([Constraint::Length(list_h), Constraint::Min(3)]).areas(body);
        self.draw_header(
            frame,
            header,
            vec![
                Span::styled("lam", BOLD),
                Span::styled(format!("  {} articles", self.articles.items.len()), META),
                Span::styled(
                    if self.articles.loading {
                        "  loading…"
                    } else {
                        ""
                    },
                    ACCENT,
                ),
            ],
        );
        let newer_style = if self.articles.day.is_some_and(|day| day < today()) {
            META
        } else {
            DIM
        };
        let label = Span::styled(day_label(self.articles.day, today()), BOLD);
        let mut navigation = if days.width < 60 {
            vec![
                Span::styled("[ ", META),
                label,
                Span::styled(" ]", newer_style),
            ]
        } else {
            vec![
                label,
                Span::styled("  [ older  ", META),
                Span::styled("] newer", newer_style),
            ]
        };
        if days.width >= 80 {
            navigation.push(Span::styled("  · America/Sao_Paulo", DIM));
        }
        frame.render_widget(Paragraph::new(Line::from(navigation)), days);
        let rows: Vec<ListItem> = self
            .articles
            .items
            .iter()
            .map(|row| {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        if row.read_at.is_some() {
                            "read    "
                        } else {
                            "unread  "
                        },
                        if row.read_at.is_some() { DIM } else { ACCENT },
                    ),
                    Span::styled(format!("{}  ", row.name), META),
                    Span::raw(row.title.clone()),
                ]))
            })
            .collect();
        let mut state = ListState::default().with_selected(Some(self.articles.selected));
        frame.render_stateful_widget(
            List::new(rows)
                .block(Block::default().borders(Borders::TOP).border_style(RULE))
                .highlight_style(Style::default().bg(SELECTION)),
            list,
            &mut state,
        );
        let mut lines = vec![Line::from(Span::styled(
            format!(
                "Filter: {}  /{}{}",
                ["all", "unread", "read"][self.articles.applied_read],
                self.articles.applied_query,
                if self.articles.query != self.articles.applied_query
                    || self.articles.read != self.articles.applied_read
                {
                    format!(
                        " · pending: {} /{}",
                        self.articles.read_filter(),
                        self.articles.query
                    )
                } else {
                    String::new()
                },
            ),
            DIM,
        ))];
        if let Some(error) = &self.articles.error {
            lines.insert(0, Line::from(Span::styled("Press R to retry", META)));
            lines.insert(0, Line::from(Span::styled(error.clone(), ACCENT)));
        }
        match self.articles.items.get(self.articles.selected) {
            Some(row) => {
                lines.push(Line::from(Span::styled(
                    format!(
                        "{} · {} · {} · {} · {}",
                        row.name,
                        row.source_host,
                        row.source_project,
                        creation_time(&row.created_at),
                        if row.read_at.is_some() {
                            "read"
                        } else {
                            "unread"
                        },
                    ),
                    META,
                )));
                lines.push(Line::from(Span::styled(row.title.clone(), BOLD)));
                lines.extend(row.summary.lines().map(|line| Line::raw(line.to_string())));
                let attachments = row
                    .assets
                    .iter()
                    .filter(|asset| {
                        matches!(
                            asset.disposition,
                            crate::client::ArticleAssetDisposition::Attachment
                        )
                    })
                    .count();
                lines.push(Line::from(Span::styled(
                    format!("{attachments} attachments"),
                    META,
                )));
            }
            None => lines.push(Line::from(Span::styled(
                if self.articles.loading {
                    "Loading articles…"
                } else if self.articles.error.is_some() {
                    "Articles could not be loaded. Press R to retry."
                } else {
                    "No articles match this filter."
                },
                META,
            ))),
        }
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::TOP).border_style(RULE)),
            detail,
        );
        let footer_text = if let Mode::ArticleDate(input) = &self.mode {
            vec![
                Line::from(vec![
                    Span::styled("date YYYY-MM-DD› ", ACCENT),
                    Span::raw(input.clone()),
                    Span::styled("█", ACCENT),
                ]),
                Line::from([key("Enter", "jump"), key("Esc", "cancel")].concat()),
            ]
        } else if matches!(self.mode, Mode::Filter) {
            vec![
                Line::from(vec![
                    Span::styled("filter› ", ACCENT),
                    Span::raw(self.articles.query.clone()),
                    Span::styled("█", ACCENT),
                ]),
                Line::from([key("Enter", "apply"), key("Esc", "clear")].concat()),
            ]
        } else {
            vec![
                Line::from(
                    [
                        key("Enter", "open"),
                        key("u", "unread"),
                        key("f", "all/unread/read"),
                    ]
                    .concat(),
                ),
                Line::from(Span::styled(
                    format!(
                        "h/l prev/next tab · j/k move · / filter · R refresh · q quit{}",
                        if self.articles.loading {
                            " · loading"
                        } else if self.articles.cursor.is_some() {
                            " · more below"
                        } else {
                            " · end"
                        },
                    ),
                    DIM,
                )),
                Line::from(
                    [
                        key("[/]", "day"),
                        key("t", "Today"),
                        key("D", "jump date"),
                        key("U", "All unread"),
                    ]
                    .concat(),
                ),
            ]
        };
        frame.render_widget(Paragraph::new(footer_text), footer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn article(id: &str, read: bool, version: u64) -> Article {
        Article {
            id: id.into(),
            title: format!("Report {id}"),
            summary: "Acceptance summary".into(),
            name: "Agent".into(),
            source_host: "local-host".into(),
            source_project: "project".into(),
            created_at: "2026-09-08".into(),
            read_at: read.then(|| "2026-09-08".into()),
            version,
            assets: vec![],
        }
    }

    fn page(items: Vec<Article>, cursor: Option<&str>) -> ArticlePage {
        ArticlePage {
            items,
            next_cursor: cursor.map(str::to_owned),
        }
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::from(KeyCode::Char(c))
    }

    fn render(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(130, 24)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn articles_share_request_header_and_selection_styling_below_day_navigation() {
        for width in [40, 80, 130] {
            let mut app = App::new("host".into());
            app.status = "live".into();
            app.set_items(vec![super::super::tests::item("req", "open", &[], "")]);
            let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
            terminal.draw(|frame| app.draw(frame)).unwrap();
            let requests = terminal.backend().buffer().clone();
            app.handle(key('h'));
            app.articles
                .add(page(vec![article("one", false, 4)], None), 0, true);
            terminal.draw(|frame| app.draw(frame)).unwrap();
            let articles = terminal.backend().buffer();
            for x in width.saturating_sub(12)..width {
                assert_eq!(
                    articles[(x, 0)],
                    requests[(x, 0)],
                    "host/status at width {width}"
                );
            }
            assert_eq!(
                articles[(0, 2)].symbol(),
                requests[(0, 1)].symbol(),
                "list keeps the shared rule below the day row"
            );
            assert_eq!(
                articles[(0, 3)].bg,
                requests[(0, 2)].bg,
                "same selection highlight"
            );
            assert_eq!(
                articles[(0, 5)].symbol(),
                requests[(0, 4)].symbol(),
                "detail keeps the shared rule"
            );
            if width == 130 {
                let header: String = (0..width).map(|x| articles[(x, 0)].symbol()).collect();
                assert!(header.contains("[articles]"));
                assert!(header.contains("● live"));
            }
        }
    }

    #[test]
    fn day_navigation_remains_above_empty_and_failed_lists_in_small_terminals() {
        for (width, height) in [(30, 8), (40, 10), (80, 24)] {
            for empty in [true, false] {
                let mut app = App::new("host".into());
                app.handle(key('h'));
                app.articles.day = Some("2026-09-08".parse().unwrap());
                let items = if empty {
                    vec![]
                } else {
                    vec![article("one", false, 0)]
                };
                app.articles.add(page(items, None), 0, true);
                app.articles.error =
                    Some("A long error message that fills the detail pane ".repeat(8));
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal.draw(|frame| app.draw(frame)).unwrap();
                let buffer = terminal.backend().buffer();
                let day_row: String = (0..width).map(|x| buffer[(x, 1)].symbol()).collect();
                assert!(
                    day_row.contains("2026-09-08"),
                    "{width}x{height}: {day_row}"
                );
                assert!(
                    day_row.contains('[') && day_row.contains(']'),
                    "{width}x{height}: {day_row}"
                );
            }
        }
    }

    #[test]
    fn selected_article_creation_time_uses_sao_paulo_calendar_day() {
        let mut app = App::new("host".into());
        app.handle(key('h'));
        let mut row = article("one", false, 0);
        row.created_at = "2026-09-09T02:15:00.000Z".into();
        app.articles.add(page(vec![row], None), 0, true);
        let rendered = render(&app);
        assert!(rendered.contains("2026-09-08 23:15 -03:00"));
        assert!(!rendered.contains("2026-09-09T02:15"));
    }

    #[test]
    fn article_error_stays_visible_above_a_long_summary() {
        let mut app = App::new("host".into());
        app.handle(key('h'));
        let mut row = article("one", false, 1);
        row.summary = "Long summary line\n".repeat(40);
        app.articles.add(page(vec![row], None), 0, true);
        app.articles.error = Some("Article open failed".into());
        let text = render(&app);
        assert!(text.contains("Article open failed"));
        assert!(text.contains("Press R to retry"));
    }

    #[test]
    fn day_navigation_replaces_rows_and_rejects_old_pages() {
        let mut app = App::new("host".into());
        app.handle(key('h'));
        app.articles.add(
            page(vec![article("old", false, 0)], Some("old-cursor")),
            0,
            true,
        );
        assert!(matches!(
            app.handle(key('[')),
            Some(Action::LoadArticles {
                cursor: None,
                generation: 1,
                ..
            })
        ));
        assert!(app.articles.items.is_empty());
        app.articles
            .add(page(vec![article("late", false, 0)], None), 0, false);
        assert!(app.articles.items.is_empty());
        assert!(render(&app).contains("Yesterday"));
        app.handle(key('t'));
        assert_eq!(app.handle(key(']')), None);
        assert!(render(&app).contains("Today"));
    }

    #[test]
    fn calendar_day_uses_sao_paulo_midnight_and_year_boundaries() {
        for (instant, expected) in [
            ("2026-01-01T02:59:59Z", "2025-12-31"),
            ("2026-01-01T03:00:00Z", "2026-01-01"),
        ] {
            let now = instant.parse::<DateTime<Utc>>().unwrap();
            assert_eq!(today_at(now).to_string(), expected);
        }
        let today = "2026-01-01".parse::<NaiveDate>().unwrap();
        assert_eq!(day_label(today.pred_opt(), today), "Yesterday · 2025-12-31");
        assert_eq!(
            day_label(Some("2025-12-30".parse().unwrap()), today),
            "Tuesday · 2025-12-30"
        );
    }

    #[test]
    fn date_jump_preserves_filters_through_paging_and_reconnect() {
        let mut app = App::new("host".into());
        app.handle(key('h'));
        app.handle(key('f'));
        app.handle(key('/'));
        for c in "report".chars() {
            app.handle(key(c));
        }
        app.handle(KeyEvent::from(KeyCode::Enter));
        app.handle(key('D'));
        for c in "2024-02-29".chars() {
            app.handle(key(c));
        }
        let action = app.handle(KeyEvent::from(KeyCode::Enter));
        assert!(
            matches!(action, Some(Action::LoadArticles { day: Some(day), query, read, cursor: None, .. }) if day == "2024-02-29" && query == "report" && read == "unread")
        );
        let generation = app.articles.generation;
        app.articles.add(
            page(vec![article("one", false, 0)], Some("next")),
            generation,
            true,
        );
        assert!(
            matches!(app.handle(key('j')), Some(Action::LoadArticles { day: Some(day), cursor: Some(cursor), .. }) if day == "2024-02-29" && cursor == "next")
        );
        app.articles.loading = false;
        app.handle(key('D'));
        for c in "2024-03".chars() {
            app.handle(key(c));
        }
        let (tx, rx) = std::sync::mpsc::channel();
        super::super::refresh_articles(&mut app, &tx);
        assert!(
            matches!(rx.recv().unwrap(), super::super::Job::Articles { day: Some(day), query, read, cursor: None, .. } if day == "2024-02-29" && query == "report" && read == "unread")
        );
        app.handle(KeyEvent::from(KeyCode::Esc));
        assert!(
            matches!(app.handle(key('U')), Some(Action::LoadArticles { day: None, query, read, .. }) if query == "report" && read == "unread")
        );
        assert!(app.articles.items.is_empty());
        assert!(render(&app).contains("All dates"));
        assert!(
            matches!(app.handle(key('f')), Some(Action::LoadArticles { day: None, read, .. }) if read == "read")
        );
        assert!(
            matches!(app.handle(key('t')), Some(Action::LoadArticles { day: Some(_), read, .. }) if read == "read")
        );
    }

    #[test]
    fn date_entry_rejects_invalid_and_future_days_without_navigation() {
        let mut app = App::new("host".into());
        app.handle(key('h'));
        for invalid in ["2026-02-30", "2999-01-01", "2026-2-01", ""] {
            app.handle(key('D'));
            for c in invalid.chars() {
                app.handle(key(c));
            }
            assert_eq!(app.handle(KeyEvent::from(KeyCode::Enter)), None);
            assert!(matches!(app.mode, Mode::ArticleDate(_)));
            assert!(render(&app).contains("Use YYYY-MM-DD"));
            app.handle(KeyEvent::from(KeyCode::Esc));
            assert_eq!(app.articles.day, Some(today()));
        }
        app.handle(key('D'));
        app.handle(key('h'));
        app.handle(key('l'));
        assert_eq!(app.tab, super::super::Tab::Articles);
        assert_eq!(app.mode, Mode::ArticleDate(String::new()));
        app.handle(KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.handle(key('q')), Some(Action::Quit));
    }

    #[test]
    fn opening_preserves_unread_and_explicit_unread_uses_selected_version() {
        let mut app = App::new("host".into());
        app.handle(key('h'));
        app.articles
            .add(page(vec![article("one", false, 4)], None), 0, true);
        assert_eq!(
            app.handle(KeyEvent::from(KeyCode::Enter)),
            Some(Action::OpenArticle("one".into()))
        );
        assert!(app.articles.items[0].read_at.is_none());
        assert_eq!(
            app.handle(key('u')),
            Some(Action::ArticleUnread {
                id: "one".into(),
                version: 4
            })
        );
        let text = render(&app);
        for value in [
            "[articles]",
            "unread",
            "Report one",
            "Acceptance summary",
            "local-host",
            "project",
            "2026-09-08",
        ] {
            assert!(text.contains(value), "missing {value}");
        }
    }

    #[test]
    fn failed_replacement_keeps_the_applied_filter_label_with_selectable_rows() {
        let mut app = App::new("host".into());
        app.handle(key('h'));
        app.articles
            .add(page(vec![article("one", true, 1)], None), 0, true);
        app.handle(key('f'));
        app.articles.loading = false;
        app.articles.error = Some("offline".into());
        assert!(render(&app).contains("Filter: all"));
        assert!(render(&app).contains("pending: unread"));
        assert_eq!(
            app.handle(KeyEvent::from(KeyCode::Enter)),
            Some(Action::OpenArticle("one".into()))
        );
    }

    #[test]
    fn typing_during_a_load_does_not_relabel_its_results_or_submit_search_on_invalidation() {
        let mut app = App::new("host".into());
        app.handle(key('h'));
        app.handle(key('/'));
        app.handle(key('x'));
        app.articles
            .add(page(vec![article("one", false, 0)], None), 0, true);
        assert!(render(&app).contains("Filter: all  / · pending: all /x"));
        let (tx, rx) = std::sync::mpsc::channel();
        super::super::refresh_articles(&mut app, &tx);
        assert!(
            matches!(rx.recv().unwrap(), super::super::Job::Articles { query, .. } if query.is_empty())
        );
        assert_eq!(app.articles.query, "x");
    }

    #[test]
    fn invalidation_during_a_load_retains_a_canonical_refresh_after_that_load() {
        let mut app = App::new("host".into());
        let (tx, rx) = std::sync::mpsc::channel();
        super::super::refresh_articles(&mut app, &tx);
        let first = rx.recv().unwrap();
        let super::super::Job::Articles { generation, .. } = first else {
            panic!("expected article load")
        };
        super::super::refresh_articles(&mut app, &tx);
        assert!(app.articles.invalidated);
        assert!(rx.try_recv().is_err());
        app.articles
            .add(page(vec![article("one", false, 0)], None), generation, true);
        if app.articles.invalidated {
            super::super::refresh_articles(&mut app, &tx);
        }
        assert!(
            matches!(rx.recv().unwrap(), super::super::Job::Articles { cursor: None, generation: next, .. } if next > generation)
        );
        app.articles.add(
            page(vec![article("one", true, 1)], None),
            app.articles.generation,
            true,
        );
        assert_eq!(app.articles.items[0].version, 1);
        assert!(app.articles.items[0].read_at.is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn failed_conflict_reload_reports_failure_and_explicit_refresh_without_retrying_write() {
        use wiremock::{
            matchers::{method, path},
            Mock, MockServer, ResponseTemplate,
        };
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(
                "/v2/articles/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa/read",
            ))
            .respond_with(ResponseTemplate::new(409))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v2/articles/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"))
            .respond_with(ResponseTemplate::new(503))
            .expect(1)
            .mount(&server)
            .await;
        let cfg = crate::config::Config {
            server: server.uri(),
            token: "test-token".into(),
            topic: "test".into(),
            ntfy: None,
        };
        let (article, error) = tokio::task::spawn_blocking(move || {
            let client = crate::client::Client::new(&cfg).unwrap();
            super::super::mark_article_unread(&client, "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", 2)
        })
        .await
        .unwrap();
        assert!(article.is_none());
        let error = error.unwrap();
        assert!(error.contains("could not reload"));
        assert!(error.contains("Press R"));
        assert!(!error.contains("Press u"));
    }

    #[test]
    fn search_filter_and_paging_keep_query_cursor_and_ignore_stale_generation() {
        let mut app = App::new("host".into());
        app.handle(key('h'));
        app.articles.add(
            page(vec![article("one", true, 1)], Some("cursor-one")),
            0,
            true,
        );
        assert!(
            matches!(app.handle(key('j')), Some(Action::LoadArticles { cursor: Some(cursor), .. }) if cursor == "cursor-one")
        );
        app.handle(key('/'));
        app.handle(key('a'));
        assert_eq!(
            app.handle(KeyEvent::from(KeyCode::Enter)),
            Some(Action::LoadArticles {
                query: "a".into(),
                read: "all".into(),
                day: Some(today().to_string()),
                cursor: None,
                generation: 1
            })
        );
        app.articles
            .add(page(vec![article("stale", false, 0)], None), 0, false);
        assert_eq!(app.articles.items.len(), 1);
        app.articles
            .add(page(vec![article("new", false, 0)], None), 1, true);
        assert_eq!(app.articles.items[0].id, "new");
        assert_eq!(
            app.handle(key('f')),
            Some(Action::LoadArticles {
                query: "a".into(),
                read: "unread".into(),
                day: Some(today().to_string()),
                cursor: None,
                generation: 2
            })
        );
    }

    #[test]
    fn empty_error_and_paged_lists_render_retry_and_end_without_phantom_actions() {
        let mut app = App::new("host".into());
        app.handle(key('h'));
        assert!(render(&app).contains("Loading articles"));
        app.articles.loading = false;
        app.articles.error = Some("offline".into());
        assert!(render(&app).contains("Press R to retry"));
        assert_eq!(app.handle(key('u')), None);
        assert_eq!(app.handle(KeyEvent::from(KeyCode::Enter)), None);
        app.articles.add(page(vec![], None), 0, true);
        assert!(render(&app).contains("No articles match"));
        assert_eq!(app.handle(key('j')), None);
        app.articles
            .add(page(vec![article("one", true, 1)], Some("next")), 0, true);
        assert!(render(&app).contains("more below"));
    }

    #[test]
    fn late_pages_cannot_undo_newer_unread_or_reintroduce_filtered_read_rows() {
        let mut pane = ArticlesPane::default();
        pane.add(page(vec![article("one", true, 1)], None), 0, true);
        pane.reconcile(article("one", false, 2));
        pane.add(page(vec![article("one", true, 1)], None), 0, true);
        assert!(pane.items[0].read_at.is_none());
        pane.read = 1;
        pane.load(true);
        pane.add(
            page(vec![article("two", true, 3)], None),
            pane.generation,
            false,
        );
        assert_eq!(pane.items.len(), 1);
    }
}
