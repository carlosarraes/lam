use anyhow::Result;
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyModifiers, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use std::cell::Cell;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::client::{Client, Item, Resolution};
use crate::config::Config;
use crate::watch;

mod draw;

const REFRESH: Duration = Duration::from_secs(30);
const HISTORY_PAGE: usize = 50;

/// What the UI asks the outside world to do; keeps `App` free of I/O so it is unit-testable.
#[derive(Debug, PartialEq)]
pub enum Action {
    Quit,
    Refresh,
    /// The history tab wants the page older than `before` (None asks for the newest page). The
    /// cursor rides along rather than being read back off `App`, so the loop stays dumb.
    LoadHistory {
        before: Option<String>,
    },
    Resolve {
        id: String,
        choice: Option<String>,
        text: Option<String>,
    },
    Dismiss(String),
    Seen {
        id: String,
        version: u64,
    },
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
    /// Live-filtering by title, agent display name, or body.
    Filter,
}

/// The queue you answer, and the record of what you already answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Requests,
    History,
}

/// One tab's list and its cursor. The tabs are independent views, so each keeps your place.
#[derive(Default)]
struct Pane {
    items: Vec<Item>,
    selected: usize,
}

pub struct App {
    tab: Tab,
    requests: Pane,
    history: Pane,
    /// `created_at` of the last row the server returned, however few of them we kept. Paging from
    /// the last *displayed* item would stall whenever a page ended on an open row.
    history_cursor: Option<String>,
    /// The server has nothing older.
    history_end: bool,
    /// A page is in flight. The network thread is serial, so there is at most one.
    history_loading: bool,
    /// The terminal granted the kitty flags, so Ctrl+1/Ctrl+2 actually arrive.
    kitty: bool,
    filter: String,
    /// Cursor within the selected item's checks.
    check_sel: usize,
    focus: Focus,
    /// A request is in flight on the network thread.
    busy: bool,
    /// Side-by-side markdown reader for long bodies (`m`).
    reader: bool,
    /// An explicitly opened FYI stays readable after it leaves the queue.
    reader_item: Option<Item>,
    seen_error: Option<String>,
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
            tab: Tab::Requests,
            requests: Pane::default(),
            history: Pane::default(),
            history_cursor: None,
            history_end: false,
            history_loading: false,
            kitty: false,
            filter: String::new(),
            check_sel: 0,
            focus: Focus::List,
            busy: false,
            reader: false,
            reader_item: None,
            seen_error: None,
            scroll: 0,
            doc_lines: Cell::new(0),
            mode: Mode::Normal,
            status: "connecting".into(),
            host,
        }
    }

    pub fn set_kitty(&mut self, kitty: bool) {
        self.kitty = kitty;
    }

    fn pane(&self) -> &Pane {
        match self.tab {
            Tab::Requests => &self.requests,
            Tab::History => &self.history,
        }
    }

    fn pane_mut(&mut self) -> &mut Pane {
        match self.tab {
            Tab::Requests => &mut self.requests,
            Tab::History => &mut self.history,
        }
    }

    /// Refreshes never move you: the same item stays selected, keeping its scroll position and
    /// check cursor. Only when it is gone (resolved, filtered away) does the cursor reset. This
    /// feeds the requests tab only — a refresh landing while you read history must not move it.
    pub fn set_items(&mut self, items: Vec<Item>) {
        let previous = self
            .visible_of(Tab::Requests)
            .get(self.requests.selected)
            .map(|i| i.id.clone());
        self.requests.items = items;
        let same = previous.and_then(|id| {
            self.visible_of(Tab::Requests)
                .iter()
                .position(|i| i.id == id)
        });
        match same {
            Some(pos) => {
                self.requests.selected = pos;
                if self.tab == Tab::Requests {
                    let checks = self.current().map_or(0, |i| i.checks.len());
                    self.check_sel = self.check_sel.min(checks.saturating_sub(1));
                }
            }
            None => self.clamp_of(Tab::Requests),
        }
    }

    /// Appends a page of closed items. Open rows are dropped — history is the closed side — and so
    /// are ids already held, because a worker that predates paging ignores the cursor and replays
    /// the whole table; a page that adds nothing is then how we learn there is nothing older.
    /// The server returns newest-first, so the last row of the page is the oldest.
    pub fn add_history(&mut self, page: Vec<Item>, end: bool) {
        self.history_loading = false;
        if let Some(last) = page.last() {
            self.history_cursor = Some(last.created_at.clone());
        }
        let before = self.history.items.len();
        for i in page {
            if i.status == "open" || self.history.items.iter().any(|h| h.id == i.id) {
                continue;
            }
            self.history.items.push(i);
        }
        self.history_end = end || self.history.items.len() == before;
        self.clamp_of(Tab::History);
    }

    /// Throws away the history pane so the next load starts from the newest page again.
    fn reset_history(&mut self) {
        self.history = Pane::default();
        self.history_cursor = None;
        self.history_end = false;
        self.history_loading = false;
    }

    pub fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
    }

    /// Clears the in-flight page without recording one, so a failed load can be retried.
    /// Deliberately not folded into `set_busy`: jobs queue, so an unrelated refresh can land
    /// while a page is still queued, and clearing the flag there would let a second request go
    /// out with the same cursor — whose duplicate rows would then look like the end of history.
    pub fn history_failed(&mut self) {
        self.history_loading = false;
    }

    /// Keeps a pane's cursor in range. The reader and check cursor follow the selected item, so
    /// they only reset when the pane being clamped is the one on screen.
    fn clamp_of(&mut self, tab: Tab) {
        let n = self.visible_of(tab).len();
        let pane = match tab {
            Tab::Requests => &mut self.requests,
            Tab::History => &mut self.history,
        };
        pane.selected = pane.selected.min(n.saturating_sub(1));
        if self.tab == tab && self.reader_item.is_none() {
            self.check_sel = 0;
            self.scroll = 0;
            self.focus = Focus::List;
        }
    }

    fn clamp(&mut self) {
        self.clamp_of(self.tab);
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
        self.actionable_current()
            .is_some_and(|i| !i.checks.is_empty())
    }

    fn nav_hint(&self) -> &'static str {
        match (self.reader, self.kitty) {
            (true, _) => "j/k move · J/K scroll · g/G top/end · m close · / filter · q quit",
            (false, true) => "h/l · ^1/^2 tabs · j/k move · m read · / filter · R refresh · q quit",
            (false, false) => "h/l tabs · j/k move · m read · / filter · R refresh · q quit",
        }
    }

    fn scroll_by(&mut self, delta: i32) {
        let max = self.doc_lines.get().saturating_sub(1) as i32;
        self.scroll = (self.scroll as i32 + delta).clamp(0, max.max(0)) as u16;
    }

    /// One tab's items matching the current filter on title, agent display name, or body. The
    /// filter is shared: it applies to whichever tab you are looking at.
    fn visible_of(&self, tab: Tab) -> Vec<&Item> {
        let items = match tab {
            Tab::Requests => &self.requests.items,
            Tab::History => &self.history.items,
        };
        if self.filter.is_empty() {
            return items.iter().collect();
        }
        let f = self.filter.to_lowercase();
        items
            .iter()
            .filter(|i| {
                draw::source(i).to_lowercase().contains(&f)
                    || i.title.to_lowercase().contains(&f)
                    || i.body.to_lowercase().contains(&f)
            })
            .collect()
    }

    fn visible(&self) -> Vec<&Item> {
        self.visible_of(self.tab)
    }

    pub fn set_status(&mut self, s: impl Into<String>) {
        self.status = s.into();
    }

    fn current(&self) -> Option<&Item> {
        self.reader_item
            .as_ref()
            .or_else(|| self.visible().get(self.pane().selected).copied())
    }

    fn leave_fyi_reader(&mut self) {
        if self.reader_item.take().is_some() {
            self.reader = false;
        }
    }

    fn open_reader(&mut self) -> Option<Action> {
        let item = self.current()?.clone();
        self.reader = true;
        self.scroll = 0;
        if !item.is_fyi() {
            return None;
        }
        let action = (item.status == "open").then(|| Action::Seen {
            id: item.id.clone(),
            version: item.version,
        });
        self.reader_item = Some(item);
        self.seen_error = None;
        action
    }

    fn seen_result(&mut self, item: Option<Item>, error: Option<String>) {
        if let Some(item) = item {
            if self
                .reader_item
                .as_ref()
                .is_some_and(|reader| reader.id == item.id)
            {
                self.reader_item = Some(item.clone());
            }
            if let Some(row) = self.requests.items.iter_mut().find(|row| row.id == item.id) {
                *row = item.clone();
            }
            if item.status != "open" {
                self.requests.items.retain(|row| row.id != item.id);
                self.clamp_of(Tab::Requests);
            }
        }
        self.set_busy(false);
        self.seen_error = error;
        self.status = self.seen_error.clone().unwrap_or_else(|| "live".into());
    }

    /// Switching keeps each tab's cursor. The reader, check cursor and focus follow the newly
    /// selected item, so they reset exactly as a cursor move does.
    fn set_tab(&mut self, tab: Tab) -> Option<Action> {
        if self.tab == tab {
            return None;
        }
        self.leave_fyi_reader();
        self.tab = tab;
        self.check_sel = 0;
        self.scroll = 0;
        self.focus = Focus::List;
        self.load_more()
    }

    /// Asks for the next history page when the tab is empty or the cursor nears the bottom.
    /// Prefetching three rows early means the list never visibly dead-ends.
    fn load_more(&mut self) -> Option<Action> {
        if self.tab != Tab::History || self.history_loading || self.history_end {
            return None;
        }
        let n = self.visible().len();
        if n != 0 && self.pane().selected + 3 < n {
            return None;
        }
        self.history_loading = true;
        Some(Action::LoadHistory {
            before: self.history_cursor.clone(),
        })
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
            // Above the choice arm below, or Ctrl+1 answers an item instead of switching tabs.
            // Only terminals speaking the kitty protocol report these at all; h/l is the fallback.
            KeyCode::Char('1') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.set_tab(Tab::Requests)
            }
            KeyCode::Char('2') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.set_tab(Tab::History)
            }
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
                self.leave_fyi_reader();
                if self.pane().selected + 1 < self.visible().len() {
                    self.pane_mut().selected += 1;
                    self.check_sel = 0;
                    self.scroll = 0;
                }
                self.load_more()
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.leave_fyi_reader();
                let up = self.pane().selected.saturating_sub(1);
                self.pane_mut().selected = up;
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
            KeyCode::Char('h') => self.set_tab(Tab::Requests),
            KeyCode::Char('l') => self.set_tab(Tab::History),
            KeyCode::Char('m') => {
                if self.reader {
                    self.reader = false;
                    self.reader_item = None;
                    self.scroll = 0;
                    None
                } else {
                    self.open_reader()
                }
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
                self.leave_fyi_reader();
                self.mode = Mode::Filter;
                None
            }
            // On history this rewinds to the newest page, so items you just closed appear.
            KeyCode::Char('R') if self.tab == Tab::History => {
                self.reset_history();
                self.load_more()
            }
            KeyCode::Char('R') => Some(Action::Refresh),
            KeyCode::Char('d') => self.open_current().map(|i| Action::Dismiss(i.id.clone())),
            KeyCode::Char('o') => self
                .current()
                .filter(|i| !i.link.is_empty())
                .map(|i| Action::OpenLink(i.link.clone())),
            KeyCode::Char('r') => {
                if self.actionable_current().is_some() {
                    self.mode = Mode::Reply(String::new());
                }
                None
            }
            KeyCode::Enter if self.current().is_some_and(Item::is_fyi) => {
                if self.reader_item.is_some() {
                    None
                } else {
                    self.open_reader()
                }
            }
            KeyCode::Enter => self
                .actionable_current()
                .filter(|i| i.choices.is_empty() && i.checks.is_empty())
                .map(|i| Action::Resolve {
                    id: i.id.clone(),
                    choice: None,
                    text: None,
                }),
            KeyCode::Char(c @ '1'..='3') => {
                let item = self.actionable_current()?;
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

    fn actionable_current(&self) -> Option<&Item> {
        self.current().filter(|i| i.is_actionable())
    }
}

