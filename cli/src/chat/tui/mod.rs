use std::collections::HashMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;

use super::{
    config::Paths,
    protocol,
    types::{FeedEvent, Message, SessionRef, Target},
};

mod draw;
mod readonly;
pub(crate) use readonly::ReadonlyFeed;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Send {
        recipients: Vec<SessionRef>,
        body: String,
    },
    Reply {
        message_id: String,
        body: String,
        all: bool,
    },
    LoadOlder,
    JumpLatest,
    Quit,
}

#[derive(Clone, Debug, Deserialize)]
struct RosterEntry {
    session: SessionRef,
    name: String,
    client: String,
    eligible: bool,
}

struct FeedRow {
    sequence: u64,
    message: Message,
    receipts: HashMap<SessionRef, String>,
    exposure: HashMap<SessionRef, String>,
}

#[derive(Default)]
struct PendingState {
    receipts: HashMap<SessionRef, String>,
    exposure: HashMap<SessionRef, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Focus {
    #[default]
    Recipients,
    Body,
    Feed,
}

pub struct App {
    focus: Focus,
    recipients: Vec<SessionRef>,
    body: String,
    search: String,
    roster: Vec<RosterEntry>,
    roster_stale: bool,
    broadcast_pending: bool,
    suggestion: usize,
    messages: Vec<FeedRow>,
    pending_events: HashMap<String, PendingState>,
    older_before: Option<u64>,
    older_end: bool,
    older_loading: bool,
    selected: usize,
    detail_scroll: u16,
    unseen: usize,
    following: bool,
    connection: String,
    status: String,
    pending: bool,
    retry_key: Option<(Action, String)>,
    reply_to: Option<(String, bool)>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            focus: Focus::Recipients,
            recipients: Vec::new(),
            body: String::new(),
            search: String::new(),
            roster: Vec::new(),
            roster_stale: false,
            broadcast_pending: false,
            suggestion: 0,
            messages: Vec::new(),
            pending_events: HashMap::new(),
            older_before: None,
            older_end: false,
            older_loading: false,
            selected: 0,
            detail_scroll: 0,
            unseen: 0,
            following: true,
            connection: "connecting".into(),
            status: String::new(),
            pending: false,
            retry_key: None,
            reply_to: None,
        }
    }
}

impl App {
    pub fn pending_recipient_count(&self) -> usize {
        self.recipients.len()
    }

    fn set_roster(&mut self, roster: Vec<RosterEntry>, stale: bool) {
        self.roster = roster;
        self.roster_stale = stale;
        self.broadcast_pending = false;
        self.suggestion = 0;
    }

    fn suggestions(&self) -> Vec<&RosterEntry> {
        let query = self.search.trim_start_matches('@').to_lowercase();
        let mut matches: Vec<_> = self
            .roster
            .iter()
            .filter(|entry| entry.eligible && fuzzy_match(&entry.name.to_lowercase(), &query))
            .collect();
        matches.sort_by_key(|entry| {
            (
                if entry.name.to_lowercase().starts_with(&query) {
                    0
                } else {
                    1
                },
                entry.name.to_lowercase(),
                entry.session.incarnation.clone(),
            )
        });
        matches
    }

    fn add_event(&mut self, event: FeedEvent) {
        let sequence = self
            .messages
            .last()
            .map(|row| row.sequence + 1)
            .unwrap_or(1);
        self.add_event_at(sequence, event, false);
    }

