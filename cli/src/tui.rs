use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::client::{Client, Item, Resolution};
use crate::config::Config;
use crate::watch;

const REFRESH: Duration = Duration::from_secs(30);

/// What the UI asks the outside world to do; keeps `App` free of I/O so it is unit-testable.
#[derive(Debug, PartialEq)]
pub enum Action {
    Quit,
    Refresh,
    Resolve {
        id: String,
        choice: Option<String>,
        text: Option<String>,
    },
    Dismiss(String),
    OpenLink(String),
}

#[derive(Debug, PartialEq)]
enum Mode {
    Normal,
    Reply(String),
}

pub struct App {
    items: Vec<Item>,
    selected: usize,
    show_all: bool,
    mode: Mode,
    status: String,
    host: String,
}

impl App {
    pub fn new(host: String) -> Self {
        Self {
            items: vec![],
            selected: 0,
            show_all: false,
            mode: Mode::Normal,
            status: "connecting".into(),
            host,
        }
    }

    pub fn set_items(&mut self, items: Vec<Item>) {
        self.items = items;
        self.selected = self.selected.min(self.items.len().saturating_sub(1));
    }

    pub fn set_status(&mut self, s: impl Into<String>) {
        self.status = s.into();
    }

    pub fn show_all(&self) -> bool {
        self.show_all
    }

    fn current(&self) -> Option<&Item> {
        self.items.get(self.selected)
    }

    /// Translates a key press into an Action. Returns None when only internal state changed.
    pub fn handle(&mut self, key: KeyEvent) -> Option<Action> {
        if let Mode::Reply(text) = &mut self.mode {
            match key.code {
                KeyCode::Esc => self.mode = Mode::Normal,
                KeyCode::Enter if !text.trim().is_empty() => {
                    let text = std::mem::take(text);
                    self.mode = Mode::Normal;
                    let id = self.current()?.id.clone();
                    return Some(Action::Resolve {
                        id,
                        choice: None,
                        text: Some(text.trim().to_string()),
                    });
                }
                KeyCode::Backspace => {
                    text.pop();
                }
                KeyCode::Char(c) => text.push(c),
                _ => {}
            }
            return None;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Some(Action::Quit),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::Quit)
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if self.selected + 1 < self.items.len() {
                    self.selected += 1;
                }
                None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                None
            }
            KeyCode::Char('a') => {
                self.show_all = !self.show_all;
                Some(Action::Refresh)
            }
            KeyCode::Char('R') => Some(Action::Refresh),
            KeyCode::Char('d') => self.open_current().map(|i| Action::Dismiss(i.id.clone())),
            KeyCode::Char('o') => self
                .current()
                .filter(|i| !i.link.is_empty())
                .map(|i| Action::OpenLink(i.link.clone())),
            KeyCode::Char('r') => {
                if self.open_current().is_some() {
                    self.mode = Mode::Reply(String::new());
                }
                None
            }
            KeyCode::Enter => self
                .open_current()
                .filter(|i| i.choices.is_empty())
                .map(|i| Action::Resolve {
                    id: i.id.clone(),
                    choice: None,
                    text: None,
                }),
            KeyCode::Char(c @ '1'..='3') => {
                let item = self.open_current()?;
                let choice = item.choices.get(c as usize - '1' as usize)?.clone();
                Some(Action::Resolve {
                    id: item.id.clone(),
                    choice: Some(choice),
                    text: None,
                })
            }
            _ => None,
        }
    }

    fn open_current(&self) -> Option<&Item> {
        self.current().filter(|i| i.status == "open")
    }

    fn draw(&self, f: &mut Frame) {
        let [header, list, detail, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(6),
            Constraint::Length(2),
        ])
        .areas(f.area());

        let open = self.items.iter().filter(|i| i.status == "open").count();
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" lam ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(format!(
                    "— {open} open{}",
                    if self.show_all { " (showing all)" } else { "" }
                )),
                Span::raw(format!("   {}  ● {}", self.host, self.status)),
            ])),
            header,
        );

        let rows: Vec<ListItem> = self.items.iter().map(row).collect();
        let mut state = ListState::default().with_selected(Some(self.selected));
        f.render_stateful_widget(
            List::new(rows)
                .block(Block::default().borders(Borders::TOP))
                .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
                .highlight_symbol("▸ "),
            list,
            &mut state,
        );

        let text = match self.current() {
            Some(i) => {
                let src = [i.source_host.as_str(), i.source_project.as_str()]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(":");
                let mut lines = vec![Line::from(Span::styled(
                    format!("({src})  {}", i.title),
                    Style::default().add_modifier(Modifier::BOLD),
                ))];
                if !i.body.is_empty() {
                    lines.push(Line::raw(i.body.clone()));
                }
                if !i.link.is_empty() {
                    lines.push(Line::from(Span::styled(
                        i.link.clone(),
                        Style::default().fg(Color::Blue),
                    )));
                }
                if let Some(answer) = i.response_choice.as_deref().or(i.response_text.as_deref()) {
                    lines.push(Line::raw(format!(
                        "{} via {}: {answer}",
                        i.status,
                        i.response_by.as_deref().unwrap_or("?")
                    )));
                }
                lines
            }
            None => vec![Line::raw("nothing here — all caught up")],
        };
        f.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::TOP)),
            detail,
        );

        let footer_text = match (&self.mode, self.current()) {
            (Mode::Reply(t), _) => vec![
                Line::from(vec![
                    Span::styled("reply> ", Style::default().fg(Color::Yellow)),
                    Span::raw(t.clone()),
                    Span::raw("█"),
                ]),
                Line::raw("Enter send · Esc cancel"),
            ],
            (_, Some(i)) if i.status == "open" => {
                let mut keys: Vec<String> = i
                    .choices
                    .iter()
                    .enumerate()
                    .map(|(n, c)| format!("[{}] {c}", n + 1))
                    .collect();
                if i.choices.is_empty() {
                    keys.push("[Enter] done".into());
                }
                if !i.link.is_empty() {
                    keys.push("[o] open link".into());
                }
                keys.push("[r] reply text".into());
                keys.push("[d] dismiss".into());
                vec![
                    Line::from(Span::styled(
                        keys.join("   "),
                        Style::default().fg(Color::Green),
                    )),
                    Line::raw("j/k move · a all/open · R refresh · q quit"),
                ]
            }
            _ => vec![
                Line::raw(""),
                Line::raw("j/k move · a all/open · R refresh · q quit"),
            ],
        };
        f.render_widget(Paragraph::new(footer_text), footer);
    }
}

