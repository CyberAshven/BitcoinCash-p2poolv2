// SPDX-FileCopyrightText: 2024 PDM Authors
// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Keys, ported from PDM's `dispatch_key` and `handle_action`.
//!
//! Up and Down move through the sidebar, which switches screens at once;
//! Left and Right switch tabs; Enter gives a screen's list the keyboard and
//! Esc gives it back. Ctrl-C always quits, and q quits unless a value is
//! being typed.

use crate::app::{App, ExplorerTrigger, MAX_INPUT_CHARS};
use crate::views::bchn::BCHN_TABS;
use crate::views::pool::POOL_TABS;
use crate::views::settings::{FieldKind, SETTINGS_FIELDS};
use crate::views::{SIDEBAR_ITEMS, Screen};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::ops::ControlFlow;
use std::time::Instant;

/// The tab of the P2Pool status screen with the miners table.
const MINERS_TAB: usize = 2;

/// Acts on one key event at `now`. `Break` means quit.
pub fn handle_key(app: &mut App, key: KeyEvent, now: Instant) -> ControlFlow<()> {
    if key.kind != KeyEventKind::Press {
        return ControlFlow::Continue(());
    }
    let typing = app.text_input_active();
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    if (control && key.code == KeyCode::Char('c')) || (!typing && key.code == KeyCode::Char('q')) {
        return ControlFlow::Break(());
    }
    if !typing && key.code == KeyCode::Char('r') && app.screen != Screen::FileExplorer {
        app.refresh_now();
        return ControlFlow::Continue(());
    }
    match app.screen {
        Screen::FileExplorer => explorer_key(app, key.code),
        Screen::Bchn => {
            if !switch_tab(key.code, &mut app.bchn_tab, BCHN_TABS.len()) {
                sidebar_key(app, key.code);
            }
        }
        Screen::PoolStatus => pool_key(app, key.code, now),
        Screen::PoolConfig => config_key(app, key),
        Screen::Settings => settings_key(app, key.code),
        Screen::Home => sidebar_key(app, key.code),
    }
    ControlFlow::Continue(())
}

fn sidebar_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Up if app.sidebar_index > 0 => app.select_sidebar(app.sidebar_index - 1),
        KeyCode::Down if app.sidebar_index + 1 < SIDEBAR_ITEMS.len() => {
            app.select_sidebar(app.sidebar_index + 1);
        }
        _ => {}
    }
}

/// Moves `tab` for Left and Right; returns whether `code` was one of them.
fn switch_tab(code: KeyCode, tab: &mut usize, count: usize) -> bool {
    match code {
        KeyCode::Left => {
            *tab = tab.saturating_sub(1);
            true
        }
        KeyCode::Right => {
            if *tab + 1 < count {
                *tab += 1;
            }
            true
        }
        _ => false,
    }
}

fn pool_key(app: &mut App, code: KeyCode, now: Instant) {
    let miner_count = app.miners.ready().map_or(0, |miners| miners.miners.len());
    if app.miners_focused {
        match code {
            KeyCode::Up => app.miners_selected = app.miners_selected.saturating_sub(1),
            KeyCode::Down if app.miners_selected + 1 < miner_count => app.miners_selected += 1,
            KeyCode::Enter => app.pick_miner(now),
            KeyCode::Esc => app.miners_focused = false,
            _ => {}
        }
    } else if !switch_tab(code, &mut app.pool_tab, POOL_TABS.len()) {
        match code {
            KeyCode::Enter if app.pool_tab == MINERS_TAB && miner_count > 0 => {
                app.miners_focused = true;
            }
            code => sidebar_key(app, code),
        }
    }
}

fn config_key(app: &mut App, key: KeyEvent) {
    if app.editor_state.input.is_some() {
        typing_key(app, key);
    } else if app.editor.is_none() {
        match key.code {
            KeyCode::Enter => app.open_explorer(ExplorerTrigger::PoolConfig, false),
            code => sidebar_key(app, code),
        }
    } else if !app.editor_state.focused {
        match key.code {
            KeyCode::Enter => app.editor_state.focused = true,
            code => sidebar_key(app, code),
        }
    } else {
        editor_key(app, key.code);
    }
}

fn typing_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Enter => app.commit_edit(),
        KeyCode::Esc => app.editor_state.input = None,
        KeyCode::Backspace => {
            if let Some(input) = app.editor_state.input.as_mut() {
                input.pop();
            }
        }
        KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(input) = app.editor_state.input.as_mut()
                && input.chars().count() < MAX_INPUT_CHARS
            {
                input.push(character);
            }
        }
        _ => {}
    }
}

fn editor_key(app: &mut App, code: KeyCode) {
    let state = &mut app.editor_state;
    match code {
        KeyCode::Up => state.selected = state.selected.saturating_sub(1),
        KeyCode::Down if state.selected + 1 < app.config_rows.len() => state.selected += 1,
        KeyCode::Enter => {
            let value = app
                .editor
                .as_ref()
                .map(|editor| editor.value_for_edit(state.selected))
                .unwrap_or_default();
            state.input = Some(value);
            state.info = None;
        }
        KeyCode::Char('s') => app.save_config(),
        KeyCode::Esc => state.focused = false,
        _ => {}
    }
}

fn settings_key(app: &mut App, code: KeyCode) {
    let selected = app.settings_state.selected;
    if !app.settings_state.focused {
        match code {
            KeyCode::Enter => app.settings_state.focused = true,
            code => sidebar_key(app, code),
        }
    } else {
        match code {
            KeyCode::Up => app.settings_state.selected = selected.saturating_sub(1),
            KeyCode::Down if selected + 1 < SETTINGS_FIELDS.len() => {
                app.settings_state.selected += 1;
            }
            KeyCode::Enter => {
                let directory_mode = SETTINGS_FIELDS
                    .get(selected)
                    .is_some_and(|&(_, kind)| kind == FieldKind::Directory);
                app.open_explorer(ExplorerTrigger::Settings(selected), directory_mode);
            }
            KeyCode::Backspace => app.clear_setting(selected),
            KeyCode::Esc => app.settings_state.focused = false,
            _ => {}
        }
    }
}

