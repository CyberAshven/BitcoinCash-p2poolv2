// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Bytes on the wire: coinbase layouts, share sizes and the limits they meet.
//!
//! The share and message sizes are counted from the encoders in
//! `p2poolv2_lib` and from the hybrid payout proposal
//! (`docs/architecture/bch-hybrid-payouts.md`), not measured from a running
//! node. Every payee is taken to be paid to P2PKH. The framing overhead is an
//! estimate.

use crate::constants::{
    ASIC_COINBASE_EVIDENCE_BYTES, BCH_MAX_COINBASE_PAYOUTS, BCH_MAX_TRANSACTION_BYTES,
    BCH_VM_STACK_ELEMENT_BYTES, COINBASE_FIXED_BYTES, COINBASE_FIXED_OUTPUTS,
    COINBASE_PREFIX_BYTES, HYBRID_COINBASE_BYTES_BESIDE_PAYOUTS, HYBRID_COINBASE_FIXED_BYTES,
    MAX_P2P_MESSAGE_BYTES, MAX_UNCLES, NO_TOKEN_COINBASE_LIMIT_BYTES, P2PKH_OUTPUT_BYTES,
    P2SH32_OUTPUT_BYTES, PROPOSED_SHARE_BODY_LIMIT_BYTES, SHARE_BODY_LIMIT_BYTES,
    SHARE_COINBASE_BYTES, SHARE_HEADER_BASE_BYTES, SHARE_HEADER_BYTES_PER_UNCLE,
    SV2_COINBASE_FIELD_BYTES, TOKEN_COINBASE_THRESHOLD_BYTES,
};
use serde::Serialize;

/// Bytes of a 32-byte hash.
const HASH_BYTES: u64 = 32;

/// Framing each P2P message carries beyond its payload, estimated: P2Pool's
/// `RawMessage` adds a length, a checksum and the message tag (9 bytes);
/// libp2p's request-response stream, yamux (12-byte frame header) and Noise
/// (2-byte length and 16-byte tag per frame) add the rest, and every request
/// opens a stream. Not measured.
pub const MESSAGE_FRAMING_BYTES: u64 = 64;

/// IPv4 and TCP headers (40 bytes) per full TCP segment of 1,460 bytes.
const PACKET_HEADER_BYTES: u64 = 40;
const SEGMENT_PAYLOAD_BYTES: u64 = 1_460;

/// How a found block's coinbase pays the PPLNS window's payees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum CoinbaseLayout {
    /// The hybrid payout proposal (`bch-hybrid-payouts.md`, section 3, after
    /// the owner's decision of 2026-10-23; proposed, not built). Payees are
    /// paid directly while their outputs fit the payout budget
    /// `limit_bytes - S`; the payee that straddles it and every later one
    /// are paid through one P2SH32 payout tree output. The limit is BCH's
    /// 1,000,000-byte `MAX_TX_SIZE` for a share without token commitments
    /// (the normal case) and `T` = 3,072 bytes for one with them.
    Hybrid {
        /// Whether the share carries merge-mined token commitments.
        tokens: bool,
        /// The coinbase limit `C`.
        limit_bytes: u64,
    },
    /// Every payee paid directly, with no limit at all: upstream's Bitcoin
    /// rule, for comparison. Past about 29,400 payees BCH rejects the block.
    Uncapped,
    /// At most `max_payouts` payout outputs, the long tail paid by one
    /// draw weighted by PPLNS weight: alpha's current share-chain rule
    /// (decision 14, `payout-cap11`), which the owner rejected, kept for
    /// comparison.
    CappedDraw {
        /// Most payout outputs.
        max_payouts: u64,
    },
}

impl CoinbaseLayout {
    /// The hybrid payout proposal for a share with or without merge-mined
    /// tokens; `token_threshold_bytes` is `T`.
    pub fn hybrid(tokens_merge_mined: bool, token_threshold_bytes: u64) -> Self {
        CoinbaseLayout::Hybrid {
            tokens: tokens_merge_mined,
            limit_bytes: if tokens_merge_mined {
                token_threshold_bytes
            } else {
                NO_TOKEN_COINBASE_LIMIT_BYTES
            },
        }
    }

    /// The hybrid proposal for a share without tokens.
    pub fn no_tokens() -> Self {
        CoinbaseLayout::hybrid(false, TOKEN_COINBASE_THRESHOLD_BYTES)
    }

    /// The hybrid proposal for a share with tokens, at the recommended `T`.
    pub fn with_tokens() -> Self {
        CoinbaseLayout::hybrid(true, TOKEN_COINBASE_THRESHOLD_BYTES)
    }

    /// Alpha's current rule.
    pub fn alpha_cap() -> Self {
        CoinbaseLayout::CappedDraw {
            max_payouts: BCH_MAX_COINBASE_PAYOUTS,
        }
    }

