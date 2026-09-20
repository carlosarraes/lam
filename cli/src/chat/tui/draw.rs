use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use super::{App, Focus, RosterEntry};
use crate::chat::types::{Actor, SessionRef, Target};

const ACCENT: Style = Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD);
const DIM: Style = Style::new().fg(Color::DarkGray);
const META: Style = Style::new().fg(Color::Gray);

impl App {
    pub(super) fn draw(&self, frame: &mut Frame, project: &str) {
        let area = frame.area();
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(8),
                Constraint::Length(2),
            ])
            .split(area);
        let label = project.rsplit(':').next().unwrap_or(project);
        let short = &label[..label.len().min(12)];
        let header = Paragraph::new(Line::from(vec![
            Span::styled("LAM", ACCENT),
            Span::raw("  /  chat"),
            Span::styled(format!("    project {short}"), META),
            Span::styled(format!("    {}", self.connection), META),
            Span::styled(format!("    {}", self.status), META),
        ]))
        .block(Block::default().borders(Borders::BOTTOM));
        frame.render_widget(header, vertical[0]);

        let content = if area.width >= 88 {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
                .split(vertical[1])
        } else {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
                .split(vertical[1])
        };
        let rows: Vec<_> = self
            .messages
            .iter()
            .map(|row| {
                let who = match &row.message.sender {
                    Actor::Human { .. } => "Carlos".into(),
                    Actor::Agent(session) => self.name_of(session),
                };
                let preview = sanitize(&row.message.draft.body).replace('\n', " ");
                let preview: String = preview.chars().take(75).collect();
                let clock = row.message.created_at.get(11..16).unwrap_or("--:--");
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{clock} "), DIM),
                    Span::styled(format!("{who} "), ACCENT),
                    Span::raw(preview),
                ]))
            })
            .collect();
        let title = if self.unseen > 0 {
            format!("feed  ·  {} new", self.unseen)
        } else {
            "feed".into()
        };
        let list = List::new(rows)
            .block(Block::default().borders(Borders::ALL).title(title))
            .highlight_style(Style::new().bg(Color::Rgb(0x2a, 0x24, 0x16)));
        let mut state = ListState::default();
        if !self.messages.is_empty() {
            state.select(Some(self.selected));
        }
        frame.render_stateful_widget(list, content[0], &mut state);

        let detail = if let Some(row) = self.messages.get(self.selected) {
            let sender = match &row.message.sender {
                Actor::Human { .. } => "Carlos".into(),
                Actor::Agent(session) => self.name_of(session),
            };
            let mut lines = vec![
                Line::from(Span::styled(
                    format!("{sender}  ·  {}", row.message.created_at),
                    META,
                )),
                Line::from(Span::styled(format!("id {}", row.message.id), DIM)),
            ];
            if let Some(parent) = &row.message.draft.reply_to {
                lines.push(Line::from(Span::styled(format!("reply to {parent}"), DIM)));
            }
            for target in &row.message.draft.to {
                match target {
                    Target::Agent(session) => {
                        let status = row
                            .receipts
                            .get(session)
                            .map(String::as_str)
                            .unwrap_or("queued");
                        let exposure = row
                            .exposure
                            .get(session)
                            .map(String::as_str)
                            .unwrap_or("not exposed");
                        lines.push(Line::from(Span::styled(
                            format!("→ {}  ·  {status}  ·  {exposure}", self.name_of(session)),
                            META,
                        )));
                    }
                    Target::Human { .. } => {
                        lines.push(Line::from(Span::styled("→ Carlos  ·  observer feed", META)))
                    }
                }
            }
            lines.push(Line::raw(""));
            lines.extend(
                sanitize(&row.message.draft.body)
                    .lines()
                    .map(|line| Line::raw(line.to_owned())),
            );
            lines
        } else {
            vec![Line::from(Span::styled(
                "No messages in this project yet.",
                META,
            ))]
        };
        frame.render_widget(
            Paragraph::new(detail)
                .wrap(Wrap { trim: false })
                .scroll((self.detail_scroll, 0))
                .block(Block::default().borders(Borders::ALL).title("message")),
            content[1],
        );

        let composer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(3)])
            .split(vertical[2]);
        let chips = self
            .recipients
            .iter()
            .map(|session| self.name_of(session))
            .collect::<Vec<_>>()
            .join(", ");
        let candidates = if self.search.is_empty() {
            String::new()
        } else {
            self.suggestions()
                .iter()
                .take(5)
                .enumerate()
                .map(|(index, entry)| {
                    format!(
                        "{}{} [{} {}]",
                        if index == self.suggestion { '›' } else { ' ' },
                        entry.name,
                        entry.client,
                        short_session(entry)
                    )
                })
                .collect::<Vec<_>>()
                .join("  ")
        };
        let to = Paragraph::new(vec![
            Line::from(vec![
                Span::styled("to  ", ACCENT),
                Span::raw(if chips.is_empty() {
                    "(select @name)".into()
                } else {
                    chips
                }),
                Span::styled(format!("  {}", self.search), META),
            ]),
            Line::from(Span::styled(candidates, META)),
        ])
        .block(Block::default().borders(Borders::ALL).title(
            if self.focus == Focus::Recipients {
                "recipients · active"
            } else {
                "recipients"
            },
        ));
        frame.render_widget(to, composer[0]);
        let body_title = if let Some((id, all)) = &self.reply_to {
            format!("reply{} to {}", if *all { " all" } else { "" }, id)
        } else if self.focus == Focus::Body {
            "message · active".into()
        } else {
            "message".into()
        };
        frame.render_widget(
            Paragraph::new(sanitize(&self.body))
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::ALL).title(body_title)),
            composer[1],
        );
        let hints = match self.focus {
            Focus::Recipients => "@name · ↑↓ choose · Enter pin · Tab write · Backspace remove",
            Focus::Body => "Enter send · Ctrl+J newline · Tab feed · Esc feed",
            Focus::Feed => {
                "j/k browse · PgUp/PgDn read · r reply · a reply all · G latest · q quit"
            }
        };
        frame.render_widget(Paragraph::new(Span::styled(hints, DIM)), vertical[3]);
    }

    fn name_of(&self, session: &SessionRef) -> String {
        self.roster
            .iter()
            .find(|entry| entry.session == *session)
            .map(|entry| entry.name.clone())
            .unwrap_or_else(|| session.incarnation.chars().take(8).collect())
    }
}

fn short_session(entry: &RosterEntry) -> String {
    entry.session.incarnation.chars().take(8).collect()
}

fn sanitize(text: &str) -> String {
    let mut safe = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\n' => safe.push('\n'),
            '\t' => safe.push_str("  "),
            character if character.is_control() => {
                safe.push_str(&format!("\\u{{{:x}}}", character as u32))
            }
            character => safe.push(character),
        }
    }
    safe
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_controls_are_displayed_as_text() {
        assert_eq!(
            sanitize("hello\x1b[31m\rworld"),
            "hello\\u{1b}[31m\\u{d}world"
        );
    }
}