    fn add_event_at(&mut self, sequence: u64, event: FeedEvent, older: bool) {
        match event {
            FeedEvent::Message { message } => {
                if self.messages.iter().any(|row| row.message.id == message.id) {
                    return;
                }
                let state = self.pending_events.remove(&message.id).unwrap_or_default();
                let row = FeedRow {
                    sequence,
                    message,
                    receipts: state.receipts,
                    exposure: state.exposure,
                };
                let position = self
                    .messages
                    .partition_point(|existing| existing.sequence < sequence);
                self.messages.insert(position, row);
                if older && position <= self.selected && self.messages.len() > 1 {
                    self.selected += 1;
                } else if self.following && position + 1 == self.messages.len() {
                    self.selected = self.messages.len() - 1;
                    self.detail_scroll = 0;
                } else if !older {
                    self.unseen += 1;
                }
                if self.messages.len() > 1000 {
                    if older {
                        self.messages.pop();
                        self.selected = self.selected.min(self.messages.len() - 1);
                        self.older_end = true;
                    } else {
                        self.messages.remove(0);
                        self.selected = self.selected.saturating_sub(1);
                    }
                }
            }
            event => {
                let message_id = match &event {
                    FeedEvent::Receipt { message_id, .. }
                    | FeedEvent::Exposure { message_id, .. }
                    | FeedEvent::Fetched { message_id, .. } => message_id,
                    FeedEvent::Message { .. } => unreachable!(),
                };
                if let Some(row) = self
                    .messages
                    .iter_mut()
                    .find(|row| row.message.id == *message_id)
                {
                    row.apply_event(&event);
                } else if self.pending_events.len() < 1000
                    || self.pending_events.contains_key(message_id)
                {
                    self.pending_events
                        .entry(message_id.clone())
                        .or_default()
                        .apply_event(&event);
                }
            }
        }
    }

    fn add_page(&mut self, page: &serde_json::Value, older: bool) {
        if older {
            self.older_loading = false;
            self.following = false;
            self.older_before = page["before"].as_u64();
            self.older_end = page["more"] != true || self.messages.len() >= 1000;
        }
        if let Some(events) = page["events"].as_array() {
            for item in events {
                if let (Some(sequence), Ok(event)) = (
                    item["sequence"].as_u64(),
                    serde_json::from_value::<FeedEvent>(item["event"].clone()),
                ) {
                    self.add_event_at(sequence, event, older);
                }
            }
        }
    }

    fn send_finished(&mut self, action: &Action, key: &str, success: bool) {
        self.pending = false;
        if success {
            self.retry_key = None;
            match action {
                Action::Send { body, .. } | Action::Reply { body, .. } if self.body == *body => {
                    self.body.clear();
                    self.reply_to = None;
                }
                _ => {}
            }
        } else {
            self.retry_key = Some((action.clone(), key.into()));
        }
    }

