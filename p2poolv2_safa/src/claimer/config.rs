// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Bounded operator configuration and secret-free RPC setup.

use std::env;
use std::fs::{self, File};
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};

use bitcoindrpc::BitcoindRpcClient;
use config::{Config as TomlConfig, File as ConfigFile, FileFormat};
use serde::{Deserialize, Serialize};

use crate::cli::read_json;
use crate::{MAX_FEE_RATE, MAX_PAYOUT_SCRIPT_BYTES, MIN_FEE_RATE, from_hex};

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// A transaction ID in display order and an output index.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VaultPoint {
    pub txid: String,
    pub vout: u32,
}

impl VaultPoint {
    /// Check the transaction ID before it can become an RPC parameter.
    pub fn validate(&self) -> Result<(), String> {
        from_hex(&self.txid, Some(32), "vault.txid").map(|_| ())
    }
}

/// Explicit RPC credentials. The password itself is never kept in JSON state.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcConfig {
    pub url: String,
    pub username: String,
    pub password_file: PathBuf,
}

/// Operator settings. Paths are resolved relative to this configuration file.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub deployment: PathBuf,
    pub vault: VaultPoint,
    pub token_file: PathBuf,
    pub wins_log: PathBuf,
    pub payout_scripts: Vec<String>,
    #[serde(default)]
    pub split_scripts: Vec<String>,
    pub p2pool_config: Option<PathBuf>,
    pub rpc: Option<RpcConfig>,
    #[serde(default = "default_fee_rate")]
    pub fee_rate: u32,
    #[serde(default = "default_delay")]
    pub delay_seconds: [u64; 2],
    #[serde(default = "default_poll")]
    pub poll_seconds: u64,
    pub journal: PathBuf,
}

fn default_fee_rate() -> u32 {
    1
}

fn default_delay() -> [u64; 2] {
    [10, 120]
}

fn default_poll() -> u64 {
    5
}

fn relative_to(directory: &Path, path: &mut PathBuf) {
    if path.is_relative() {
        *path = directory.join(&*path);
    }
}

/// Resolve symlinks in existing ancestors and normalize a prospective file path.
fn resolved_path(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|_| "cannot resolve configuration file paths")?
            .join(path)
    };
    let mut ancestor = absolute;
    let mut missing = Vec::with_capacity(4);
    let mut resolved = None;
    while resolved.is_none() {
        match fs::canonicalize(&ancestor) {
            Ok(path) => resolved = Some(path),
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let filename = ancestor
                    .file_name()
                    .ok_or("cannot resolve configuration file paths")?
                    .to_os_string();
                missing.push(filename);
                if !ancestor.pop() {
                    return Err("cannot resolve configuration file paths".into());
                }
            }
            Err(_) => return Err("cannot resolve configuration file paths".into()),
        }
    }
    let mut resolved = resolved.ok_or("cannot resolve configuration file paths")?;
    for component in missing.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    if cfg!(windows) {
        left.as_os_str().to_string_lossy().to_lowercase()
            == right.as_os_str().to_string_lossy().to_lowercase()
    } else {
        left == right
    }
}

fn validate_rpc_url(url: &str) -> Result<(), String> {
    let authority = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .ok_or("RPC URL must use http or https")?
        .split(['/', '?', '#'])
        .next()
        .ok_or("RPC URL must name a host")?;
    if authority.is_empty() || authority.contains('@') {
        return Err("RPC URL must name a host without embedded credentials".into());
    }
    Ok(())
}

/// Read a bounded regular UTF-8 file without including its contents in errors.
fn read_secret_file(path: &Path) -> Result<String, String> {
    let file = File::open(path).map_err(|_| "cannot open RPC credential file")?;
    if !file
        .metadata()
        .map_err(|_| "cannot inspect RPC credential file")?
        .is_file()
    {
        return Err("RPC credential input must be a regular file".into());
    }
    let mut bytes = Vec::with_capacity(MAX_CONFIG_BYTES as usize + 1);
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read RPC credential file")?;
    if bytes.len() > MAX_CONFIG_BYTES as usize {
        return Err("RPC credential file exceeds 1 MiB".into());
    }
    String::from_utf8(bytes).map_err(|_| "RPC credential file must contain UTF-8".into())
}

