// SPDX-FileCopyrightText: 2024 PDM Authors
// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The file picker, ported from PDM's file explorer view, with ASCII
//! markers in place of its icons.

use crate::explorer::{Entry, FileExplorer};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, List, ListItem, ListState},
};
use std::path::Path;

pub fn render(frame: &mut Frame, area: Rect, explorer: &FileExplorer) {
    let items: Vec<ListItem> = explorer
        .entries
        .iter()
        .map(|entry| ListItem::new(entry_label(entry)))
        .collect();
    let what = if explorer.directory_mode {
        "Select Directory"
    } else {
        "Select File"
    };
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {what} (in {}) ", explorer.current_dir.display())),
        )
        .highlight_style(Style::default().bg(Color::Blue).fg(Color::White))
        .highlight_symbol(">> ");
    let mut state = ListState::default();
    state.select(Some(explorer.selected_index));
    frame.render_stateful_widget(list, area, &mut state);
}

fn entry_label(entry: &Entry) -> String {
    match entry {
        Entry::UseThisDirectory => "[ Use this directory ]".to_string(),
        Entry::Parent => "[dir]  ..".to_string(),
        Entry::Directory(path) => format!("[dir]  {}", name(path)),
        Entry::File(path) => format!("       {}", name(path)),
    }
}

fn name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::screen_text;
    use std::fs;

    #[test]
    fn test_explorer_marks_directories() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("node1")).unwrap();
        fs::write(dir.path().join("config.toml"), "").unwrap();
        let explorer = FileExplorer::new(dir.path().to_path_buf(), false);
        let text = screen_text(200, 8, |frame| {
            let area = frame.area();
            super::render(frame, area, &explorer);
        });
        assert!(text.contains(">> [dir]  .."), "{text}");
        assert!(text.contains("[dir]  node1"), "{text}");
        assert!(text.contains("       config.toml"), "{text}");
    }

    #[test]
    fn test_explorer_in_directory_mode_offers_this_directory() {
        let dir = tempfile::tempdir().unwrap();
        let explorer = FileExplorer::new(dir.path().to_path_buf(), true);
        let text = screen_text(200, 6, |frame| {
            let area = frame.area();
            super::render(frame, area, &explorer);
        });
        assert!(text.contains("Select Directory"), "{text}");
        assert!(text.contains("[ Use this directory ]"), "{text}");
    }
}
