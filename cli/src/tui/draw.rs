use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Padding, Paragraph, Wrap};
use ratatui::Frame;

use super::{App, Focus, Mode};
use crate::client::Item;

// Palette: one accent (amber) for "pressable" and attention; priority in red/blue so amber stays unique.
const ACCENT: Style = Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD);
const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
const META: Style = Style::new().fg(Color::Gray);
const DIM: Style = Style::new().fg(Color::DarkGray);
const RULE: Style = Style::new().fg(Color::DarkGray);
const LINK: Style = Style::new().fg(Color::Cyan);
const SELECTION: Color = Color::Rgb(0x2a, 0x24, 0x16);

impl App {
    /// The item's body as markdown with its checklist. The link is appended after rendering —
    /// as markdown it would print twice, once as text and once as the destination.
    fn reader_markdown(item: &Item) -> String {
        let mut md = format!("# {}\n\n", item.title);
        if !item.body.is_empty() {
            md.push_str(&item.body);
            md.push_str("\n\n");
        }
        for c in &item.checks {
            md.push_str(&format!(
                "- [{}] {}\n",
                if c.done { "x" } else { " " },
                c.label
            ));
        }
        md
    }

    /// The reader document: markdown restyled into lam's palette, then the link.
    fn reader_document(item: &Item) -> Text<'static> {
        let mut doc = adopt_palette(tui_markdown::from_str(&Self::reader_markdown(item)));
        if !item.link.is_empty() {
            doc.lines.push(Line::raw(""));
            doc.lines
                .push(Line::from(Span::styled(item.link.clone(), LINK)));
        }
        doc
    }

    pub(super) fn draw(&self, f: &mut Frame) {
        let visible = self.visible();
        // The list asks for exactly its rows (plus its rule) and never more than a third of the
        // screen, so the body — which is where the agent's message lives — keeps the rest.
        let list_h = (visible.len() as u16 + 1).clamp(3, (f.area().height / 3).max(3));
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .areas(f.area());
        let (list, detail) = if self.reader {
            let [l, r] =
                Layout::horizontal([Constraint::Length(34), Constraint::Min(20)]).areas(body);
            (l, r)
        } else {
            let [l, d] =
                Layout::vertical([Constraint::Length(list_h), Constraint::Min(3)]).areas(body);
            (l, d)
        };
        let open = visible.iter().filter(|i| i.status == "open").count();
        let live = self.status == "live";
        let [head_l, head_r] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Length(40)]).areas(header);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("lam", BOLD),
                Span::styled(format!("  {open} open"), META),
                Span::styled(if self.show_all { "  · all" } else { "" }, DIM),
                Span::styled(if self.busy { "  working…" } else { "" }, ACCENT),
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
                lines.extend(i.body.lines().map(|l| Line::raw(l.to_string())));
                if !i.link.is_empty() {
                    lines.push(Line::from(Span::styled(i.link.clone(), LINK)));
                }
                for (n, c) in i.checks.iter().enumerate() {
                    let cursor = n == self.check_sel && i.status == "open";
                    let focused = cursor && self.focus == Focus::Checks;
                    let label = if c.done {
                        DIM
                    } else if focused {
                        Style::new().add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    lines.push(Line::from(vec![
                        Span::styled(if cursor { "▸ " } else { "  " }, ACCENT),
                        Span::styled(
                            format!("{} {}", if c.done { "✔" } else { "○" }, c.label),
                            label,
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
        let pane = if let (true, Some(item)) = (self.reader, self.current()) {
            let doc = Self::reader_document(item);
            self.doc_lines.set(doc.lines.len() as u16);
            Paragraph::new(doc)
                .wrap(Wrap { trim: false })
                .scroll((self.scroll, 0))
                .block(
                    Block::default()
                        .borders(Borders::LEFT)
                        .border_style(RULE)
                        .padding(Padding::horizontal(1)),
                )
        } else {
            self.doc_lines.set(0);
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::TOP).border_style(RULE))
        };
        f.render_widget(pane, detail);

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
                    spans.extend(key(
                        "Space",
                        &format!("tick ({}/{})", i.checks_done(), i.checks.len()),
                    ));
                    spans.extend(match self.focus {
                        Focus::List => key("Tab", "pick checks"),
                        Focus::Checks => key("j/k", "pick · Tab back"),
                    });
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
                    Line::from(Span::styled(self.nav_hint(), DIM)),
                ]
            }
            _ => vec![
                Line::raw(""),
                Line::from(Span::styled(self.nav_hint(), DIM)),
            ],
        };
        f.render_widget(Paragraph::new(footer_text), footer);
    }
}