#[derive(Deserialize)]
struct PoolConfig {
    bitcoinrpc: PoolRpcConfig,
}

#[derive(Deserialize)]
struct PoolRpcConfig {
    url: String,
    username: String,
    password: String,
}

impl Config {
    /// Load at most 1 MiB, validate settings, and resolve configuration paths.
    pub fn load(path: &Path) -> Result<Self, String> {
        let input = read_json(path).map_err(|error| {
            if error == "input JSON exceeds 1 MiB" {
                error
            } else {
                "cannot read claimer configuration as a regular JSON file".into()
            }
        })?;
        let mut configuration: Self = serde_json::from_value(input)
            .map_err(|_| "invalid claimer configuration fields or types")?;
        configuration.validate()?;
        let directory = path.parent().unwrap_or_else(|| Path::new("."));
        relative_to(directory, &mut configuration.deployment);
        relative_to(directory, &mut configuration.token_file);
        relative_to(directory, &mut configuration.wins_log);
        relative_to(directory, &mut configuration.journal);
        if let Some(pool_config) = &mut configuration.p2pool_config {
            relative_to(directory, pool_config);
        }
        if let Some(rpc) = &mut configuration.rpc {
            relative_to(directory, &mut rpc.password_file);
        }
        configuration.validate_paths(path)?;
        Ok(configuration)
    }

    /// Reject ambiguous RPC sources and settings that cannot make valid claims.
    pub fn validate(&self) -> Result<(), String> {
        self.vault.validate()?;
        let paths = [
            &self.deployment,
            &self.token_file,
            &self.wins_log,
            &self.journal,
        ];
        if paths.iter().any(|path| path.as_os_str().is_empty())
            || self
                .p2pool_config
                .as_ref()
                .is_some_and(|path| path.as_os_str().is_empty())
            || self
                .rpc
                .as_ref()
                .is_some_and(|rpc| rpc.password_file.as_os_str().is_empty())
        {
            return Err("configuration file paths must not be empty".into());
        }
        if self.p2pool_config.is_some() == self.rpc.is_some() {
            return Err("set exactly one of p2pool_config or rpc".into());
        }
        if !(MIN_FEE_RATE..=MAX_FEE_RATE).contains(&self.fee_rate) {
            return Err("fee_rate must be from 1 to 3".into());
        }
        if self.delay_seconds[0] > self.delay_seconds[1] {
            return Err("delay_seconds must be [minimum, maximum] in ascending order".into());
        }
        if self.poll_seconds == 0 {
            return Err("poll_seconds must be positive".into());
        }
        if self.payout_scripts.is_empty() {
            return Err("payout_scripts must contain at least one locking bytecode".into());
        }
        for script in self.payout_scripts.iter().chain(&self.split_scripts) {
            let bytes = from_hex(script, None, "locking bytecode")?;
            if bytes.is_empty() || bytes.len() > MAX_PAYOUT_SCRIPT_BYTES {
                return Err(format!(
                    "locking bytecode must contain 1 to {MAX_PAYOUT_SCRIPT_BYTES} bytes"
                ));
            }
        }
        if let Some(rpc) = &self.rpc {
            validate_rpc_url(&rpc.url)?;
        }
        Ok(())
    }

