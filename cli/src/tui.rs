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
    SetCheck {
        id: String,
        index: usize,
        done: bool,
    },
}

#[derive(Debug, PartialEq)]
enum Mode {
    Normal,
    Reply(String),
    /// Live-filtering by agent name or title.
    Filter,
}

pub struct App {
    items: Vec<Item>,
    filter: String,
    selected: usize,
    /// Cursor within the selected item's checks.
    check_sel: usize,
    show_all: bool,
    mode: Mode,
    status: String,
    host: String,
}

impl App {
    pub fn new(host: String) -> Self {
        Self {
            items: vec![],
            filter: String::new(),
            selected: 0,
            check_sel: 0,
            show_all: false,
            mode: Mode::Normal,
            status: "connecting".into(),
            host,
        }
    }

    pub fn set_items(&mut self, items: Vec<Item>) {
        self.items = items;
        self.clamp();
    }

    fn clamp(&mut self) {
        self.selected = self.selected.min(self.visible().len().saturating_sub(1));
        self.check_sel = 0;
    }

    /// Items matching the current filter, which matches on agent name or title.
    fn visible(&self) -> Vec<&Item> {
        if self.filter.is_empty() {
            return self.items.iter().collect();
        }
        let f = self.filter.to_lowercase();
        self.items
            .iter()
            .filter(|i| i.name.to_lowercase().contains(&f) || i.title.to_lowercase().contains(&f))
            .collect()
    }

    pub fn set_status(&mut self, s: impl Into<String>) {
        self.status = s.into();
    }

    pub fn show_all(&self) -> bool {
        self.show_all
    }

    fn current(&self) -> Option<&Item> {
        self.visible().get(self.selected).copied()
    }

    /// Translates a key press into an Action. Returns None when only internal state changed.
    pub fn handle(&mut self, key: KeyEvent) -> Option<Action> {
        if matches!(self.mode, Mode::Filter) {
            match key.code {
                KeyCode::Esc => {
                    self.filter.clear();
                    self.mode = Mode::Normal;
                    self.clamp();
                }
                KeyCode::Enter => self.mode = Mode::Normal,
                KeyCode::Backspace => {
                    self.filter.pop();
                    self.clamp();
                }
                KeyCode::Char(c) => {
                    self.filter.push(c);
                    self.clamp();
                }
                _ => {}
            }
            return None;
        }
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
                if self.selected + 1 < self.visible().len() {
                    self.selected += 1;
                    self.check_sel = 0;
                }
                None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                self.check_sel = 0;
                None
            }
            KeyCode::Tab => {
                if let Some(i) = self.open_current() {
                    if !i.checks.is_empty() {
                        self.check_sel = (self.check_sel + 1) % i.checks.len();
                    }
                }
                None
            }
            KeyCode::Char(' ') => {
                let item = self.open_current()?;
                let check = item.checks.get(self.check_sel)?;
                Some(Action::SetCheck {
                    id: item.id.clone(),
                    index: self.check_sel,
                    done: !check.done,
                })
            }
            KeyCode::Char('a') => {
                self.show_all = !self.show_all;
                Some(Action::Refresh)
            }
            KeyCode::Char('/') => {
                self.mode = Mode::Filter;
                None
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
                .filter(|i| i.choices.is_empty() && i.checks.is_empty())
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
        let checks = self.current().map_or(0, |i| i.checks.len() as u16);
        let [header, list, detail, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(6 + checks),
            Constraint::Length(2),
        ])
        .areas(f.area());

