// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::shares::bch_headers::MinDifficultyMark;
use crate::shares::coinbase_proof::MAX_COINBASE_MERKLE_BRANCH_LENGTH;
use crate::stratum::sv2::ProvidedTemplate;
use crate::stratum::work::error::WorkError;
use bitcoin::block::Header;
use bitcoin::consensus::encode::VarInt;
use bitcoin::hashes::Hash;
use bitcoin::script::PushBytesBuf;
use bitcoin::{BlockHash, CompactTarget, Target, Transaction, TxMerkleNode, Txid};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The largest encoding of a block's transaction count, a 9-byte
/// CompactSize. A light template does not say how many transactions the
/// node keeps for it.
const MAX_TRANSACTION_COUNT_SIZE: u64 = 9;

/// A light template's job (`template_source = "rpc_light"`): the node kept
/// the template's transactions and gave their merkle branch instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightJob {
    /// The node's `job_id`, which `submitblocklight` names the job by.
    pub job_id: String,
    /// The coinbase's merkle branch over the node's transactions, the
    /// template's `merkle`, in the order and byte order of
    /// `build_merkle_branches_for_template` on the same transactions.
    pub merkle_branch: Vec<TxMerkleNode>,
    /// `getmininginfo.miningblocksizelimit`, the most bytes the node fills a
    /// block it builds with. The node is assumed to keep the template's
    /// transactions within it; no other bound on their size is known.
    pub mining_block_size_limit: u64,
}

/// Transaction data in the block template
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TemplateTransaction {
    pub data: String,
    pub txid: String,
    #[serde(default)]
    pub hash: String,
    /// Indexes of earlier template transactions this one spends. BCHN omits
    /// the field: its templates are in canonical order, not dependency order.
    #[serde(default)]
    pub depends: Vec<u32>,
    pub fee: u64,
    pub sigops: u32,
    #[serde(default)]
    pub weight: u32,
}

/// Template fields shared by Bitcoin Core and BCHN. BCH has no block weight.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct BlockTemplate<T = TemplateTransaction> {
    pub version: i32,
    #[serde(default)]
    pub rules: Vec<String>,
    #[serde(default)]
    pub vbavailable: std::collections::HashMap<String, i32>,
    #[serde(default)]
    pub vbrequired: u32,
    pub previousblockhash: String,
    pub transactions: Vec<T>,
    pub coinbaseaux: HashMap<String, String>,
    pub coinbasevalue: u64,
    /// BIP 22's long polling id. Only a node that serves long polling sends
    /// it (Bitcoin Core and BCHN do, Knuth does not), and the pool never
    /// long-polls, so a template without it reads as an empty id.
    #[serde(default)]
    pub longpollid: String,
    pub target: String,
    pub mintime: u32,
    pub mutable: Vec<String>,
    pub noncerange: String,
    /// The block's SigChecks limit, which BCHN reports as `sigoplimit`
    /// (`sizelimit / 141`). Bitcoin Core reports its sigop cost limit here.
    pub sigoplimit: u64,
    /// The size limit of the block the template builds, in bytes. On Bitcoin
    /// Cash it is BCHN's next-block limit: a 32 MB floor that ABLA raises with
    /// no fixed maximum, so it is held in 64 bits.
    pub sizelimit: u64,
    #[serde(default)]
    pub weightlimit: u32,
    pub curtime: u32,
    pub bits: String,
    pub height: u32,
    #[serde(
        rename = "default_witness_commitment",
        skip_serializing_if = "Option::is_none"
    )]
    pub default_witness_commitment: Option<String>,
    /// Set when a Stratum V2 Template Provider sent this template: a block
    /// found on it goes back to that provider with `SubmitSolution`. Never
    /// part of a node's JSON template.
    #[serde(skip)]
    pub provided: Option<ProvidedTemplate>,
    /// Set when the node sent a light template: `transactions` is then empty
    /// and a block found on it goes back with `submitblocklight`. Never part
    /// of a node's JSON template.
    #[serde(skip)]
    pub light: Option<LightJob>,
    /// Set by P2Pool on a test network whose minimum-difficulty rule ties
    /// `bits` to the block's time, from the template's known previous block
    /// ([`Self::bound_by_min_difficulty_mark`]). Never part of a node's JSON
    /// template; `None` on mainnet.
    #[serde(skip)]
    pub min_difficulty_mark: Option<MinDifficultyMark>,
}

