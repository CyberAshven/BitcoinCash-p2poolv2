// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Numbers the model takes from Bitcoin Cash and from this repository.
//!
//! Each constant names its source. The Bitcoin Cash ones come from Bitcoin
//! Cash Node (BCHN) source on GitLab, the ABLA, P2SH32 and VM limits CHIPs
//! and BIP152; none are Bitcoin values. The P2Pool ones are copies of
//! constants in `p2poolv2_lib` and `p2poolv2_node`: this crate does not
//! depend on the library (it would pull in RocksDB and libp2p, and the `sim`
//! feature shortens some of them), so a change there must be copied here by
//! hand. Values marked "estimate" are this model's assumptions, not
//! measurements.

// ---------------------------------------------------------------------------
// Bitcoin Cash
// ---------------------------------------------------------------------------

/// Target block interval, seconds. BCHN `src/chainparams.cpp`
/// (`nPowTargetSpacing`), kept by the ASERT difficulty rule.
pub const BCH_BLOCK_INTERVAL_SECONDS: f64 = 600.0;

/// Largest transaction BCH consensus accepts, the coinbase included.
/// BCHN `src/consensus/consensus.h` (`MAX_TX_SIZE = ONE_MEGABYTE`), checked
/// for the coinbase by `CheckCoinbase` -> `CheckTransactionCommon` in
/// `src/consensus/tx_check.cpp` ("bad-txns-oversize").
pub const BCH_MAX_TRANSACTION_BYTES: u64 = 1_000_000;

/// Largest block BCHN accepts today, whatever ABLA allows. BCHN
/// `src/consensus/consensus.h` (`MAX_CONSENSUS_BLOCK_SIZE`, 2,000 MB),
/// described there as a temporary limit until 32-bit platforms are dropped.
pub const BCH_MAX_CONSENSUS_BLOCK_BYTES: u64 = 2_000_000_000;

/// Mainnet ABLA floor and the limit since the May 2024 upgrade. BCHN
/// `src/consensus/consensus.h` (`DEFAULT_CONSENSUS_BLOCK_SIZE`) and
/// CHIP-2023-04 (epsilon0 + beta0 = 16,000,000 + 16,000,000).
pub const BCH_MAINNET_ABLA_FLOOR_BYTES: u64 = 32_000_000;

/// Chipnet's ABLA floor: CHIP-2023-04 (epsilon0 = beta0 = 1,000,000) and the
/// BCHN test network documentation (2 MB).
pub const BCH_CHIPNET_ABLA_FLOOR_BYTES: u64 = 2_000_000;

/// Scalenet's starting limit: BCHN `src/chainparams.cpp`
/// (`nDefaultConsensusBlockSize = 256 * ONE_MEGABYTE` for scalenet) and
/// CHIP-2023-04, which says 256 MB blocks have been extensively tested.
pub const BCH_SCALENET_LIMIT_BYTES: u64 = 256_000_000;

/// A P2PKH output: 8-byte value, 1-byte script length, 25-byte script.
pub const P2PKH_OUTPUT_BYTES: u64 = 34;

/// A P2SH32 output, the root of a payout tree: 8-byte value, 1-byte script
/// length and the 35-byte `OP_HASH256 <32 bytes> OP_EQUAL` script
/// (CHIP-2022-05 P2SH32, active since May 2023).
pub const P2SH32_OUTPUT_BYTES: u64 = 44;

/// Largest stack element the Bitcoin Cash VM handles since the May 2025
/// upgrade (CHIP-2021-05 VM Limits). A merge-mining token claim that hashes
/// the whole coinbase in the VM must fit it in one element.
pub const BCH_VM_STACK_ELEMENT_BYTES: u64 = 10_000;

/// A one-input, two-output P2PKH transfer with an ECDSA signature:
/// 4 version + 1 input count + 148 input (36 outpoint, 1 script length,
/// 107 signature and key, 4 sequence) + 1 output count + 2 * 34 outputs +
/// 4 locktime. Used as the average transaction when turning a block size
/// into a transaction count; real blocks mix other sizes.
pub const TYPICAL_TRANSACTION_BYTES: u64 = 226;

/// Bytes per transaction in a BIP152 compact block (short transaction ID).
/// BCHN implements BIP152 (`cmpctblock`, `getblocktxn`, `blocktxn` in
/// `src/protocol.h`).
pub const COMPACT_BLOCK_SHORT_ID_BYTES: u64 = 6;

/// Fixed part of a BIP152 compact block: 80-byte header and 8-byte nonce,
/// plus small length prefixes.
pub const COMPACT_BLOCK_FIXED_BYTES: u64 = 96;

/// Outbound peers a BCHN node keeps: `MAX_OUTBOUND_CONNECTIONS` in BCHN
/// `src/net.h`. A listening node has more (inbound); the model uses this.
pub const BCHN_OUTBOUND_PEERS: u64 = 8;

