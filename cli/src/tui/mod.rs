use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use std::cell::Cell;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::client::{Client, Item, Resolution};
use crate::config::Config;
use crate::watch;

mod draw;

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

/// Which pane the arrow/vim keys drive.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Focus {
    List,
    Checks,
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
    focus: Focus,
    /// A request is in flight on the network thread.
    busy: bool,
    show_all: bool,
    /// Side-by-side markdown reader for long bodies (`m`).
    reader: bool,
    scroll: u16,
    /// Rendered length of the reader document, recorded while drawing so scrolling can clamp.
    doc_lines: Cell<u16>,
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
            focus: Focus::List,
            busy: false,
            show_all: false,
            reader: false,
            scroll: 0,
            doc_lines: Cell::new(0),
            mode: Mode::Normal,
            status: "connecting".into(),
            host,
        }
    }

    /// Refreshes never move you: the same item stays selected, keeping its scroll position and
    /// check cursor. Only when it is gone (resolved, filtered away) does the cursor reset.
    pub fn set_items(&mut self, items: Vec<Item>) {
        let previous = self.current().map(|i| i.id.clone());
        self.items = items;
        let same = previous.and_then(|id| self.visible().iter().position(|i| i.id == id));
        match same {
            Some(pos) => {
                self.selected = pos;
                let checks = self.current().map_or(0, |i| i.checks.len());
                self.check_sel = self.check_sel.min(checks.saturating_sub(1));
            }
            None => self.clamp(),
        }
    }

    pub fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
    }

    fn clamp(&mut self) {
        self.selected = self.selected.min(self.visible().len().saturating_sub(1));
        self.check_sel = 0;
        self.scroll = 0;
        self.focus = Focus::List;
    }

    /// The next unticked check after `from`, wrapping — so repeated Space walks the whole list.
    fn next_unchecked(&self, from: usize) -> Option<usize> {
        let item = self.current()?;
        let n = item.checks.len();
        if n == 0 {
            return None;
        }
        (1..=n)
            .map(|step| (from + step) % n)
            .find(|&i| !item.checks[i].done)
    }

    fn move_check(&mut self, delta: isize) {
        let n = self.current().map_or(0, |i| i.checks.len());
        if n > 0 {
            self.check_sel = (self.check_sel as isize + delta).rem_euclid(n as isize) as usize;
        }
    }

    fn has_checks(&self) -> bool {
        self.open_current().is_some_and(|i| !i.checks.is_empty())
    }

    fn nav_hint(&self) -> &'static str {
        if self.reader {
            "j/k move · J/K scroll · g/G top/end · m close · / filter · q quit"
        } else {
            "j/k move · m read · / filter · a all · R refresh · q quit"
        }
    }

    fn scroll_by(&mut self, delta: i32) {
        let max = self.doc_lines.get().saturating_sub(1) as i32;
        self.scroll = (self.scroll as i32 + delta).clamp(0, max.max(0)) as u16;
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
            KeyCode::Esc if self.focus == Focus::Checks => {
                self.focus = Focus::List;
                None
            }
            KeyCode::Char('q') | KeyCode::Esc => Some(Action::Quit),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::Quit)
            }
            KeyCode::Char('j') | KeyCode::Down if self.focus == Focus::Checks => {
                self.move_check(1);
                None
            }
            KeyCode::Char('k') | KeyCode::Up if self.focus == Focus::Checks => {
                self.move_check(-1);
                None
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if self.selected + 1 < self.visible().len() {
                    self.selected += 1;
                    self.check_sel = 0;
                    self.scroll = 0;
                }
                None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                self.check_sel = 0;
                self.scroll = 0;
                None
            }
            KeyCode::Tab => {
                if self.has_checks() {
                    self.focus = match self.focus {
                        Focus::List => Focus::Checks,
                        Focus::Checks => Focus::List,
                    };
                }
                None
            }
            KeyCode::Char(' ') | KeyCode::Enter if self.has_checks() => {
                let item = self.open_current()?;
                let check = item.checks.get(self.check_sel)?;
                let index = self.check_sel;
                let done = !check.done;
                let action = Action::SetCheck {
                    id: item.id.clone(),
                    index,
                    done,
                };
                // Ticking moves on to the next thing you still have to do; unticking stays put.
                if done {
                    if let Some(next) = self.next_unchecked(index) {
                        self.check_sel = next;
                    }
                }
                Some(action)
            }
            KeyCode::Char('a') => {
                self.show_all = !self.show_all;
                Some(Action::Refresh)
            }
            KeyCode::Char('m') => {
                self.reader = !self.reader;
                self.scroll = 0;
                None
            }
            KeyCode::Char('J') | KeyCode::PageDown => {
                self.scroll_by(if key.code == KeyCode::PageDown { 10 } else { 1 });
                None
            }
            KeyCode::Char('K') | KeyCode::PageUp => {
                self.scroll_by(if key.code == KeyCode::PageUp { -10 } else { -1 });
                None
            }
            KeyCode::Char('g') => {
                self.scroll = 0;
                None
            }
            KeyCode::Char('G') => {
                self.scroll_by(i32::MAX / 2);
                None
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
}