impl TryFrom<&TemplateTransaction> for Transaction {
    type Error = WorkError;

    fn try_from(transaction: &TemplateTransaction) -> Result<Self, Self::Error> {
        let bytes = hex::decode(&transaction.data).map_err(|error| WorkError {
            message: format!("Invalid template transaction hex: {error}"),
        })?;
        bitcoin::consensus::deserialize(&bytes).map_err(|error| WorkError {
            message: format!("Invalid template transaction: {error}"),
        })
    }
}

impl TryFrom<TemplateTransaction> for Transaction {
    type Error = WorkError;

    fn try_from(transaction: TemplateTransaction) -> Result<Self, Self::Error> {
        Transaction::try_from(&transaction)
    }
}

impl BlockTemplate {
    /// Decode template transactions, returning malformed node data as an error.
    pub fn decode_transactions(&self) -> Result<Vec<Transaction>, WorkError> {
        self.transactions
            .iter()
            .map(Transaction::try_from)
            .collect()
    }

    /// Bound this template's block times by `mark`: a block on it may then
    /// carry only the times its `bits` allow ([`Self::allows_ntime`]), and
    /// `curtime`, the time its jobs carry, moves to the nearest of them,
    /// never below `mintime`.
    pub fn bound_by_min_difficulty_mark(&mut self, mark: MinDifficultyMark) {
        self.curtime = mark.nearest_allowed(self.curtime).max(self.mintime);
        self.min_difficulty_mark = Some(mark);
    }

    /// Whether a block on this template may be dated `ntime` with its
    /// `bits`: always, unless a test network's minimum-difficulty rule bounds
    /// them ([`Self::bound_by_min_difficulty_mark`]).
    pub fn allows_ntime(&self, ntime: u32) -> bool {
        self.min_difficulty_mark
            .is_none_or(|mark| mark.allows(ntime))
    }