    fn send_key(&self, action: &Action) -> String {
        self.retry_key
            .as_ref()
            .filter(|(previous, _)| previous == action)
            .map(|(_, key)| key.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Some(Action::Quit);
        }
        match (self.focus, key.code, key.modifiers) {
            (Focus::Body, KeyCode::Enter, KeyModifiers::NONE) => {
                if self.body.trim().is_empty() || self.pending {
                    return None;
                }
                if let Some((message_id, all)) = &self.reply_to {
                    self.pending = true;
                    return Some(Action::Reply {
                        message_id: message_id.clone(),
                        body: self.body.clone(),
                        all: *all,
                    });
                }
                if self.recipients.is_empty() {
                    return None;
                }
                self.pending = true;
                Some(Action::Send {
                    recipients: self.recipients.clone(),
                    body: self.body.clone(),
                })
            }
            (Focus::Recipients, KeyCode::Enter, KeyModifiers::NONE) => {
                if self.search.trim().is_empty() {
                    return None;
                }
                if self.search == "@all" {
                    if self.roster_stale && !self.broadcast_pending {
                        self.broadcast_pending = true;
                        self.status =
                            "Roster is stale. Press Enter again to send to its known members."
                                .into();
                        return None;
                    }
                    for entry in self.roster.iter().filter(|entry| entry.eligible) {
                        if !self.recipients.contains(&entry.session) {
                            self.recipients.push(entry.session.clone());
                        }
                    }
                    self.search.clear();
                    self.broadcast_pending = false;
                    return None;
                }
                let chosen = self
                    .suggestions()
                    .get(self.suggestion)
                    .map(|entry| entry.session.clone());
                if let Some(session) = chosen {
                    if !self.recipients.contains(&session) {
                        self.recipients.push(session);
                    }
                    self.search.clear();
                    self.suggestion = 0;
                }
                None
            }
            (_, KeyCode::Enter, _) => None,
            (Focus::Body, KeyCode::Char('j'), KeyModifiers::CONTROL) => {
                if self.body.len() < 65_536 {
                    self.body.push('\n');
                }
                None
            }
            (Focus::Body, KeyCode::Char(c), modifiers)
                if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT =>
            {
                if self.body.len() + c.len_utf8() <= 65_536 {
                    self.body.push(c);
                }
                None
            }
            (Focus::Body, KeyCode::Backspace, _) => {
                self.body.pop();
                None
            }
            (Focus::Recipients, KeyCode::Char(c), modifiers)
                if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT =>
            {
                self.search.push(c);
                self.suggestion = 0;
                self.broadcast_pending = false;
                None
            }
            (Focus::Recipients, KeyCode::Backspace, _) => {
                if self.search.is_empty() {
                    self.recipients.pop();
                } else {
                    self.search.pop();
                }
                self.suggestion = 0;
                None
            }
            (Focus::Recipients, KeyCode::Down, _) => {
                self.suggestion =
                    (self.suggestion + 1).min(self.suggestions().len().saturating_sub(1));
                None
            }
            (Focus::Recipients, KeyCode::Up, _) => {
                self.suggestion = self.suggestion.saturating_sub(1);
                None
            }
            (_, KeyCode::Tab, _) => {
                self.focus = match self.focus {
                    Focus::Recipients => Focus::Body,
                    Focus::Body => Focus::Feed,
                    Focus::Feed => Focus::Recipients,
                };
                None
            }
            (_, KeyCode::Esc, _) => {
                self.focus = Focus::Feed;
                None
            }
            (Focus::Feed, KeyCode::Down | KeyCode::Char('j'), _) => {
                self.selected = (self.selected + 1).min(self.messages.len().saturating_sub(1));
                self.detail_scroll = 0;
                self.following = self.selected + 1 == self.messages.len();
                if self.following {
                    self.unseen = 0;
                }
                None
            }
            (Focus::Feed, KeyCode::Up | KeyCode::Char('k'), _) => {
                self.selected = self.selected.saturating_sub(1);
                self.detail_scroll = 0;
                self.following = false;
                None
            }
            (Focus::Feed, KeyCode::PageDown, _) => {
                self.detail_scroll = self.detail_scroll.saturating_add(8);
                None
            }
            (Focus::Feed, KeyCode::PageUp, _) => {
                self.detail_scroll = self.detail_scroll.saturating_sub(8);
                None
            }
            (Focus::Feed, KeyCode::Char('G'), _) => {
                self.selected = self.messages.len().saturating_sub(1);
                self.following = true;
                self.unseen = 0;
                Some(Action::JumpLatest)
            }
            (Focus::Feed, KeyCode::Char('r' | 'a'), _) => {
                if let Some(row) = self.messages.get(self.selected) {
                    self.reply_to = Some((row.message.id.clone(), key.code == KeyCode::Char('a')));
                    self.body.clear();
                    self.focus = Focus::Body;
                }
                None
            }
            (Focus::Feed, KeyCode::Home, _) => Some(Action::LoadOlder),
            (Focus::Feed, KeyCode::Char('q'), _) => Some(Action::Quit),
            _ => None,
        }
    }
}

impl FeedRow {
    fn apply_event(&mut self, event: &FeedEvent) {
        apply_delivery_event(&mut self.receipts, &mut self.exposure, event);
    }
}

impl PendingState {
    fn apply_event(&mut self, event: &FeedEvent) {
        apply_delivery_event(&mut self.receipts, &mut self.exposure, event);
    }
}