    /// Prevent journal, token state, and lock writes from replacing any input.
    fn validate_paths(&self, config_path: &Path) -> Result<(), String> {
        let mut lock_name = self.journal.as_os_str().to_os_string();
        lock_name.push(".lock");
        let lock_path = PathBuf::from(lock_name);
        let mut token_lock_name = self.token_file.as_os_str().to_os_string();
        token_lock_name.push(".lock");
        let outputs = [
            resolved_path(&self.token_file)?,
            resolved_path(&self.journal)?,
            resolved_path(&lock_path)?,
            resolved_path(&PathBuf::from(token_lock_name))?,
        ];
        let mut inputs = Vec::with_capacity(5);
        inputs.push(resolved_path(config_path)?);
        inputs.push(resolved_path(&self.deployment)?);
        inputs.push(resolved_path(&self.wins_log)?);
        if let Some(path) = &self.p2pool_config {
            inputs.push(resolved_path(path)?);
        }
        if let Some(rpc) = &self.rpc {
            inputs.push(resolved_path(&rpc.password_file)?);
        }
        for (index, output) in outputs.iter().enumerate() {
            if outputs[index + 1..]
                .iter()
                .any(|other| paths_equal(output, other))
                || inputs.iter().any(|input| paths_equal(output, input))
            {
                return Err("token_file, journal, and their locks must be distinct from each other and all input files".into());
            }
        }
        Ok(())
    }

    /// Build the shared workspace RPC client without exposing credential values.
    pub fn rpc_client(&self) -> Result<BitcoindRpcClient, String> {
        let credentials = if let Some(path) = &self.p2pool_config {
            let text = read_secret_file(path)?;
            let pool: PoolConfig = TomlConfig::builder()
                .add_source(ConfigFile::from_str(&text, FileFormat::Toml))
                .build()
                .and_then(TomlConfig::try_deserialize)
                .map_err(|_| "cannot parse P2Pool bitcoinrpc configuration")?;
            pool.bitcoinrpc
        } else if let Some(rpc) = &self.rpc {
            let text = read_secret_file(&rpc.password_file)?;
            let password = text.trim_end_matches(['\r', '\n']).to_owned();
            if password.is_empty() || password.contains(['\r', '\n']) {
                return Err("RPC password file must contain one nonempty line".into());
            }
            PoolRpcConfig {
                url: rpc.url.clone(),
                username: rpc.username.clone(),
                password,
            }
        } else {
            return Err("set exactly one of p2pool_config or rpc".into());
        };
        validate_rpc_url(&credentials.url)?;
        BitcoindRpcClient::new(
            &credentials.url,
            &credentials.username,
            &credentials.password,
        )
        .map_err(|_| "cannot initialize RPC client".into())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use tempfile::tempdir;

    use super::Config;

    #[test]
    fn config_defaults_and_paths_are_relative_to_config_file() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("claimer.json");
        fs::write(
            &path,
            json!({
                "deployment":"deployment.json", "vault":{"txid":"11".repeat(32),"vout":3},
                "token_file":"tokens.json", "wins_log":"wins.jsonl", "payout_scripts":["51"],
                "rpc":{"url":"http://localhost:1234","username":"operator","password_file":"rpc-password"},
                "journal":"journal.json"
            }).to_string(),
        ).unwrap();
        let configuration = Config::load(&path).unwrap();
        assert_eq!(
            configuration.deployment,
            directory.path().join("deployment.json")
        );
        assert_eq!(
            configuration.token_file,
            directory.path().join("tokens.json")
        );
        assert_eq!(
            configuration.rpc.unwrap().password_file,
            directory.path().join("rpc-password")
        );
        assert_eq!(configuration.delay_seconds, [10, 120]);
        assert_eq!(configuration.poll_seconds, 5);
        assert_eq!(configuration.fee_rate, 1);
    }

    #[test]
    fn config_rejects_ambiguous_rpc_sources() {
        let configuration: Config = serde_json::from_value(json!({
            "deployment":"deployment.json", "vault":{"txid":"11".repeat(32),"vout":3},
            "token_file":"tokens.json", "wins_log":"wins.jsonl", "payout_scripts":["51"],
            "p2pool_config":"config.toml",
            "rpc":{"url":"http://localhost:1234","username":"operator","password_file":"rpc-password"},
            "journal":"journal.json"
        })).unwrap();
        assert!(
            configuration
                .validate()
                .unwrap_err()
                .contains("exactly one")
        );
    }

