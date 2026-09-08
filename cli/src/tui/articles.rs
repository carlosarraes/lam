use super::{Action, App, Mode};
use crate::client::{Article, ArticlePage};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

#[derive(Default)]
pub(super) struct ArticlesPane {
    pub items: Vec<Article>,
    pub selected: usize,
    pub cursor: Option<String>,
    pub loaded: bool,
    pub loading: bool,
    pub query: String,
    pub read: usize,
    applied_query: String,
    applied_read: usize,
    requested_query: String,
    requested_read: usize,
    pub invalidated: bool,
    pub generation: u64,
    pub error: Option<String>,
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
            cursor: self.cursor.clone(),
            generation: self.generation,
        })
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
        let [header, list, detail, footer] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(7),
            Constraint::Length(3),
        ])
        .areas(frame.area());
        frame.render_widget(
            Paragraph::new(format!(
                "lam  requests  history  [articles]   {}\nFilter: {}  /{}{}{}",
                self.host,
                ["all", "unread", "read"][self.articles.applied_read],
                self.articles.applied_query,
                if self.articles.query != self.articles.applied_query
                    || self.articles.read != self.articles.applied_read
                    || matches!(self.mode, Mode::Filter)
                {
                    format!(
                        " · pending: {} /{}",
                        self.articles.read_filter(),
                        self.articles.query
                    )
                } else {
                    String::new()
                },
                if matches!(self.mode, Mode::Filter) {
                    "█"
                } else {
                    ""
                }
            )),
            header,
        );
        let rows: Vec<ListItem> = self
            .articles
            .items
            .iter()
            .map(|row| {
                ListItem::new(format!(
                    "{}  {}  {}",
                    if row.read_at.is_some() {
                        "read  "
                    } else {
                        "unread"
                    },
                    row.name,
                    row.title
                ))
            })
            .collect();
        let mut state = ListState::default().with_selected(Some(self.articles.selected));
        frame.render_stateful_widget(
            List::new(rows)
                .block(Block::default().borders(Borders::TOP))
                .highlight_style(Style::default().fg(Color::Yellow)),
            list,
            &mut state,
        );
        let text = match self.articles.items.get(self.articles.selected) {
            Some(row) => format!(
                "{}\n{}\n{} · {} · {}\n{} attachments · {}",
                row.title,
                row.summary,
                row.source_host,
                row.source_project,
                row.created_at,
                row.assets
                    .iter()
                    .filter(|asset| matches!(
                        asset.disposition,
                        crate::client::ArticleAssetDisposition::Attachment
                    ))
                    .count(),
                if row.read_at.is_some() {
                    "read"
                } else {
                    "unread"
                }
            ),
            None if self.articles.loading => "Loading articles…".into(),
            None if self.articles.error.is_some() => {
                "Articles could not be loaded. Press R to retry.".into()
            }
            None => "No articles match this filter.".into(),
        };
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::TOP)),
            detail,
        );
        frame.render_widget(Paragraph::new(format!("h requests · l history · a articles · ^3 articles · j/k move · Enter open · u unread\n/ search · Enter apply · f all/unread/read · R refresh · q quit\n{}{}", self.articles.error.as_deref().unwrap_or(&self.status), if self.articles.loading { " · loading" } else if self.articles.cursor.is_some() { " · more below" } else { " · end" })), footer);
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
    fn opening_preserves_unread_and_explicit_unread_uses_selected_version() {
        let mut app = App::new("host".into());
        app.handle(key('a'));
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
        app.handle(key('a'));
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
        app.handle(key('a'));
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
        app.handle(key('a'));
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
                cursor: None,
                generation: 2
            })
        );
    }

    #[test]
    fn empty_error_and_paged_lists_render_retry_and_end_without_phantom_actions() {
        let mut app = App::new("host".into());
        app.handle(key('a'));
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