fn apply_delivery_event(
    receipts: &mut HashMap<SessionRef, String>,
    exposure: &mut HashMap<SessionRef, String>,
    event: &FeedEvent,
) {
    match event {
        FeedEvent::Receipt {
            recipient, state, ..
        } => {
            receipts.insert(recipient.clone(), state.clone());
        }
        FeedEvent::Exposure {
            recipient, state, ..
        } => {
            exposure.insert(recipient.clone(), state.clone());
        }
        FeedEvent::Fetched { recipient, .. } => {
            exposure.insert(recipient.clone(), "fetched".into());
        }
        FeedEvent::Message { .. } => {}
    }
}

fn fuzzy_match(candidate: &str, query: &str) -> bool {
    if query.is_empty() || candidate.contains(query) {
        return true;
    }
    let mut letters = candidate.chars();
    query
        .chars()
        .all(|needle| letters.by_ref().any(|letter| letter == needle))
}

enum Network {
    ReadonlyInit(String, Option<serde_json::Value>),
    ReadonlyError(String),
    Feed(serde_json::Value),
    Older(serde_json::Value),
    OlderFailed(String),
    Roster(Vec<RosterEntry>, bool),
    Completed(Action, String, anyhow::Result<()>),
    Status(String),
}

enum Work {
    Send(Action, String),
    RefreshRoster,
    LoadOlder(u64),
}

fn roster_update(paths: &Paths, project: &str) -> Option<Network> {
    let page = super::commands::observer_request(
        paths,
        protocol::Operation::Sessions {
            project: project.into(),
        },
    )
    .ok()?;
    let roster = serde_json::from_value(page["sessions"].clone()).ok()?;
    Some(Network::Roster(roster, page["stale"] == true))
}

pub fn run(paths: &Paths, project: &str) -> anyhow::Result<()> {
    use anyhow::Context;
    let initial = super::commands::observer_request(
        paths,
        protocol::Operation::Sessions {
            project: project.into(),
        },
    )?;
    let mut app = App::default();
    let roster: Vec<RosterEntry> =
        serde_json::from_value(initial["sessions"].clone()).context("invalid Chat roster")?;
    app.set_roster(roster, initial["stale"] == true);
    let tail = super::commands::observer_request(
        paths,
        protocol::Operation::HistoryTail {
            project: project.into(),
            before: None,
            limit: 100,
        },
    )?;
    app.older_before = tail["before"].as_u64();
    app.older_end = tail["more"] != true;
    app.add_page(&tail, false);
    let cursor = tail["subscribe_cursor"]
        .as_str()
        .context("missing live Chat cursor")?
        .to_owned();

    let (network_tx, network_rx) = mpsc::channel();
    let (work_tx, work_rx) = mpsc::channel();
    spawn_subscription(
        paths.clone(),
        project.to_owned(),
        Some(cursor),
        network_tx.clone(),
    );
    spawn_worker(paths.clone(), project.to_owned(), work_rx, network_tx);

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app, project, &network_rx, &work_tx);
    ratatui::restore();
    result
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    project: &str,
    network: &mpsc::Receiver<Network>,
    work: &mpsc::Sender<Work>,
) -> anyhow::Result<()> {
    let mut last_roster = Instant::now();
    loop {
        while let Ok(message) = network.try_recv() {
            match message {
                Network::ReadonlyInit(_, _) | Network::ReadonlyError(_) => {}
                Network::Feed(page) => app.add_page(&page, false),
                Network::Older(page) => app.add_page(&page, true),
                Network::OlderFailed(error) => {
                    app.older_loading = false;
                    app.status = format!("older history failed: {error}");
                }
                Network::Roster(roster, stale) => app.set_roster(roster, stale),
                Network::Completed(action, key, result) => {
                    let success = result.is_ok();
                    app.send_finished(&action, &key, success);
                    app.status = match result {
                        Ok(()) => "queued; waiting for native receipt".into(),
                        Err(error) => {
                            format!("send not confirmed; Enter retries same key: {error}")
                        }
                    };
                }
                Network::Status(status) => app.connection = status,
            }
        }
        if last_roster.elapsed() >= Duration::from_secs(15) {
            let _ = work.send(Work::RefreshRoster);
            last_roster = Instant::now();
        }
        terminal.draw(|frame| app.draw(frame, project))?;
        if !event::poll(Duration::from_millis(150))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind == event::KeyEventKind::Press => {
                if let Some(action) = app.handle_key(key) {
                    match action {
                        Action::Quit => return Ok(()),
                        Action::Send { .. } | Action::Reply { .. } => {
                            let key = app.send_key(&action);
                            if work.send(Work::Send(action, key)).is_err() {
                                app.pending = false;
                                app.status = "Chat sender stopped".into();
                            }
                        }
                        Action::LoadOlder => {
                            if app.older_end {
                                app.status = if app.messages.len() >= 1000 {
                                    "This view keeps 1000 rows; use chat history for more.".into()
                                } else {
                                    "Beginning of retained history.".into()
                                };
                            } else if !app.older_loading {
                                if let Some(before) = app.older_before {
                                    app.older_loading = true;
                                    if work.send(Work::LoadOlder(before)).is_err() {
                                        app.older_loading = false;
                                        app.status = "Chat history worker stopped".into();
                                    }
                                }
                            }
                        }
                        Action::JumpLatest => {}
                    }
                }
            }
            Event::Paste(text) if app.focus == Focus::Body => {
                let available = 65_536usize.saturating_sub(app.body.len());
                let mut end = available.min(text.len());
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                app.body.push_str(&text[..end]);
            }
            _ => {}
        }
    }
}

