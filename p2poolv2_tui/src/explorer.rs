// SPDX-FileCopyrightText: 2024 PDM Authors
// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A file and directory picker, ported from PDM's `FileExplorer`.
//!
//! It keeps the current directory, its sorted entries and the selection.
//! A directory with more than [`MAX_ENTRIES`] entries lists only the first
//! of them, so a huge directory cannot grow the list without bound.

use std::fs;
use std::path::{Path, PathBuf};

/// Most entries listed for one directory.
pub const MAX_ENTRIES: usize = 4096;

/// One row of the listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    /// Picks the current directory, in directory mode.
    UseThisDirectory,
    /// Goes up to the parent directory.
    Parent,
    Directory(PathBuf),
    File(PathBuf),
}

/// The picker's state.
#[derive(Clone, Debug)]
pub struct FileExplorer {
    pub current_dir: PathBuf,
    pub entries: Vec<Entry>,
    pub selected_index: usize,
    /// Pick a directory instead of a file.
    pub directory_mode: bool,
}

impl FileExplorer {
    /// A picker starting at `start`.
    pub fn new(start: PathBuf, directory_mode: bool) -> Self {
        let mut explorer = Self {
            current_dir: start,
            entries: Vec::new(),
            selected_index: 0,
            directory_mode,
        };
        explorer.load_directory();
        explorer
    }

    /// Reads `current_dir` into `entries`: directories first, then files,
    /// each sorted, after `..` when there is a parent.
    pub fn load_directory(&mut self) {
        self.entries.clear();
        self.selected_index = 0;
        if self.directory_mode {
            self.entries.push(Entry::UseThisDirectory);
        }
        if self.current_dir.parent().is_some() {
            self.entries.push(Entry::Parent);
        }
        let Ok(read_dir) = fs::read_dir(&self.current_dir) else {
            return;
        };
        let mut directories = Vec::new();
        let mut files = Vec::new();
        for path in read_dir
            .flatten()
            .map(|entry| entry.path())
            .take(MAX_ENTRIES)
        {
            if path.is_dir() {
                directories.push(path);
            } else if !self.directory_mode {
                files.push(path);
            }
        }
        directories.sort();
        files.sort();
        self.entries.reserve(directories.len() + files.len());
        self.entries
            .extend(directories.into_iter().map(Entry::Directory));
        self.entries.extend(files.into_iter().map(Entry::File));
    }

    /// Moves the selection down, wrapping at the end.
    pub fn next(&mut self) {
        if !self.entries.is_empty() {
            self.selected_index = (self.selected_index + 1) % self.entries.len();
        }
    }

    /// Moves the selection up, wrapping at the start.
    pub fn previous(&mut self) {
        if !self.entries.is_empty() {
            self.selected_index = self
                .selected_index
                .checked_sub(1)
                .unwrap_or(self.entries.len() - 1);
        }
    }

    /// Goes up to the parent directory, if there is one.
    pub fn parent(&mut self) {
        if let Some(parent) = self.current_dir.parent().map(Path::to_path_buf) {
            self.current_dir = parent;
            self.load_directory();
        }
    }

    /// Acts on the selected entry: enters a directory, goes up for `..`,
    /// and returns the path picked, a file or (in directory mode) the
    /// current directory.
    pub fn select(&mut self) -> Option<PathBuf> {
        match self.entries.get(self.selected_index).cloned()? {
            Entry::UseThisDirectory => Some(self.current_dir.clone()),
            Entry::Parent => {
                self.parent();
                None
            }
            Entry::Directory(path) => {
                self.current_dir = path;
                self.load_directory();
                None
            }
            Entry::File(path) => Some(path),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lists_directories_before_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.toml"), "").unwrap();
        fs::create_dir(dir.path().join("z")).unwrap();
        let explorer = FileExplorer::new(dir.path().to_path_buf(), false);
        assert_eq!(
            explorer.entries,
            vec![
                Entry::Parent,
                Entry::Directory(dir.path().join("z")),
                Entry::File(dir.path().join("a.toml")),
            ]
        );
    }

    #[test]
    fn test_directory_mode_hides_files_and_offers_this_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.toml"), "").unwrap();
        let explorer = FileExplorer::new(dir.path().to_path_buf(), true);
        assert_eq!(
            explorer.entries,
            vec![Entry::UseThisDirectory, Entry::Parent]
        );
    }

    #[test]
    fn test_select_a_file_returns_it() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "").unwrap();
        let mut explorer = FileExplorer::new(dir.path().to_path_buf(), false);
        explorer.next();
        assert_eq!(explorer.select(), Some(dir.path().join("config.toml")));
    }

    #[test]
    fn test_select_a_directory_enters_it() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("node1")).unwrap();
        let mut explorer = FileExplorer::new(dir.path().to_path_buf(), false);
        explorer.next();
        assert_eq!(explorer.select(), None);
        assert_eq!(explorer.current_dir, dir.path().join("node1"));
    }

    #[test]
    fn test_select_this_directory_returns_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut explorer = FileExplorer::new(dir.path().to_path_buf(), true);
        assert_eq!(explorer.select(), Some(dir.path().to_path_buf()));
    }

    #[test]
    fn test_previous_wraps_to_the_last_entry() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.toml"), "").unwrap();
        let mut explorer = FileExplorer::new(dir.path().to_path_buf(), false);
        explorer.previous();
        assert_eq!(explorer.selected_index, 1);
    }

    #[test]
    fn test_parent_goes_up() {
        let dir = tempfile::tempdir().unwrap();
        let mut explorer = FileExplorer::new(dir.path().join("missing"), false);
        explorer.parent();
        assert_eq!(explorer.current_dir, dir.path());
    }
}
