// SPDX-FileCopyrightText: 2024 PDM Authors
// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Edits the node's config.toml, ported from PDM's `p2poolv2_config.rs`
//! and its save path.
//!
//! PDM edited a loaded `Config` and wrote its values back. That would write
//! `P2POOL__` environment overrides, such as a password kept out of the
//! file on purpose, into the file. Here the file itself is the document:
//! edits go into a `toml_edit` document, which keeps comments and layout,
//! and the node's own parser checks the result before it is saved. The
//! fields are the fork's, Bitcoin Cash first; settings not listed stay as
//! they are in the file.

use crate::format;
use crate::views::config::ConfigRow;
use p2poolv2_config::{Chain, Config};
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item, Value};

/// The largest config file opened.
pub const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// What a setting's value must be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueKind {
    Text,
    Integer,
    Boolean,
    /// One of these words.
    Choice(&'static [&'static str]),
    /// Words separated by commas, stored as an array of strings.
    List,
}

/// One setting the editor lists.
#[derive(Clone, Copy, Debug)]
pub struct FieldSpec {
    pub section: &'static str,
    pub key: &'static str,
    pub description: &'static str,
    pub type_hint: &'static str,
    pub kind: ValueKind,
    /// The node refuses the file without it.
    pub required: bool,
    /// What the node uses when the file leaves it out.
    pub default: Option<&'static str>,
    /// A password, token or secret key: never shown.
    pub sensitive: bool,
}

const fn spec(
    section: &'static str,
    key: &'static str,
    description: &'static str,
    kind: ValueKind,
    required: bool,
    default: Option<&'static str>,
) -> FieldSpec {
    let type_hint = match kind {
        ValueKind::Text => "text",
        ValueKind::Integer => "whole number",
        ValueKind::Boolean => "true | false",
        ValueKind::Choice(_) => "one of the words in the description",
        ValueKind::List => "comma-separated list",
    };
    FieldSpec {
        section,
        key,
        description,
        type_hint,
        kind,
        required,
        default,
        sensitive: false,
    }
}

const fn required(
    section: &'static str,
    key: &'static str,
    description: &'static str,
    kind: ValueKind,
) -> FieldSpec {
    spec(section, key, description, kind, true, None)
}

const fn optional(
    section: &'static str,
    key: &'static str,
    description: &'static str,
    kind: ValueKind,
    default: Option<&'static str>,
) -> FieldSpec {
    spec(section, key, description, kind, false, default)
}

const fn secret(field: FieldSpec) -> FieldSpec {
    FieldSpec {
        sensitive: true,
        ..field
    }
}

const NETWORKS: &[&str] = &["mainnet", "chipnet", "regtest"];
const CHAINS: &[&str] = &["bitcoin-cash", "bitcoin"];
const SHARE_CHAINS: &[&str] = &["main", "mini"];
const TEMPLATE_SOURCES: &[&str] = &["rpc", "sv2"];
const MODES: &[&str] = &["p2poolv2", "hydrapool"];
const LOG_LEVELS: &[&str] = &["error", "warn", "info", "debug", "trace"];

