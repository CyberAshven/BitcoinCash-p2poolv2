// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The experimental BCH large templates extension (`0x4C54`, "LT").
//!
//! Standard Job Declaration was sized for Bitcoin. `DeclareMiningJob` lists
//! at most 65,535 transactions, `ProvideMissingTransactions` names them by
//! `u16` position, and one `ProvideMissingTransactions.Success` must fit one
//! frame (`2^24 - 1` bytes). A Bitcoin Cash template under ABLA (a 32 MB
//! floor that grows) passes all three. The extension lets a Full-Template
//! declaration carry any template the block size limit allows, without
//! changing a standard message:
//!
//! - `DeclareMiningJob.Transactions` (`0x01`, client to node) sends the
//!   declared transaction ids in pages before the `DeclareMiningJob` with the
//!   same request id; the declared list is every page in order, then the core
//!   message's list.
//! - `ProvideMissingTransactions.Positions` (`0x02`, node to client) asks for
//!   transactions by `u32` position in the whole list, at most 65,535 at a
//!   time.
//! - On a connection that negotiated the extension, a client answers either
//!   request with the longest prefix of the asked transactions that fits one
//!   frame, and the node asks again for the rest.
//!
//! Template Distribution has the same limits: one
//! `RequestTransactionData.Success` lists at most 65,535 transactions and
//! must fit one frame. A Template Provider that grants the extension pushes
//! every template's transactions after its `NewTemplate`, so the client never
//! asks:
//!
//! - `RequestTransactionData.Transactions` (`0x03`, provider to client) is one
//!   page of a template's transactions, in order; the core
//!   `RequestTransactionData.Success` carries the rest and ends the push.
//! - `RequestTransactionData.Kept` (`0x04`, provider to client) starts a
//!   delta: a bitmap of the base template's transactions this template keeps
//!   ([`kept_entries`]). The pages and the Success then carry the added
//!   transactions, and the list is the kept and the added ones in CTOR.
//!
//! The design is shared with Knuth's client and Template Provider
//! (`docs/architecture/sv2-large-templates-extension.md`).

use super::session::encode_extension_frame;
use binary_sv2::{B016M, Deserialize, Seq064K, Serialize, U256};

/// The extension's `extension_type`.
pub const LARGE_TEMPLATES_EXTENSION: u16 = 0x4C54;
/// `DeclareMiningJob.Transactions`: a page of declared transaction ids.
pub const DECLARE_MINING_JOB_TRANSACTIONS: u8 = 0x01;
/// `ProvideMissingTransactions.Positions`: a request by `u32` position.
pub const PROVIDE_MISSING_TRANSACTIONS_POSITIONS: u8 = 0x02;
/// `RequestTransactionData.Transactions`: a page of a template's
/// transactions, pushed by a Template Provider.
pub const REQUEST_TRANSACTION_DATA_TRANSACTIONS: u8 = 0x03;
/// `RequestTransactionData.Kept`: the base template's transactions a pushed
/// delta keeps.
pub const REQUEST_TRANSACTION_DATA_KEPT: u8 = 0x04;

/// Most transaction ids in one page, and most positions in one request:
/// what a `SEQ0_64K` holds.
pub const MAX_IDS_PER_MESSAGE: usize = 65_535;

/// Bitcoin Cash's smallest transaction since Upgrade 9, in bytes (BCHN's
/// `MIN_TX_SIZE_UPGRADE9`). A block of `size_limit` bytes holds at most
/// `size_limit / 65` transactions, which bounds a declaration's list.
pub const MIN_TRANSACTION_SIZE: u64 = 65;

/// `DeclareMiningJob.Transactions` (`0x01`).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct DeclareMiningJobTransactions<'decoder> {
    /// The request id of the `DeclareMiningJob` the page belongs to.
    pub request_id: u32,
    /// Transaction ids, in the template's order (CTOR on Bitcoin Cash).
    pub txid_list: Seq064K<'decoder, U256<'decoder>>,
}

/// `ProvideMissingTransactions.Positions` (`0x02`).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ProvideMissingTransactionsPositions<'decoder> {
    /// The declaration's request id.
    pub request_id: u32,
    /// 0-based positions in the whole declared list, pages first, the
    /// coinbase not counted.
    pub unknown_tx_position_list: Seq064K<'decoder, u32>,
}

/// `RequestTransactionData.Transactions` (`0x03`).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct RequestTransactionDataTransactions<'decoder> {
    /// The template the page belongs to.
    pub template_id: u64,
    /// Serialized transactions, in the order the push gives them.
    pub transaction_list: Seq064K<'decoder, B016M<'decoder>>,
}

/// `RequestTransactionData.Kept` (`0x04`).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct RequestTransactionDataKept<'decoder> {
    /// The template the delta builds.
    pub template_id: u64,
    /// The last template whose transactions were pushed complete on the
    /// connection.
    pub base_template_id: u64,
    /// One bit per base transaction, in block order without the coinbase
    /// ([`kept_entries`]).
    pub kept: B016M<'decoder>,
}

/// The entries of `base` that a `RequestTransactionData.Kept` bitmap keeps,
/// in order.
///
/// Bit `i`, `(kept[i / 8] >> (i % 8)) & 1`, keeps `base[i]`. The bitmap is
/// exactly `ceil(base.len() / 8)` bytes, and the unused high bits of its last
/// byte are clear; any other bitmap is an error.
pub fn kept_entries<T: Clone>(base: &[T], kept: &[u8]) -> Result<Vec<T>, String> {
    if kept.len() != base.len().div_ceil(8) {
        return Err(format!(
            "kept bitmap of {} bytes for a base of {} transactions",
            kept.len(),
            base.len()
        ));
    }
    let used_bits = base.len() % 8;
    if used_bits != 0 && kept.last().is_some_and(|last| *last >> used_bits != 0) {
        return Err("kept bitmap sets bits past the base".to_string());
    }
    let count = kept
        .iter()
        .map(|byte| byte.count_ones() as usize)
        .sum::<usize>();
    let mut entries = Vec::with_capacity(count);
    entries.extend(
        base.iter()
            .enumerate()
            .filter(|(index, _)| (kept[index / 8] >> (index % 8)) & 1 == 1)
            .map(|(_, entry)| entry.clone()),
    );
    Ok(entries)
}

/// The most transactions a declaration for a block of `size_limit` bytes can
/// list.
pub fn max_declared_transactions(size_limit: u64) -> usize {
    usize::try_from(size_limit / MIN_TRANSACTION_SIZE).unwrap_or(usize::MAX)
}

/// A `ProvideMissingTransactions.Positions` frame asking for the
/// `count` transactions from `start`.
pub(crate) fn positions_frame(
    request_id: u32,
    start: usize,
    count: usize,
) -> Result<Vec<u8>, String> {
    let positions = (start..start + count)
        .map(|position| {
            u32::try_from(position).map_err(|_| "declared position past u32".to_string())
        })
        .collect::<Result<Vec<_>, String>>()?;
    let payload = binary_sv2::to_bytes(ProvideMissingTransactionsPositions {
        request_id,
        unknown_tx_position_list: Seq064K::new(positions).map_err(|error| format!("{error:?}"))?,
    })
    .map_err(|error| format!("{error:?}"))?;
    encode_extension_frame(
        LARGE_TEMPLATES_EXTENSION,
        PROVIDE_MISSING_TRANSACTIONS_POSITIONS,
        false,
        &payload,
    )
}
