// SPDX-FileCopyrightText: 2024 PDM Authors
// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The node's systemd user service, PDM's `systemd.rs`: whether it runs, and
//! start, stop or restart it with `systemctl --user`.
//!
//! A node config at `$XDG_CONFIG_HOME/p2poolv2/config-<instance>.toml`
//! (`~/.config` without `XDG_CONFIG_HOME`) belongs to the user service
//! `p2poolv2@<instance>`, which `systemd/user/p2poolv2@.service` runs with
//! that config. Any other config, such as one under `/etc/p2poolv2` that the
//! system service in the repository root runs, has no service here.
//!
//! Unlike PDM the `systemctl` program is a value, so the tests run a fake
//! one without changing `PATH`, which this crate's `forbid(unsafe_code)`
//! rules out.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

const SERVICE_PREFIX: &str = "p2poolv2@";

pub fn user_p2pool_config_dir() -> Option<PathBuf> {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;

    Some(config_home.join("p2poolv2"))
}

/// The service instance of the node config at `path`, if it has one.
pub fn instance_from_config_path(path: &Path) -> Option<String> {
    let config_dir = user_p2pool_config_dir()?;
    instance_from_config_path_in_dir(path, &config_dir)
}

fn instance_from_config_path_in_dir(path: &Path, config_dir: &Path) -> Option<String> {
    if path.parent()? != config_dir {
        return None;
    }

    let filename = path.file_name()?.to_str()?;
    let instance = filename.strip_prefix("config-")?.strip_suffix(".toml")?;

    if !is_valid_instance(instance) {
        return None;
    }

    Some(instance.to_string())
}

fn is_valid_instance(instance: &str) -> bool {
    !instance.is_empty()
        && instance.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        })
}

fn service_name(instance: &str) -> String {
    format!("{SERVICE_PREFIX}{instance}")
}

/// A state-changing systemd action that can be applied to a P2Poolv2 service instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceAction {
    Start,
    Stop,
    Restart,
}

impl ServiceAction {
    fn systemctl_action(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }

    /// Human-readable name for use in UI messages.
    pub fn label(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::Stop => "Stop",
            Self::Restart => "Restart",
        }
    }
}

/// `systemctl --user` for the node's service instances.
#[derive(Clone, Debug)]
pub struct P2PoolV2Service {
    systemctl: OsString,
}

impl Default for P2PoolV2Service {
    fn default() -> Self {
        Self {
            systemctl: OsString::from("systemctl"),
        }
    }
}

impl P2PoolV2Service {
    /// Runs `systemctl` as the program at `systemctl` instead.
    pub fn with_systemctl(systemctl: impl Into<OsString>) -> Self {
        Self {
            systemctl: systemctl.into(),
        }
    }

    pub fn is_running(&self, instance: &str) -> Result<bool, String> {
        let service = validated_service_name(instance)?;
        let output = Command::new(&self.systemctl)
            .args(["--user", "is-active", &service])
            .output()
            .map_err(|error| format!("failed to execute systemctl: {error}"))?;

        Ok(output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "active")
    }

    pub fn run(&self, action: ServiceAction, instance: &str) -> Result<(), String> {
        let action = action.systemctl_action();
        let service = validated_service_name(instance)?;
        let output = Command::new(&self.systemctl)
            .args(["--user", action, &service])
            .output()
            .map_err(|error| format!("failed to execute systemctl {action}: {error}"))?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!(
                "systemctl --user {action} {service} failed: {}",
                stderr.trim()
            ))
        }
    }
}

