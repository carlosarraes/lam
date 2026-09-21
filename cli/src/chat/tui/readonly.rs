use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{layout::Rect, Frame};

use super::{roster_update, spawn_subscription, App, Network};
use crate::chat::{config, protocol};

/// Main-TUI observer. It has no composer or participant credential path.
#[derive(Default)]
pub(crate) struct ReadonlyFeed {
    pub(super) app: App,
    started: bool,
    network: Option<mpsc::Receiver<Network>>,
    project: Option<String>,
    error: Option<String>,
}

impl ReadonlyFeed {
    pub(crate) fn start(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        let (tx, rx) = mpsc::channel();
        self.network = Some(rx);
        std::thread::spawn(move || {
            if let Err(error) = start_observer(tx.clone()) {
                let _ = tx.send(Network::ReadonlyError(error.to_string()));
            }
        });
    }

    pub(crate) fn poll(&mut self) {
        let Some(network) = &self.network else {
            return;
        };
        while let Ok(message) = network.try_recv() {
            match message {
                Network::ReadonlyInit(project, page) => {
                    self.project = Some(project);
                    if let Some(page) = page {
                        self.app.add_page(&page, false);
                    }
                }
                Network::ReadonlyError(error) => self.error = Some(error),
                Network::Feed(page) => self.app.add_page(&page, false),
                Network::Roster(roster, stale) => self.app.set_roster(roster, stale),
                Network::Status(status) => self.app.connection = status,
                _ => {}
            }
        }
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.app.selected =
                    (self.app.selected + 1).min(self.app.messages.len().saturating_sub(1));
                self.app.detail_scroll = 0;
                self.app.following = self.app.selected + 1 == self.app.messages.len();
                if self.app.following {
                    self.app.unseen = 0;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.app.selected = self.app.selected.saturating_sub(1);
                self.app.detail_scroll = 0;
                self.app.following = false;
            }
            KeyCode::PageDown | KeyCode::Char('J') => {
                self.app.detail_scroll = self.app.detail_scroll.saturating_add(8);
            }
            KeyCode::PageUp | KeyCode::Char('K') => {
                self.app.detail_scroll = self.app.detail_scroll.saturating_sub(8);
            }
            KeyCode::Char('G') => {
                self.app.selected = self.app.messages.len().saturating_sub(1);
                self.app.following = true;
                self.app.unseen = 0;
                self.app.detail_scroll = 0;
            }
            _ => {}
        }
    }

    pub(crate) fn draw(&self, frame: &mut Frame, area: Rect) {
        self.app
            .draw_readonly(frame, area, self.project.as_deref(), self.error.as_deref());
    }
}

fn start_observer(tx: mpsc::Sender<Network>) -> Result<()> {
    let paths = config::Paths::discover()?;
    let cfg = config::Config::load_existing(&paths.config)
        .context("Chat is not configured; run `lam chat status` to set it up")?;
    let project = select_project(&cfg, &std::env::current_dir()?)?;
    let label = cfg
        .projects
        .iter()
        .find(|mapping| mapping.id == project)
        .and_then(|mapping| mapping.roots.first())
        .and_then(|root| root.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| project.clone());
    let tail = super::super::commands::observer_request(
        &paths,
        protocol::Operation::HistoryTail {
            project: project.clone(),
            before: None,
            limit: 100,
        },
    );
    let cursor = tail
        .as_ref()
        .ok()
        .and_then(|page| page["subscribe_cursor"].as_str().map(str::to_owned));
    let _ = tx.send(Network::ReadonlyInit(label, tail.ok()));
    spawn_roster(paths.clone(), project.clone(), tx.clone());
    spawn_subscription(paths, project, cursor, tx);
    Ok(())
}

fn spawn_roster(paths: config::Paths, project: String, output: mpsc::Sender<Network>) {
    std::thread::spawn(move || loop {
        if let Some(update) = roster_update(&paths, &project) {
            if output.send(update).is_err() {
                return;
            }
        }
        std::thread::sleep(Duration::from_secs(15));
    });
}

fn select_project(cfg: &config::Config, cwd: &Path) -> Result<String> {
    let cwd = cwd.canonicalize()?;
    let mut matches = Vec::new();
    for project in &cfg.projects {
        for root in &project.roots {
            if let Ok(root) = root.canonicalize() {
                if cwd.starts_with(&root) {
                    matches.push((root.components().count(), project.id.clone()));
                }
            }
        }
    }
    matches.sort_by_key(|(depth, _)| *depth);
    if let Some((_, id)) = matches.pop() {
        return Ok(id);
    }
    if cfg.projects.len() == 1 {
        return Ok(cfg.projects[0].id.clone());
    }
    if cfg.projects.is_empty() {
        bail!("No Chat project is configured; map a project in chat.toml");
    }
    bail!("Several Chat projects are configured; open `lam` inside one mapped project")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::types::SessionRef;

    #[test]
    fn readonly_roster_update_names_agents() {
        let mut feed = ReadonlyFeed::default();
        let (tx, rx) = mpsc::channel();
        feed.network = Some(rx);
        tx.send(Network::Roster(
            vec![super::super::RosterEntry {
                session: SessionRef {
                    machine: "11111111-1111-4111-8111-111111111111".into(),
                    incarnation: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into(),
                },
                name: "pm".into(),
                client: "codex".into(),
                eligible: true,
            }],
            false,
        ))
        .unwrap();
        feed.poll();
        assert_eq!(feed.app.roster[0].name, "pm");
    }

    #[test]
    fn sole_project_is_selected_outside_its_root() {
        let cfg = config::Config {
            schema_version: 1,
            machine: "11111111-1111-4111-8111-111111111111".into(),
            inline_bytes: 4096,
            batch_bytes: 8192,
            projects: vec![config::ProjectMapping {
                id: "22222222-2222-4222-8222-222222222222".into(),
                roots: vec![std::env::temp_dir()],
            }],
            peers: vec![],
        };
        assert_eq!(
            select_project(&cfg, Path::new("/")).unwrap(),
            cfg.projects[0].id
        );
    }

    #[test]
    fn deepest_mapping_wins_and_unmapped_multiple_projects_are_ambiguous() {
        let cwd = std::env::current_dir().unwrap();
        let cfg = config::Config {
            schema_version: 1,
            machine: "11111111-1111-4111-8111-111111111111".into(),
            inline_bytes: 4096,
            batch_bytes: 8192,
            projects: vec![
                config::ProjectMapping {
                    id: "22222222-2222-4222-8222-222222222222".into(),
                    roots: vec![cwd.parent().unwrap().to_path_buf()],
                },
                config::ProjectMapping {
                    id: "33333333-3333-4333-8333-333333333333".into(),
                    roots: vec![cwd.clone()],
                },
            ],
            peers: vec![],
        };
        assert_eq!(select_project(&cfg, &cwd).unwrap(), cfg.projects[1].id);
        assert!(select_project(&cfg, Path::new("/")).is_err());
    }
}
