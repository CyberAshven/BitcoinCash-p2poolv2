---
name: Chain Store RocksDB Schema
description: Captures the rocksdb schema design currently in use
---

**The architecture files are LLM generated. The goal is make it easier
for new comers to the code base and other LLMs to quickly understand
the design and remain consistent with the design choices.**

# Chain Store RocksDB Schema

This document captures the RocksDB storage schema used by `Store` and `ChainStore` in P2Poolv2.

## Overview

- **Database**: RocksDB with column families for independent compaction
- **Serialization**: Bitcoin consensus encoding for most types
- **Location**: `p2poolv2_lib/src/store/` and `p2poolv2_lib/src/shares/chain/chain_store_handle.rs`

## Architecture

```
ChainStore (high-level chain operations)
    └── Store (low-level RocksDB operations)
            └── RocksDB (column families)
```

### Chain State (in-memory, thread-safe)

Managed by `Store` using `Arc<RwLock<T>>`:

| Field                | Type                 | Description                         |
|----------------------|----------------------|-------------------------------------|
| `genesis_block_hash` | `Option<BlockHash>`  | Hash of genesis block               |
| `chain_tip`          | `BlockHash`          | Current chain tip (highest work)    |
| `tips`               | `HashSet<BlockHash>` | All current chain tips (DAG leaves) |

---

## Column Families

### 1. `BlockMetadata` - Block Metadata

Stores block metadata (height, chain work, status).

| Key                    | Value                            | Notes                                    |
|------------------------|----------------------------------|------------------------------------------|
| `blockhash` (32 bytes) | `BlockMetadata` (serialized)     | Height, chain_work, and status           |

**BlockMetadata Structure:**
```rust
pub struct BlockMetadata {
    pub expected_height: Option<u32>,      // Expected block height in chain, if known
    pub chain_work: Work,         // Parent's chain_work + own work + named uncles' own work (32 bytes LE)
}
```

---

### 1b. `Header` - Share Headers

Stores share headers independently, enabling header-first sync before full blocks arrive.

| Key                    | Value                        | Notes                                |
|------------------------|------------------------------|--------------------------------------|
| `blockhash` (32 bytes) | `ShareHeader` (serialized)   | Written during header sync and block storage |

The serialized header carries its `CoinbaseProof` and encodes each address as a
one-byte network class (main, test, regtest) followed by its script pubkey. It
holds no share merkle root and no bitcoin coinbase data: every header field is
bound to the proof of work, and the coinbase lives in `BitcoinCoinbase`.

### 1c. `TemplateMerkleBranches` - Coinbase Merkle Branches

The coinbase merkle branch a header's `CoinbaseProof` is checked against.

| Key                    | Value                                  | Notes |
|------------------------|----------------------------------------|-------|
| `blockhash` (32 bytes) | `MerkleBranches` (count + 32-byte nodes) | Written with a full block, and during header sync for headers held without their body, so they can be served on |

### 1d. `BitcoinCoinbase` - Bitcoin Coinbase

