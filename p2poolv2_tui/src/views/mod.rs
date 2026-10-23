// SPDX-FileCopyrightText: 2024 PDM Authors
// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Everything drawn on screen, from plain data.
//!
//! Each view takes a `...Screen` struct of references to the data it shows
//! and draws it into an area; none of them fetches or stores anything. The
//! frame around them, the sidebar and the status bar, is [`render_shell`],
//! PDM's `ui.rs` and `status_bar.rs`.

pub mod bchn;
pub mod config;
pub mod explorer;
pub mod home;
pub mod pool;
pub mod settings;

use crate::remote::Remote;
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Row, Tabs, Wrap},
};

/// The screens, PDM's `CurrentScreen` without Lightning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Home,
    Bchn,
    PoolConfig,
    PoolStatus,
    Settings,
    FileExplorer,
}

/// The sidebar, in order.
pub const SIDEBAR_ITEMS: [(&str, Screen); 5] = [
    ("Home", Screen::Home),
    ("BCHN Status", Screen::Bchn),
    ("P2Pool Config", Screen::PoolConfig),
    ("P2Pool Status", Screen::PoolStatus),
    ("Settings", Screen::Settings),
];

/// Width of the sidebar.
const SIDEBAR_WIDTH: u16 = 25;

/// Width of a label in a `label : value` line.
const LABEL_WIDTH: usize = 24;

/// A key and what it does, for the status bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hint {
    pub key: &'static str,
    pub action: &'static str,
}

impl Hint {
    pub const fn new(key: &'static str, action: &'static str) -> Self {
        Self { key, action }
    }
}

/// Draws the sidebar and the status bar and returns the area left for the
/// screen. `notice`, when given, takes the status bar's place in red.
pub fn render_shell(
    frame: &mut Frame,
    sidebar_index: usize,
    sidebar_focused: bool,
    hints: &[Hint],
    notice: Option<&str>,
) -> Rect {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(frame.area());
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(0)])
        .split(rows[0]);

    render_sidebar(frame, columns[0], sidebar_index, sidebar_focused);
    render_status_bar(frame, rows[1], hints, notice);
    columns[1]
}

fn render_sidebar(frame: &mut Frame, area: Rect, selected: usize, focused: bool) {
    let items: Vec<ListItem> = SIDEBAR_ITEMS
        .iter()
        .map(|&(label, _)| ListItem::new(label))
        .collect();
    let border_style = if focused {
        Style::default()
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let sidebar = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" P2Pool BCH ")
                .border_style(border_style),
        )
        .highlight_style(Style::default().bg(Color::Gray).fg(Color::Black));
    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(sidebar, area, &mut state);
}

fn render_status_bar(frame: &mut Frame, area: Rect, hints: &[Hint], notice: Option<&str>) {
    let spans: Vec<Span> = match notice {
        Some(notice) => vec![Span::styled(
            format!(" ! {notice}  "),
            Style::default().fg(Color::Red),
        )],
        None => hints
            .iter()
            .flat_map(|hint| {
                [
                    Span::styled(
                        format!(" {} ", hint.key),
                        Style::default().bg(Color::DarkGray).fg(Color::White),
                    ),
                    Span::styled(
                        format!(" {}  ", hint.action),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]
            })
            .collect(),
    };
    let bar = Paragraph::new(Line::from(spans)).style(Style::default().bg(Color::Black));
    frame.render_widget(bar, area);
}

/// A tab bar over a screen, PDM's ` Info ` block.
pub(crate) fn tab_bar<'a>(titles: &[&'a str], selected: usize) -> Tabs<'a> {
    Tabs::new(titles.to_vec())
        .block(Block::default().borders(Borders::ALL).title(" Info "))
        .select(selected)
        .highlight_style(Style::default().bg(Color::Gray).fg(Color::Black))
}

/// Splits `area` into a tab bar and the content below it.
pub(crate) fn tabs_and_content(area: Rect) -> (Rect, Rect) {
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);
    (parts[0], parts[1])
}

/// A bordered block with a title.
pub(crate) fn titled(title: &str) -> Block<'_> {
    Block::default().borders(Borders::ALL).title(title)
}

/// A line in grey, for hints and waiting.
pub(crate) fn muted(text: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(
        text.into(),
        Style::default().fg(Color::DarkGray),
    ))
}

/// A line in red, for errors and warnings.
pub(crate) fn alert(text: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(text.into(), Style::default().fg(Color::Red)))
}

/// A `label : value` line with the labels aligned.
pub(crate) fn field(label: &str, value: impl Into<String>) -> Line<'static> {
    Line::from(format!("{label:<LABEL_WIDTH$} : {}", value.into()))
}

/// The lines for a value read from the node or BCHN: `ready` draws it
/// once it is there; otherwise a line says why it is not.
pub(crate) fn remote_lines<T>(
    remote: &Remote<T>,
    what: &str,
    unconfigured: &str,
    ready: impl FnOnce(&T) -> Vec<Line<'static>>,
) -> Vec<Line<'static>> {
    match remote {
        Remote::Unconfigured => vec![muted(unconfigured)],
        Remote::Loading => vec![muted(format!("Loading {what}..."))],
        Remote::Failed(error) => vec![alert(format!("Failed to fetch {what}: {error}"))],
        Remote::Ready(value) => ready(value),
    }
}

/// What to show in place of a table that has no rows: why, or `empty`
/// when the answer had none.
pub(crate) fn remote_message<T>(
    remote: &Remote<T>,
    what: &str,
    unconfigured: &str,
    empty: &str,
) -> Line<'static> {
    match remote {
        Remote::Unconfigured => muted(unconfigured),
        Remote::Loading => muted(format!("Loading {what}...")),
        Remote::Failed(error) => alert(format!("Failed to fetch {what}: {error}")),
        Remote::Ready(_) => muted(empty),
    }
}

/// Draws `message` in a block titled `title`, in place of a table with no
/// rows.
pub(crate) fn render_message(frame: &mut Frame, area: Rect, title: &str, message: Line<'static>) {
    let paragraph = Paragraph::new(message)
        .block(titled(title))
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}

/// A table's header row.
pub(crate) fn header_row(titles: &[&'static str]) -> Row<'static> {
    Row::new(titles.to_vec())
        .style(
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        )
        .bottom_margin(1)
}

/// What a view says when no node config is loaded.
pub(crate) const PICK_A_CONFIG: &str =
    "Pick the node's config.toml in P2Pool Config or Settings to load this.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::screen_text;

    #[test]
    fn test_shell_draws_sidebar_and_hints() {
        let text = screen_text(80, 8, |frame| {
            render_shell(
                frame,
                1,
                true,
                &[Hint::new("q", "Quit"), Hint::new("r", "Refresh")],
                None,
            );
        });
        insta::assert_snapshot!(text);
    }

    #[test]
    fn test_shell_shows_a_notice_in_place_of_hints() {
        let text = screen_text(80, 8, |frame| {
            render_shell(
                frame,
                0,
                false,
                &[Hint::new("q", "Quit")],
                Some("Save failed: denied"),
            );
        });
        assert!(text.contains("! Save failed: denied"), "{text}");
        assert!(!text.contains("Quit"), "{text}");
    }

    #[test]
    fn test_shell_returns_the_area_right_of_the_sidebar() {
        let mut body = Rect::default();
        screen_text(80, 8, |frame| {
            body = render_shell(frame, 0, true, &[], None);
        });
        assert_eq!(body, Rect::new(SIDEBAR_WIDTH, 0, 80 - SIDEBAR_WIDTH, 7));
    }
}