/// One entry of a BCH `inv` message: 4-byte type and 32-byte hash. Every
/// transaction is announced over each peer link that does not already have
/// it.
pub const INVENTORY_ENTRY_BYTES: u64 = 36;

/// JSON bytes `getblocktemplate` adds per transaction beyond its hex data,
/// counted from BCHN's field list (`txid`, `hash`, `depends`, `fee`,
/// `sigops`, `sigchecks`, keys and punctuation). An estimate.
pub const TEMPLATE_JSON_BYTES_PER_TRANSACTION: u64 = 208;

// ---------------------------------------------------------------------------
// P2Pool (this repository)
// ---------------------------------------------------------------------------

/// Target share interval, seconds: `IDEAL_BLOCK_TIME` in
/// `p2poolv2_lib/src/sim_overrides.rs` (the share chain's own ASERT target;
/// `docs/bch-design.md`, "Share window and difficulty").
pub const SHARE_INTERVAL_SECONDS: f64 = 10.0;

/// Most shares a coinbase pays: `PPLNS_WINDOW_SHARES` in
/// `p2poolv2_lib/src/accounting/payout/sharechain_pplns/pplns_window.rs`.
pub const PPLNS_WINDOW_SHARES: u64 = 120_960;

/// BCH blocks of work the Bitcoin Cash PPLNS window holds:
/// `BCH_PPLNS_WINDOW_BLOCKS` in `pplns_window.rs`
/// (`docs/architecture/payout-distribution.md`, "Bitcoin Cash window").
pub const BCH_PPLNS_WINDOW_BLOCKS: f64 = 3.0;

/// Uncles one share may name: `MAX_UNCLES` in
/// `p2poolv2_lib/src/shares/validation/mod.rs`.
pub const MAX_UNCLES: usize = 3;

/// How far below its nephew an uncle may be: `MAX_UNCLES_DEPTH` in
/// `p2poolv2_lib/src/store/dag_store.rs`.
pub const MAX_UNCLES_DEPTH: usize = 3;

/// Share of its own work an uncle is paid (9/10): `UNCLE_SCALED_WEIGHT` over
/// `DIFFICULTY_SCALE` in `pplns_window.rs`. The nephew gets the other tenth.
pub const UNCLE_PAID_FRACTION: f64 = 0.9;

/// Largest share body a node accepts today, its share transactions plus the
/// bitcoin coinbase: `BLOCK_TXS_SIZE_LIMIT` (200 KiB) in
/// `p2poolv2_lib/src/shares/validation/mod.rs`. A P2Pool share-chain rule,
/// taken from upstream, a Bitcoin pool.
pub const SHARE_BODY_LIMIT_BYTES: u64 = 200 * 1024;

/// The share body limit the hybrid payout proposal sets in its place: a full
/// 1,000,000-byte coinbase (BCH's `MAX_TX_SIZE`) plus 200 KiB of share
/// transactions (`docs/architecture/bch-hybrid-payouts.md`, section 8.1).
/// Proposed, not built.
pub const PROPOSED_SHARE_BODY_LIMIT_BYTES: u64 = BCH_MAX_TRANSACTION_BYTES + 200 * 1024;

/// Largest P2P message a node decodes today: `MAX_P2P_MESSAGE_SIZE` (1 MiB)
/// in `p2poolv2_lib/src/node/messages.rs`. An implementation limit of this
/// node, not a share-chain rule: a larger share cannot be relayed at all. The
/// hybrid payout proposal re-derives it from the share body limit.
pub const MAX_P2P_MESSAGE_BYTES: u64 = 1024 * 1024;

/// Payout outputs a Bitcoin Cash coinbase may hold under alpha's current
/// share-chain rule (decision 14, `payout-cap11`): `BCH_MAX_COINBASE_PAYOUTS`
/// in `p2poolv2_lib/src/accounting/payout/coinbase_budget.rs`. The owner
/// rejected it on 2026-10-23; the model keeps it for comparison.
pub const BCH_MAX_COINBASE_PAYOUTS: u64 = 11;

/// The largest Bitcoin Cash coinbase alpha builds today apart from its
/// payouts: 51 bytes of transaction fields, a 100-byte scriptSig, the
/// donation (34), the fee (at most 51), the merge-mining output (55), the
/// padding output (at most 74) and the commitment output (43).
/// `MAX_BCH_COINBASE_BYTES_BESIDE_PAYOUTS` in
/// `p2poolv2_lib/src/stratum/work/coinbase.rs`. Used for alpha's rule.
pub const COINBASE_FIXED_BYTES: u64 = 408;

/// Coinbase outputs other than the payouts, today and in the proposal:
/// merge-mining commitment, donation, fee, padding and share commitment.
pub const COINBASE_FIXED_OUTPUTS: u64 = 5;

/// The hybrid payout proposal's coinbase limit for a share without token
/// commitments: BCH's own `MAX_TX_SIZE`. Payees past it go to the payout
/// tree (`bch-hybrid-payouts.md`, section 3.3).
pub const NO_TOKEN_COINBASE_LIMIT_BYTES: u64 = BCH_MAX_TRANSACTION_BYTES;

