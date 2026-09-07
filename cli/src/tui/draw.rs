use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Padding, Paragraph, Wrap};
use ratatui::Frame;

use super::{App, Focus, Mode, Tab};
use crate::client::Item;

/// Rough width the side blocks need (`lam  12 open` and `hostname  ● live`); below the tab bar
/// plus this, the bar steps down rather than colliding with them.
const HEADER_SIDES: u16 = 38;
/// Rows the detail pane keeps on the history tab; the list takes everything else.
const HISTORY_DETAIL: u16 = 8;

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
        if item.checks.is_empty() {
            if let Some(recommendation) = &item.recommendation {
                md.push_str("## Recommendation\n\n");
                if let Some(choice) = &item.recommended_choice {
                    md.push_str(&format!("Recommended choice: {choice}\n\n"));
                }
                md.push_str(recommendation);
                md.push_str("\n\n");
            }
        }
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
        if item.checks.is_empty() && item.recommendation.is_some() {
            if let Some(choice) = &item.recommended_choice {
                let label = format!("Recommended choice: {choice}");
                if let Some(line) = doc
                    .lines
                    .iter_mut()
                    .find(|line| line.to_string().starts_with("Recommended choice: "))
                {
                    *line = Line::from(Span::styled(label, ACCENT));
                }
            }
        }
        if !item.link.is_empty() {
            doc.lines.push(Line::raw(""));
            doc.lines
                .push(Line::from(Span::styled(item.link.clone(), LINK)));
        }
        doc
    }

    /// `requests  [history]` — the active tab bracketed. Both states are the same width, so the
    /// centred bar never shifts as you switch. Degrades to the active label alone, then to
    /// nothing, so a narrow terminal loses the bar rather than colliding with the side blocks.
    fn tab_spans(&self, width: u16) -> Vec<Span<'static>> {
        // The brackets move with the active label, so both states are exactly 19 columns.
        let (left, right, a, b) = match self.tab {
            Tab::Requests => ("[requests]", "  history", ACCENT, DIM),
            Tab::History => ("requests  ", "[history]", DIM, ACCENT),
        };
        let full = (left.len() + right.len()) as u16;
        if width >= full + HEADER_SIDES {
            return vec![
                Span::styled(left.to_string(), a),
                Span::styled(right.to_string(), b),
            ];
        }
        let active = match self.tab {
            Tab::Requests => left,
            Tab::History => right,
        };
        if width >= active.len() as u16 + HEADER_SIDES {
            return vec![Span::styled(active.to_string(), ACCENT)];
        }
        vec![]
    }

    /// The nav hint, plus where history stands. This rides in the footer rather than as a
    /// trailing list row because ratatui scrolls to keep the selected *item* in view: a marker
    /// one past the last item is never reachable, so at the bottom you would never see it.
    fn nav_line(&self) -> Line<'static> {
        let mut spans = vec![Span::styled(self.nav_hint(), DIM)];
        if self.tab == Tab::History {
            let note = match (self.history_loading, self.history_end) {
                (true, _) => Some(format!(
                    "   ⋯ loading older ({} so far)",
                    self.history.items.len()
                )),
                (_, true) => Some(format!(
                    "   · {} closed, nothing older",
                    self.history.items.len()
                )),
                _ => None,
            };
            if let Some(note) = note {
                spans.push(Span::styled(note, DIM));
            }
        }
        Line::from(spans)
    }

    pub(super) fn draw(&self, f: &mut Frame) {
        let visible = self.visible();
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .areas(f.area());
        // On requests the list asks for exactly its rows (plus its rule) and never more than a
        // third of the screen, so the body — where the agent's message lives — keeps the rest.
        // History is a browsing view, so there the list takes the screen and the detail is fixed.
        let list_h = match self.tab {
            Tab::Requests => (visible.len() as u16 + 1).clamp(3, (f.area().height / 3).max(3)),
            Tab::History => body.height.saturating_sub(HISTORY_DETAIL).max(3),
        };
        let (list, detail) = if self.reader {
            let [l, r] =
                Layout::horizontal([Constraint::Length(34), Constraint::Min(20)]).areas(body);
            (l, r)
        } else {
            let [l, d] =
                Layout::vertical([Constraint::Length(list_h), Constraint::Min(3)]).areas(body);
            (l, d)
        };
        // Counts the queue, not what is on screen: `0 open` while browsing history would lie.
        let open = self.visible_of(Tab::Requests).len();
        let live = self.status == "live";
        let tabs = self.tab_spans(header.width);
        let tabs_w: u16 = tabs.iter().map(|s| s.width() as u16).sum();
        // Equal fills are what actually centres the bar; a fixed-width right block would not.
        let [head_l, head_c, head_r] = Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Length(tabs_w),
            Constraint::Fill(1),
        ])
        .areas(header);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("lam", BOLD),
                Span::styled(format!("  {open} open"), META),
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
        f.render_widget(Paragraph::new(Line::from(tabs)), head_c);
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

        let rows: Vec<ListItem> = visible
            .iter()
            .map(|i| ListItem::new(row(i, self.tab, list.width)))
            .collect();
        let mut state = ListState::default().with_selected(Some(self.pane().selected));
        f.render_stateful_widget(
            List::new(rows)
                .block(Block::default().borders(Borders::TOP).border_style(RULE))
                .highlight_style(Style::default().bg(SELECTION)),
            list,
            &mut state,
        );

        let text = match self.current() {
            Some(i) => {
                // Once an item is closed, when it closed is the fact you want, not when it was
                // raised — "created 90d ago" says nothing in a history view.
                let (when, stamp) = match i.resolved_at.as_deref() {
                    Some(at) if i.status != "open" => ("closed", age(at)),
                    _ => ("raised", age(&i.created_at)),
                };
                let mut lines = vec![Line::from(Span::styled(
                    format!("{} · {} · {when} {stamp} ago", source(i), i.priority),
                    META,
                ))];
                // Keep the canonical answer visible in history's fixed-height detail pane.
                if let Some(answer) = i.response_choice.as_deref().or(i.response_text.as_deref()) {
                    lines.push(Line::from(Span::styled(
                        format!(
                            "{} via {}: {answer}",
                            i.status,
                            i.response_by.as_deref().unwrap_or("?")
                        ),
                        outcome(i).1,
                    )));
                }
                if i.checks.is_empty() {
                    if let Some(recommendation) = &i.recommendation {
                        lines.push(Line::from(Span::styled("Recommendation", BOLD)));
                        if let Some(choice) = &i.recommended_choice {
                            lines.push(Line::from(Span::styled(
                                format!("Recommended choice: {choice}"),
                                ACCENT,
                            )));
                        }
                        lines.extend(recommendation.lines().map(|l| Line::raw(l.to_string())));
                        lines.push(Line::raw(""));
                    }
                }
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
                lines
            }
            None => vec![Line::from(Span::styled(
                match self.tab {
                    Tab::Requests => "nothing here — all caught up",
                    Tab::History => "nothing closed yet",
                },
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
                    .flat_map(|(n, c)| {
                        let mut hint = key(&(n + 1).to_string(), c);
                        if i.checks.is_empty() && i.recommended_choice.as_ref() == Some(c) {
                            hint[1] = Span::styled(format!(" {c} (recommended)   "), ACCENT);
                        }
                        hint
                    })
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
                vec![Line::from(spans), self.nav_line()]
            }
            _ => vec![Line::raw(""), self.nav_line()],
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
pub(super) fn source(i: &Item) -> String {
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

/// Truncate to `max` *characters* — a reply is free text, so slicing bytes would panic on UTF-8.
fn ellipsis(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
}

/// What became of it: the answer given, otherwise the outcome itself.
pub(super) fn outcome(i: &Item) -> (&'static str, Style, String) {
    let answer = i
        .response_choice
        .as_deref()
        .or(i.response_text.as_deref())
        .map(|s| ellipsis(s.lines().next().unwrap_or("").trim(), 24))
        .filter(|s| !s.is_empty());
    match i.status.as_str() {
        // A checklist resolves itself with no answer attached, hence the fallback.
        "resolved" => (
            "✓",
            Style::new().fg(Color::Green),
            answer.unwrap_or_else(|| "done".into()),
        ),
        "dismissed" => (
            "✗",
            Style::new().fg(Color::Red),
            answer.unwrap_or_else(|| "dismissed".into()),
        ),
        "retracted" => ("⤺", META, "retracted".into()),
        "expired" => ("⋯", DIM, "expired".into()),
        other => ("·", DIM, other.to_string()),
    }
}

/// Columns a row spends on everything but the title: gutter, id, agent, then the outcome (history
/// only) and the age. The title takes what is left, so the right-hand columns always survive.
const HISTORY_FIXED: usize = 2 + 7 + 21 + 17 + 5;
const REQUESTS_FIXED: usize = 2 + 7 + 21 + 5;

/// Fit a queue column in terminal cells, including wide characters and the ellipsis.
fn column(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let mut used = Span::raw(text).width();
    let mut result = text.to_string();
    if used > width {
        result.clear();
        used = 0;
        for c in text.chars() {
            let cells = Span::raw(c.to_string()).width();
            if used + cells >= width {
                break;
            }
            result.push(c);
            used += cells;
        }
        result.push('…');
        used += 1;
    }
    result.push_str(&" ".repeat(width - used));
    result
}

/// Returns the `Line` rather than a `ListItem` so tests can read back what was rendered.
fn row(i: &Item, tab: Tab, width: u16) -> Line<'_> {
    let open = i.status == "open";
    let title = if i.checks.is_empty() {
        i.title.clone()
    } else {
        format!("{}  {}/{}", i.title, i.checks_done(), i.checks.len())
    };
    // In their own tab closed items are the subject, not intruders in the queue, so they are not
    // dimmed — and their gutter carries the outcome, priority being moot once an item is closed.
    // The title is cut to fit because the outcome is the column you came here to read: letting a
    // long title push it off the edge would hide the answer.
    if tab == Tab::History {
        let (glyph, style, label) = outcome(i);
        let title_w = (width as usize).saturating_sub(HISTORY_FIXED).max(10);
        return Line::from(vec![
            Span::styled("▍ ", style),
            Span::styled(format!("{:<6} ", i.id), META),
            Span::styled(format!("{:<20} ", source(i)), LINK),
            Span::styled(
                format!("{:<title_w$}", ellipsis(&title, title_w.saturating_sub(1))),
                Style::default(),
            ),
            Span::styled(format!("{glyph} {:<14} ", ellipsis(&label, 14)), style),
            Span::styled(format!("{:>4}", age(&i.created_at)), DIM),
        ]);
    }
    let gutter = match (open, i.priority.as_str()) {
        (true, "critical") => Style::default().fg(Color::Red),
        (true, "low") => DIM,
        (true, _) => Style::default().fg(Color::Blue),
        (false, _) => RULE,
    };
    let text = if open { Style::default() } else { DIM };
    let signal = if i.checks.is_empty() {
        if open && i.recommendation.is_none() {
            "! recommendation missing".into()
        } else {
            i.recommended_choice
                .as_ref()
                .map(|c| format!("→ {c}"))
                .unwrap_or_default()
        }
    } else {
        String::new()
    };
    let signal = column(
        &signal,
        Span::raw(&signal)
            .width()
            .min(24)
            .min((width as usize).saturating_sub(3)),
    );
    let signal_w = if signal.is_empty() {
        0
    } else {
        Span::raw(&signal).width() + 1
    };
    // Reader-width rows keep the title and signal; metadata returns when it fits.
    let metadata = width as usize >= REQUESTS_FIXED + signal_w + 10;
    let fixed = if metadata { REQUESTS_FIXED } else { 2 };
    let title_w = (width as usize).saturating_sub(fixed + signal_w);
    let mut spans = vec![Span::styled(column("▍ ", 2.min(width as usize)), gutter)];
    if metadata {
        spans.push(Span::styled(
            format!("{} ", column(&i.id, 6)),
            if open { META } else { DIM },
        ));
        spans.push(Span::styled(
            format!("{} ", column(&source(i), 20)),
            if open { LINK } else { DIM },
        ));
    }
    spans.push(Span::styled(
        if title_w == 0 {
            String::new()
        } else {
            format!("{} ", column(&title, title_w - 1))
        },
        text,
    ));
    if !signal.is_empty() {
        spans.push(Span::styled(
            format!(" {signal}"),
            Style::new().fg(Color::Yellow),
        ));
    }
    if metadata {
        spans.push(Span::styled(format!("{:>5}", age(&i.created_at)), DIM));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::super::tests::item;
    use super::*;

    fn render(app: &App, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let mut term =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
        term.backend().buffer().clone()
    }

    fn screen_text(buf: &ratatui::buffer::Buffer) -> String {
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn recommended() -> Item {
        let mut i = item("rec01", "open", &["wait", "ship"], "");
        i.title = "Release decision".into();
        i.body = "Body follows the rationale.".into();
        i.recommendation = Some("Tests passed. Ship this version.".into());
        i.recommended_choice = Some("ship".into());
        i
    }

    #[test]
    fn recommendation_rows_keep_the_signal_visible_at_reader_width() {
        let mut i = recommended();
        i.title = "A very long title that competes with the recommendation".into();
        for width in [34, 60, 100] {
            let line = row(&i, Tab::Requests, width);
            let text = row_text(&i, Tab::Requests, width);
            assert!(text.contains("→ ship"), "{width}: {text}");
            assert!(line.width() <= width as usize, "{width}: {text}");
            assert!(line
                .spans
                .iter()
                .any(|s| s.content.contains("→ ship") && s.style.fg == Some(Color::Yellow)));
        }
    }

    #[test]
    fn recommendation_rows_fit_tiny_terminals_and_wide_characters() {
        let mut i = recommended();
        i.title = "发布版本 🚀 发布版本 🚀".into();
        i.name = "a long agent name that exceeds its column".into();
        i.recommended_choice = Some("发布🚀版本".into());
        for width in [1, 10, 20, 34, 60, 100] {
            let line = row(&i, Tab::Requests, width);
            assert!(line.width() <= width as usize, "width {width}: {line}");
            let mut a = App::new("host".into());
            a.set_items(vec![i.clone()]);
            for reader in [false, true] {
                a.reader = reader;
                render(&a, width, 12);
            }
        }
    }

    #[test]
    fn recommendation_text_without_a_choice_still_precedes_the_body() {
        let mut i = recommended();
        i.choices.clear();
        i.recommended_choice = None;
        let mut a = App::new("host".into());
        a.set_items(vec![i]);
        for reader in [false, true] {
            a.reader = reader;
            let text = screen_text(&render(&a, 110, 24));
            assert!(text.find("Tests passed.").unwrap() < text.find("Body follows").unwrap());
            assert!(!text.contains("! recommendation missing"));
            assert!(!text.contains("Recommended choice:"));
        }
    }

    #[test]
    fn reader_keeps_the_recommended_choice_literal() {
        let mut i = recommended();
        i.choices = vec!["**ship**".into()];
        i.recommended_choice = Some("**ship**".into());
        let doc = App::reader_document(&i);
        assert!(doc
            .lines
            .iter()
            .any(|line| line.to_string() == "Recommended choice: **ship**"));
    }

    #[test]
    fn recommendation_missing_warning_is_only_for_open_non_checklists() {
        let mut i = item("old01", "open", &[], "");
        for width in [34, 100] {
            let text = row_text(&i, Tab::Requests, width);
            assert!(text.contains("! recommendation missing"), "{text}");
            assert!(row(&i, Tab::Requests, width).width() <= width as usize);
        }
        i.checks.push(crate::client::Check {
            label: "Verify".into(),
            done: false,
            at: None,
        });
        assert!(!row_text(&i, Tab::Requests, 100).contains("recommendation"));
        i.checks.clear();
        for status in ["resolved", "dismissed", "expired", "retracted"] {
            i.status = status.into();
            for tab in [Tab::Requests, Tab::History] {
                assert!(!row_text(&i, tab, 100).contains("recommendation"));
            }
        }
    }

    #[test]
    fn recommendation_precedes_the_body_in_detail_and_reader() {
        let mut a = App::new("host".into());
        a.set_items(vec![recommended()]);
        for reader in [false, true] {
            a.reader = reader;
            let buf = render(&a, 110, 24);
            let text = screen_text(&buf);
            let heading = text.find("Recommendation").expect(&text);
            let choice = text.find("Recommended choice: ship").expect(&text);
            let rationale = text.find("Tests passed. Ship this version.").expect(&text);
            let body = text.find("Body follows the rationale.").expect(&text);
            assert!(
                heading < choice && choice < rationale && rationale < body,
                "{text}"
            );
            let choice_cells = buf
                .content
                .windows("Recommended choice: ship".len())
                .find(|cells| {
                    cells.iter().map(|c| c.symbol()).collect::<String>()
                        == "Recommended choice: ship"
                })
                .unwrap();
            assert!(
                choice_cells.iter().all(|c| c.fg == Color::Yellow),
                "reader: {reader}"
            );
        }
    }

    #[test]
    fn recommended_footer_keeps_choice_numbers_and_actions() {
        let mut a = App::new("host".into());
        a.set_items(vec![recommended()]);
        let buf = render(&a, 110, 24);
        let footer: String = (0..110).map(|x| buf[(x, 22)].symbol()).collect();
        assert!(footer.contains("1 wait"), "{footer}");
        assert!(footer.contains("2 ship (recommended)"), "{footer}");
        let marker = footer.find("ship (recommended)").unwrap() as u16;
        assert_eq!(buf[(marker, 22)].fg, Color::Yellow);
        assert_eq!(
            a.handle(super::super::tests::key('2')),
            Some(super::super::Action::Resolve {
                id: "rec01".into(),
                choice: Some("ship".into()),
                text: None,
            })
        );
        assert_eq!(
            a.handle(super::super::tests::key('1')),
            Some(super::super::Action::Resolve {
                id: "rec01".into(),
                choice: Some("wait".into()),
                text: None,
            })
        );
    }

    #[test]
    fn history_keeps_actual_outcome_when_it_differs_from_recommendation() {
        let mut i = recommended();
        i.status = "resolved".into();
        i.response_choice = Some("wait".into());
        i.response_by = Some("phone".into());
        let mut a = App::new("host".into());
        a.add_history(vec![i], true);
        a.tab = Tab::History;
        let text = screen_text(&render(&a, 110, 24));
        assert!(text.contains("✓ wait"), "{text}");
        assert!(text.contains("resolved via phone: wait"), "{text}");
        assert!(text.contains("Recommended choice: ship"), "{text}");
        assert!(!text.contains("2 ship (recommended)"), "{text}");
        assert_eq!(a.handle(super::super::tests::key('2')), None);
    }

    #[test]
    fn a_long_recommendation_does_not_hide_the_history_response() {
        let mut i = recommended();
        i.status = "resolved".into();
        i.recommendation = Some("A line of rationale.\n".repeat(10));
        i.response_choice = Some("wait".into());
        i.response_by = Some("phone".into());
        let mut a = App::new("host".into());
        a.add_history(vec![i], true);
        a.tab = Tab::History;
        let text = screen_text(&render(&a, 110, 24));
        assert!(text.contains("resolved via phone: wait"), "{text}");
    }

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

    fn closed(id: &str, status: &str, choice: Option<&str>, text: Option<&str>) -> Item {
        let mut i = item(id, status, &[], "");
        i.response_choice = choice.map(Into::into);
        i.response_text = text.map(Into::into);
        i
    }

    #[test]
    fn outcome_shows_the_answer_then_the_status() {
        let (g, st, label) = outcome(&closed("a", "resolved", Some("yes"), None));
        assert_eq!((g, label.as_str()), ("✓", "yes"));
        assert_eq!(st.fg, Some(Color::Green));

        // a reply is free text: first line only, truncated, and never sliced mid-character
        let long = "ship it — but rebase onto main first, then squash é😀 the fixups";
        let (_, _, label) = outcome(&closed("b", "resolved", None, Some(long)));
        assert_eq!(label.chars().count(), 24);
        assert!(label.ends_with('…'));
        let (_, _, label) = outcome(&closed("c", "resolved", None, Some("line one\nline two")));
        assert_eq!(label, "line one");

        // a checklist resolves itself with nothing attached
        assert_eq!(outcome(&closed("d", "resolved", None, None)).2, "done");
        assert_eq!(
            outcome(&closed("e", "dismissed", None, None)).2,
            "dismissed"
        );
        assert_eq!(
            outcome(&closed("f", "retracted", None, None)).2,
            "retracted"
        );
        let (g, st, label) = outcome(&closed("g", "expired", None, None));
        assert_eq!((g, label.as_str()), ("⋯", "expired"));
        assert_eq!(st, DIM);
    }

    /// The flat text of a rendered row, which is what actually has to fit the terminal.
    fn row_text(i: &Item, tab: Tab, width: u16) -> String {
        row(i, tab, width)
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn a_long_title_never_pushes_the_right_hand_columns_off_screen() {
        let mut i = item("aaa", "resolved", &[], "");
        i.title = "Approve MON-3120: parallelize the hottest E2E group and drop serial mode".into();
        i.response_choice = Some("approve".into());
        i.created_at = (chrono::Utc::now() - chrono::Duration::hours(3)).to_rfc3339();

        let h = row_text(&i, Tab::History, 100);
        assert!(h.chars().count() <= 100, "history row overflows: {h:?}");
        assert!(h.contains("✓ approve"), "the outcome survives: {h:?}");
        assert!(h.trim_end().ends_with("3h"), "and so does the age: {h:?}");
        assert!(h.contains('…'), "the title is what gives way: {h:?}");

        i.status = "open".into();
        let r = row_text(&i, Tab::Requests, 100);
        assert!(r.chars().count() <= 100, "requests row overflows: {r:?}");
        assert!(r.trim_end().ends_with("3h"), "the age survives too: {r:?}");
        assert!(r.contains('…'));

        // a title that already fits is left alone
        i.title = "short".into();
        assert!(!row_text(&i, Tab::Requests, 100).contains('…'));
    }

    #[test]
    fn the_tab_bar_is_one_width_and_degrades_narrow() {
        let mut a = App::new("host".into());
        let width = |a: &App, w| -> usize {
            a.tab_spans(w)
                .iter()
                .map(|s| s.content.chars().count())
                .sum()
        };
        assert_eq!(width(&a, 80), 19);
        a.handle(super::super::tests::key('l'));
        assert_eq!(
            width(&a, 80),
            19,
            "both states are the same width, so it never jitters"
        );

        assert_eq!(width(&a, 50), 9, "squeezed down to the active label");
        assert_eq!(width(&a, 40), 0, "and out entirely rather than colliding");
    }

    #[test]
    fn the_header_centres_the_tab_bar() {
        let mut a = App::new("archlinux".into());
        a.set_items(vec![item("aaa", "open", &[], "")]);
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(71, 12)).unwrap();
        term.draw(|f| a.draw(f)).unwrap();
        let buf = term.backend().buffer().clone();
        let head: String = (0..71).map(|x| buf[(x, 0)].symbol()).collect();

        assert!(head.starts_with("lam  1 open"), "{head:?}");
        assert!(
            head.trim_end().ends_with("archlinux  ● connecting"),
            "{head:?}"
        );
        // 71 columns less the 19-column bar leaves 26 on each side
        assert_eq!(&head[26..45], "[requests]  history", "{head:?}");
    }
}