    /// Short name used in tables.
    pub fn name(&self) -> String {
        match self {
            CoinbaseLayout::Hybrid {
                tokens: false,
                limit_bytes,
            } if *limit_bytes == NO_TOKEN_COINBASE_LIMIT_BYTES => "no-tokens".to_string(),
            CoinbaseLayout::Hybrid {
                tokens: false,
                limit_bytes,
            } => format!("no-tokens-{limit_bytes}"),
            CoinbaseLayout::Hybrid {
                tokens: true,
                limit_bytes,
            } => format!("tokens-{limit_bytes}"),
            CoinbaseLayout::Uncapped => "uncapped".to_string(),
            CoinbaseLayout::CappedDraw { max_payouts } => format!("cap-{max_payouts}"),
        }
    }

    /// Whether a share under this layout carries a merge-mining token claim
    /// that has to hash its coinbase in the VM.
    pub fn carries_token_claim(&self) -> bool {
        matches!(self, CoinbaseLayout::Hybrid { tokens: true, .. })
    }
}

/// Bytes of a Bitcoin compact size integer.
pub fn compact_size_bytes(value: u64) -> u64 {
    if value < 0xfd {
        1
    } else if value <= 0xffff {
        3
    } else if value <= 0xffff_ffff {
        5
    } else {
        9
    }
}

/// How a coinbase pays its payees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PayoutSplit {
    /// Payees with their own P2PKH output.
    pub direct_outputs: u64,
    /// Payees paid through the payout tree.
    pub tree_payees: u64,
    /// Payees paid only through the weighted tail draw (alpha's rule).
    pub drawn_payees: u64,
    /// Bytes of the coinbase.
    pub coinbase_bytes: u64,
    /// Bytes of the coinbase that depend on the payees: the direct payout
    /// outputs and the tree output. A peer that rebuilds the payouts itself
    /// does not need them sent.
    pub payout_bytes: u64,
}

/// Bytes of a proposal coinbase with `direct_outputs` P2PKH payouts and,
/// when `tree` is set, one P2SH32 payout tree output, at the largest size of
/// everything else.
pub fn hybrid_coinbase_bytes(direct_outputs: u64, tree: bool) -> u64 {
    let tree_outputs = u64::from(tree);
    let outputs = direct_outputs + tree_outputs + COINBASE_FIXED_OUTPUTS;
    // HYBRID_COINBASE_FIXED_BYTES counts a one-byte output count.
    let extra_count_bytes = compact_size_bytes(outputs) - 1;
    HYBRID_COINBASE_FIXED_BYTES
        + P2PKH_OUTPUT_BYTES * direct_outputs
        + P2SH32_OUTPUT_BYTES * tree_outputs
        + extra_count_bytes
}

/// Bytes of an alpha coinbase with `payouts` P2PKH payouts.
pub fn alpha_coinbase_bytes(payouts: u64) -> u64 {
    let extra_count_bytes = compact_size_bytes(payouts + COINBASE_FIXED_OUTPUTS) - 1;
    COINBASE_FIXED_BYTES + P2PKH_OUTPUT_BYTES * payouts + extra_count_bytes
}

/// The proposal's straddle rule (`bch-hybrid-payouts.md`, section 3.4) for
/// `payees` P2PKH payees under a coinbase limit of `limit_bytes`: with the
/// payout budget `P = limit_bytes - S`, everyone is paid directly if their
/// outputs fit `P` plus the unused tree output; otherwise payees are paid
/// directly while their outputs fit `P`, and the straddler and the rest go to
/// the tree.
fn hybrid_split(payees: u64, limit_bytes: u64) -> PayoutSplit {
    let budget = limit_bytes.saturating_sub(HYBRID_COINBASE_BYTES_BESIDE_PAYOUTS);
    let everyone_fits = payees.saturating_mul(P2PKH_OUTPUT_BYTES) <= budget + P2SH32_OUTPUT_BYTES;
    if everyone_fits {
        PayoutSplit {
            direct_outputs: payees,
            tree_payees: 0,
            drawn_payees: 0,
            coinbase_bytes: hybrid_coinbase_bytes(payees, false),
            payout_bytes: P2PKH_OUTPUT_BYTES * payees,
        }
    } else {
        let direct = (budget / P2PKH_OUTPUT_BYTES).min(payees);
        PayoutSplit {
            direct_outputs: direct,
            tree_payees: payees - direct,
            drawn_payees: 0,
            coinbase_bytes: hybrid_coinbase_bytes(direct, true),
            payout_bytes: P2PKH_OUTPUT_BYTES * direct + P2SH32_OUTPUT_BYTES,
        }
    }
}