/// Work handed to the network thread. Nothing in the UI thread ever calls the server, so a slow
/// or hung request cannot freeze the screen — and `q`/Ctrl-C keep working, which in raw mode they
/// only do if the loop is still reading keys.
enum Job {
    Refresh,
    History {
        before: Option<String>,
    },
    Resolve {
        id: String,
        choice: Option<String>,
        text: Option<String>,
    },
    Dismiss(String),
    Seen {
        id: String,
        version: u64,
    },
    SetCheck {
        id: String,
        index: usize,
        done: bool,
    },
}

enum Msg {
    Items(Vec<Item>),
    Seen {
        item: Option<Box<Item>>,
        error: Option<String>,
    },
    /// A page of items newest-first; `end` when the server returned fewer rows than we asked for.
    History {
        items: Vec<Item>,
        end: bool,
    },
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

fn mark_seen(client: &Client, id: &str, version: u64, tx: &mpsc::Sender<Msg>) {
    let (item, error) = match client.mark_seen(id, version) {
        Ok(item) => (Some(item), None),
        Err(error) => (client.show(id).ok(), Some(format!("seen failed: {error}"))),
    };
    let _ = tx.send(Msg::Seen {
        item: item.map(Box::new),
        error,
    });
}

pub fn run(silent: bool) -> Result<i32> {
    let cfg = Config::load()?;
    let client = Client::new(&cfg)?;
    let mut app = App::new(hostname::get()?.to_string_lossy().into_owned());

    let (tx, rx) = mpsc::channel::<Msg>();
    let (jobs, work) = mpsc::channel::<Job>();

    let net_tx = tx.clone();
    std::thread::spawn(move || {
        while let Ok(job) = work.recv() {
            // History short-circuits: every other job ends by re-reading the open queue, and a
            // page of closed items has no reason to pay for that.
            if let Job::History { before } = job {
                let _ = net_tx.send(match client.page(HISTORY_PAGE, before.as_deref()) {
                    Ok(items) => Msg::History {
                        end: items.len() < HISTORY_PAGE,
                        items,
                    },
                    Err(e) => Msg::Failed(format!("history failed: {e}")),
                });
                continue;
            }
            let outcome = match job {
                Job::Refresh => Ok(()),
                Job::Resolve { id, choice, text } => client
                    .resolve(&id, &Resolution { choice, text })
                    .map(|_| ()),
                Job::Dismiss(id) => client.dismiss(&id).map(|_| ()),
                Job::Seen { id, version } => {
                    mark_seen(&client, &id, version, &net_tx);
                    Ok(())
                }
                Job::SetCheck { id, index, done } => client.set_check(&id, index, done).map(|_| ()),
                Job::History { .. } => unreachable!("handled above"),
            };
            if let Err(e) = outcome {
                let _ = net_tx.send(Msg::Failed(format!("{e}")));
            }
            // Every job ends by re-reading the queue, so the screen always catches up.
            let _ = net_tx.send(match client.list(Some("open")) {
                Ok(items) => Msg::Items(items),
                Err(e) => Msg::Failed(format!("refresh failed: {e}")),
            });
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
    // Ctrl+1/Ctrl+2 have no legacy encoding — only the kitty protocol reports them at all. Ask
    // after init() so the query runs with raw mode already on; h/l works either way, so this is
    // pure upside. ratatui's panic hook restores the screen but knows nothing about the flags,
    // so wrap it to pop first or a panic strands the terminal in kitty mode.
    let kitty = crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
    if kitty {
        let _ = execute!(
            std::io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
            hook(info);
        }));
    }
    app.set_kitty(kitty);
    let result = event_loop(&mut terminal, &mut app, &jobs, &rx, silent);
    if kitty {
        let _ = execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
    }
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
    let refresh = || {
        let _ = jobs.send(Job::Refresh);
    };
    refresh();
    let mut last_refresh = Instant::now();
    loop {
        terminal.draw(|f| app.draw(f))?;

        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Items(items) => {
                    app.set_items(items);
                    app.set_busy(false);
                    app.set_status(app.seen_error.clone().unwrap_or_else(|| "live".into()));
                }
                Msg::Seen { item, error } => app.seen_result(item.map(|item| *item), error),
                Msg::History { items, end } => {
                    app.add_history(items, end);
                    app.set_busy(false);
                }
                Msg::Failed(e) => {
                    app.set_busy(false);
                    app.history_failed();
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
                    refresh();
                }
                Msg::Status(s) => app.set_status(s),
            }
        }
        if last_refresh.elapsed() > REFRESH {
            refresh();
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
            Action::Refresh => Job::Refresh,
            Action::LoadHistory { before } => Job::History { before },
            Action::Resolve { id, choice, text } => Job::Resolve { id, choice, text },
            Action::Dismiss(id) => Job::Dismiss(id),
            Action::Seen { id, version } => Job::Seen { id, version },
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
            kind: crate::client::ItemKind::Request,
            seen_at: None,
            name: "0:lam".into(),
            title: "t".into(),
            body: String::new(),
            source_host: "h".into(),
            source_project: "p".into(),
            priority: "normal".into(),
            choices: choices.iter().map(|s| s.to_string()).collect(),
            recommendation: None,
            recommended_choice: None,
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

    pub(in crate::tui) fn key(c: char) -> KeyEvent {
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

    pub(super) fn fyi() -> Item {
        let mut i = item("news1", "open", &[], "");
        i.kind = crate::client::ItemKind::Fyi;
        i.title = "Release published".into();
        i.body = "The deployment completed successfully.".into();
        i.version = 7;
        i
    }

    #[test]
    fn fyi_selection_is_inert_and_explicit_reader_open_marks_seen() {
        for open in [key('m'), KeyEvent::from(KeyCode::Enter)] {
            let mut a = App::new("host".into());
            a.set_items(vec![item("first", "open", &[], ""), fyi()]);
            assert_eq!(a.handle(key('j')), None);
            for c in ['1', '2', '3', 'r', ' '] {
                assert_eq!(a.handle(key(c)), None);
                assert_eq!(a.mode, Mode::Normal);
            }
            assert_eq!(a.handle(key('d')), Some(Action::Dismiss("news1".into())));
            assert_eq!(
                a.handle(open),
                Some(Action::Seen {
                    id: "news1".into(),
                    version: 7
                })
            );
            assert!(a.reader);
            assert_eq!(
                a.current().unwrap().body,
                "The deployment completed successfully."
            );
        }
    }

    #[test]
    fn fyi_reader_survives_queue_removal_until_closed() {
        let mut a = App::new("host".into());
        a.set_items(vec![fyi(), item("next1", "open", &[], "")]);
        a.handle(key('m'));
        a.doc_lines.set(40);
        a.handle(key('J'));
        a.set_items(vec![item("next1", "open", &[], "")]);
        assert!(a.reader);
        assert_eq!(a.current().unwrap().id, "news1");
        assert_eq!(a.scroll, 1);
        a.handle(key('m'));
        assert!(!a.reader);
        assert_eq!(a.current().unwrap().id, "next1");
    }

    #[test]
    fn fyi_seen_failure_keeps_item_open_and_reader_available() {
        let mut a = App::new("host".into());
        a.set_items(vec![fyi()]);
        a.handle(key('m'));
        a.seen_result(None, Some("seen failed: conflict".into()));
        a.set_items(vec![fyi()]);
        assert_eq!(a.requests.items[0].status, "open");
        assert!(a.requests.items[0].seen_at.is_none());
        assert!(a.reader);
        assert_eq!(a.current().unwrap().id, "news1");
        a.handle(key('m'));
        assert_eq!(
            a.handle(key('m')),
            Some(Action::Seen {
                id: "news1".into(),
                version: 7
            })
        );
    }

    #[test]
    fn fyi_enter_opens_after_navigating_from_a_request_reader() {
        let mut a = App::new("host".into());
        a.set_items(vec![item("req01", "open", &[], ""), fyi()]);
        a.handle(key('m'));
        assert_eq!(a.handle(key('j')), None);
        assert_eq!(
            a.handle(KeyEvent::from(KeyCode::Enter)),
            Some(Action::Seen {
                id: "news1".into(),
                version: 7
            })
        );
        assert_eq!(
            a.handle(KeyEvent::from(KeyCode::Enter)),
            None,
            "already opened"
        );
    }

    #[tokio::test]
    async fn fyi_seen_job_uses_canonical_response_and_refreshes_conflicts() {
        use wiremock::matchers::{body_json, header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        for conflict in [false, true] {
            let server = MockServer::start().await;
            let mut canonical = fyi();
            canonical.version = 8;
            if !conflict {
                canonical.status = "dismissed".into();
                canonical.seen_at = Some("2026-09-07T12:00:00Z".into());
            }
            let response = if conflict {
                ResponseTemplate::new(409)
            } else {
                ResponseTemplate::new(200).set_body_json(&canonical)
            };
            Mock::given(method("POST"))
                .and(path("/v2/items/news1/seen"))
                .and(header("authorization", "Bearer tok"))
                .and(body_json(serde_json::json!({"version": 7})))
                .respond_with(response)
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/items/news1"))
                .respond_with(ResponseTemplate::new(200).set_body_json(&canonical))
                .expect(if conflict { 1 } else { 0 })
                .mount(&server)
                .await;
            let cfg = Config {
                server: server.uri(),
                token: "tok".into(),
                topic: "top".into(),
                ntfy: None,
            };
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let client = Client::new(&cfg).unwrap();
                mark_seen(&client, "news1", 7, &tx);
            })
            .join()
            .unwrap();
            let mut a = App::new("host".into());
            a.set_items(vec![fyi()]);
            a.handle(key('m'));
            let Msg::Seen { item, error } = rx.recv().unwrap() else {
                panic!("expected seen result")
            };
            a.seen_result(item.map(|item| *item), error);
            assert_eq!(a.current().unwrap().version, 8);
            if conflict {
                assert_eq!(a.requests.items[0].status, "open");
                assert!(a.current().unwrap().seen_at.is_none());
                assert!(a.status.contains("409"));
            } else {
                assert!(a.requests.items.is_empty());
                assert_eq!(a.current().unwrap().status, "dismissed");
                assert!(a.current().unwrap().seen_at.is_some());
                assert!(a.current().unwrap().response_choice.is_none());
                assert!(a.current().unwrap().response_text.is_none());
            }
        }
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
        assert_eq!(a.requests.selected, 2);
        assert_eq!(a.handle(key('o')), None);
        assert_eq!(
            a.handle(key('a')),
            None,
            "the history tab replaced show-all"
        );
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
        assert_eq!(a.requests.selected, 2, "j/k still move items");
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
        a.set_items(vec![
            item("other", "open", &[], ""),
            a.requests.items[0].clone(),
        ]);
        a.handle(key('j'));
        let selected = a.current().unwrap().id.clone();
        a.handle(KeyEvent::from(KeyCode::Tab));
        a.handle(key('j'));
        a.doc_lines.set(50);
        a.handle(key('m'));
        a.handle(key('J'));
        let (check_sel, scroll) = (a.check_sel, a.scroll);

        // the same items arrive again in a different order, as a refresh may deliver them
        let mut items = a.requests.items.clone();
        items.reverse();
        a.set_items(items);

        assert_eq!(a.current().unwrap().id, selected, "still on the same item");
        assert_eq!(a.check_sel, check_sel, "check cursor survives a refresh");
        assert_eq!(a.scroll, scroll, "reading position survives a refresh");
    }

    #[test]
    fn slash_searches_body_and_display_name_without_losing_selection_clamping() {
        for query in ["release", "agent", "rationale", "h:p"] {
            let mut a = App::new("host".into());
            let mut target = item("match", "open", &["yes"], "");
            target.title = "Release approval".into();
            target.name = if query == "h:p" { "" } else { "Agent" }.into();
            target.body = "RATIONALE only in the body".into();
            let mut other = item("other", "open", &[], "");
            other.source_host = "elsewhere".into();
            a.set_items(vec![target, other]);
            a.handle(key('j'));
            a.handle(key('/'));
            for c in query.chars() {
                a.handle(key(c));
            }
            assert_eq!(a.visible().len(), 1, "query: {query}");
            assert_eq!(a.current().unwrap().id, "match", "query: {query}");
            a.handle(KeyEvent::from(KeyCode::Enter));
            assert_eq!(
                a.handle(key('1')),
                Some(Action::Resolve {
                    id: "match".into(),
                    choice: Some("yes".into()),
                    text: None,
                })
            );
            a.handle(key('/'));
            a.handle(key('z'));
            assert!(a.current().is_none());
            a.handle(KeyEvent::from(KeyCode::Esc));
            assert_eq!(a.visible().len(), 2);
        }
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
        assert_eq!(a.requests.selected, 0);
        a.set_items(vec![]);
        assert_eq!(a.handle(key('1')), None);
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    /// A closed item `n` minutes back, titled after its id so a filter can tell it from the queue.
    fn closed(id: &str, status: &str, minutes: i64) -> Item {
        let mut i = item(id, status, &[], "");
        i.title = id.into();
        i.created_at = (chrono::Utc::now() - chrono::Duration::minutes(minutes)).to_rfc3339();
        i
    }

    #[test]
    fn tabs_switch_with_h_l_and_ctrl_digits() {
        let mut a = app();
        assert_eq!(
            a.handle(key('l')),
            Some(Action::LoadHistory { before: None }),
            "the first visit loads the newest page"
        );
        assert_eq!(a.tab, Tab::History);
        assert_eq!(a.handle(key('l')), None, "already there");
        assert_eq!(a.handle(key('h')), None);
        assert_eq!(a.tab, Tab::Requests);

        assert!(a.handle(ctrl('2')).is_none(), "the page is already loading");
        assert_eq!(a.tab, Tab::History);
        a.handle(ctrl('1'));
        assert_eq!(a.tab, Tab::Requests);
    }

    #[test]
    fn ctrl_digits_do_not_pick_choices() {
        let mut a = app();
        assert_eq!(
            a.handle(ctrl('1')),
            None,
            "Ctrl+1 switches tabs; it must not answer the item"
        );
        assert_eq!(
            a.tab,
            Tab::Requests,
            "already on requests, so nothing moved"
        );
        // and the bare digit still answers
        assert_eq!(
            a.handle(key('1')),
            Some(Action::Resolve {
                id: "aaa".into(),
                choice: Some("yes".into()),
                text: None
            })
        );
    }

    #[test]
    fn history_holds_only_closed_items() {
        let mut a = app();
        a.add_history(
            vec![
                closed("op1", "open", 1),
                closed("res", "resolved", 2),
                closed("dis", "dismissed", 3),
                closed("ret", "retracted", 4),
                closed("exp", "expired", 5),
            ],
            true,
        );
        let ids: Vec<&str> = a.history.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["res", "dis", "ret", "exp"], "the open row is dropped");
    }

    #[test]
    fn history_pages_from_the_oldest_row_returned() {
        let mut a = app();
        // The page ends on an open row, which history drops. The cursor must still advance past
        // it, or the next request asks for the same page forever.
        let tail = closed("op1", "open", 9);
        a.add_history(vec![closed("res", "resolved", 2), tail.clone()], false);
        assert_eq!(a.history.items.len(), 1, "the open tail is not kept");
        assert_eq!(
            a.history_cursor.as_deref(),
            Some(tail.created_at.as_str()),
            "the cursor is the last raw row, not the last kept one"
        );
    }

    #[test]
    fn a_refresh_landing_mid_page_does_not_release_the_paging_guard() {
        let mut a = app();
        assert!(a.handle(key('l')).is_some());
        // an unrelated refresh completes while the history page is still queued
        a.set_items(vec![item("new", "open", &[], "")]);
        a.set_busy(false);
        assert!(a.history_loading, "the page is still in flight");
        assert_eq!(
            a.handle(key('j')),
            None,
            "a second request with the same cursor would look like the end of history"
        );
    }

    #[test]
    fn history_stops_when_a_page_adds_nothing() {
        let mut a = app();
        a.handle(key('l'));
        let page = vec![closed("res", "resolved", 2)];
        a.add_history(page.clone(), false);
        assert!(!a.history_end);
        // a worker that predates paging ignores the cursor and replays the same rows
        a.add_history(page, false);
        assert!(
            a.history_end,
            "a page that adds nothing means nothing older"
        );
        assert_eq!(a.handle(key('j')), None, "and paging stops asking");
    }

    #[test]
    fn history_pages_when_the_cursor_nears_the_bottom() {
        let mut a = app();
        a.handle(key('l'));
        let page: Vec<Item> = (0..HISTORY_PAGE)
            .map(|n| closed(&format!("i{n:03}"), "resolved", n as i64 + 1))
            .collect();
        a.add_history(page, false);
        for _ in 0..46 {
            assert_eq!(a.handle(key('j')), None, "still far from the bottom");
        }
        assert_eq!(a.pane().selected, 46);
        assert!(
            matches!(a.handle(key('j')), Some(Action::LoadHistory { .. })),
            "three rows from the end it fetches the next page"
        );
        assert_eq!(a.handle(key('j')), None, "and does not ask twice at once");
    }

    #[test]
    fn history_items_are_inert() {
        let mut a = app();
        a.handle(key('l'));
        a.add_history(vec![closed("res", "resolved", 2)], true);
        for k in ['1', 'd', 'r', ' '] {
            assert_eq!(
                a.handle(key(k)),
                None,
                "{k} must do nothing to a closed item"
            );
        }
        assert_eq!(a.handle(KeyEvent::from(KeyCode::Enter)), None);
    }

    #[test]
    fn each_tab_keeps_its_own_cursor() {
        let mut a = app();
        a.handle(key('j'));
        assert_eq!(a.requests.selected, 1);
        a.handle(key('l'));
        a.add_history(
            vec![closed("r1", "resolved", 2), closed("r2", "dismissed", 3)],
            true,
        );
        a.handle(key('j'));
        assert_eq!(a.history.selected, 1);
        a.handle(key('h'));
        assert_eq!(a.requests.selected, 1, "requests kept its place");
        a.handle(key('l'));
        assert_eq!(a.history.selected, 1, "and so did history");
    }

    #[test]
    fn a_refresh_does_not_move_the_history_cursor() {
        let mut a = app();
        a.handle(key('l'));
        a.add_history(
            vec![closed("r1", "resolved", 2), closed("r2", "dismissed", 3)],
            true,
        );
        a.handle(key('j'));
        a.set_items(vec![item("new", "open", &[], "")]);
        assert_eq!(a.history.selected, 1, "history stays where you left it");
        assert_eq!(a.tab, Tab::History);
    }

    #[test]
    fn the_filter_applies_to_the_active_tab_only() {
        let mut a = app();
        a.add_history(vec![closed("r1", "resolved", 2)], true);
        a.handle(key('/'));
        a.handle(key('t'));
        assert_eq!(
            a.visible_of(Tab::Requests).len(),
            3,
            "every queued item is titled t"
        );
        a.handle(key('l'));
        assert_eq!(
            a.visible().len(),
            0,
            "the history row is titled r1, so it hides"
        );
    }
}