    #[test]
    fn config_rejects_fee_outside_covenant_range() {
        let configuration: Config = serde_json::from_value(json!({
            "deployment":"deployment.json", "vault":{"txid":"11".repeat(32),"vout":3},
            "token_file":"tokens.json", "wins_log":"wins.jsonl", "payout_scripts":["51"],
            "p2pool_config":"config.toml", "journal":"journal.json", "fee_rate":4
        }))
        .unwrap();
        assert!(configuration.validate().unwrap_err().contains("fee_rate"));
    }

    #[test]
    fn config_rejects_reversed_delay_range() {
        let configuration: Config = serde_json::from_value(json!({
            "deployment":"deployment.json", "vault":{"txid":"11".repeat(32),"vout":3},
            "token_file":"tokens.json", "wins_log":"wins.jsonl", "payout_scripts":["51"],
            "p2pool_config":"config.toml", "journal":"journal.json", "delay_seconds":[20,10]
        }))
        .unwrap();
        assert!(
            configuration
                .validate()
                .unwrap_err()
                .contains("delay_seconds")
        );
    }

    #[test]
    fn config_rejects_zero_poll_interval() {
        let configuration: Config = serde_json::from_value(json!({
            "deployment":"deployment.json", "vault":{"txid":"11".repeat(32),"vout":3},
            "token_file":"tokens.json", "wins_log":"wins.jsonl", "payout_scripts":["51"],
            "p2pool_config":"config.toml", "journal":"journal.json", "poll_seconds":0
        }))
        .unwrap();
        assert!(
            configuration
                .validate()
                .unwrap_err()
                .contains("poll_seconds")
        );
    }

