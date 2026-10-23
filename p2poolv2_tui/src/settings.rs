// SPDX-FileCopyrightText: 2024 PDM Authors
// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! This tool's own settings in `settings.toml`, ported from PDM's
//! `settings.rs`.
//!
//! The default directory is `P2POOL_TUI_CONFIG_DIR` when set, otherwise
//! `p2poolv2_tui` in the platform's configuration directory:
//! `$XDG_CONFIG_HOME` or `~/.config` on Linux, `~/Library/Application
//! Support` on macOS and `%APPDATA%` on Windows. With a settings directory
//! chosen, the settings are written there and a copy stays in the default
//! directory, so the choice is found on the next start. The functions take
//! the directory, so nothing here reads the environment but
//! [`default_dir`].
//!
//! PDM found the platform directory with the `directories` crate; its
//! `option-ext` dependency is MPL-2.0, which the workspace's `deny.toml`
//! does not allow, and the three rules above are all it was used for.

use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// The environment variable naming the default settings directory.
pub const CONFIG_DIR_VARIABLE: &str = "P2POOL_TUI_CONFIG_DIR";

const FILE_NAME: &str = "settings.toml";

/// This tool's directory inside the platform's configuration directory.
const APP_DIR: &str = "p2poolv2_tui";

/// The settings.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Settings {
    /// The node's config.toml, which names the API, BCHN and the network.
    pub node_config_path: Option<PathBuf>,
    /// A directory chosen to keep `settings.toml` in.
    pub settings_dir_override: Option<PathBuf>,
}

/// Where `settings.toml` is by default.
pub fn default_dir() -> Result<PathBuf, String> {
    if let Some(dir) = env::var_os(CONFIG_DIR_VARIABLE) {
        return Ok(PathBuf::from(dir));
    }
    platform_config_dir()
        .map(|dir| dir.join(APP_DIR))
        .ok_or_else(|| format!("Cannot find a settings directory; set {CONFIG_DIR_VARIABLE}"))
}

/// An environment variable that is set and not empty, as a path.
fn path_variable(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(windows)]
fn platform_config_dir() -> Option<PathBuf> {
    path_variable("APPDATA")
}

#[cfg(target_os = "macos")]
fn platform_config_dir() -> Option<PathBuf> {
    path_variable("HOME").map(|home| home.join("Library").join("Application Support"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_config_dir() -> Option<PathBuf> {
    path_variable("XDG_CONFIG_HOME")
        .or_else(|| path_variable("HOME").map(|home| home.join(".config")))
}

/// The settings in `default_dir`, or in the directory they name. A
/// missing file gives the defaults; a file that does not parse is an error,
/// so it is not overwritten unread.
pub fn load(default_dir: &Path) -> Result<Settings, String> {
    let settings = read(&default_dir.join(FILE_NAME))?;
    match &settings.settings_dir_override {
        Some(dir) if dir.join(FILE_NAME).is_file() => read(&dir.join(FILE_NAME)),
        Some(_) | None => Ok(settings),
    }
}

fn read(path: &Path) -> Result<Settings, String> {
    match fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text)
            .map_err(|error| format!("{} does not parse: {}", path.display(), error.message())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(Settings::default()),
        Err(error) => Err(format!("Cannot read {}: {error}", path.display())),
    }
}

/// Writes the settings to the directory they name, if any, and to
/// `default_dir`.
pub fn save(settings: &Settings, default_dir: &Path) -> Result<(), String> {
    let text = toml::to_string_pretty(settings)
        .map_err(|error| format!("Cannot write the settings: {error}"))?;
    if let Some(dir) = &settings.settings_dir_override {
        write(dir, &text)?;
    }
    write(default_dir, &text)
}

fn write(dir: &Path, text: &str) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|error| format!("Cannot create {}: {error}", dir.display()))?;
    let path = dir.join(FILE_NAME);
    fs::write(&path, text).map_err(|error| format!("Cannot write {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_without_a_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()).unwrap(), Settings::default());
    }

    #[test]
    fn test_save_then_load() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings {
            node_config_path: Some(PathBuf::from("/srv/p2pool/config.toml")),
            settings_dir_override: None,
        };
        save(&settings, dir.path()).unwrap();
        assert_eq!(load(dir.path()).unwrap(), settings);
    }

    #[test]
    fn test_an_override_directory_gets_the_settings_and_the_default_a_copy() {
        let default_dir = tempfile::tempdir().unwrap();
        let chosen = tempfile::tempdir().unwrap();
        let settings = Settings {
            node_config_path: Some(PathBuf::from("node1/config.toml")),
            settings_dir_override: Some(chosen.path().to_path_buf()),
        };
        save(&settings, default_dir.path()).unwrap();
        assert!(chosen.path().join(FILE_NAME).is_file());
        assert_eq!(load(default_dir.path()).unwrap(), settings);
    }

    #[test]
    fn test_load_prefers_the_override_directorys_file() {
        let default_dir = tempfile::tempdir().unwrap();
        let chosen = tempfile::tempdir().unwrap();
        let pointer = Settings {
            node_config_path: None,
            settings_dir_override: Some(chosen.path().to_path_buf()),
        };
        write(default_dir.path(), &toml::to_string(&pointer).unwrap()).unwrap();
        let chosen_settings = Settings {
            node_config_path: Some(PathBuf::from("node2/config.toml")),
            settings_dir_override: Some(chosen.path().to_path_buf()),
        };
        write(chosen.path(), &toml::to_string(&chosen_settings).unwrap()).unwrap();
        assert_eq!(load(default_dir.path()).unwrap(), chosen_settings);
    }

    #[test]
    fn test_a_broken_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(FILE_NAME), "node_config_path = [").unwrap();
        assert!(load(dir.path()).unwrap_err().contains("does not parse"));
    }
}