fn spawn_worker(
    paths: Paths,
    project: String,
    jobs: mpsc::Receiver<Work>,
    output: mpsc::Sender<Network>,
) {
    std::thread::spawn(move || {
        while let Ok(job) = jobs.recv() {
            match job {
                Work::LoadOlder(before) => {
                    let outcome = super::commands::observer_request(
                        &paths,
                        protocol::Operation::HistoryTail {
                            project: project.clone(),
                            before: Some(before),
                            limit: 100,
                        },
                    );
                    let _ = output.send(match outcome {
                        Ok(page) => Network::Older(page),
                        Err(error) => Network::OlderFailed(error.to_string()),
                    });
                }
                Work::RefreshRoster => {
                    if let Some(update) = roster_update(&paths, &project) {
                        let _ = output.send(update);
                    }
                }
                Work::Send(action, key) => {
                    let operation = match &action {
                        Action::Send { recipients, body } => protocol::Operation::Send {
                            draft: super::types::Draft {
                                key: key.clone(),
                                project: project.clone(),
                                to: recipients.iter().cloned().map(Target::Agent).collect(),
                                body: body.clone(),
                                reply_to: None,
                            },
                        },
                        Action::Reply {
                            message_id,
                            body,
                            all,
                        } => protocol::Operation::Reply {
                            id: message_id.clone(),
                            key: key.clone(),
                            body: body.clone(),
                            all: *all,
                        },
                        _ => continue,
                    };
                    let result = super::commands::observer_request(&paths, operation.clone())
                        .or_else(|_| super::commands::observer_request(&paths, operation))
                        .map(|_| ());
                    let _ = output.send(Network::Completed(action, key, result));
                }
            }
        }
    });
}

