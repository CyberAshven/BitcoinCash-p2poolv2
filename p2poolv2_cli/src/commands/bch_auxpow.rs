// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Offline building and verification of merge-mining v2 commitments and
//! proofs (`docs/bch-auxpow.md`). Public data only: no node, wallet, keys or
//! config.

use super::bch_share_address::parse_network;
use clap::Subcommand;
use p2poolv2_lib::config::BitcoinCashNetwork;
use p2poolv2_lib::shares::auxpow::{AuxProof, AuxiliaryTree, SplitSlot, TokenLeaf, TokenSplit};
use p2poolv2_lib::shares::bch_asert::BchAsert;
use p2poolv2_lib::shares::merge_mining::MAX_MERGE_MINING_TREE_HEIGHT;
use serde::Deserialize;
use serde_json::json;
use std::error::Error;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;

const MAX_INPUT_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Subcommand)]
pub enum AuxpowCommand {
    /// Build output 0 and each token's branch from {"tokens":[...]} JSON
    Build { file: PathBuf },
    /// Verify a hex AuxProof v2 for one token whose leaf JSON is given
    /// separately, as the token's own rules would supply it
    Verify {
        /// File holding the proof's bytes in hex
        proof: PathBuf,
        /// File holding the token's leaf fields as JSON
        token: PathBuf,
        /// The tallest tree the token allows
        #[arg(long, default_value_t = MAX_MERGE_MINING_TREE_HEIGHT)]
        max_height: u8,
        /// Check the parent header's nBits against this network's ASERT
        /// (mainnet or chipnet; regtest has no ASERT anchor and skips it)
        #[arg(long, value_parser = parse_network)]
        network: Option<BitcoinCashNetwork>,
    },
}

/// A token leaf's fields: 32-byte values in hex as hashed, and the compact
/// target as eight hex digits (`"207fffff"`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenJson {
    category: String,
    lock_hash: String,
    state_hash: String,
    payout_hash: String,
    target_bits: String,
    /// The leaf's `ext` in hex; omit it, or give `split` instead.
    #[serde(default)]
    ext: Option<String>,
    /// The token's split, from which `ext` is computed.
    #[serde(default)]
    split: Option<SplitJson>,
}

/// A token's split: each slot's rate in hundredths of a percent and the
/// recipient's locking bytecode in hex; a rate of 0 takes no script.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SplitJson {
    donation_bps: u16,
    #[serde(default)]
    donation_script: String,
    fee_bps: u16,
    #[serde(default)]
    fee_script: String,
}