/// Work handed to the network thread. Nothing in the UI thread ever calls the server, so a slow
/// or hung request cannot freeze the screen — and `q`/Ctrl-C keep working, which in raw mode they
/// only do if the loop is still reading keys.
enum Job {
    Refresh {
        all: bool,
    },
    Resolve {
        id: String,
        choice: Option<String>,
        text: Option<String>,
    },
    Dismiss(String),
    SetCheck {
        id: String,
        index: usize,
        done: bool,
    },
}

enum Msg {
    Items(Vec<Item>),
    Failed(String),
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
    let (jobs, work) = mpsc::channel::<Job>();

    let net_tx = tx.clone();
    std::thread::spawn(move || {
        let mut show_all = false;
        while let Ok(job) = work.recv() {
            let outcome = match job {
                Job::Refresh { all } => {
                    show_all = all;
                    Ok(())
                }
                Job::Resolve { id, choice, text } => client
                    .resolve(&id, &Resolution { choice, text })
                    .map(|_| ()),
                Job::Dismiss(id) => client.dismiss(&id).map(|_| ()),
                Job::SetCheck { id, index, done } => client.set_check(&id, index, done).map(|_| ()),
            };
            if let Err(e) = outcome {
                let _ = net_tx.send(Msg::Failed(format!("{e}")));
            }
            // Every job ends by re-reading the queue, so the screen always catches up.
            let _ = net_tx.send(
                match client.list(if show_all { None } else { Some("open") }) {
                    Ok(items) => Msg::Items(items),
                    Err(e) => Msg::Failed(format!("refresh failed: {e}")),
                },
            );
        }
    });

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
    let result = event_loop(&mut terminal, &mut app, &jobs, &rx, silent);
    ratatui::restore();
    result.map(|_| 0)
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    jobs: &mpsc::Sender<Job>,
    rx: &mpsc::Receiver<Msg>,
    silent: bool,
) -> Result<()> {
    let refresh = |app: &App| {
        let _ = jobs.send(Job::Refresh {
            all: app.show_all(),
        });
    };
    refresh(app);
    let mut last_refresh = Instant::now();
    loop {
        terminal.draw(|f| app.draw(f))?;

        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Items(items) => {
                    app.set_items(items);
                    app.set_busy(false);
                    app.set_status("live");
                }
                Msg::Failed(e) => {
                    app.set_busy(false);
                    app.set_status(e);
                }
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
                    refresh(app);
                }
                Msg::Status(s) => app.set_status(s),
            }
        }
        if last_refresh.elapsed() > REFRESH {
            refresh(app);
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
        let job = match action {
            Action::Quit => return Ok(()),
            Action::Refresh => Job::Refresh {
                all: app.show_all(),
            },
            Action::Resolve { id, choice, text } => Job::Resolve { id, choice, text },
            Action::Dismiss(id) => Job::Dismiss(id),
            Action::SetCheck { id, index, done } => Job::SetCheck { id, index, done },
            Action::OpenLink(url) => {
                if let Err(e) = open_link(&url) {
                    app.set_status(format!("open failed: {e}"));
                }
                continue;
            }
        };
        app.set_busy(true);
        if jobs.send(job).is_err() {
            app.set_status("network thread stopped");
        }
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
    // Null stdio: a browser helper writing to stderr would corrupt the alternate screen.
    std::process::Command::new(opener)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn item(id: &str, status: &str, choices: &[&str], link: &str) -> Item {
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

    fn checklist(done: [bool; 3]) -> App {
        let mut a = App::new("host".into());
        let mut i = item("chk", "open", &[], "");
        i.checks = done
            .iter()
            .enumerate()
            .map(|(n, &d)| crate::client::Check {
                label: format!("part {n}"),
                done: d,
                at: None,
            })
            .collect();
        a.set_items(vec![i]);
        a
    }

    #[test]
    fn space_ticks_and_walks_to_the_next_thing_left_to_do() {
        let mut a = checklist([false, false, false]);
        assert_eq!(
            a.handle(key(' ')),
            Some(Action::SetCheck {
                id: "chk".into(),
                index: 0,
                done: true
            })
        );
        assert_eq!(a.check_sel, 1, "Space alone advances — no Tab dance");
        assert_eq!(
            a.handle(key(' ')),
            Some(Action::SetCheck {
                id: "chk".into(),
                index: 1,
                done: true
            })
        );
        assert_eq!(a.check_sel, 2);
    }

    #[test]
    fn space_skips_checks_already_done_and_unticking_stays_put() {
        let mut a = checklist([false, true, false]);
        a.handle(key(' '));
        assert_eq!(a.check_sel, 2, "index 1 is already done, so it is skipped");

        let mut a = checklist([false, true, true]);
        a.handle(key(' '));
        assert_eq!(a.check_sel, 0, "nothing left to do: the cursor stays");

        let mut a = checklist([true, false, false]);
        assert_eq!(
            a.handle(key(' ')),
            Some(Action::SetCheck {
                id: "chk".into(),
                index: 0,
                done: false
            }),
            "Space on a ticked check unticks it"
        );
        assert_eq!(a.check_sel, 0, "unticking does not move the cursor");
    }

    #[test]
    fn tab_focuses_the_checklist_where_jk_picks_and_esc_returns() {
        let mut a = checklist([false, false, false]);
        assert_eq!(a.focus, Focus::List);
        a.handle(KeyEvent::from(KeyCode::Tab));
        assert_eq!(a.focus, Focus::Checks);

        a.handle(key('j'));
        a.handle(key('j'));
        assert_eq!(
            a.check_sel, 2,
            "j/k move between checks while they have focus"
        );
        a.handle(key('k'));
        assert_eq!(a.check_sel, 1);
        a.handle(key('j'));
        a.handle(key('j'));
        assert_eq!(a.check_sel, 0, "and they wrap");

        assert_eq!(
            a.handle(KeyEvent::from(KeyCode::Esc)),
            None,
            "Esc leaves the checks, it does not quit"
        );
        assert_eq!(a.focus, Focus::List);
        assert_eq!(a.handle(KeyEvent::from(KeyCode::Esc)), Some(Action::Quit));
    }

    #[test]
    fn an_item_without_checks_keeps_the_old_keys() {
        let mut a = app();
        a.handle(KeyEvent::from(KeyCode::Tab));
        assert_eq!(a.focus, Focus::List, "nothing to focus without checks");
        a.handle(key('j'));
        a.handle(key('j'));
        assert_eq!(a.selected, 2, "j/k still move items");
        assert_eq!(
            a.handle(KeyEvent::from(KeyCode::Enter)),
            Some(Action::Resolve {
                id: "ccc".into(),
                choice: None,
                text: None
            })
        );
    }

    #[test]
    fn a_refresh_does_not_move_the_cursor_or_lose_your_place() {
        let mut a = checklist([false, false, false]);
        a.set_items(vec![item("other", "open", &[], ""), a.items[0].clone()]);
        a.handle(key('j'));
        let selected = a.current().unwrap().id.clone();
        a.handle(KeyEvent::from(KeyCode::Tab));
        a.handle(key('j'));
        a.doc_lines.set(50);
        a.handle(key('m'));
        a.handle(key('J'));
        let (check_sel, scroll) = (a.check_sel, a.scroll);

        // the same items arrive again in a different order, as a refresh may deliver them
        let mut items = a.items.clone();
        items.reverse();
        a.set_items(items);

        assert_eq!(a.current().unwrap().id, selected, "still on the same item");
        assert_eq!(a.check_sel, check_sel, "check cursor survives a refresh");
        assert_eq!(a.scroll, scroll, "reading position survives a refresh");
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
    fn reader_toggles_and_scrolls_within_the_document() {
        let mut a = app();
        assert_eq!(a.handle(key('m')), None);
        assert!(a.reader);
        a.doc_lines.set(30);

        a.handle(key('J'));
        a.handle(key('J'));
        assert_eq!(a.scroll, 2);
        a.handle(KeyEvent::from(KeyCode::PageDown));
        assert_eq!(a.scroll, 12);
        a.handle(key('G'));
        assert_eq!(a.scroll, 29, "G stops at the last line, never past it");
        a.handle(key('K'));
        assert_eq!(a.scroll, 28);
        a.handle(key('g'));
        assert_eq!(a.scroll, 0);
        a.handle(key('K'));
        assert_eq!(a.scroll, 0, "scrolling up at the top is a no-op");

        a.handle(key('J'));
        a.handle(key('j'));
        assert_eq!(a.scroll, 0, "a new item starts at the top of its document");

        a.handle(key('m'));
        assert!(!a.reader);
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