The coinbase a share block carries: the transaction the miner hashed,
holding the aux flags, extranonce, nanosecond timestamp and payouts (and,
in the retained Bitcoin mode, the BIP141 witness commitment; Bitcoin Cash
has no witness data). Not part of the block hash; its txid is the one the
header's `CoinbaseProof` gives. On Bitcoin Cash it is at most 1,000,000
bytes, or 3,072 bytes for a token share
([storage budget](async-flow.md#bch-storage-and-memory-budget-2026-10-30)).

| Key                    | Value                                  | Notes |
|------------------------|----------------------------------------|-------|
| `blockhash` (32 bytes) | `Transaction` (consensus serialized)   | Written with a full block only; a header held without its body has none, and `get_share` returns `None` without it |

Body data: deleted by the pruner below the prune target.

### 1e. `WeakShares` and `WeakSummaries` - Carried Weak Shares (Bitcoin Cash)

The weak shares a Bitcoin Cash share carries (`shares::weak`), and the
summary the PPLNS window and the carried-once rule read instead of them.

| Key                    | Value                                  | Notes |
|------------------------|----------------------------------------|-------|
| `blockhash` (32 bytes) | `WeakShares`: count + each `WeakShare` (header, scriptSig, value, branch) | Written with a full block that carries weak shares |
| `blockhash` (32 bytes) | `WeakSummaries`: the weak shares' proof-of-work hashes, then their credit groups (parent, payout address, share owner, donation, fee, BCH prev, count) | Written with the weak shares |

Body data. `WeakSummaries` goes with the body at the prune target;
`WeakShares` goes one payout depth below the confirmed tip
([Weak Shares](pruning.md#weak-shares)).

---

### 2. `BlockIndex` - Parent->Children Index

Tracks parent-to-children relationships for DAG traversal. Uses merge operator for atomic appends.

| Key                 | Value                         |
|---------------------|-------------------------------|
| `blockhash` + `_bi` | `Vec<BlockHash>` (serialized) |

---

### 3. `BlockHeight` - Height Indexes

Three key kinds share this column family. The merge operator is registered
for it and only the `h:` keys use it.

| Key                                  | Value                         | Notes |
|--------------------------------------|-------------------------------|-------|
| `h:` + `height` (4 bytes BE)         | `Vec<BlockHash>` (serialized) | Every block at that height, forks and uncles included. Kept forever |
| `height` (4 bytes BE) + `:c`         | `blockhash` (32 bytes)        | Candidate chain entry. Deleted by the pruner below the prune target |
| `height` (4 bytes BE) + `:f`         | `blockhash` (32 bytes)        | Confirmed chain entry. Kept forever |

---

### 4. `BlockTxids` - Block→Transaction IDs

Maps share blocks to their sharechain transaction IDs.

| Key                    | Value                          |
|------------------------|--------------------------------|
| `blockhash` + `_txids` | `Txids` (`Vec<Txid>` serialized) |

`share_block_exists` keys off this entry, so a block with no entry is held
header-only. Body data: deleted by the pruner below the prune target.

---

### 5. `BitcoinTxids` - Block→Bitcoin Transaction IDs

Maps share blocks to their Bitcoin transaction IDs (separate from sharechain txs).

| Key                            | Value                          |
|--------------------------------|--------------------------------|
| `blockhash` + `_bitcoin_txids` | `Txids` (`Vec<Txid>` serialized) |

Nothing writes this column family yet. The pruner still deletes its key for
every pruned block, so a future writer cannot leak data.

---

### 6. `Tx` - Transaction Metadata

Stores transaction metadata (not full tx data).

| Key               | Value                     |
|-------------------|---------------------------|
| `txid` (32 bytes) | `TxMetadata` (serialized) |

**TxMetadata Structure:**
```rust
pub struct TxMetadata {
    pub txid: Txid,
    pub version: transaction::Version,
    pub lock_time: absolute::LockTime,
    pub input_count: u32,
    pub output_count: u32,
    pub validated: bool,              // Script validation passed
}
```

---

### 7. `Inputs` - Transaction Inputs

Stores individual transaction inputs.

| Key                       | Value               |
|---------------------------|---------------------|
| `{txid}:{index}` (string) | `TxIn` (serialized) |

---

### 8. `Outputs` - Transaction Outputs

Stores individual transaction outputs.

| Key                       | Value                |
|---------------------------|----------------------|
| `{txid}:{index}` (string) | `StoredTxOut` (serialized): the `TxOut`, an `is_coinbase` byte, then the coinbase root: tag `0` and a 4-byte height, or tag `1` for `CoinbaseRoot::Unknown` |

The coinbase root is the oldest coinbase in the output's spending chain,
stamped at write time. `Unknown` marks an output with an input this node did
not hold when it was written; spending it is rejected
([Missing Input at Write Time](pruning.md#missing-input-at-write-time)).

---

### 9. `TxidsBlocks` - Transaction→Blocks Index

The blocks that include a transaction. Uses the merge operator for appends.
The pruner reads it to keep a transaction that a kept block still includes,
and rewrites the list without the pruned blocks.

| Key               | Value                         |
|-------------------|-------------------------------|
| `txid` (32 bytes) | `Vec<BlockHash>` (serialized) |

---

### 9b. `SpendsIndex` - Outpoint→Spending Input

Records which input on the confirmed chain spends an outpoint. Written only
from the confirmation path (`put_confirmed_entry`) and cleared on reorg-out,
so presence means "spent on the confirmed chain". This replaces the old
`UnspentOutputs` set.

| Key                       | Value                                              |
|---------------------------|----------------------------------------------------|
| `{txid}:{index}` (string) | spending `txid` (32 bytes) + input index (4 bytes) |

---

### 9c. `Uncles` - Uncle→Nephews Index

The blocks that declare a block as an uncle. Uses the merge operator.

| Key                    | Value                         |
|------------------------|-------------------------------|
| `blockhash` (32 bytes) | `Vec<BlockHash>` (serialized) |

---

### 9d. `ScriptHistory` - Script→Confirmed Transactions

The confirmed transactions that touch each script, for wallet history. A
transaction touches a script when an output pays it (`OP_RETURN` outputs
excluded) or an input spends an output that paid it, with one row per
distinct script per transaction. A prefix scan returns a script's history in
height then position order. See `store/script_history.rs`.

Bitcoin Cash fork: rows are written only while the node serves Electrum
(`[electrum] enabled`). `Metadata` key `script_history_from` holds the first
confirmed height with history (`u32::MAX` while none is written; absent for
a store that always kept it), and reads start no lower, so turning Electrum
on later never serves a partial history.

| Key (40 bytes)                                                                  | Value             |
|---------------------------------------------------------------------------------|-------------------|
| `sha256(script_pubkey)` (32 bytes) + height (4 bytes BE) + position (4 bytes BE) | `txid` (32 bytes) |

`position` is the transaction's index in its block.

- **Window:** spendable depth, heights from `candidate tip + 1 -
  PAYOUT_DEPTH` (`BCH_PAYOUT_DEPTH` on Bitcoin Cash, here and in the
  filter threshold). Rows are written only for blocks confirmed inside it
  (`put_confirmed_entry`, and `setup_genesis` for genesis), and reads never
  go below it.
- **Reorg-out:** the reorged-out block's keys are recomputed from its
  transactions and deleted in the reorg batch.
- **Old rows:** never deleted explicitly. A compaction filter drops rows
  below `confirmed tip - PAYOUT_DEPTH - PRUNE_INTERVAL` (threshold raised by
  the prune task while the node is current), and periodic compaction filters
  every file at least daily.

---

### Pruning

`Tx`, `Inputs`, `Outputs`, `TxidsBlocks` and `SpendsIndex` rows are deleted
with a pruned block unless a kept block still includes the transaction.
`BlockTxids`, `BitcoinTxids`, `BitcoinCoinbase` and candidate `BlockHeight`
entries are deleted below the prune target. `ScriptHistory` is not pruned:
its compaction filter drops rows at spendable depth (section 9d). Headers, metadata, the block
index, `h:` and confirmed height entries, `TemplateMerkleBranches`, `Uncles`
and both `FoundBlocks` and `AcceptedFoundBlocks` are kept forever. See
`pruning.md`.

---

### 10. `Share` - PPLNS Shares

Stores local miner PPLNS shares for reward tracking.

| Key (24 bytes)                                                      | Value                           |
|---------------------------------------------------------------------|---------------------------------|
| `n_time` (8 bytes BE) + `user_id` (8 bytes BE) + `seq` (8 bytes BE) | `SimplePplnsShare` (serialized) |

**SimplePplnsShare Structure:**
```rust
pub struct SimplePplnsShare {
    pub user_id: u64,           // Internal user ID
    pub difficulty: u64,        // Share difficulty
    pub btcaddress: Option<String>,  // Skipped in serialization
    pub workername: Option<String>,  // Skipped in serialization
    pub n_time: u64,            // Mining timestamp
    pub job_id: String,         // Job ID
    pub extranonce2: String,    // Extranonce2 from submit
    pub nonce: String,          // Nonce from submit
}
```

---

### 11. `User` - User Registry

Stores user data indexed by internal ID.

| Key                    | Value                     |
|------------------------|---------------------------|
| `user_id` (8 bytes BE) | `StoredUser` (serialized) |

**StoredUser Structure:**
```rust
pub struct StoredUser {
    pub user_id: u64,           // Internal unique ID
    pub btcaddress: String,     // Payout address from the username (CashAddr on BCH)
    pub created_at: u64,        // Timestamp (seconds since epoch)
}
```

---

### 12. `UserIndex` - Payout Address→User ID

Secondary index for looking up users by payout address: on Bitcoin Cash the
CashAddr in the miner's username. The key and field keep the upstream name
`btcaddress`.

| Key                   | Value                  |
|-----------------------|------------------------|
| `btcaddress` (string) | `user_id` (8 bytes BE) |

---

### 13. `Metadata` - General Metadata

Store-wide values.

| Key            | Value                  | Notes |
|----------------|------------------------|-------|
| `store_format` | `u32` (4 bytes LE)     | `STORE_FORMAT_VERSION`, written with genesis. Startup refuses a store with another version or none, and tells the operator to delete it and resync |

`STORE_FORMAT_VERSION` (`store/mod.rs`) is 12. Stores written before
version 1 have no version.

---

### 14. `FoundBlocks` - Unverified Bitcoin Cash Block Candidates

One entry per block carried by a confirmed share or an uncle whose BCH
header meets the target its own bits claim. Written in `confirm_blocks`,
in the same batch as confirmation, from header data only, including
prune-zone header-only confirmation. Not proof of node acceptance.
Re-confirmation rewrites identical bytes; never pruned.

| Key                                                     | Value                                                                                      |
|---------------------------------------------------------|--------------------------------------------------------------------------------------------|
| `bitcoin_height` (8 bytes BE) + `bitcoin_blockhash` (32 bytes) | consensus-encoded `share_blockhash` (32 bytes), `bitcoin_time` (u32), `miner_bitcoin_address` (string) |

### 14a. `AcceptedFoundBlocks` - BCHN-Accepted Pool Blocks

Same encoding as `FoundBlocks`. Written through StoreWriter only after a
positive `submitblock` verdict or the node's chain hash matching the
candidate at its height (`FoundBlockVerifier`); idempotent. Only this
column family feeds the found-block API and metrics, each entry joined to
its `AcceptedBlockEffort` snapshot (14b). No share validity or payout rule
depends on it. Never pruned.

A read-write open creates it in an older store. A read-only open cannot
(RocksDB rejects a listed column family the store lacks, even with
`create_missing_column_families`), so read-only tools fail on a store until
a read-write open has created it.

### 14b. `AcceptedBlockEffort` - Per-Block Effort Snapshots

One snapshot per accepted block, captured at the effort reset so a restart
never recomputes it against a moved difficulty: the pool work since the
previous find (`work_since_last_block` before the reset) and the network
difficulty from the found block header's own nBits (the units of the
`network_difficulty` gauge), `None` when that header cannot be read.
Written through StoreWriter right after `AcceptedFoundBlocks`; if the write
fails the block still stands, with no effort. Blocks accepted before this
column family existed have none, and `/found_blocks` reports
`"effort": null` (no backfill). Never pruned.

| Key                            | Value                                                                                                       |
|--------------------------------|-------------------------------------------------------------------------------------------------------------|
| `bitcoin_blockhash` (32 bytes) | consensus-encoded `EffortSnapshot`: `work` as f64 bits (u64, 8 bytes), presence tag (`1` = difficulty follows, `0` = `None`), then `difficulty` as f64 bits (u64, 8 bytes) when present |

```rust
pub struct EffortSnapshot {
    pub work: f64,
    pub network_difficulty: Option<f64>,
}
```

`get_found_blocks` joins the snapshot onto each accepted block, so the API,
metrics and dashboards read effort with no second query. The store format
does not change: a read-write open creates the family, and read-only tools
(`verify_chain`, `db_query`) never request it, so they open older stores
and report no effort.

---

### 15. `HashrateSamples` - Dashboard Hashrate History

One entry per dashboard hashrate sample, written by the API server's
sampler so a restart reloads the chart. Retention is 30 days
(`RETENTION_SECS` in `store/hashrate_samples.rs`), evicted by a range delete
below the cutoff. Only a read-write open registers it (creating it on first
open); read-only tools never request it, so older binaries still open the
store and a rollback starts from an empty chart.

| Key                          | Value                                             | Notes                                        |
|------------------------------|---------------------------------------------------|----------------------------------------------|
| `time` (8 bytes BE)          | consensus-encoded `HashrateSample`                | One sample per minute; same time overwrites  |

**HashrateSample Structure:**
```rust
pub struct HashrateSample {
    pub time: u64,             // Unix seconds the sample was taken
    pub pool: Option<f64>,      // Whole pool, hashes per second
    pub local: Option<f64>,     // This node's miners, hashes per second
    pub network: Option<f64>,   // Bitcoin Cash network, hashes per second
    pub miners: Option<u64>,    // This node's miners with a worker online
    pub workers: Option<u64>,   // This node's online workers in all
}
```

Each `Option` is a tag byte (`1` = value, `0` = `None`, else a decode
error) then the value (`f64` or `u64`, little endian) when present. A row
written before the count fields existed ends after `network`; a clean end
of input where a tag would sit decodes as `None`, so old rows load with
unknown counts. A row is read to its end, so a binary older than the count
fields skips the newer rows as corrupt. Loading skips a corrupt,
key-mismatched or future-dated row with a warning.

---

### 16. Other Bitcoin Cash column families

- `PayoutTrees`: the payout tree of each found block whose coinbase pays
  through one, keyed by coinbase txid, kept so the tree can be spent after
  maturity (`store/payout_tree.rs`; [BCH hybrid payouts](bch-hybrid-payouts.md)).
- `BchHeaders`, `BchBestChain`: the Bitcoin Cash headers the node knows
  (`KnownBchHeader`, kept forever) and its node's best chain, height to
  hash (`store/bch_headers.rs`; [Current chain](../sharechain/current-chain.md)).

## Key Constants

```rust
// store/dag_store.rs
pub const MAX_UNCLES_DEPTH: u8 = 3;                  // Max uncle inclusion depth
// shares/chain/chain_store_handle.rs
pub(crate) const COMMON_ANCESTOR_DEPTH: usize = 2160; // Ancestor search depth (6 hours)
```

The depths that size the PPLNS window, full validation and pruning
(`PPLNS_WINDOW_SHARES`, `COINBASE_MATURITY`, `PAYOUT_DEPTH`, `PRUNE_DEPTH`,
`PRUNE_INTERVAL`) are in `accounting/payout/sharechain_pplns/pplns_window.rs`;
see `pruning.md`.

---

## Key Operations

### Adding a Share

1. Store transactions in `Inputs`, `Outputs` and `Tx`, and append the block to
   each transaction's `TxidsBlocks` list
2. Store `blockhash → txids` in `BlockTxids`
3. Store the bitcoin coinbase in `BitcoinCoinbase`
4. Update `BlockIndex` (parent→child relationship)
5. Update `BlockHeight` (height→blockhash mapping)
6. Store `BlockMetadata` (height + chain_work)
7. Store `ShareHeader` in `Header`
8. Handle reorg if new chain has more work

### Reorg Logic

- Find common ancestor with current tip
- If shares have common ancestor: compare cumulative `chain_work`, which
  includes the work of the uncles each chain names
- If disjoint chains: compare work over PPLNS window
- Update tips: remove prev_share_blockhash and uncles from tips

---

## File Locations

| Component          | Path                                              | Description                              |
|--------------------|---------------------------------------------------|------------------------------------------|
| Store (main)       | `p2poolv2_lib/src/store/mod.rs`                   | Core Store struct, chain state, genesis  |
| Column Families    | `p2poolv2_lib/src/store/column_families.rs`       | ColumnFamily enum definitions            |
| Block/Tx Metadata  | `p2poolv2_lib/src/store/block_tx_metadata.rs`     | BlockMetadata, TxMetadata structs        |
| DAG Store          | `p2poolv2_lib/src/store/dag_store.rs`             | DAG traversal, common ancestor, locators |
| Share Store        | `p2poolv2_lib/src/store/share_store.rs`           | Share block storage and retrieval        |
| Transaction Store  | `p2poolv2_lib/src/store/transaction_store.rs`     | Transaction storage, inputs, outputs     |
| User Store         | `p2poolv2_lib/src/store/user.rs`                  | User CRUD operations                     |
| StoredUser         | `p2poolv2_lib/src/store/stored_user.rs`           | StoredUser struct definition             |
| PPLNS Shares       | `p2poolv2_lib/src/store/pplns_shares.rs`          | PPLNS share storage and filtering        |
| Block Pruning      | `p2poolv2_lib/src/store/prune_blocks.rs`          | Prune boundaries and body deletion       |
| Background Tasks   | `p2poolv2_node/src/background_tasks.rs`           | Periodic pruning, per pool mode          |
| ChainStoreHandle   | `p2poolv2_lib/src/shares/chain/chain_store_handle.rs` | High-level chain operations          |
| SimplePplnsShare   | `p2poolv2_lib/src/accounting/payout/simple_pplns/mod.rs` | PPLNS share struct definition            |
