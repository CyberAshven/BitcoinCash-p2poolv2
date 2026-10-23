// SPDX-FileCopyrightText: 2024 PDM Authors
// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The node config editor, ported from PDM's P2Pool config view: the
//! settings on the left, the selected one with its value or input on the
//! right. Secrets are always masked, even while typed.

use crate::views::{alert, muted, titled};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

/// Shown in place of a secret.
const MASK: &str = "********";

/// One setting of the node config, as the editor lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigRow {
    pub section: &'static str,
    pub key: &'static str,
    pub description: &'static str,
    /// What the value must be, e.g. `integer` or `mainnet | chipnet | regtest`.
    pub type_hint: &'static str,
    /// The value in the file, `None` when the file leaves it out.
    pub value: Option<String>,
    /// What the node uses when the file leaves it out.
    pub default: Option<&'static str>,
    /// Never shown: a password, token or secret key.
    pub sensitive: bool,
}

/// What the config editor shows.
pub struct ConfigScreen<'a> {
    /// The node config file, `None` before one is picked.
    pub path: Option<&'a str>,
    pub rows: &'a [ConfigRow],
    pub selected: usize,
    /// The list has the keyboard rather than the sidebar.
    pub focused: bool,
    /// The text being typed into the selected setting.
    pub input: Option<&'a str>,
    /// A problem to show above the editor, in red.
    pub warning: Option<&'a str>,
    /// A message to show above the editor, in green.
    pub info: Option<&'a str>,
    /// There are edits not saved yet.
    pub dirty: bool,
}