        let visible = self.visible();
        let open = visible.iter().filter(|i| i.status == "open").count();
        let live = self.status == "live";
        let [head_l, head_r] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Length(40)]).areas(header);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("lam", BOLD),
                Span::styled(format!("  {open} open"), META),
                Span::styled(if self.show_all { "  · all" } else { "" }, DIM),
                Span::styled(
                    if self.filter.is_empty() {
                        String::new()
                    } else {
                        format!("  /{}", self.filter)
                    },
                    ACCENT,
                ),
                Span::styled(
                    if matches!(self.mode, Mode::Filter) {
                        "█"
                    } else {
                        ""
                    },
                    ACCENT,
                ),
            ])),
            head_l,
        );
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{}  ", self.host), META),
                Span::styled(
                    "●",
                    if live {
                        Style::default().fg(Color::Green)
                    } else {
                        Style::default().fg(Color::Yellow)
                    },
                ),
                Span::styled(format!(" {}", self.status), META),
            ]))
            .right_aligned(),
            head_r,
        );

        let rows: Vec<ListItem> = visible.iter().map(|i| row(i)).collect();
        let mut state = ListState::default().with_selected(Some(self.selected));
        f.render_stateful_widget(
            List::new(rows)
                .block(Block::default().borders(Borders::TOP).border_style(RULE))
                .highlight_style(Style::default().bg(SELECTION)),
            list,
            &mut state,
        );

        let text = match self.current() {
            Some(i) => {
                let mut lines = vec![Line::from(Span::styled(
                    format!(
                        "{} · {} · {} ago",
                        source(i),
                        i.priority,
                        age(&i.created_at)
                    ),
                    META,
                ))];
                if !i.body.is_empty() {
                    lines.push(Line::raw(i.body.clone()));
                }
                if !i.link.is_empty() {
                    lines.push(Line::from(Span::styled(i.link.clone(), LINK)));
                }
                for (n, c) in i.checks.iter().enumerate() {
                    let cursor = n == self.check_sel && i.status == "open";
                    lines.push(Line::from(vec![
                        Span::styled(if cursor { "▸ " } else { "  " }, ACCENT),
                        Span::styled(
                            format!("{} {}", if c.done { "✔" } else { "○" }, c.label),
                            if c.done { DIM } else { Style::default() },
                        ),
                    ]));
                }
                if let Some(answer) = i.response_choice.as_deref().or(i.response_text.as_deref()) {
                    lines.push(Line::from(Span::styled(
                        format!(
                            "{} via {}: {answer}",
                            i.status,
                            i.response_by.as_deref().unwrap_or("?")
                        ),
                        META,
                    )));
                }
                lines
            }
            None => vec![Line::from(Span::styled(
                "nothing here — all caught up",
                META,
            ))],
        };
        f.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::TOP).border_style(RULE)),
            detail,
        );

        let footer_text = match (&self.mode, self.current()) {
            (Mode::Filter, _) => vec![
                Line::from(vec![
                    Span::styled("filter› ", ACCENT),
                    Span::raw(self.filter.clone()),
                    Span::styled("█", ACCENT),
                ]),
                Line::from([key("Enter", "keep"), key("Esc", "clear")].concat()),
            ],
            (Mode::Reply(t), _) => vec![
                Line::from(vec![
                    Span::styled("reply› ", ACCENT),
                    Span::raw(t.clone()),
                    Span::styled("█", ACCENT),
                ]),
                Line::from([key("Enter", "send"), key("Esc", "cancel")].concat()),
            ],
            (_, Some(i)) if i.status == "open" => {
                let mut spans: Vec<Span> = i
                    .choices
                    .iter()
                    .enumerate()
                    .flat_map(|(n, c)| key(&(n + 1).to_string(), c))
                    .collect();
                if !i.checks.is_empty() {
                    spans.extend(key("Tab", "next check"));
                    spans.extend(key(
                        "Space",
                        &format!("toggle {}/{}", i.checks_done(), i.checks.len()),
                    ));
                } else if i.choices.is_empty() {
                    spans.extend(key("Enter", "done"));
                }
                if !i.link.is_empty() {
                    spans.extend(key("o", "open"));
                }
                spans.extend(key("r", "reply"));
                spans.extend(key("d", "dismiss"));
                vec![
                    Line::from(spans),
                    Line::from(Span::styled(
                        "j/k move · / filter · a all · R refresh · q quit",
                        DIM,
                    )),
                ]
            }
            _ => vec![
                Line::raw(""),
                Line::from(Span::styled(
                    "j/k move · / filter · a all · R refresh · q quit",
                    DIM,
                )),
            ],
        };
        f.render_widget(Paragraph::new(footer_text), footer);
    }
}