/// Split `payees` window payees under `layout`.
pub fn split_payees(layout: CoinbaseLayout, payees: u64) -> PayoutSplit {
    match layout {
        CoinbaseLayout::Hybrid { limit_bytes, .. } => hybrid_split(payees, limit_bytes),
        CoinbaseLayout::Uncapped => PayoutSplit {
            direct_outputs: payees,
            tree_payees: 0,
            drawn_payees: 0,
            coinbase_bytes: hybrid_coinbase_bytes(payees, false),
            payout_bytes: P2PKH_OUTPUT_BYTES * payees,
        },
        CoinbaseLayout::CappedDraw { max_payouts } => {
            let outputs = payees.min(max_payouts);
            // Past the cap the paid set keeps one slot for the drawn payee.
            let drawn = if payees > max_payouts {
                payees - max_payouts.saturating_sub(1)
            } else {
                0
            };
            PayoutSplit {
                direct_outputs: outputs,
                tree_payees: 0,
                drawn_payees: drawn,
                coinbase_bytes: alpha_coinbase_bytes(outputs),
                payout_bytes: P2PKH_OUTPUT_BYTES * outputs,
            }
        }
    }
}

/// Transactions in a block of `block_bytes`, the coinbase included, at
/// `average_transaction_bytes` each. At least one.
pub fn block_transaction_count(block_bytes: u64, average_transaction_bytes: u64) -> u64 {
    (block_bytes / average_transaction_bytes.max(1)).max(1)
}

/// Hashes in the coinbase's merkle branch for a block of
/// `transaction_count` transactions: the merkle tree's depth.
pub fn merkle_branch_hashes(transaction_count: u64) -> u64 {
    if transaction_count <= 1 {
        0
    } else {
        u64::from(u64::BITS - (transaction_count - 1).leading_zeros())
    }
}

/// Sizes of one share for a scenario.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ShareSizes {
    /// How the coinbase pays its payees.
    pub split: PayoutSplit,
    /// Hashes in the coinbase merkle branch.
    pub merkle_branch_hashes: u64,
    /// Bytes of the share header without uncles.
    pub header_bytes: u64,
    /// Bytes of share transactions beyond the share's own coinbase, such as
    /// share trades.
    pub extra_transaction_bytes: u64,
    /// Bytes of the whole share (`ShareBlock`) without uncles.
    pub body_bytes: u64,
}

impl ShareSizes {
    /// Sizes of a share whose coinbase pays `payees` under `layout`, mined
    /// on a block template of `block_bytes`, carrying
    /// `extra_transaction_bytes` of share transactions beside its own
    /// coinbase.
    pub fn new(
        layout: CoinbaseLayout,
        payees: u64,
        block_bytes: u64,
        average_transaction_bytes: u64,
        extra_transaction_bytes: u64,
    ) -> Self {
        let split = split_payees(layout, payees);
        let merkle_branch_hashes = merkle_branch_hashes(block_transaction_count(
            block_bytes,
            average_transaction_bytes,
        ));
        let header_bytes = SHARE_HEADER_BASE_BYTES;
        let body_bytes = header_bytes
            + compact_size_bytes(1)
            + SHARE_COINBASE_BYTES
            + extra_transaction_bytes
            + compact_size_bytes(merkle_branch_hashes)
            + HASH_BYTES * merkle_branch_hashes
            + split.coinbase_bytes;
        Self {
            split,
            merkle_branch_hashes,
            header_bytes,
            extra_transaction_bytes,
            body_bytes,
        }
    }

    /// Bytes of the Bitcoin Cash coinbase the share carries.
    pub fn coinbase_bytes(&self) -> u64 {
        self.split.coinbase_bytes
    }

    /// Whole share bytes with `uncles` uncle hashes.
    pub fn body_bytes_with_uncles(&self, uncles: usize) -> u64 {
        self.body_bytes + SHARE_HEADER_BYTES_PER_UNCLE * uncles as u64
    }

    /// Payload bytes of the `ShareBlock` message carrying the share: the
    /// one-byte message tag and the share.
    pub fn message_payload_bytes(&self, uncles: usize) -> u64 {
        1 + self.body_bytes_with_uncles(uncles)
    }

    /// Payload bytes of a share sent without the payout outputs every peer
    /// can rebuild, as the hybrid payout proposal suggests
    /// (`bch-hybrid-payouts.md`, section 8.1). An estimate: the format is
    /// not designed yet.
    pub fn message_payload_bytes_without_payouts(&self, uncles: usize) -> u64 {
        self.message_payload_bytes(uncles) - self.split.payout_bytes
    }