    /// Check BCH template data before computing merkle branches or issuing jobs.
    /// CTOR is checked in displayed transaction ID order; the coinbase is separate.
    /// A light template lists no transactions, and its merkle branch is no
    /// longer than a share's coinbase proof may be.
    pub fn validate_bch(&self) -> Result<(), WorkError> {
        let invalid = |message: &str| WorkError {
            message: format!("Invalid BCH template: {message}"),
        };
        self.previousblockhash
            .parse::<BlockHash>()
            .map_err(|_| invalid("invalid previous block hash"))?;
        let bits = u32::from_str_radix(&self.bits, 16)
            .map(CompactTarget::from_consensus)
            .map_err(|_| invalid("invalid compact target"))?;
        let target = Target::from_compact(bits);
        let target_bytes: [u8; 32] = hex::decode(&self.target)
            .map_err(|_| invalid("invalid target hex"))?
            .try_into()
            .map_err(|_| invalid("target must be 32 bytes"))?;
        if self.bits.len() != 8
            || target == Target::ZERO
            || bits.to_consensus() & 0x00800000 != 0
            || target.to_compact_lossy() != bits
            || target.to_be_bytes() != target_bytes
        {
            return Err(invalid("target and compact bits disagree or are invalid"));
        }
        if self.curtime < self.mintime || self.sizelimit < 81 || self.sigoplimit == 0 {
            return Err(invalid("invalid timestamp or block limits"));
        }
        if self.coinbasevalue > bitcoin::Amount::MAX_MONEY.to_sat() {
            return Err(invalid("coinbase value exceeds maximum money"));
        }
        if self.default_witness_commitment.is_some()
            || self.weightlimit != 0
            || self.rules.iter().any(|rule| {
                matches!(
                    rule.trim_start_matches('!'),
                    "segwit" | "signet" | "taproot"
                )
            })
        {
            return Err(invalid("Bitcoin-only consensus fields"));
        }
        if let Some(light) = &self.light {
            if !self.transactions.is_empty() {
                return Err(invalid("light template lists transactions"));
            }
            if light.merkle_branch.len() > MAX_COINBASE_MERKLE_BRANCH_LENGTH {
                return Err(invalid("light template merkle branch is too long"));
            }
        }
        let mut previous_txid: Option<Txid> = None;
        let mut transaction_bytes = 0usize;
        for template_transaction in &self.transactions {
            let transaction = Transaction::try_from(template_transaction)?;
            let txid = transaction.compute_txid();
            if template_transaction.txid.parse::<Txid>().ok() != Some(txid)
                || (!template_transaction.hash.is_empty()
                    && template_transaction.hash.parse::<Txid>().ok() != Some(txid))
            {
                return Err(invalid("transaction ID does not match its bytes"));
            }
            if transaction.is_coinbase()
                || transaction
                    .input
                    .iter()
                    .any(|input| !input.witness.is_empty())
            {
                return Err(invalid(
                    "coinbase or witness transaction in transaction list",
                ));
            }
            if previous_txid.is_some_and(|previous| {
                previous
                    .as_byte_array()
                    .iter()
                    .rev()
                    .cmp(txid.as_byte_array().iter().rev())
                    != std::cmp::Ordering::Less
            }) {
                return Err(invalid("transactions are not in strict CTOR order"));
            }
            transaction_bytes = transaction_bytes
                .checked_add(bitcoin::consensus::serialize(&transaction).len())
                .ok_or_else(|| invalid("transaction size overflow"))?;
            previous_txid = Some(txid);
        }
        if (transaction_bytes as u64).saturating_add(81) > self.sizelimit {
            return Err(invalid("transactions exceed node block size limit"));
        }
        Ok(())
    }

    /// Check the final size of an already validated BCH template with its coinbase.
    /// A light template is checked with the node's mining size limit in place
    /// of its transactions' bytes ([`LightJob::validate_block_size`]).
    pub(crate) fn validate_bch_block_size(&self, coinbase: &Transaction) -> Result<(), WorkError> {
        if let Some(light) = &self.light {
            return light.validate_block_size(self.sizelimit, coinbase);
        }
        let prefix_size = Header::SIZE
            + VarInt(self.transactions.len() as u64 + 1).size()
            + coinbase.total_size();
        let block_size = self
            .transactions
            .iter()
            .try_fold(prefix_size, |size, transaction| {
                size.checked_add(transaction.data.len() / 2)
            });
        if block_size.is_none_or(|size| size as u64 > self.sizelimit) {
            let size = block_size.map_or_else(
                || "more than usize::MAX".to_string(),
                |size| size.to_string(),
            );
            return Err(WorkError {
                message: format!(
                    "BCH block of {size} bytes including its {}-byte coinbase exceeds BCHN's block \
                     size limit of {} bytes. Lower BCHN's -blockmaxsize or \
                     -percentblockmaxsize so its templates leave room for the pool's coinbase",
                    coinbase.total_size(),
                    self.sizelimit
                ),
            });
        }
        Ok(())
    }
}