// Palette: one accent (amber) for "pressable" and attention; priority in red/blue so amber stays unique.
const ACCENT: Style = Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD);
const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
const META: Style = Style::new().fg(Color::Gray);
const DIM: Style = Style::new().fg(Color::DarkGray);
const RULE: Style = Style::new().fg(Color::DarkGray);
const LINK: Style = Style::new().fg(Color::Cyan);
const SELECTION: Color = Color::Rgb(0x2a, 0x24, 0x16);

/// A footer hint: the key in accent, the label dimmed.
fn key<'a>(k: &str, label: &str) -> Vec<Span<'a>> {
    vec![
        Span::styled(k.to_string(), ACCENT),
        Span::styled(format!(" {label}   "), META),
    ]
}

/// Who is asking: the agent's name, falling back to host:project for pre-name items.
fn source(i: &Item) -> String {
    if !i.name.is_empty() {
        return i.name.clone();
    }
    [i.source_host.as_str(), i.source_project.as_str()]
        .iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(":")
}

/// Compact relative age like `2m`, `1h`, `3d` from an RFC 3339 timestamp.
pub fn age(created_at: &str) -> String {
    let Ok(t) = chrono::DateTime::parse_from_rfc3339(created_at) else {
        return String::new();
    };
    let secs = (chrono::Utc::now() - t.with_timezone(&chrono::Utc))
        .num_seconds()
        .max(0);
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

fn row(i: &Item) -> ListItem<'_> {
    let open = i.status == "open";
    let gutter = match (open, i.priority.as_str()) {
        (true, "critical") => Style::default().fg(Color::Red),
        (true, "low") => DIM,
        (true, _) => Style::default().fg(Color::Blue),
        (false, _) => RULE,
    };
    let title = if i.checks.is_empty() {
        i.title.clone()
    } else {
        format!("{}  {}/{}", i.title, i.checks_done(), i.checks.len())
    };
    let text = if open { Style::default() } else { DIM };
    ListItem::new(Line::from(vec![
        Span::styled("▍ ", gutter),
        Span::styled(format!("{:<6} ", i.id), if open { META } else { DIM }),
        Span::styled(format!("{:<20} ", source(i)), if open { LINK } else { DIM }),
        Span::styled(title, text),
        Span::styled(format!("   {}", age(&i.created_at)), DIM),
    ]))
}

enum Msg {
    /// A push arrived; `fresh` is true for new items (not closed/updated notices).
    Push {
        fresh: bool,
        title: String,
        body: String,
        critical: bool,
    },
    Status(String),
}

pub fn run(silent: bool) -> Result<i32> {
    let cfg = Config::load()?;
    let client = Client::new(&cfg)?;
    let mut app = App::new(hostname::get()?.to_string_lossy().into_owned());

    let (tx, rx) = mpsc::channel::<Msg>();
    let stream_cfg = cfg.clone();
    std::thread::spawn(move || {
        watch::subscribe(
            &stream_cfg,
            |ev| {
                let fresh = ev.tags.iter().any(|t| t == "eyes" || t == "rotating_light");
                let _ = tx.send(Msg::Push {
                    fresh,
                    title: ev.title,
                    body: ev.message,
                    critical: ev.priority >= 5,
                });
            },
            |s| {
                let _ = tx.send(Msg::Status(s.to_string()));
            },
        )
    });

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app, &client, &rx, silent);
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
    silent: bool,
) -> Result<()> {
    refresh(app, client);
    let mut last_refresh = Instant::now();
    loop {
        terminal.draw(|f| app.draw(f))?;

        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Push {
                    fresh,
                    title,
                    body,
                    critical,
                } => {
                    if fresh && !silent {
                        // The bell reaches you through ssh; the desktop popup is left to
                        // `lam watch` when it owns notifications here, so it never fires twice.
                        let _ = std::io::Write::write_all(&mut std::io::stdout(), b"\x07");
                        if !crate::notify::watch_running() {
                            let _ = crate::notify::desktop(&title, &body, critical);
                        }
                    }
                    refresh(app, client);
                }
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
            Action::SetCheck { id, index, done } => client.set_check(&id, index, done).map(|_| ()),
        };
        if let Err(e) = outcome {
            app.set_status(format!("error: {e}"));
        }
        refresh(app, client);
        last_refresh = Instant::now();
    }
}