impl SplitJson {
    fn split(&self) -> Result<TokenSplit, Box<dyn Error>> {
        Ok(TokenSplit {
            donation: SplitSlot::new(self.donation_bps, &hex::decode(&self.donation_script)?)?,
            fee: SplitSlot::new(self.fee_bps, &hex::decode(&self.fee_script)?)?,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenSelection {
    tokens: Vec<TokenJson>,
}

impl TokenJson {
    fn leaf(&self) -> Result<TokenLeaf, Box<dyn Error>> {
        Ok(TokenLeaf {
            category: hash32(&self.category, "category")?,
            lock_hash: hash32(&self.lock_hash, "lock_hash")?,
            state_hash: hash32(&self.state_hash, "state_hash")?,
            payout_hash: hash32(&self.payout_hash, "payout_hash")?,
            target_bits: u32::from_str_radix(&self.target_bits, 16)
                .map_err(|_| "target_bits must be eight hexadecimal digits")?,
            ext: match (&self.ext, &self.split) {
                (Some(_), Some(_)) => return Err("give either ext or split, not both".into()),
                (Some(ext), None) => hash32(ext, "ext")?,
                (None, Some(split)) => split.split()?.ext(),
                (None, None) => [0; 32],
            },
        })
    }
}

fn hash32(text: &str, name: &str) -> Result<[u8; 32], Box<dyn Error>> {
    hex::decode(text)?
        .try_into()
        .map_err(|_| format!("{name} must be 32 bytes of hex").into())
}

/// Run a public-data-only command.
pub fn execute(command: &AuxpowCommand) -> Result<(), Box<dyn Error>> {
    let output = match command {
        AuxpowCommand::Build { file } => {
            let selection: TokenSelection =
                serde_json::from_slice(&read_bounded(File::open(file)?)?)?;
            let leaves = selection
                .tokens
                .iter()
                .map(TokenJson::leaf)
                .collect::<Result<Vec<_>, _>>()?;
            let tree = AuxiliaryTree::build(&leaves)?;
            json!({
                "commitment": {
                    "root": hex::encode(tree.commitment.root),
                    "height": tree.commitment.height,
                    "salt": tree.commitment.salt,
                },
                "output": hex::encode(bitcoin::consensus::serialize(&tree.commitment.output())),
                "tokens": tree.branches.iter().map(|branch| json!({
                    "category": hex::encode(branch.category),
                    "slot": branch.slot,
                    "branch": branch.branch.iter().map(hex::encode).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            })
        }
        AuxpowCommand::Verify {
            proof,
            token,
            max_height,
            network,
        } => {
            let proof_hex = String::from_utf8(read_bounded(File::open(proof)?)?)?;
            let proof = AuxProof::from_bytes(&hex::decode(proof_hex.trim())?)?;
            let token: TokenJson = serde_json::from_slice(&read_bounded(File::open(token)?)?)?;
            proof.verify(
                &token.leaf()?,
                *max_height,
                network.and_then(BchAsert::for_network),
            )?;
            json!({
                "valid_aux_proof": true,
                "parent_hash": proof.header.block_hash().to_string(),
                "coinbase_height": proof.coinbase_height(),
            })
        }
    };
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn read_bounded(reader: impl Read) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut bytes = Vec::with_capacity(4096);
    reader.take(MAX_INPUT_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err("merge-mining input exceeds 1 MiB".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{Cli, Commands};
    use clap::Parser;
    use std::io::Cursor;

    #[test]
    fn build_command_needs_no_config() {
        let cli =
            Cli::try_parse_from(["p2poolv2_cli", "bch-auxpow", "build", "tokens.json"]).unwrap();
        assert!(cli.config.is_none());
        assert!(matches!(
            cli.command,
            Some(Commands::BchAuxpow {
                command: AuxpowCommand::Build { .. }
            })
        ));
    }

    #[test]
    fn verify_command_requires_the_token_independently() {
        assert!(
            Cli::try_parse_from(["p2poolv2_cli", "bch-auxpow", "verify", "proof.hex"]).is_err()
        );
        let cli = Cli::try_parse_from([
            "p2poolv2_cli",
            "bch-auxpow",
            "verify",
            "proof.hex",
            "token.json",
            "--network",
            "chipnet",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::BchAuxpow {
                command: AuxpowCommand::Verify {
                    max_height: 16,
                    network: Some(BitcoinCashNetwork::Chipnet),
                    ..
                }
            })
        ));
    }

    /// Pickaxe's token A from the merge-mining v2 golden vectors.
    #[test]
    fn token_json_gives_the_golden_leaf() {
        let token: TokenJson = serde_json::from_value(json!({
            "category": "11".repeat(32),
            "lock_hash": "953ccfa596a6c6d39e5980194539124fdcff116a571455a212baed811f585ee0",
            "state_hash": "aa".repeat(32),
            "payout_hash": "22650758a42ebfd7306e771ad24c442cad795ed9e549392233729f647a2b1be5",
            "target_bits": "207fffff",
            "ext": "00".repeat(32),
        }))
        .unwrap();
        assert_eq!(
            hex::encode(token.leaf().unwrap().hash()),
            "0ab882d24f8c85b880ca7cfb6f60a7773f377222e84ac28afd14f3ce46bf307a"
        );
        let short = TokenJson {
            category: "11".into(),
            ..token
        };
        assert!(short.leaf().is_err());
    }

    /// Pickaxe's split leaf: token A with a 1% donation and a 1% fee,
    /// given as a split rather than an `ext`.
    #[test]
    fn token_json_split_gives_the_golden_split_leaf() {
        let p2pkh = |byte: &str| format!("76a914{}88ac", byte.repeat(20));
        let mut fields = json!({
            "category": "11".repeat(32),
            "lock_hash": "953ccfa596a6c6d39e5980194539124fdcff116a571455a212baed811f585ee0",
            "state_hash": "aa".repeat(32),
            "payout_hash": "22650758a42ebfd7306e771ad24c442cad795ed9e549392233729f647a2b1be5",
            "target_bits": "207fffff",
            "split": {
                "donation_bps": 100,
                "donation_script": p2pkh("d0"),
                "fee_bps": 100,
                "fee_script": p2pkh("fe"),
            },
        });
        let token: TokenJson = serde_json::from_value(fields.clone()).unwrap();
        assert_eq!(
            hex::encode(token.leaf().unwrap().hash()),
            "c40271368b4287df8fe3430e1bc3f9410b554874de698144fa04fe5df7033e41"
        );
        fields["ext"] = json!("00".repeat(32));
        let both: TokenJson = serde_json::from_value(fields).unwrap();
        assert!(both.leaf().is_err());
    }

    #[test]
    fn bounded_read_accepts_the_limit_and_rejects_one_more_byte() {
        let exact = vec![b' '; MAX_INPUT_BYTES as usize];
        assert_eq!(
            read_bounded(Cursor::new(&exact)).unwrap().len(),
            exact.len()
        );
        let excessive = vec![b' '; MAX_INPUT_BYTES as usize + 1];
        assert!(read_bounded(Cursor::new(excessive)).is_err());
    }
}