/// The settings listed, in order.
pub const FIELDS: [FieldSpec; 44] = [
    // The network switch and the chain come first: they decide the rest.
    optional(
        "stratum",
        "bitcoin_cash_network",
        "The Bitcoin Cash network: mainnet, chipnet or regtest. The only switch between them.",
        ValueKind::Choice(NETWORKS),
        None,
    ),
    optional(
        "stratum",
        "chain",
        "Block chain: bitcoin-cash for this tool (bitcoin is upstream's).",
        ValueKind::Choice(CHAINS),
        Some("bitcoin"),
    ),
    optional(
        "stratum",
        "bch_share_chain",
        "Share chain: main, or mini (256 times easier shares, for small miners).",
        ValueKind::Choice(SHARE_CHAINS),
        Some("main"),
    ),
    required(
        "stratum",
        "hostname",
        "Stratum V1 listen address",
        ValueKind::Text,
    ),
    required("stratum", "port", "Stratum V1 port", ValueKind::Integer),
    optional(
        "stratum",
        "sv2_port",
        "Stratum V2 port (unset: no SV2 socket)",
        ValueKind::Integer,
        None,
    ),
    optional(
        "stratum",
        "miner_address",
        "Share chain address (p2pool:, tp2pool:, rp2pool:) owning every share this node mines",
        ValueKind::Text,
        None,
    ),
    required(
        "stratum",
        "bootstrap_address",
        "CashAddr the first jobs pay before any share exists",
        ValueKind::Text,
    ),
    optional(
        "stratum",
        "solo_address",
        "CashAddr for solo mining payouts",
        ValueKind::Text,
        None,
    ),
    optional(
        "stratum",
        "donation",
        "Donation in basis points (100 = 1%); 0 opts out",
        ValueKind::Integer,
        Some("150"),
    ),
    optional(
        "stratum",
        "donation_address",
        "Donation address; on Bitcoin Cash only the built-in one is accepted",
        ValueKind::Text,
        None,
    ),
    optional(
        "stratum",
        "fee",
        "Operator fee in basis points, taken after the donation (both at most 10%)",
        ValueKind::Integer,
        None,
    ),
    optional(
        "stratum",
        "fee_address",
        "CashAddr the operator fee is paid to",
        ValueKind::Text,
        None,
    ),
    required(
        "stratum",
        "start_difficulty",
        "Difficulty new miners start at",
        ValueKind::Integer,
    ),
    required(
        "stratum",
        "minimum_difficulty",
        "Lowest difficulty a miner is given",
        ValueKind::Integer,
    ),
    optional(
        "stratum",
        "maximum_difficulty",
        "Highest difficulty a miner is given (unset: no limit)",
        ValueKind::Integer,
        None,
    ),
    optional(
        "stratum",
        "template_source",
        "Where block templates come from: rpc (BCHN) or sv2 (a Template Provider)",
        ValueKind::Choice(TEMPLATE_SOURCES),
        Some("rpc"),
    ),
    optional(
        "stratum",
        "template_provider_address",
        "HOST:PORT of the Template Provider, with template_source = sv2",
        ValueKind::Text,
        None,
    ),
    optional(
        "stratum",
        "template_provider_authority_public_key",
        "The Template Provider's authority public key (needed unless it is on loopback)",
        ValueKind::Text,
        None,
    ),
    optional(
        "stratum",
        "sv2_authority_public_key",
        "This node's Stratum V2 authority public key (Noise)",
        ValueKind::Text,
        None,
    ),
    secret(optional(
        "stratum",
        "sv2_authority_secret_key",
        "This node's Stratum V2 authority secret key; prefer P2POOL__STRATUM__SV2_AUTHORITY_SECRET_KEY",
        ValueKind::Text,
        None,
    )),
    required(
        "stratum",
        "zmqpubhashblock",
        "BCHN's ZMQ hashblock publisher, e.g. tcp://127.0.0.1:28332",
        ValueKind::Text,
    ),
    required(
        "stratum",
        "network",
        "Bitcoin network name; ignored on Bitcoin Cash (taken from bitcoin_cash_network)",
        ValueKind::Text,
    ),
    required(
        "stratum",
        "version_mask",
        "Version-rolling mask, hex (e.g. 1fffe000)",
        ValueKind::Text,
    ),
    optional(
        "stratum",
        "pool_signature",
        "Text in each coinbase (at most 16 bytes)",
        ValueKind::Text,
        None,
    ),
    optional(
        "stratum",
        "wait_for_chain_sync",
        "Wait for the share chain to catch up before taking miners",
        ValueKind::Boolean,
        Some("true"),
    ),
    optional(
        "stratum",
        "mode",
        "p2poolv2 (share chain) or hydrapool (standalone PPLNS)",
        ValueKind::Choice(MODES),
        Some("p2poolv2"),
    ),
    required(
        "bitcoinrpc",
        "url",
        "BCHN JSON-RPC URL, e.g. http://127.0.0.1:48332 on chipnet",
        ValueKind::Text,
    ),
    required("bitcoinrpc", "username", "BCHN RPC user", ValueKind::Text),
    secret(required(
        "bitcoinrpc",
        "password",
        "BCHN RPC password",
        ValueKind::Text,
    )),
    required(
        "network",
        "listen_address",
        "Share chain P2P listen address (multiaddr)",
        ValueKind::Text,
    ),
    optional(
        "network",
        "dial_peers",
        "Share chain peers to dial, comma-separated multiaddrs",
        ValueKind::List,
        None,
    ),
    optional(
        "network",
        "external_address",
        "Address advertised to peers when behind NAT (multiaddr)",
        ValueKind::Text,
        None,
    ),
    required(
        "network",
        "max_established_incoming",
        "Most inbound peers",
        ValueKind::Integer,
    ),
    required(
        "network",
        "max_established_outgoing",
        "Most outbound peers",
        ValueKind::Integer,
    ),
    required("store", "path", "Share store directory", ValueKind::Text),
    optional(
        "logging",
        "level",
        "Log level: error, warn, info, debug or trace",
        ValueKind::Choice(LOG_LEVELS),
        Some("info"),
    ),
    optional(
        "logging",
        "file",
        "Log file (unset: no file)",
        ValueKind::Text,
        None,
    ),
    required("api", "hostname", "API listen address", ValueKind::Text),
    required("api", "port", "API port", ValueKind::Integer),
    optional(
        "api",
        "auth_user",
        "API login user (unset: no login)",
        ValueKind::Text,
        None,
    ),
    secret(optional(
        "api",
        "auth_password",
        "API password this tool logs in with (the node checks auth_token)",
        ValueKind::Text,
        None,
    )),
    optional(
        "api",
        "public_dashboard",
        "Serve the dashboard and share chain routes without the login",
        ValueKind::Boolean,
        Some("false"),
    ),
    optional(
        "api",
        "web_root",
        "Folder of a self-hosted frontend to serve",
        ValueKind::Text,
        None,
    ),
];