impl LightJob {
    /// Check that a block on this job fits `size_limit` with `coinbase`.
    ///
    /// Its transactions' bytes are not known, only that the node keeps them
    /// within `mining_block_size_limit`. The block fits whatever they are
    /// when the header, the largest transaction count, the coinbase and that
    /// limit together do.
    fn validate_block_size(
        &self,
        size_limit: u64,
        coinbase: &Transaction,
    ) -> Result<(), WorkError> {
        let block_size_bound =
            (Header::SIZE as u64 + MAX_TRANSACTION_COUNT_SIZE + coinbase.total_size() as u64)
                .checked_add(self.mining_block_size_limit);
        if block_size_bound.is_none_or(|bound| bound > size_limit) {
            let bound = block_size_bound.map_or_else(
                || "more than u64::MAX".to_string(),
                |bound| bound.to_string(),
            );
            return Err(WorkError {
                message: format!(
                    "BCH block of up to {bound} bytes, with its {}-byte coinbase and the node's \
                     mining size limit of {} bytes for the light template's transactions, \
                     exceeds the block size limit of {size_limit} bytes. Lower the node's \
                     -blockmaxsize (or equivalent) so its light templates leave room for the \
                     pool's coinbase",
                    coinbase.total_size(),
                    self.mining_block_size_limit
                ),
            });
        }
        Ok(())
    }
}