    #[test]
    fn config_rejects_oversized_input() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("claimer.json");
        fs::write(&path, vec![b' '; 1024 * 1024 + 1]).unwrap();
        assert!(Config::load(&path).err().unwrap().contains("exceeds 1 MiB"));
    }

    #[test]
    fn malformed_toml_never_exposes_password_in_error() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, "[bitcoinrpc]\npassword = secret-do-not-log\n").unwrap();
        let configuration: Config = serde_json::from_value(json!({
            "deployment":"deployment.json", "vault":{"txid":"11".repeat(32),"vout":3},
            "token_file":"tokens.json", "wins_log":"wins.jsonl", "payout_scripts":["51"],
            "p2pool_config":path, "journal":"journal.json"
        }))
        .unwrap();
        let error = configuration.rpc_client().unwrap_err();
        assert_eq!(error, "cannot parse P2Pool bitcoinrpc configuration");
        assert!(!error.contains("secret-do-not-log"));
    }

    #[tokio::test]
    async fn p2pool_rpc_reads_only_bitcoinrpc_and_accepts_other_sections() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(
            &path,
            "[bitcoinrpc]\nurl = 'http://127.0.0.1:1'\nusername = 'operator'\npassword = 'test-only-password'\n[stratum]\nmerge_mining_tokens = 'tokens.json'\n",
        ).unwrap();
        let configuration: Config = serde_json::from_value(json!({
            "deployment":"deployment.json", "vault":{"txid":"11".repeat(32),"vout":3},
            "token_file":"tokens.json", "wins_log":"wins.jsonl", "payout_scripts":["51"],
            "p2pool_config":path, "journal":"journal.json"
        }))
        .unwrap();
        assert!(configuration.rpc_client().is_ok());
    }

    #[tokio::test]
    async fn explicit_rpc_password_file_accepts_one_line_with_crlf() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("password.txt");
        fs::write(&path, "test-only-password\r\n").unwrap();
        let configuration: Config = serde_json::from_value(json!({
            "deployment":"deployment.json", "vault":{"txid":"11".repeat(32),"vout":3},
            "token_file":"tokens.json", "wins_log":"wins.jsonl", "payout_scripts":["51"],
            "rpc":{"url":"http://127.0.0.1:1","username":"operator","password_file":path},
            "journal":"journal.json"
        }))
        .unwrap();
        assert!(configuration.rpc_client().is_ok());
    }

    #[test]
    fn explicit_rpc_password_file_rejects_extra_lines_without_disclosing_contents() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("password.txt");
        fs::write(&path, "first-secret\nsecond-secret").unwrap();
        let configuration: Config = serde_json::from_value(json!({
            "deployment":"deployment.json", "vault":{"txid":"11".repeat(32),"vout":3},
            "token_file":"tokens.json", "wins_log":"wins.jsonl", "payout_scripts":["51"],
            "rpc":{"url":"http://127.0.0.1:1","username":"operator","password_file":path},
            "journal":"journal.json"
        }))
        .unwrap();
        assert_eq!(
            configuration.rpc_client().unwrap_err(),
            "RPC password file must contain one nonempty line"
        );
    }

    #[test]
    fn malformed_claimer_fields_never_echo_supplied_values() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("claimer.json");
        fs::write(&path, json!({
            "deployment":"deployment.json", "vault":{"txid":"11".repeat(32),"vout":3},
            "token_file":"tokens.json", "wins_log":"wins.jsonl", "payout_scripts":["51"],
            "p2pool_config":"config.toml", "journal":"journal.json", "fee_rate":"secret-do-not-log"
        }).to_string()).unwrap();
        let error = Config::load(&path).err().unwrap();
        assert_eq!(error, "invalid claimer configuration fields or types");
        assert!(!error.contains("secret-do-not-log"));
    }

    #[test]
    fn config_rejects_empty_paths_before_relative_resolution() {
        let configuration: Config = serde_json::from_value(json!({
            "deployment":"deployment.json", "vault":{"txid":"11".repeat(32),"vout":3},
            "token_file":"", "wins_log":"wins.jsonl", "payout_scripts":["51"],
            "p2pool_config":"config.toml", "journal":"journal.json"
        }))
        .unwrap();
        assert_eq!(
            configuration.validate().unwrap_err(),
            "configuration file paths must not be empty"
        );
    }

    #[test]
    fn config_rejects_journal_aliasing_token_file_before_writing() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("claimer.json");
        fs::write(
            &path,
            json!({
                "deployment":"deployment.json", "vault":{"txid":"11".repeat(32),"vout":3},
                "token_file":"tokens.json", "wins_log":"wins.jsonl", "payout_scripts":["51"],
                "p2pool_config":"config.toml", "journal":"./tokens.json"
            })
            .to_string(),
        )
        .unwrap();
        assert!(
            Config::load(&path)
                .err()
                .unwrap()
                .contains("must be distinct")
        );
        assert!(!directory.path().join("tokens.json").exists());
    }

    #[test]
    fn config_rejects_token_file_aliasing_password_file_before_writing() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("claimer.json");
        let credential = directory.path().join("password.txt");
        fs::write(&credential, "test-only-password").unwrap();
        fs::write(&path, json!({
            "deployment":"deployment.json", "vault":{"txid":"11".repeat(32),"vout":3},
            "token_file":"./password.txt", "wins_log":"wins.jsonl", "payout_scripts":["51"],
            "rpc":{"url":"http://127.0.0.1:1","username":"operator","password_file":"password.txt"},
            "journal":"journal.json"
        }).to_string()).unwrap();
        assert!(
            Config::load(&path)
                .err()
                .unwrap()
                .contains("must be distinct")
        );
        assert_eq!(
            fs::read_to_string(credential).unwrap(),
            "test-only-password"
        );
    }
}