fn validated_service_name(instance: &str) -> Result<String, String> {
    if !is_valid_instance(instance) {
        return Err(format!("invalid P2Poolv2 service instance: {instance}"));
    }

    Ok(service_name(instance))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::fs;
    #[cfg(unix)]
    use tempfile::TempDir;

    /// A fake `systemctl` that records its arguments in `args` beside it:
    /// `is-active` answers `active` or `inactive`, and other actions succeed
    /// or fail as `systemctl` does without permission.
    #[cfg(unix)]
    fn fake_systemctl(fail_actions: bool, active: bool) -> (TempDir, P2PoolV2Service, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("systemctl");
        let args_file = dir.path().join("args");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nif [ \"$2\" = \"is-active\" ]; then\n  {}\nelse\n  {}\nfi\n",
                args_file.display(),
                if active {
                    "echo active; exit 0"
                } else {
                    "echo inactive; exit 3"
                },
                if fail_actions {
                    "echo permission denied >&2; exit 1"
                } else {
                    "exit 0"
                }
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let service = P2PoolV2Service::with_systemctl(&script);
        (dir, service, args_file)
    }

    #[cfg(unix)]
    #[test]
    fn service_actions_succeed_and_active_service_is_running() {
        let (_dir, service, args_file) = fake_systemctl(false, true);

        for action in [
            ServiceAction::Start,
            ServiceAction::Stop,
            ServiceAction::Restart,
        ] {
            service.run(action, "chipnet").unwrap();
            assert_eq!(
                fs::read_to_string(&args_file).unwrap(),
                format!("--user\n{}\np2poolv2@chipnet\n", action.systemctl_action())
            );
        }
        assert!(service.is_running("chipnet").unwrap());
        assert_eq!(
            fs::read_to_string(args_file).unwrap(),
            "--user\nis-active\np2poolv2@chipnet\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn service_actions_report_systemctl_failures() {
        let (_dir, service, _args_file) = fake_systemctl(true, false);

        for (action, name) in [
            (ServiceAction::Start, "start"),
            (ServiceAction::Stop, "stop"),
            (ServiceAction::Restart, "restart"),
        ] {
            let error = service.run(action, "chipnet").unwrap_err();
            assert!(
                error.contains(&format!(
                    "{name} p2poolv2@chipnet failed: permission denied"
                )),
                "{error}"
            );
        }
        assert!(!service.is_running("chipnet").unwrap());
    }

    #[test]
    fn a_missing_systemctl_is_an_error() {
        let service = P2PoolV2Service::with_systemctl("p2poolv2-tui-no-such-systemctl");
        assert!(
            service
                .is_running("chipnet")
                .unwrap_err()
                .contains("failed to execute systemctl")
        );
        assert!(service.run(ServiceAction::Start, "chipnet").is_err());
    }

    #[test]
    fn instance_validation_accepts_safe_names() {
        for instance in ["mainnet", "chipnet", "regtest", "foo-bar", "foo_bar"] {
            assert!(validated_service_name(instance).is_ok(), "{instance}");
        }
    }

    #[test]
    fn instance_validation_rejects_unsafe_names() {
        for instance in ["foo/bar", "foo bar", "foo@bar", "../foo", ""] {
            assert!(validated_service_name(instance).is_err(), "{instance}");
            let service = P2PoolV2Service::default();
            assert!(service.run(ServiceAction::Start, instance).is_err());
        }
    }

    #[test]
    fn config_path_derives_instance_only_in_config_directory() {
        let config_dir = PathBuf::from("/home/user/.config/p2poolv2");

        assert_eq!(
            instance_from_config_path_in_dir(&config_dir.join("config-chipnet.toml"), &config_dir),
            Some("chipnet".to_string())
        );
        assert_eq!(
            instance_from_config_path_in_dir(&config_dir.join("config-mainnet.toml"), &config_dir),
            Some("mainnet".to_string())
        );

        for path in [
            "/etc/p2poolv2/config-chipnet.toml",
            "/home/user/p2pool/config-chipnet.toml",
            "/home/user/p2pool/config.toml",
            "/home/user/.config/p2poolv2/config.toml",
            "/home/user/.config/p2poolv2/config-.toml",
            "/home/user/.config/p2poolv2/config-a b.toml",
        ] {
            assert_eq!(
                instance_from_config_path_in_dir(Path::new(path), &config_dir),
                None,
                "{path}"
            );
        }
    }

    #[test]
    #[ignore = "requires a user systemd session and an installed p2poolv2@.service"]
    fn start_and_stop_service() {
        let service = P2PoolV2Service::default();
        assert!(!service.is_running("chipnet").unwrap());

        service.run(ServiceAction::Start, "chipnet").unwrap();
        let running = service.is_running("chipnet");
        let stop_result = service.run(ServiceAction::Stop, "chipnet");

        assert!(running.unwrap());
        stop_result.unwrap();
        assert!(!service.is_running("chipnet").unwrap());
    }
}