/// The index in [`FIELDS`] of `section.key`.
pub fn field_index(section: &str, key: &str) -> Option<usize> {
    FIELDS
        .iter()
        .position(|field| field.section == section && field.key == key)
}

/// A node config file being edited.
#[derive(Debug)]
pub struct ConfigEditor {
    path: PathBuf,
    document: DocumentMut,
    /// There are edits not saved yet.
    pub dirty: bool,
}

impl ConfigEditor {
    /// Opens the config file at `path`.
    pub fn open(path: &Path) -> Result<Self, String> {
        let size = fs::metadata(path)
            .map_err(|error| format!("Cannot read {}: {error}", path.display()))?
            .len();
        if size > MAX_CONFIG_BYTES {
            return Err(format!(
                "{} is larger than {MAX_CONFIG_BYTES} bytes; not a node config",
                path.display()
            ));
        }
        let text = fs::read_to_string(path)
            .map_err(|error| format!("Cannot read {}: {error}", path.display()))?;
        let document = text.parse::<DocumentMut>().map_err(|error| {
            format!(
                "{} is not valid TOML: {}",
                path.display(),
                located(&text, error.span(), error.message())
            )
        })?;
        Ok(Self {
            path: path.to_path_buf(),
            document,
            dirty: false,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The listed settings with their values in the document.
    pub fn rows(&self) -> Vec<ConfigRow> {
        FIELDS
            .iter()
            .map(|field| ConfigRow {
                section: field.section,
                key: field.key,
                description: field.description,
                type_hint: field.type_hint,
                value: self.item(field).and_then(shown),
                default: field.default,
                sensitive: field.sensitive,
            })
            .collect()
    }

    fn item(&self, field: &FieldSpec) -> Option<&Item> {
        self.document
            .get(field.section)?
            .as_table_like()?
            .get(field.key)
    }

    /// The value to start editing setting `index` from: what is in the file,
    /// or nothing for a secret, so a secret is typed anew rather than shown.
    pub fn value_for_edit(&self, index: usize) -> String {
        match FIELDS.get(index) {
            Some(field) if !field.sensitive => self.item(field).and_then(shown).unwrap_or_default(),
            Some(_) | None => String::new(),
        }
    }

    /// Sets setting `index` to `input`, typed as the setting says, and
    /// returns whether the document changed. An empty input removes an
    /// optional setting, so the node uses its default; a secret is never
    /// offered to edit from, so for a secret an empty input keeps it.
    pub fn set(&mut self, index: usize, input: &str) -> Result<bool, String> {
        let field = FIELDS
            .get(index)
            .ok_or_else(|| format!("There is no setting {index}"))?;
        let input = input.trim();
        if input.is_empty() && field.sensitive {
            return Ok(false);
        }
        if input.is_empty() {
            if field.required {
                return Err(format!(
                    "{}.{} is required; it cannot be empty",
                    field.section, field.key
                ));
            }
            if let Some(table) = self
                .document
                .get_mut(field.section)
                .and_then(Item::as_table_like_mut)
            {
                table.remove(field.key);
            }
        } else {
            let mut value = parse(field, input)?;
            let table = self
                .document
                .entry(field.section)
                .or_insert(toml_edit::table())
                .as_table_like_mut()
                .ok_or_else(|| format!("[{}] is not a table in the file", field.section))?;
            // Keep the spacing and a trailing comment of the value replaced.
            if let Some(existing) = table.get(field.key).and_then(Item::as_value) {
                *value.decor_mut() = existing.decor().clone();
            }
            table.insert(field.key, Item::Value(value));
        }
        self.dirty = true;
        Ok(true)
    }

    /// Checks the document as the node would read it: the node's parser,
    /// Bitcoin Cash, and its address and key checks.
    pub fn validate(&self) -> Result<(), String> {
        let text = self.document.to_string();
        let config: Config =
            toml::from_str(&text).map_err(|error| located(&text, error.span(), error.message()))?;
        if config.stratum.chain != Chain::BitcoinCash {
            return Err("stratum.chain must be bitcoin-cash for this tool".to_string());
        }
        config
            .with_bitcoin_cash_network_derived()
            .stratum
            .parse()
            .map(|_| ())
            .map_err(|error| error.message)
    }

    /// Writes the document over the file, after [`Self::validate`]. It is
    /// written to a file beside it, with the original's permissions, and
    /// renamed over it, so a failed write leaves the original whole.
    pub fn save(&mut self) -> Result<(), String> {
        self.validate()?;
        let file_name = self
            .path
            .file_name()
            .ok_or_else(|| format!("{} is not a file", self.path.display()))?;
        let mut temporary_name = file_name.to_os_string();
        temporary_name.push(".p2poolv2_tui.tmp");
        let temporary = self.path.with_file_name(temporary_name);
        let permissions = fs::metadata(&self.path)
            .map_err(|error| format!("Cannot read {}: {error}", self.path.display()))?
            .permissions();
        fs::write(&temporary, self.document.to_string())
            .map_err(|error| format!("Cannot write {}: {error}", temporary.display()))?;
        fs::set_permissions(&temporary, permissions)
            .and_then(|()| fs::rename(&temporary, &self.path))
            .map_err(|error| {
                // The temporary file is only a leftover now; failing to
                // remove it changes nothing about the error reported.
                let _ = fs::remove_file(&temporary);
                format!("Cannot replace {}: {error}", self.path.display())
            })?;
        self.dirty = false;
        Ok(())
    }
}

/// The value of `item` as the editor shows it: strings without quotes,
/// arrays as comma-separated words.
fn shown(item: &Item) -> Option<String> {
    let value = item.as_value()?;
    Some(match value {
        Value::String(text) => text.value().clone(),
        Value::Integer(number) => number.value().to_string(),
        Value::Float(number) => number.value().to_string(),
        Value::Boolean(flag) => flag.value().to_string(),
        Value::Array(array) => array
            .iter()
            .map(|element| {
                element
                    .as_str()
                    .map_or_else(|| element.to_string().trim().to_string(), str::to_string)
            })
            .collect::<Vec<_>>()
            .join(", "),
        Value::Datetime(_) | Value::InlineTable(_) => value.to_string().trim().to_string(),
    })
}

/// `input` as a TOML value of `field`'s kind. A secret's input is never
/// quoted back in an error.
fn parse(field: &FieldSpec, input: &str) -> Result<Value, String> {
    let quoted = if field.sensitive {
        "the value".to_string()
    } else {
        format!("'{input}'")
    };
    match field.kind {
        ValueKind::Text => Ok(Value::from(input)),
        ValueKind::Integer => input.parse::<i64>().map(Value::from).map_err(|_| {
            format!(
                "{}.{}: {quoted} is not a whole number",
                field.section, field.key
            )
        }),
        ValueKind::Boolean => match input {
            "true" => Ok(Value::from(true)),
            "false" => Ok(Value::from(false)),
            _ => Err(format!(
                "{}.{}: {quoted} is not true or false",
                field.section, field.key
            )),
        },
        ValueKind::Choice(options) => {
            if options.contains(&input) {
                Ok(Value::from(input))
            } else {
                Err(format!(
                    "{}.{}: {quoted} is not one of {}",
                    field.section,
                    field.key,
                    options.join(", ")
                ))
            }
        }
        ValueKind::List => Ok(Value::Array(
            input
                .split(',')
                .map(str::trim)
                .filter(|word| !word.is_empty())
                .collect(),
        )),
    }
}

/// `message` with the line `span` starts on. Parsers quote the offending
/// line after their message, and a type error quotes the value; only the
/// message without quoted strings and the line number are kept, as either
/// could be a password.
fn located(text: &str, span: Option<Range<usize>>, message: &str) -> String {
    let message = format::without_quoted(message.trim());
    let line = span
        .and_then(|span| text.get(..span.start))
        .map(|before| before.matches('\n').count() + 1);
    match line {
        Some(line) => format!("line {line}: {message}"),
        None => message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{BCH_CHIPNET_CONFIG, write_config};

    #[test]
    fn test_rows_show_the_files_values() {
        let dir = tempfile::tempdir().unwrap();
        let editor = ConfigEditor::open(&write_config(dir.path(), BCH_CHIPNET_CONFIG)).unwrap();
        let rows = editor.rows();
        assert_eq!(rows[0].key, "bitcoin_cash_network");
        assert_eq!(rows[0].value.as_deref(), Some("chipnet"));
        assert_eq!(
            rows[field_index("api", "public_dashboard").unwrap()].value,
            None
        );
        assert!(rows[field_index("bitcoinrpc", "password").unwrap()].sensitive);
    }

    #[test]
    fn test_value_for_edit_of_a_secret_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let editor = ConfigEditor::open(&write_config(dir.path(), BCH_CHIPNET_CONFIG)).unwrap();
        assert_eq!(
            editor.value_for_edit(field_index("bitcoinrpc", "password").unwrap()),
            ""
        );
        assert_eq!(
            editor.value_for_edit(field_index("stratum", "port").unwrap()),
            "3333"
        );
    }

    #[test]
    fn test_save_keeps_comments_and_writes_typed_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(dir.path(), BCH_CHIPNET_CONFIG);
        let mut editor = ConfigEditor::open(&path).unwrap();
        editor
            .set(field_index("stratum", "port").unwrap(), "3334")
            .unwrap();
        editor
            .set(field_index("api", "public_dashboard").unwrap(), "true")
            .unwrap();
        assert!(editor.dirty);
        editor.save().unwrap();
        assert!(!editor.dirty);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("# A chipnet node, for tests."), "{text}");
        assert!(text.contains("port = 3334 # the SV1 port"), "{text}");
        assert!(text.contains("public_dashboard = true"), "{text}");
    }

    #[test]
    fn test_an_empty_value_removes_an_optional_setting() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(dir.path(), BCH_CHIPNET_CONFIG);
        let mut editor = ConfigEditor::open(&path).unwrap();
        editor
            .set(field_index("stratum", "pool_signature").unwrap(), "")
            .unwrap();
        editor.save().unwrap();
        assert!(
            !fs::read_to_string(&path)
                .unwrap()
                .contains("pool_signature")
        );
    }