fn row(i: &Item) -> ListItem<'_> {
    let icon = match (i.status.as_str(), i.priority.as_str()) {
        ("open", "critical") => "🚨",
        ("open", _) => "👀",
        ("resolved", _) => "✅",
        ("retracted", _) => "↩ ",
        _ => "✖ ",
    };
    let src = [i.source_host.as_str(), i.source_project.as_str()]
        .iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(":");
    let style = if i.status == "open" {
        Style::default()
    } else {
        Style::default().fg(Color::DarkGray)
    };
    ListItem::new(Line::from(vec![
        Span::raw(format!("{icon} {:<6} ", i.id)),
        Span::styled(format!("{src:<22} "), Style::default().fg(Color::Cyan)),
        Span::raw(i.title.clone()),
    ]))
    .style(style)
}

enum Msg {
    Push,
    Status(String),
}

pub fn run() -> Result<i32> {
    let cfg = Config::load()?;
    let client = Client::new(&cfg)?;
    let mut app = App::new(hostname::get()?.to_string_lossy().into_owned());

    let (tx, rx) = mpsc::channel::<Msg>();
    let stream_cfg = cfg.clone();
    std::thread::spawn(move || {
        watch::subscribe(
            &stream_cfg,
            |_| {
                let _ = tx.send(Msg::Push);
            },
            |s| {
                let _ = tx.send(Msg::Status(s.to_string()));
            },
        )
    });

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app, &client, &rx);
    ratatui::restore();
    result.map(|_| 0)
}