/// tui-markdown paints H1 on a cyan block (a line-level style) and inline code on black, which
/// fights any terminal theme. Drop every background and restate the hierarchy in lam's palette.
fn adopt_palette(text: Text<'_>) -> Text<'static> {
    const MD_HEADING_BG: Option<Color> = Some(Color::Cyan);
    const MD_CODE_BG: Option<Color> = Some(Color::Black);
    let lines = text
        .lines
        .into_iter()
        .map(|line| {
            let line_style = if line.style.bg == MD_HEADING_BG {
                ACCENT
            } else {
                let mut style = line.style;
                style.bg = None;
                style
            };
            let spans = line
                .spans
                .into_iter()
                .map(|span| {
                    let style = if span.style.bg == MD_CODE_BG {
                        Style::new().fg(Color::Magenta)
                    } else {
                        let mut style = span.style;
                        style.bg = None;
                        style
                    };
                    Span::styled(span.content.into_owned(), style)
                })
                .collect::<Vec<_>>();
            Line::from(spans).style(line_style)
        })
        .collect::<Vec<_>>();
    Text::from(lines)
}

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

#[cfg(test)]
mod tests {
    use super::super::tests::item;
    use super::*;

    #[test]
    fn reader_markdown_carries_title_link_and_checks() {
        let mut i = item("aaa", "open", &[], "https://x/pr/1");
        i.title = "Approve MON-3120?".into();
        i.body = "## Summary\n\nTwo files changed.".into();
        i.checks = vec![
            crate::client::Check {
                label: "PR 1".into(),
                done: true,
                at: None,
            },
            crate::client::Check {
                label: "PR 2".into(),
                done: false,
                at: None,
            },
        ];
        let md = App::reader_markdown(&i);
        assert!(md.starts_with("# Approve MON-3120?"));
        assert!(md.contains("## Summary\n\nTwo files changed."));
        assert!(md.contains("- [x] PR 1"));
        assert!(md.contains("- [ ] PR 2"));
        assert!(
            !md.contains("https://"),
            "the link is added after rendering, not as markdown"
        );

        let doc = App::reader_document(&i);
        let flat: Vec<String> = doc
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let link_lines: Vec<&String> = flat
            .iter()
            .filter(|l| l.contains("https://x/pr/1"))
            .collect();
        assert_eq!(
            link_lines.len(),
            1,
            "the link appears exactly once: {flat:?}"
        );
        assert!(flat.iter().any(|l| l.contains("Approve MON-3120?")));

        // no line or span may keep a background: those are the crate's cyan/black blocks
        for line in &doc.lines {
            assert!(
                matches!(line.style.bg, None | Some(Color::Reset)),
                "line background leaked"
            );
            for span in &line.spans {
                assert!(
                    matches!(span.style.bg, None | Some(Color::Reset)),
                    "background leaked into the reader: {:?}",
                    span.style
                );
            }
        }
        let heading = doc
            .lines
            .iter()
            .find(|l| {
                l.spans
                    .iter()
                    .any(|s| s.content.contains("Approve MON-3120?"))
            })
            .expect("heading line");
        assert_eq!(
            heading.style.fg,
            Some(Color::Yellow),
            "H1 is amber, not a cyan block"
        );
        assert!(heading.style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(heading.style.bg, None);
    }

    #[test]
    fn age_is_compact_and_tolerant() {
        let t = (chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
        assert_eq!(age(&t), "5m");
        let t = (chrono::Utc::now() - chrono::Duration::hours(26)).to_rfc3339();
        assert_eq!(age(&t), "1d");
        assert_eq!(age("garbage"), "");
    }
}
