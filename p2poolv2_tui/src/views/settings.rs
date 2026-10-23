// SPDX-FileCopyrightText: 2024 PDM Authors
// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The settings screen, ported from PDM's settings view: the node config
//! file and where this tool keeps its own settings.

use ratatui::{
    prelude::*,
    widgets::{Block, Borders, List, ListItem, ListState},
};

/// What a setting picks when Enter is pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    File,
    Directory,
}

/// The settings, in order: `(label, kind)`.
pub const SETTINGS_FIELDS: [(&str, FieldKind); 2] = [
    ("Node config file (config.toml)", FieldKind::File),
    ("Settings directory", FieldKind::Directory),
];

/// What the settings screen shows.
pub struct SettingsScreen<'a> {
    pub node_config: Option<&'a str>,
    /// The directory chosen for `settings.toml`, if not the default.
    pub settings_dir_override: Option<&'a str>,
    /// The default directory, shown when there is no override.
    pub default_settings_dir: &'a str,
    pub selected: usize,
    /// The list has the keyboard rather than the sidebar.
    pub focused: bool,
}

pub fn render(frame: &mut Frame, area: Rect, screen: &SettingsScreen) {
    let values = [
        screen.node_config.map(str::to_string),
        screen.settings_dir_override.map(str::to_string),
    ];
    let items: Vec<ListItem> = SETTINGS_FIELDS
        .iter()
        .zip(values)
        .enumerate()
        .map(|(index, (&(label, _), value))| {
            let (shown, style) = match value {
                Some(value) => (
                    value,
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                None if index == 1 => (
                    format!("{} (default)", screen.default_settings_dir),
                    Style::default().fg(Color::DarkGray),
                ),
                None => (
                    "(not set)".to_string(),
                    Style::default().fg(Color::DarkGray),
                ),
            };
            ListItem::new(vec![
                Line::from(Span::styled(label, Style::default().fg(Color::Gray))),
                Line::from(Span::styled(shown, style)),
            ])
        })
        .collect();
    let border_style = if screen.focused {
        Style::default()
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Settings ")
                .border_style(border_style),
        )
        .highlight_style(Style::default().bg(Color::DarkGray));
    let mut state = ListState::default();
    state.select(Some(screen.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::screen_text;

    #[test]
    fn test_settings_with_defaults() {
        let text = screen_text(70, 8, |frame| {
            let area = frame.area();
            super::render(
                frame,
                area,
                &SettingsScreen {
                    node_config: None,
                    settings_dir_override: None,
                    default_settings_dir: "/home/miner/.config/p2poolv2_tui",
                    selected: 0,
                    focused: false,
                },
            );
        });
        insta::assert_snapshot!(text);
    }

    #[test]
    fn test_settings_with_a_node_config() {
        let text = screen_text(70, 8, |frame| {
            let area = frame.area();
            super::render(
                frame,
                area,
                &SettingsScreen {
                    node_config: Some("/srv/p2pool/config.toml"),
                    settings_dir_override: Some("/srv/p2pool/tui"),
                    default_settings_dir: "/home/miner/.config/p2poolv2_tui",
                    selected: 1,
                    focused: true,
                },
            );
        });
        assert!(text.contains("/srv/p2pool/config.toml"), "{text}");
        assert!(text.contains("/srv/p2pool/tui"), "{text}");
        assert!(!text.contains("(default)"), "{text}");
    }
}