    /// Which limits the share and its coinbase meet.
    pub fn limit_check(&self, layout: CoinbaseLayout) -> LimitCheck {
        let coinbase = self.coinbase_bytes();
        let share_body = SHARE_COINBASE_BYTES + self.extra_transaction_bytes + coinbase;
        LimitCheck {
            coinbase_within_bch_transaction_limit: coinbase <= BCH_MAX_TRANSACTION_BYTES,
            share_body_within_today_limit: share_body <= SHARE_BODY_LIMIT_BYTES,
            share_body_within_proposed_limit: share_body <= PROPOSED_SHARE_BODY_LIMIT_BYTES,
            message_within_today_p2p_limit: self.message_payload_bytes(MAX_UNCLES)
                <= MAX_P2P_MESSAGE_BYTES,
            coinbase_within_sv2_extended_fields: coinbase.saturating_sub(COINBASE_PREFIX_BYTES)
                <= SV2_COINBASE_FIELD_BYTES,
            token_claim_within_vm_element: layout
                .carries_token_claim()
                .then_some(coinbase <= BCH_VM_STACK_ELEMENT_BYTES),
            coinbase_within_asic_evidence: coinbase <= ASIC_COINBASE_EVIDENCE_BYTES,
        }
    }
}

/// The limits a share meets. Each says whose limit it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LimitCheck {
    /// The coinbase fits BCH's 1,000,000-byte transaction limit, a Bitcoin
    /// Cash consensus rule: a larger coinbase makes the block invalid.
    pub coinbase_within_bch_transaction_limit: bool,
    /// The share's transactions and bitcoin coinbase fit today's
    /// `BLOCK_TXS_SIZE_LIMIT` (200 KiB), a P2Pool share-chain rule: peers
    /// reject a larger share.
    pub share_body_within_today_limit: bool,
    /// They fit the 1,204,800 bytes the hybrid payout proposal sets instead.
    pub share_body_within_proposed_limit: bool,
    /// The `ShareBlock` message, with three uncles, fits today's 1 MiB P2P
    /// message limit, an implementation limit of this node: a larger share
    /// cannot be relayed at all.
    pub message_within_today_p2p_limit: bool,
    /// The coinbase after the extranonce fits one 65,535-byte field of a
    /// Stratum V2 extended job or declared job.
    pub coinbase_within_sv2_extended_fields: bool,
    /// For a share with a token claim: the coinbase fits one 10,000-byte VM
    /// stack element, so the claim can hash it. `None` without a claim.
    pub token_claim_within_vm_element: Option<bool>,
    /// The coinbase is no larger than the 784 bytes stock Antminer firmware
    /// is known to take (evidence from another pool, not a rule; not listed
    /// by `broken`).
    pub coinbase_within_asic_evidence: bool,
}

impl LimitCheck {
    /// Short names of the rule and protocol limits the share breaks, worst
    /// first. Device evidence is left out.
    pub fn broken(&self) -> Vec<&'static str> {
        let mut broken = Vec::with_capacity(6);
        if !self.coinbase_within_bch_transaction_limit {
            broken.push("BCH 1 MB transaction");
        }
        if !self.share_body_within_proposed_limit {
            broken.push("proposed share body");
        }
        if !self.message_within_today_p2p_limit {
            broken.push("today's 1 MiB P2P message");
        }
        if !self.share_body_within_today_limit {
            broken.push("today's 200 KiB share body");
        }
        if self.token_claim_within_vm_element == Some(false) {
            broken.push("VM 10,000-byte element");
        }
        if !self.coinbase_within_sv2_extended_fields {
            broken.push("SV2 extended 64 KiB field");
        }
        broken
    }
}

/// The largest payee count whose uncapped coinbase still meets `fits`.
pub fn largest_direct_payee_count(fits: impl Fn(&ShareSizes) -> bool) -> u64 {
    let mut low = 0u64;
    let mut high = 1u64 << 32;
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        let sizes = ShareSizes::new(CoinbaseLayout::Uncapped, middle, 1, 1, 0);
        if fits(&sizes) {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    low
}

/// The most payees `layout` pays without a payout tree.
pub fn largest_payee_count_without_tree(layout: CoinbaseLayout) -> u64 {
    let mut low = 0u64;
    let mut high = 1u64 << 32;
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        if split_payees(layout, middle).tree_payees == 0 {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    low
}

/// Payload bytes of the `Ack` a node answers each pushed share with: the
/// one-byte message tag.
pub const ACK_PAYLOAD_BYTES: u64 = 1;

/// Bytes a message of `payload_bytes` puts on the wire: payload, framing
/// and TCP/IP headers.
pub fn wire_bytes(payload_bytes: u64) -> u64 {
    let framed = payload_bytes + MESSAGE_FRAMING_BYTES;
    let segments = framed.div_ceil(SEGMENT_PAYLOAD_BYTES).max(1);
    framed + segments * PACKET_HEADER_BYTES
}

#[cfg(test)]
mod tests;