#[cfg(unix)]
fn spawn_subscription(
    paths: Paths,
    project: String,
    mut cursor: Option<String>,
    output: mpsc::Sender<Network>,
) {
    use std::os::unix::net::UnixStream;
    std::thread::spawn(move || {
        let mut delay = Duration::from_millis(250);
        loop {
            let connected = (|| -> anyhow::Result<UnixStream> {
                super::daemon::validate_socket(&paths.observer_socket)?;
                let mut stream = UnixStream::connect(&paths.observer_socket)?;
                super::daemon::peer_identity(&stream)?;
                protocol::write_frame(
                    &mut stream,
                    &serde_json::to_value(protocol::Request {
                        version: 1,
                        operation: protocol::Operation::Subscribe {
                            project: project.clone(),
                            cursor: cursor.clone(),
                            limit: 100,
                        },
                    })?,
                )?;
                Ok(stream)
            })();
            if let Ok(mut stream) = connected {
                let _ = output.send(Network::Status("live".into()));
                delay = Duration::from_millis(250);
                while let Ok(response) = protocol::read_frame(&mut stream) {
                    if response["version"] != 1 || response["ok"] != true {
                        break;
                    }
                    let page = response["data"].clone();
                    let Some(next_cursor) = page["cursor"].as_str() else {
                        break;
                    };
                    cursor = Some(next_cursor.into());
                    if output.send(Network::Feed(page)).is_err() {
                        return;
                    }
                }
            }
            if output.send(Network::Status("reconnecting".into())).is_err() {
                return;
            }
            std::thread::sleep(delay);
            delay = (delay * 2).min(Duration::from_secs(5));
        }
    });
}