/// The hybrid payout proposal's coinbase limit `T` for a share that carries
/// token commitments, so that its token claim can hash the coinbase in the
/// VM: the auditor's recommendation, 3,072 bytes, still to be confirmed by
/// the owner. Payees past it are paid through a payout tree.
pub const TOKEN_COINBASE_THRESHOLD_BYTES: u64 = 3_072;

/// `S` in the hybrid payout proposal: the most a coinbase takes besides its
/// PPLNS payouts, every field at its worst case: 51 bytes of transaction
/// fields, a 100-byte scriptSig, 2 more bytes of output count, the 53-byte
/// CTMM commitment, the donation (34), the fee (at most 44, P2SH32), the
/// tree output (44), the padding (at most 74) and the share commitment (43)
/// (`bch-hybrid-payouts.md`, section 3.3). The payout budget is `C - S`.
pub const HYBRID_COINBASE_BYTES_BESIDE_PAYOUTS: u64 = 445;

/// The same coinbase without the tree output and with a one-byte output
/// count: the part of a proposal coinbase that does not depend on payees.
pub const HYBRID_COINBASE_FIXED_BYTES: u64 =
    HYBRID_COINBASE_BYTES_BESIDE_PAYOUTS - P2SH32_OUTPUT_BYTES - 2;

/// Most bytes a Stratum V2 `NewExtendedMiningJob` (and a declared job)
/// carries in each of its coinbase prefix and suffix fields (`B0_64K` in the
/// Stratum V2 specification). The suffix holds the payouts.
pub const SV2_COINBASE_FIELD_BYTES: u64 = 65_535;

/// Coinbase bytes before the miner's extranonce ends, which an SV2 job puts
/// in its prefix: version (4), input count (1), outpoint (36), scriptSig
/// length (1) and the largest scriptSig (100).
pub const COINBASE_PREFIX_BYTES: u64 = 142;

/// Bytes of the coinbase scriptSig the miner fills in (extranonce), which a
/// Stratum V1 job leaves out of coinb1 and coinb2: 12 bytes in
/// `stratum/work/coinbase.rs` (`MAX_COINBASE_SCRIPT_SIG_BYTES` is a declared
/// prefix of at most 88 bytes plus the 12-byte extranonce).
pub const EXTRANONCE_BYTES: u64 = 12;

/// A coinbase size seen to work on stock Antminer firmware (784 bytes, an
/// OCEAN block cited in upstream discussion #668): `BCH_COINBASE_BYTE_BUDGET`
/// in `p2poolv2_lib/src/stratum/work/coinbase.rs`. Evidence from another
/// pool, not a measurement on this pool's devices; the hybrid payout
/// proposal drops the check. Reported so a reader can see how far a coinbase
/// is from what such firmware is known to take.
pub const ASIC_COINBASE_EVIDENCE_BYTES: u64 = 784;

/// Coinbase sizes OCEAN's DATUM gateway allows per device family
/// (`src/datum_coinbaser.c` and `src/datum_stratum.c` in OCEAN-xyz/datum_gateway),
/// as cited in `docs/architecture/bch-hybrid-payouts.md`, section 8.2.
/// Evidence from another pool's software, not measurements on this pool's
/// devices.
pub const DEVICE_COINBASE_EVIDENCE: [(&str, u64); 4] = [
    ("older stock S19 firmware", 755),
    ("stock S21 firmware", 2_250),
    ("Whatsminer and Bitaxe", 6_500),
    ("ePIC and vnish firmware", 16_000),
];

/// How often a node asks BCHN for a full block template even without a ZMQ
/// block notice: `GBT_POLL_INTERVAL` in `p2poolv2_node/src/lib.rs`. Each
/// template also becomes a new Stratum job for every miner.
pub const TEMPLATE_POLL_SECONDS: f64 = 10.0;

/// Bytes of a share header (`ShareHeader::consensus_encode_with` in
/// `p2poolv2_lib/src/shares/share_block/mod.rs`) with P2PKH payout, donation
/// and fee addresses and no uncles: 32 previous share, 1 uncle count,
/// 27 payout address, 22 share owner, 80 bitcoin header, 4 bits, 4 time,
/// 28 donation address, 2 donation, 28 fee address, 2 fee, 8 bitcoin height
/// and 68 coinbase proof.
pub const SHARE_HEADER_BASE_BYTES: u64 = 305;

/// Bytes each uncle adds to a share header.
pub const SHARE_HEADER_BYTES_PER_UNCLE: u64 = 32;

/// Bytes of the share chain's own coinbase for a Bitcoin Cash P2PKH owner
/// (`build_sharechain_coinbase_for_witness_root` in
/// `p2poolv2_lib/src/shares/transactions/coinbase.rs`): 4 version, 1 input
/// count, 36 outpoint, 1 + 33 scriptSig, 4 sequence, 1 output count,
/// 34 output, 4 locktime.
pub const SHARE_COINBASE_BYTES: u64 = 118;