/// The value as listed: masked when sensitive, or what is used without it.
pub fn shown_value(row: &ConfigRow) -> (String, Style) {
    match (&row.value, row.sensitive) {
        (Some(_), true) => (
            MASK.to_string(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        (Some(value), false) => (
            value.clone(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        (None, _) => (
            match row.default {
                Some(default) => format!("(default: {default})"),
                None => "(not set)".to_string(),
            },
            Style::default().fg(Color::DarkGray),
        ),
    }
}

pub fn render(frame: &mut Frame, area: Rect, screen: &ConfigScreen) {
    let Some(path) = screen.path else {
        let message = screen
            .warning
            .unwrap_or("Press Enter to pick the node's config.toml.");
        let style = if screen.warning.is_some() {
            Style::default().fg(Color::Red)
        } else {
            Style::default()
        };
        let paragraph = Paragraph::new(message)
            .style(style)
            .block(titled(" P2Pool Config "))
            .wrap(Wrap { trim: true });
        frame.render_widget(paragraph, area);
        return;
    };

    let message = screen
        .warning
        .map(|warning| alert(warning.to_string()))
        .or_else(|| {
            screen.info.map(|info| {
                Line::from(Span::styled(
                    info.to_string(),
                    Style::default().fg(Color::Green),
                ))
            })
        });
    let content_area = match message {
        Some(line) => {
            let parts = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(0)])
                .split(area);
            frame.render_widget(
                Paragraph::new(line)
                    .block(titled(" Status "))
                    .wrap(Wrap { trim: true }),
                parts[0],
            );
            parts[1]
        }
        None => area,
    };
    let panels = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(content_area);
    render_list(frame, panels[0], screen, path);
    render_detail(frame, panels[1], screen);
}

fn render_list(frame: &mut Frame, area: Rect, screen: &ConfigScreen, path: &str) {
    let items: Vec<ListItem> = screen
        .rows
        .iter()
        .map(|row| {
            let (value, value_style) = shown_value(row);
            ListItem::new(vec![
                Line::from(vec![
                    Span::styled(
                        format!("[{}] ", row.section),
                        Style::default().fg(Color::Blue),
                    ),
                    Span::styled(row.description, Style::default().fg(Color::Gray)),
                ]),
                Line::from(vec![
                    Span::styled(format!("{} = ", row.key), Style::default().fg(Color::Cyan)),
                    Span::styled(value, value_style),
                ]),
            ])
        })
        .collect();
    let unsaved = if screen.dirty { " (unsaved)" } else { "" };
    let border_style = if screen.focused {
        Style::default()
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {path}{unsaved} "))
                .border_style(border_style),
        )
        .highlight_style(Style::default().bg(Color::DarkGray));
    let mut state = ListState::default();
    state.select(Some(screen.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_detail(frame: &mut Frame, area: Rect, screen: &ConfigScreen) {
    let block = titled(" Detail ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(row) = screen.rows.get(screen.selected) else {
        return;
    };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);
    frame.render_widget(
        Paragraph::new(row.description)
            .style(Style::default().fg(Color::White))
            .wrap(Wrap { trim: true }),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(format!(
            "[{}] {}  type: {}",
            row.section, row.key, row.type_hint
        ))
        .style(Style::default().fg(Color::Gray)),
        rows[1],
    );
    frame.render_widget(
        Paragraph::new("Value:").style(Style::default().fg(Color::Gray)),
        rows[3],
    );
    let value = match screen.input {
        Some(input) => Paragraph::new(typed(input, row.sensitive))
            .block(Block::default().borders(Borders::ALL))
            .style(Style::default().fg(Color::Yellow)),
        None => {
            let (value, style) = shown_value(row);
            Paragraph::new(value)
                .block(Block::default().borders(Borders::ALL))
                .style(style)
        }
    };
    frame.render_widget(value, rows[4]);
    let note = if row.sensitive {
        Line::from(Span::styled(
            "! secret: never shown; leave it empty to keep it",
            Style::default().fg(Color::Yellow),
        ))
    } else if row.value.is_some() {
        muted("An empty value removes an optional setting.")
    } else {
        Line::from("")
    };
    frame.render_widget(Paragraph::new(note), rows[5]);
}

/// The input as typed, masked for a secret, with a cursor.
fn typed(input: &str, sensitive: bool) -> String {
    if sensitive {
        "*".repeat(input.chars().count()) + "_"
    } else {
        format!("{input}_")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{sample_config_rows, screen_text};

    #[test]
    fn test_shown_value_masks_a_secret() {
        let rows = sample_config_rows();
        assert_eq!(shown_value(&rows[1]).0, MASK);
    }

    #[test]
    fn test_shown_value_names_the_default() {
        let rows = sample_config_rows();
        assert_eq!(shown_value(&rows[2]).0, "(default: false)");
    }

    #[test]
    fn test_editor_lists_settings_and_never_shows_a_secret() {
        let rows = sample_config_rows();
        let text = screen_text(110, 16, |frame| {
            let area = frame.area();
            super::render(
                frame,
                area,
                &ConfigScreen {
                    path: Some("node1/config.toml"),
                    rows: &rows,
                    selected: 1,
                    focused: true,
                    input: None,
                    warning: None,
                    info: Some("Saved."),
                    dirty: false,
                },
            );
        });
        assert!(!text.contains("hunter2"), "{text}");
        insta::assert_snapshot!(text);
    }

    #[test]
    fn test_editor_masks_a_secret_while_typed() {
        let rows = sample_config_rows();
        let text = screen_text(110, 16, |frame| {
            let area = frame.area();
            super::render(
                frame,
                area,
                &ConfigScreen {
                    path: Some("node1/config.toml"),
                    rows: &rows,
                    selected: 1,
                    focused: true,
                    input: Some("new-secret"),
                    warning: None,
                    info: None,
                    dirty: true,
                },
            );
        });
        assert!(!text.contains("new-secret"), "{text}");
        assert!(text.contains("**********_"), "{text}");
        assert!(text.contains("(unsaved)"), "{text}");
    }

    #[test]
    fn test_editor_before_a_config_is_picked() {
        let text = screen_text(80, 6, |frame| {
            let area = frame.area();
            super::render(
                frame,
                area,
                &ConfigScreen {
                    path: None,
                    rows: &[],
                    selected: 0,
                    focused: false,
                    input: None,
                    warning: None,
                    info: None,
                    dirty: false,
                },
            );
        });
        assert!(
            text.contains("Press Enter to pick the node's config.toml."),
            "{text}"
        );
    }
}