#[cfg(not(unix))]
fn spawn_subscription(
    _paths: Paths,
    _project: String,
    _cursor: Option<String>,
    _output: mpsc::Sender<Network>,
) {
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::types::{Actor, Draft};

    #[test]
    fn readonly_feed_never_enters_composer_or_sends() {
        let mut feed = ReadonlyFeed::default();
        feed.app.add_event(FeedEvent::Message {
            message: message("first", "hello"),
        });
        for key in ['r', 'a', 'x'] {
            feed.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
        }
        feed.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(feed.app.focus, Focus::Recipients);
        assert!(feed.app.body.is_empty());
        assert!(feed.app.reply_to.is_none());
        assert!(!feed.app.pending);
    }

    #[test]
    fn readonly_feed_preserves_selection_when_a_message_arrives() {
        let mut feed = ReadonlyFeed::default();
        feed.app.add_event(FeedEvent::Message {
            message: message("first", "one"),
        });
        feed.app.add_event(FeedEvent::Message {
            message: message("second", "two"),
        });
        feed.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        feed.app.add_event(FeedEvent::Message {
            message: message("third", "three"),
        });
        assert_eq!(feed.app.messages[feed.app.selected].message.id, "first");
        assert_eq!(feed.app.unseen, 1);
    }

    fn message(id: &str, body: &str) -> Message {
        Message {
            id: id.into(),
            sender: Actor::Agent(SessionRef {
                machine: "11111111-1111-4111-8111-111111111111".into(),
                incarnation: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into(),
            }),
            sender_seq: 1,
            created_at: "2026-09-20T19:00:00Z".into(),
            draft: Draft {
                key: "test".into(),
                project: "22222222-2222-4222-8222-222222222222".into(),
                to: vec![Target::Human {
                    machine: "11111111-1111-4111-8111-111111111111".into(),
                }],
                body: body.into(),
                reply_to: None,
            },
        }
    }
    #[test]
    fn enter_cannot_send_without_recipients() {
        let mut app = App::default();
        assert_eq!(app.pending_recipient_count(), 0);
        assert!(app
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .is_none());
        app.focus = Focus::Body;
        app.body = "an otherwise complete message".into();
        assert!(app
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .is_none());
    }

    #[test]
    fn mention_completion_pins_exact_id_across_roster_changes() {
        let mut app = App::default();
        let first = SessionRef {
            machine: "11111111-1111-4111-8111-111111111111".into(),
            incarnation: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into(),
        };
        let other = SessionRef {
            machine: first.machine.clone(),
            incarnation: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb".into(),
        };
        app.set_roster(
            vec![RosterEntry {
                session: first.clone(),
                name: "pm".into(),
                client: "codex".into(),
                eligible: true,
            }],
            false,
        );
        app.search = "@p".into();
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.pending_recipient_count(), 1);
        app.set_roster(
            vec![RosterEntry {
                session: other,
                name: "pm".into(),
                client: "pi".into(),
                eligible: true,
            }],
            false,
        );
        app.focus = Focus::Body;
        app.body = "pause the others".into();
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(Action::Send {
                recipients: vec![first],
                body: "pause the others".into()
            })
        );
    }

    #[test]
    fn only_explicit_at_all_expands_the_roster() {
        let first = SessionRef {
            machine: "11111111-1111-4111-8111-111111111111".into(),
            incarnation: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into(),
        };
        let second = SessionRef {
            machine: first.machine.clone(),
            incarnation: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb".into(),
        };
        let mut app = App::default();
        app.set_roster(
            vec![
                RosterEntry {
                    session: first.clone(),
                    name: "all".into(),
                    client: "pi".into(),
                    eligible: true,
                },
                RosterEntry {
                    session: second.clone(),
                    name: "pm".into(),
                    client: "codex".into(),
                    eligible: true,
                },
            ],
            false,
        );
        app.search = "all".into();
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.recipients, vec![first.clone()]);
        app.recipients.clear();
        app.search = "@all".into();
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.recipients, vec![first, second]);
    }

    #[test]
    fn body_mentions_never_add_recipients() {
        let mut app = App {
            focus: Focus::Body,
            body: "@all please pause".into(),
            ..App::default()
        };
        assert_eq!(app.pending_recipient_count(), 0);
        assert!(app
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .is_none());
    }

    #[test]
    fn incoming_feed_keeps_selected_row_and_counts_unseen_until_jump() {
        let mut app = App::default();
        app.add_event(FeedEvent::Message {
            message: message("first", "one"),
        });
        app.add_event(FeedEvent::Message {
            message: message("second", "two"),
        });
        app.focus = Focus::Feed;
        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.messages[app.selected].message.id, "first");
        app.add_event(FeedEvent::Message {
            message: message("third", "three"),
        });
        assert_eq!(app.messages[app.selected].message.id, "first");
        assert_eq!(app.unseen, 1);
        app.handle_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT));
        assert_eq!(app.messages[app.selected].message.id, "third");
        assert_eq!(app.unseen, 0);
    }

    #[test]
    fn older_page_preserves_selection_and_applies_later_receipts() {
        let mut app = App::default();
        let recipient = SessionRef {
            machine: "11111111-1111-4111-8111-111111111111".into(),
            incarnation: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into(),
        };
        app.add_page(&serde_json::json!({"events": [
            {"sequence": 2, "event": FeedEvent::Message { message: message("second", "two") }},
            {"sequence": 3, "event": FeedEvent::Receipt { message_id: "first".into(), recipient: recipient.clone(), attempt_id: None, state: "accepted".into(), outcome: None }},
        ]}), false);
        assert_eq!(app.messages[app.selected].message.id, "second");
        app.add_page(
            &serde_json::json!({"before":1,"more":false,"events":[
                {"sequence":1,"event":FeedEvent::Message { message: message("first", "one") }}
            ]}),
            true,
        );
        assert_eq!(
            app.messages
                .iter()
                .map(|row| row.message.id.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        assert_eq!(app.messages[app.selected].message.id, "second");
        assert_eq!(
            app.messages[0].receipts.get(&recipient).map(String::as_str),
            Some("accepted")
        );
        assert!(app.older_end);
    }

    #[test]
    fn explicit_reply_uses_selected_message_and_retains_body_until_success() {
        let mut app = App::default();
        app.add_event(FeedEvent::Message {
            message: message("first", "one"),
        });
        app.focus = Focus::Feed;
        app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        app.body = "reply".into();
        let action = app
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(
            action,
            Action::Reply {
                message_id: "first".into(),
                body: "reply".into(),
                all: false
            }
        );
        assert_eq!(app.body, "reply");
        app.send_finished(&action, "retry-key", false);
        assert_eq!(app.body, "reply");
        let again = app
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(again, action);
        assert_eq!(app.send_key(&again), "retry-key");
        app.send_finished(&again, "retry-key", true);
        assert!(app.body.is_empty());
    }
}