fn refresh(app: &mut App, client: &Client) {
    match client.list(if app.show_all() { None } else { Some("open") }) {
        Ok(items) => app.set_items(items),
        Err(e) => app.set_status(format!("refresh failed: {e}")),
    }
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    client: &Client,
    rx: &mpsc::Receiver<Msg>,
) -> Result<()> {
    refresh(app, client);
    let mut last_refresh = Instant::now();
    loop {
        terminal.draw(|f| app.draw(f))?;

        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Push => refresh(app, client),
                Msg::Status(s) => app.set_status(s),
            }
        }
        if last_refresh.elapsed() > REFRESH {
            refresh(app, client);
            last_refresh = Instant::now();
        }

        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != event::KeyEventKind::Press {
            continue;
        }
        let Some(action) = app.handle(key) else {
            continue;
        };
        let outcome = match action {
            Action::Quit => return Ok(()),
            Action::Refresh => Ok(()),
            Action::Resolve { id, choice, text } => client
                .resolve(&id, &Resolution { choice, text })
                .map(|_| ()),
            Action::Dismiss(id) => client.dismiss(&id).map(|_| ()),
            Action::OpenLink(url) => open_link(&url),
        };
        if let Err(e) = outcome {
            app.set_status(format!("error: {e}"));
        }
        refresh(app, client);
        last_refresh = Instant::now();
    }
}

fn open_link(url: &str) -> Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener).arg(url).spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, status: &str, choices: &[&str], link: &str) -> Item {
        Item {
            id: id.into(),
            title: "t".into(),
            body: String::new(),
            source_host: "h".into(),
            source_project: "p".into(),
            priority: "normal".into(),
            choices: choices.iter().map(|s| s.to_string()).collect(),
            link: link.into(),
            status: status.into(),
            response_choice: None,
            response_text: None,
            response_by: None,
            created_at: String::new(),
            resolved_at: None,
            expires_at: None,
        }
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::from(KeyCode::Char(c))
    }

    fn app() -> App {
        let mut a = App::new("host".into());
        a.set_items(vec![
            item("aaa", "open", &["yes", "no"], "https://x"),
            item("bbb", "resolved", &[], ""),
            item("ccc", "open", &[], ""),
        ]);
        a
    }

    #[test]
    fn number_keys_pick_choices_on_open_items_only() {
        let mut a = app();
        assert_eq!(
            a.handle(key('2')),
            Some(Action::Resolve {
                id: "aaa".into(),
                choice: Some("no".into()),
                text: None
            })
        );
        assert_eq!(a.handle(key('3')), None, "no third choice");
        a.handle(key('j'));
        assert_eq!(a.handle(key('1')), None, "closed item is inert");
        assert_eq!(a.handle(key('d')), None);
    }

    #[test]
    fn enter_marks_done_when_no_choices_and_reply_mode_collects_text() {
        let mut a = app();
        a.handle(key('j'));
        a.handle(key('j'));
        assert_eq!(
            a.handle(KeyEvent::from(KeyCode::Enter)),
            Some(Action::Resolve {
                id: "ccc".into(),
                choice: None,
                text: None
            })
        );
        assert_eq!(a.handle(key('r')), None);
        for c in "go B ".chars() {
            a.handle(key(c));
        }
        assert_eq!(a.handle(KeyEvent::from(KeyCode::Backspace)), None);
        assert_eq!(
            a.handle(key('q')),
            None,
            "typing q in reply mode must not quit"
        );
        assert_eq!(
            a.handle(KeyEvent::from(KeyCode::Enter)),
            Some(Action::Resolve {
                id: "ccc".into(),
                choice: None,
                text: Some("go Bq".into())
            })
        );
        assert_eq!(a.mode, Mode::Normal);
    }

    #[test]
    fn navigation_clamps_and_link_only_when_present() {
        let mut a = app();
        a.handle(key('k'));
        assert_eq!(
            a.handle(key('o')),
            Some(Action::OpenLink("https://x".into()))
        );
        for _ in 0..5 {
            a.handle(key('j'));
        }
        assert_eq!(a.selected, 2);
        assert_eq!(a.handle(key('o')), None);
        assert_eq!(a.handle(key('a')), Some(Action::Refresh));
        assert!(a.show_all());
        assert_eq!(a.handle(key('q')), Some(Action::Quit));
    }

    #[test]
    fn set_items_keeps_selection_in_bounds() {
        let mut a = app();
        a.handle(key('j'));
        a.handle(key('j'));
        a.set_items(vec![item("aaa", "open", &[], "")]);
        assert_eq!(a.selected, 0);
        a.set_items(vec![]);
        assert_eq!(a.handle(key('1')), None);
    }
}