fn explorer_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Up => app.explorer.previous(),
        KeyCode::Down => app.explorer.next(),
        KeyCode::Enter => {
            if let Some(path) = app.explorer.select() {
                app.file_picked(path);
            }
        }
        KeyCode::Backspace => app.explorer.parent(),
        KeyCode::Esc => app.close_explorer(),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_editor::field_index;
    use crate::remote::Remote;
    use crate::settings::Settings;
    use crate::test_utils::{BCH_CHIPNET_CONFIG, sample_miners, write_config};
    use std::path::PathBuf;
    use tokio::runtime::Handle;

    #[tokio::test]
    async fn test_q_quits() {
        let mut app = App::new(Settings::default(), PathBuf::from("."), Handle::current());
        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(handle_key(&mut app, key, Instant::now()).is_break());
    }

    #[tokio::test]
    async fn test_ctrl_c_quits_while_typing() {
        let mut app = App::new(Settings::default(), PathBuf::from("."), Handle::current());
        app.screen = Screen::PoolConfig;
        app.editor_state.input = Some(String::new());
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(handle_key(&mut app, key, Instant::now()).is_break());
    }

    #[tokio::test]
    async fn test_down_moves_to_the_next_screen() {
        let mut app = App::new(Settings::default(), PathBuf::from("."), Handle::current());
        let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        assert!(handle_key(&mut app, key, Instant::now()).is_continue());
        assert_eq!(app.screen, Screen::Bchn);
        assert_eq!(app.sidebar_index, 1);
    }

    #[tokio::test]
    async fn test_right_switches_the_bchn_tab_and_stops_at_the_last() {
        let mut app = App::new(Settings::default(), PathBuf::from("."), Handle::current());
        app.select_sidebar(1);
        let key = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        assert!(handle_key(&mut app, key, Instant::now()).is_continue());
        assert!(handle_key(&mut app, key, Instant::now()).is_continue());
        assert!(handle_key(&mut app, key, Instant::now()).is_continue());
        assert_eq!(app.bchn_tab, BCHN_TABS.len() - 1);
    }

    #[tokio::test]
    async fn test_enter_on_the_miners_tab_focuses_the_table() {
        let mut app = App::new(Settings::default(), PathBuf::from("."), Handle::current());
        app.select_sidebar(3);
        app.pool_tab = MINERS_TAB;
        app.miners = Remote::Ready(sample_miners());
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert!(handle_key(&mut app, enter, Instant::now()).is_continue());
        assert!(app.miners_focused);
        let escape = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(handle_key(&mut app, escape, Instant::now()).is_continue());
        assert!(!app.miners_focused);
    }

    #[tokio::test]
    async fn test_editing_a_setting_by_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(dir.path(), BCH_CHIPNET_CONFIG);
        let mut app = App::new(Settings::default(), dir.path().into(), Handle::current());
        app.open_node_config(path);
        app.select_sidebar(2);
        let now = Instant::now();
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert!(handle_key(&mut app, enter, now).is_continue());
        assert!(app.editor_state.focused);
        app.editor_state.selected = field_index("stratum", "pool_signature").unwrap();
        assert!(handle_key(&mut app, enter, now).is_continue());
        assert_eq!(app.editor_state.input.as_deref(), Some("P2Poolv2"));
        let backspace = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);
        assert!(handle_key(&mut app, backspace, now).is_continue());
        assert!(handle_key(&mut app, backspace, now).is_continue());
        let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(handle_key(&mut app, q, now).is_continue());
        assert!(handle_key(&mut app, enter, now).is_continue());
        assert_eq!(app.editor_state.input, None);
        assert_eq!(
            app.config_rows[field_index("stratum", "pool_signature").unwrap()]
                .value
                .as_deref(),
            Some("P2Poolq")
        );
    }

    #[tokio::test]
    async fn test_enter_without_a_config_opens_the_file_picker() {
        let mut app = App::new(Settings::default(), PathBuf::from("."), Handle::current());
        app.select_sidebar(2);
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert!(handle_key(&mut app, enter, Instant::now()).is_continue());
        assert_eq!(app.screen, Screen::FileExplorer);
        assert_eq!(app.explorer_trigger, Some(ExplorerTrigger::PoolConfig));
        let escape = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(handle_key(&mut app, escape, Instant::now()).is_continue());
        assert_eq!(app.screen, Screen::PoolConfig);
    }

    #[tokio::test]
    async fn test_the_settings_directory_field_picks_a_directory() {
        let mut app = App::new(Settings::default(), PathBuf::from("."), Handle::current());
        app.select_sidebar(4);
        let now = Instant::now();
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert!(handle_key(&mut app, enter, now).is_continue());
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        assert!(handle_key(&mut app, down, now).is_continue());
        assert!(handle_key(&mut app, enter, now).is_continue());
        assert_eq!(app.screen, Screen::FileExplorer);
        assert!(app.explorer.directory_mode);
        assert_eq!(app.explorer_trigger, Some(ExplorerTrigger::Settings(1)));
    }

    #[tokio::test]
    async fn test_a_key_release_is_ignored() {
        let mut app = App::new(Settings::default(), PathBuf::from("."), Handle::current());
        let mut key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        key.kind = KeyEventKind::Release;
        assert!(handle_key(&mut app, key, Instant::now()).is_continue());
        assert_eq!(app.screen, Screen::Home);
    }
}