/// Extract flags from template coinbaseaux and convert to PushBytesBuf.
/// If flags are empty or absent, use a single byte with value 0.
pub(crate) fn parse_flags(flags: Option<String>) -> Result<PushBytesBuf, WorkError> {
    match flags {
        Some(flags) if flags.is_empty() => Ok(PushBytesBuf::from(&[0u8])),
        Some(flags) => {
            let decoded = hex::decode(&flags).map_err(|error| WorkError {
                message: format!("Invalid hex in coinbaseaux flags '{flags}': {error}"),
            })?;
            PushBytesBuf::try_from(decoded).map_err(|error| WorkError {
                message: format!("Coinbaseaux flags too large: {error}"),
            })
        }
        None => Ok(PushBytesBuf::from(&[0u8])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{bch_template_transaction, test_coinbase_transaction};
    use bitcoin::consensus::serialize;

    const BCHN_TEMPLATE: &str =
        include_str!("../../../../bitcoindrpc/test_data/bchn-regtest-template.json");

    #[test]
    fn bchn_template_without_bitcoin_fields_is_valid() {
        let template: BlockTemplate = serde_json::from_str(BCHN_TEMPLATE).unwrap();
        template.validate_bch().unwrap();
        assert_eq!(template.sizelimit, 32_000_000);
        assert_eq!(template.weightlimit, 0);
    }

    /// A node's template carries no mark, so it allows any time, as on
    /// mainnet.
    #[test]
    fn a_template_without_a_mark_allows_any_time() {
        let template: BlockTemplate = serde_json::from_str(BCHN_TEMPLATE).unwrap();
        assert_eq!(template.min_difficulty_mark, None);
        assert!(template.allows_ntime(0));
        assert!(template.allows_ntime(u32::MAX));
    }

    /// A template with ASERT's bits whose time is past its mark is moved
    /// back to the mark, which it then allows, and not a second later.
    #[test]
    fn a_template_with_asert_bits_is_moved_back_to_its_mark() {
        let mut template: BlockTemplate = serde_json::from_str(BCHN_TEMPLATE).unwrap();
        let mark = template.curtime - 5;
        template.bound_by_min_difficulty_mark(MinDifficultyMark::Until(mark));
        assert_eq!(template.curtime, mark);
        assert!(template.allows_ntime(mark));
        assert!(!template.allows_ntime(mark + 1));
        template.validate_bch().unwrap();
    }

    /// A template with the limit's bits whose time is not past its mark is
    /// moved to the second after it, which it then allows, and not the mark.
    #[test]
    fn a_template_with_the_limits_bits_is_moved_past_its_mark() {
        let mut template: BlockTemplate = serde_json::from_str(BCHN_TEMPLATE).unwrap();
        let mark = template.curtime;
        template.bound_by_min_difficulty_mark(MinDifficultyMark::After(mark));
        assert_eq!(template.curtime, mark + 1);
        assert!(!template.allows_ntime(mark));
        assert!(template.allows_ntime(mark + 1));
    }

    /// A mark below `mintime` cannot move `curtime` below it: no time is
    /// then allowed, and every share on the template is refused.
    #[test]
    fn a_mark_never_moves_curtime_below_mintime() {
        let mut template: BlockTemplate = serde_json::from_str(BCHN_TEMPLATE).unwrap();
        let mark = template.mintime - 1;
        template.bound_by_min_difficulty_mark(MinDifficultyMark::Until(mark));
        assert_eq!(template.curtime, template.mintime);
        assert!(!template.allows_ntime(template.curtime));
    }

    /// ABLA has no fixed maximum, so a limit past 32 bits must parse and hold.
    #[test]
    fn bchn_size_limit_above_four_gigabytes_is_kept() {
        let mut json: serde_json::Value = serde_json::from_str(BCHN_TEMPLATE).unwrap();
        let size_limit: u64 = 8_000_000_000;
        json["sizelimit"] = size_limit.into();
        json["sigoplimit"] = (size_limit / 141).into();
        let template: BlockTemplate = serde_json::from_value(json).unwrap();
        template.validate_bch().unwrap();
        assert_eq!(template.sizelimit, size_limit);
        assert_eq!(template.sigoplimit, size_limit / 141);
        template
            .validate_bch_block_size(&test_coinbase_transaction(1))
            .unwrap();
    }

    #[test]
    fn bch_rejects_mismatched_target() {
        let mut template: BlockTemplate = serde_json::from_str(BCHN_TEMPLATE).unwrap();
        template.target = "00".repeat(32);
        assert!(
            template
                .validate_bch()
                .unwrap_err()
                .message
                .contains("target")
        );
    }

    #[test]
    fn bch_rejects_coinbase_value_above_maximum_money() {
        let mut template: BlockTemplate = serde_json::from_str(BCHN_TEMPLATE).unwrap();
        template.coinbasevalue = bitcoin::Amount::MAX_MONEY.to_sat() + 1;
        assert!(
            template
                .validate_bch()
                .unwrap_err()
                .message
                .contains("maximum money")
        );
    }

    #[test]
    fn bch_rejects_witness_commitment() {
        let mut template: BlockTemplate = serde_json::from_str(BCHN_TEMPLATE).unwrap();
        template.default_witness_commitment = Some("6a24aa21a9ed".into());
        assert!(
            template
                .validate_bch()
                .unwrap_err()
                .message
                .contains("Bitcoin-only")
        );
    }

    #[test]
    fn bch_rejects_transaction_id_mismatch() {
        let mut template: BlockTemplate = serde_json::from_str(BCHN_TEMPLATE).unwrap();
        let mut transaction = bch_template_transaction(1);
        transaction.txid = "00".repeat(32);
        template.transactions.push(transaction);
        assert!(
            template
                .validate_bch()
                .unwrap_err()
                .message
                .contains("transaction ID")
        );
    }

    #[test]
    fn bch_rejects_witness_transactions() {
        let mut template: BlockTemplate = serde_json::from_str(BCHN_TEMPLATE).unwrap();
        let mut transaction = bch_template_transaction(1);
        let mut decoded = Transaction::try_from(&transaction).unwrap();
        decoded.input[0].witness.push([1u8]);
        transaction.data = bitcoin::consensus::encode::serialize_hex(&decoded);
        template.transactions.push(transaction);
        assert!(
            template
                .validate_bch()
                .unwrap_err()
                .message
                .contains("witness transaction")
        );
    }

    #[test]
    fn bch_rejects_duplicate_transaction_ids() {
        let mut template: BlockTemplate = serde_json::from_str(BCHN_TEMPLATE).unwrap();
        let transaction = bch_template_transaction(1);
        template.transactions = vec![transaction.clone(), transaction];
        assert!(
            template
                .validate_bch()
                .unwrap_err()
                .message
                .contains("CTOR")
        );
    }

    #[test]
    fn bch_uses_display_order_for_ctor_and_rejects_reversed_order() {
        let mut template: BlockTemplate = serde_json::from_str(BCHN_TEMPLATE).unwrap();
        template.transactions = vec![bch_template_transaction(1), bch_template_transaction(2)];
        template
            .transactions
            .sort_by(|left, right| left.txid.cmp(&right.txid));
        template.validate_bch().unwrap();
        template.transactions.reverse();
        assert!(
            template
                .validate_bch()
                .unwrap_err()
                .message
                .contains("CTOR")
        );
    }

    #[test]
    fn bch_rejects_transactions_over_node_size_limit() {
        let mut template: BlockTemplate = serde_json::from_str(BCHN_TEMPLATE).unwrap();
        template.transactions.push(bch_template_transaction(1));
        template.sizelimit = 81;
        assert!(
            template
                .validate_bch()
                .unwrap_err()
                .message
                .contains("block size limit")
        );
    }

    #[test]
    fn bch_block_size_accepts_the_exact_limit_and_rejects_one_byte_less() {
        let mut template: BlockTemplate = serde_json::from_str(BCHN_TEMPLATE).unwrap();
        template.transactions = vec![bch_template_transaction(1)];
        let coinbase = test_coinbase_transaction(1);
        let transactions = vec![
            coinbase.clone(),
            Transaction::try_from(&template.transactions[0]).unwrap(),
        ];
        template.sizelimit = (Header::SIZE + serialize(&transactions).len()) as u64;
        template.validate_bch().unwrap();
        template.validate_bch_block_size(&coinbase).unwrap();
        template.sizelimit -= 1;
        assert!(template.validate_bch_block_size(&coinbase).is_err());
    }

    #[test]
    fn bch_block_size_counts_the_compact_size_transaction_prefix() {
        let mut template: BlockTemplate = serde_json::from_str(BCHN_TEMPLATE).unwrap();
        let transaction = bch_template_transaction(1);
        let decoded = Transaction::try_from(&transaction).unwrap();
        template.transactions = vec![transaction; 252];
        let coinbase = test_coinbase_transaction(1);
        let mut transactions = vec![decoded; 253];
        transactions[0] = coinbase.clone();
        template.sizelimit = (Header::SIZE + serialize(&transactions).len()) as u64;
        template.validate_bch_block_size(&coinbase).unwrap();
        template.sizelimit -= 1;
        assert!(template.validate_bch_block_size(&coinbase).is_err());
    }

    /// A light block fits when the header, a 9-byte transaction count, the
    /// coinbase and the node's mining size limit together do.
    #[test]
    fn bch_light_block_size_accepts_the_bound_and_rejects_one_byte_less() {
        let mut template: BlockTemplate = serde_json::from_str(BCHN_TEMPLATE).unwrap();
        template.light = Some(LightJob {
            job_id: "ec631dfd0a785808f1341c9c6596d8bd25c3e13a".to_string(),
            merkle_branch: vec![TxMerkleNode::all_zeros()],
            mining_block_size_limit: 16_000_000,
        });
        let coinbase = test_coinbase_transaction(1);
        template.sizelimit = (Header::SIZE + 9 + coinbase.total_size()) as u64 + 16_000_000;
        template.validate_bch().unwrap();
        template.validate_bch_block_size(&coinbase).unwrap();
        template.sizelimit -= 1;
        let error = template.validate_bch_block_size(&coinbase).unwrap_err();
        assert!(error.message.contains("-blockmaxsize"), "{}", error.message);
    }

    #[test]
    fn bch_rejects_a_light_merkle_branch_longer_than_a_coinbase_proof() {
        let mut template: BlockTemplate = serde_json::from_str(BCHN_TEMPLATE).unwrap();
        template.light = Some(LightJob {
            job_id: "ec631dfd0a785808f1341c9c6596d8bd25c3e13a".to_string(),
            merkle_branch: vec![TxMerkleNode::all_zeros(); MAX_COINBASE_MERKLE_BRANCH_LENGTH + 1],
            mining_block_size_limit: 16_000_000,
        });
        assert!(
            template
                .validate_bch()
                .unwrap_err()
                .message
                .contains("merkle branch")
        );
    }

    #[test]
    fn malformed_template_hex_returns_an_error() {
        let mut transaction = bch_template_transaction(1);
        transaction.data = "invalid".into();
        assert!(Transaction::try_from(transaction).is_err());
    }

    #[test]
    fn malformed_template_transaction_returns_an_error() {
        let mut transaction = bch_template_transaction(1);
        transaction.data = "00".into();
        assert!(Transaction::try_from(transaction).is_err());
    }

    #[test]
    fn test_parse_flags() {
        let flags = parse_flags(Some(String::from(""))).unwrap();
        assert_eq!(flags.as_bytes(), &[0u8]);

        let flags = parse_flags(None).unwrap();
        assert_eq!(flags.as_bytes(), &[0u8]);

        let flags = parse_flags(Some(String::from("deadbeef"))).unwrap();
        assert_eq!(flags.as_bytes(), &[0xde, 0xad, 0xbe, 0xef]);
    }

    #[test]
    fn test_parse_flags_invalid_hex_returns_error() {
        let result = parse_flags(Some(String::from("not_valid_hex")));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .message
                .contains("Invalid hex in coinbaseaux flags")
        );
    }

    #[test]
    fn test_template_transaction_conversion() {
        let tx_data = TemplateTransaction {
            data: "02000000000101d6c83b002c07d56399a5e0c887ddc7b74071b301a3ffa630c15754c63d8bee750000000000fdffffff02ac78f62901000000160014aecdd0cfae0829ee25e172cc8a94b2aa702869a040420f0000000000160014230a8d012b7cfce1a3118c617d6c50ce9f7482d602473044022037aede936712b0e32aaeba0413833f66929b1bff3726414294b1b140cc93595402204aa43cce61d2c8a9e8131aa335d884212bc2d63f74fea0c13c8ade3640f4b291012103555f1c1815b0a5a5ce7eeac3b7da8923e2c440ed94fc40e9d2685ed45f5335b5bb030000".to_string(),
            txid: "74d7b9bf9f51dd7447e117b6a835a20b6f7d5d807285d1435da37574563ca525".to_string(),
            hash: "000002d6ed0236ae93ae9affa9d02f7a4ebf21430a202a86a7ef4ae4f95bad00".to_string(),
            depends: vec![],
            fee: 5000,
            sigops: 4,
            weight: 892,
        };

        let bitcoin_tx = Transaction::try_from(tx_data).unwrap();
        assert_eq!(
            bitcoin_tx.compute_txid().to_string(),
            "74d7b9bf9f51dd7447e117b6a835a20b6f7d5d807285d1435da37574563ca525"
        );
    }

    #[test]
    fn test_load_transactions_from_json() {
        // Load the test JSON file
        let json_content = include_str!(
            "../../../../p2poolv2_tests/test_data/validation/stratum/gbt_with_transactions.json"
        );

        // Parse the JSON into BlockTemplate
        let block_template: BlockTemplate =
            serde_json::from_str(json_content).expect("Failed to parse JSON into BlockTemplate");

        // Verify we have transactions
        assert!(!block_template.transactions.is_empty());

        // Take the first transaction for detailed testing
        let first_tx = &block_template.transactions[0];

        // Convert to Bitcoin Transaction
        let btc_tx = Transaction::try_from(first_tx).unwrap();

        // Verify transaction properties
        assert_eq!(btc_tx.compute_txid().to_string(), first_tx.txid);

        assert_eq!(btc_tx.input.len(), 1);
        assert_eq!(btc_tx.output.len(), 2);
        assert_eq!(btc_tx.input[0].previous_output.vout, 0);
        assert_eq!(
            btc_tx.input[0].previous_output.txid.to_string(),
            "75ee8b3dc65457c130a6ffa301b37140b7c7dd87c8e0a59963d5072c003bc8d6"
        );
        assert_eq!(
            btc_tx.output[0].value,
            bitcoin::Amount::from_btc(49.98985900).unwrap()
        );
    }
}