    #[test]
    fn test_an_empty_value_is_refused_for_a_required_setting() {
        let dir = tempfile::tempdir().unwrap();
        let mut editor = ConfigEditor::open(&write_config(dir.path(), BCH_CHIPNET_CONFIG)).unwrap();
        assert_eq!(
            editor
                .set(field_index("bitcoinrpc", "url").unwrap(), " ")
                .unwrap_err(),
            "bitcoinrpc.url is required; it cannot be empty"
        );
        assert!(!editor.dirty);
    }

    #[test]
    fn test_an_empty_value_keeps_a_secret() {
        let dir = tempfile::tempdir().unwrap();
        let text = BCH_CHIPNET_CONFIG.replace(
            "port = 46884",
            "port = 46884\nauth_user = \"admin\"\nauth_password = \"api-secret\"",
        );
        let path = write_config(dir.path(), &text);
        let mut editor = ConfigEditor::open(&path).unwrap();
        let changed = editor
            .set(field_index("api", "auth_password").unwrap(), "")
            .unwrap();
        assert!(!changed);
        assert!(!editor.dirty);
        assert!(
            editor.rows()[field_index("api", "auth_password").unwrap()]
                .value
                .is_some()
        );
    }

    #[test]
    fn test_a_choice_outside_its_words_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut editor = ConfigEditor::open(&write_config(dir.path(), BCH_CHIPNET_CONFIG)).unwrap();
        assert_eq!(
            editor
                .set(
                    field_index("stratum", "bitcoin_cash_network").unwrap(),
                    "testnet4"
                )
                .unwrap_err(),
            "stratum.bitcoin_cash_network: 'testnet4' is not one of mainnet, chipnet, regtest"
        );
    }

    #[test]
    fn test_a_bad_secret_is_never_quoted() {
        let field = secret(optional(
            "api",
            "auth_password",
            "",
            ValueKind::Integer,
            None,
        ));
        let error = parse(&field, "hunter2").unwrap_err();
        assert!(!error.contains("hunter2"), "{error}");
    }

    #[test]
    fn test_a_list_is_stored_as_an_array() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(dir.path(), BCH_CHIPNET_CONFIG);
        let mut editor = ConfigEditor::open(&path).unwrap();
        editor
            .set(
                field_index("network", "dial_peers").unwrap(),
                "/ip4/10.0.0.2/tcp/6884, /ip4/10.0.0.3/tcp/6884",
            )
            .unwrap();
        editor.save().unwrap();
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains(r#"dial_peers = ["/ip4/10.0.0.2/tcp/6884", "/ip4/10.0.0.3/tcp/6884"]"#)
        );
    }

    #[test]
    fn test_save_refuses_an_address_of_another_network() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(dir.path(), BCH_CHIPNET_CONFIG);
        let mut editor = ConfigEditor::open(&path).unwrap();
        editor
            .set(
                field_index("stratum", "bitcoin_cash_network").unwrap(),
                "mainnet",
            )
            .unwrap();
        assert!(editor.save().is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), BCH_CHIPNET_CONFIG);
    }

    #[test]
    fn test_validate_accepts_the_chipnet_sample() {
        let dir = tempfile::tempdir().unwrap();
        let editor = ConfigEditor::open(&write_config(dir.path(), BCH_CHIPNET_CONFIG)).unwrap();
        editor.validate().unwrap();
    }

    #[test]
    fn test_validate_names_the_line_without_quoting_it() {
        let dir = tempfile::tempdir().unwrap();
        let text = BCH_CHIPNET_CONFIG.replace("port = 46884", "port = \"chipnet-rpc-secret\"");
        let editor = ConfigEditor::open(&write_config(dir.path(), &text)).unwrap();
        let error = editor.validate().unwrap_err();
        assert!(error.starts_with("line "), "{error}");
        assert!(!error.contains("chipnet-rpc-secret"), "{error}");
    }

    #[test]
    fn test_open_reports_invalid_toml_without_quoting_it() {
        let dir = tempfile::tempdir().unwrap();
        let text = BCH_CHIPNET_CONFIG.replace(
            "password = \"chipnet-rpc-secret\"",
            "password = \"chipnet-rpc-secret",
        );
        let error = ConfigEditor::open(&write_config(dir.path(), &text)).unwrap_err();
        assert!(error.contains("is not valid TOML: line "), "{error}");
        assert!(!error.contains("chipnet-rpc-secret"), "{error}");
    }

    #[test]
    fn test_every_field_is_listed_once() {
        let mut keys: Vec<(&str, &str)> = FIELDS
            .iter()
            .map(|field| (field.section, field.key))
            .collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), FIELDS.len());
    }
}