fn open_link(url: &str) -> Result<()> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        anyhow::bail!("refusing to open non-http link: {url}");
    }
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
            name: "0:lam".into(),
            title: "t".into(),
            body: String::new(),
            source_host: "h".into(),
            source_project: "p".into(),
            priority: "normal".into(),
            choices: choices.iter().map(|s| s.to_string()).collect(),
            link: link.into(),
            checks: vec![],
            version: 0,
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
    fn checklist_keys_toggle_and_cycle() {
        let mut a = App::new("host".into());
        let mut i = item("chk", "open", &[], "");
        i.checks = vec![
            crate::client::Check {
                label: "one".into(),
                done: false,
                at: None,
            },
            crate::client::Check {
                label: "two".into(),
                done: true,
                at: None,
            },
        ];
        a.set_items(vec![i]);
        assert_eq!(
            a.handle(KeyEvent::from(KeyCode::Enter)),
            None,
            "Enter is not 'done' on a checklist"
        );
        assert_eq!(
            a.handle(key(' ')),
            Some(Action::SetCheck {
                id: "chk".into(),
                index: 0,
                done: true
            })
        );
        a.handle(KeyEvent::from(KeyCode::Tab));
        assert_eq!(
            a.handle(key(' ')),
            Some(Action::SetCheck {
                id: "chk".into(),
                index: 1,
                done: false
            })
        );
        a.handle(KeyEvent::from(KeyCode::Tab));
        assert_eq!(a.check_sel, 0, "Tab wraps");
        a.set_items(vec![item("plain", "open", &[], "")]);
        assert_eq!(a.handle(key(' ')), None);
    }

    #[test]
    fn slash_filters_by_name_and_esc_restores() {
        let mut a = App::new("host".into());
        let mut alpha = item("aaa", "open", &[], "");
        alpha.name = "0:alpha".into();
        let mut beta = item("bbb", "open", &[], "");
        beta.name = "1:beta".into();
        a.set_items(vec![alpha, beta]);

        assert_eq!(a.handle(key('/')), None);
        for c in "beta".chars() {
            a.handle(key(c));
        }
        assert_eq!(a.visible().len(), 1);
        assert_eq!(a.current().unwrap().id, "bbb");
        assert_eq!(
            a.handle(key('q')),
            None,
            "typing q while filtering must not quit"
        );
        a.handle(KeyEvent::from(KeyCode::Backspace));

        a.handle(KeyEvent::from(KeyCode::Enter));
        assert_eq!(a.visible().len(), 1, "Enter keeps the filter");
        assert_eq!(
            a.handle(key('d')),
            Some(Action::Dismiss("bbb".into())),
            "actions target the filtered item"
        );

        a.handle(key('/'));
        a.handle(KeyEvent::from(KeyCode::Esc));
        assert_eq!(
            a.visible().len(),
            2,
            "Esc clears back to the previous filter"
        );
    }

    #[test]
    fn age_is_compact_and_tolerant() {
        let t = (chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
        assert_eq!(age(&t), "5m");
        let t = (chrono::Utc::now() - chrono::Duration::hours(26)).to_rfc3339();
        assert_eq!(age(&t), "1d");
        assert_eq!(age("garbage"), "");
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
