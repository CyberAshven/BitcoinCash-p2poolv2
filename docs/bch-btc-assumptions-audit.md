# Bitcoin assumptions in the Bitcoin Cash port

P2Poolv2 was written for Bitcoin, and this fork runs it on Bitcoin Cash. This
audit lists the limits in the code that came from Bitcoin, or from Bitcoin
pools, rather than from Bitcoin Cash, and what was done about each. It
started from `alpha` at `71231a42` (2026-10-23): six sets of Bitcoin Cash
facts, each checked against BCHN source, BCHN documentation, the upgrade
specifications and the CHIPs, and eight sweeps of the code, whose 106 raw
findings merge into items A1 to A27. Most were fixed on PR #33.

"Share-chain rule" means a rule of P2Pool's own share chain, which BCHN never
sees. "BCH consensus" means the rules BCHN enforces on Bitcoin Cash blocks,
which this project never changes.

## The owner's decisions

These override the findings.

- **BCHN is the manager.** P2Pool must not duplicate or tighten node limits.
  It governs only what it builds itself: the coinbase, the share chain, share
  transactions and merge-mining proofs.
- **Block size.** 32 MB floor, no fixed maximum. P2Pool takes the limit from
  BCHN, and its types hold any size (`u64`).
- **First-seen is respected.** No replace-by-fee anywhere.
- **No coinbase payout cap.** Every miner is paid directly, with a payout
  tree only for shares carrying merge-mining token commitments past a
  threshold ([Hybrid payouts on Bitcoin Cash](architecture/bch-hybrid-payouts.md)),
  and the share body allows a full 1,000,000-byte coinbase.
- **The 100 KB standard transaction size is BCHN's node policy**, not a BCH
  consensus rule and not a P2Pool limit. The BCH consensus maximum is
  1,000,000 bytes; Knuth has no lower limit (Knuth PR #728).

## Open

| Item | What remains |
|---|---|
| A7: Stratum V2 large-block limits | Templates past 65,535 transactions or one 16 MiB frame travel over the BCH large templates extension `0x4C54`, for Job Declaration (PR #63) and from a Template Provider (PR #76). An extended job still carries at most 65,535 bytes of coinbase on each side of the extranonce (`MAX_EXTENDED_JOB_COINBASE_PART`, the SV2 field), so a larger coinbase (about 1,900 P2PKH payouts) is refused for that channel |
| A9: Provider coinbase reserve | 0 SigChecks are reserved now, since a coinbase runs no scripts ([SigChecks spec](https://upgradespecs.bitcoincashnode.org/2020-05-15-sigchecks/)). The output reserve is still 65,382 bytes, the most `SubmitSolution`'s 64 KiB coinbase field can carry; a larger coinbase goes to the node over RPC. Widening SV2 fields is separate work |
| A19: Test networks | Scalenet is a network (`sp2pool:` share addresses, its own share genesis, a 256 MB template floor), but has not been run here. Testnet4 is not added, and not decided against |
| A22: Simulation and tools | The swarm runs on BCHN 29.2.0. `pplns_compare` in `p2poolv2_sim` remains an offline four-miner model, and the JMeter plan stays on Bitcoin signet for CKPool on purpose. Nothing yet shows large-block capacity |
| A25: Share pool refuses unconfirmed parents | `shares/transaction_pool.rs` holds only transactions whose inputs spend confirmed share outputs, so never a chain of unconfirmed transactions. BCH dropped its unconfirmed-chain limit on 2021-05-15 ([upgrade](https://upgradespecs.bitcoincashnode.org/2021-05-15-upgrade/)); allow in-pool parents after measuring trade use |
| A27: API names | Metrics and Grafana labels no longer say `bitcoin` or `btc`. The JSON API keeps `miner_bitcoin_address`, `bitcoin_height` and the like: they are the share header's field names, read by the TUI, the dashboard and outside frontends, so renaming them is a separate API change |

## Measure on Bitcoin Cash

Nothing in this audit was measured on Bitcoin Cash mining devices or at
Bitcoin Cash block sizes; byte counts are arithmetic from the code. Each
test runs on chipnet unless it needs block sizes chipnet cannot reach.

| # | Question | Test | Decides | Status |
|---|---|---|---|---|
| M1 | How large a coinbase each mining device accepts | Add CPU-miner connections with distinct CashAddr usernames until the coinbase reaches 1 to 64 KB; at each size point a stock S19, an S21, an Avalon, a Bitaxe and a Pickaxe at it over SV1 and through the SV2 translator for 30 minutes; record coinb1 and coinb2 lengths and whether valid shares come back | Per-connection device limits | Open |
| M2 | What a large share body costs in relay | Three chipnet nodes in different places; pad share bodies to 10, 50, 100 and 200 KB for 6 hours each; measure propagation with `cargo share-latency` and the uncle rate | The body limit, and whether to carry a compact coinbase | Open |
| M3 | What full templates cost as blocks grow | Fill the mempool past 2 MB; measure `getblocktemplate` time, node memory and notify latency, then with `getblocktemplatelight`; repeat on scalenet at 32 and 256 MB | Light templates by default | BCHN regtest, 5.3 MB (2026-10-31): full template 14.9 MB in about 150 ms and a median 800 ms to the miner's notify, light 1.6 KB in about 1 ms and 275 ms; about 20 MB of node memory per full template held, 34 MB flat with light ones. Chipnet and scalenet open |
| M4 | Whether the Template Provider path follows ABLA | Mine chipnet blocks of about 1.5 MB until `ablastate.nextblocksizelimit` passes 2,000,000; check that a provider template larger than 2,000,000 - 65,624 bytes is taken | Confirms A4 live | Open |
| M5 | How often the window cap binds | Log the window length of every block the chipnet pool finds for two weeks | A11 | Answered in simulation: the cap binds only under about 0.015% of BCH with one-minute blocks (PR #119) |
| M6 | Uncle rate at 10-second shares on BCH | Three chipnet nodes on different continents for 48 hours; count uncles against shares | The share period | Open on chipnet; the network simulation puts it at 2.1% on 100 nodes |
| M7 | Today's live block size limit | `getblockheader <tip> true` on a mainnet and a chipnet BCHN, reading `ablastate` | The floor figures | Open |
| M8 | Share-chain reorg depths | Count reorg depths in the chipnet nodes' logs for a week | `BCH_COINBASE_MATURITY` and a share buyer's confirmations | Open |
| M9 | Coinbase room at full blocks | Fill the mempool past the limit with `-percentblockmaxsize=100` and a coinbase over 2 KB; the node must keep serving work and warn | A8 | Open |

## Done

S: changes share-chain rules. Rule changes went out under new protocol tags
on a fresh store.

| Item | Bitcoin assumption | Fix | S | PR |
|---|---|---|---|---|
| A1 | Coinbase capped at 11 payouts in 784 bytes, the tail drawn by lottery, from one OCEAN Bitcoin block (upstream discussion #668) | The hybrid payout: every payee direct within 1,000,000 bytes, 3,072 bytes for token shares, then a payout tree | yes | #30, #33 |
| A2 | 200 KiB share body holding the carried coinbase, 1 MiB frame | A coinbase of up to 1,000,000 bytes beside 200 KiB of share transactions, a 1.25 MiB message | yes | #33 |
| A3 | Hydrapool stopped the node past about 15 payees | The 784-byte check is gone | no | #33 |
| A4 | Template Provider templates checked against a fixed 32 MB floor | BCHN's `ablastate.nextblocksizelimit` for the parent, the floor only as a fallback | no | #33 |
| A5 | 32-bit `sizelimit` and `sigoplimit` | `u64` | no | #33 |
| A6 | Full template every poll, decoded several times, an unread decoded cache | Cache dropped; light templates with `template_source = "rpc_light"`, `submitblocklight` for their blocks | no | #33, #41 |
| A8 | An oversized final block stopped the node | Work is withdrawn until a template fits, and the operator is told to lower `-blockmaxsize` or `-percentblockmaxsize`; BCHN's template is never trimmed | no | #37 |
| A10 | Payouts limited to what `rust-bitcoin`'s `Address` holds | Token-aware CashAddr; P2PKH, P2SH20 and P2SH32 payout scripts; payees ordered by weight, ties by script bytes | yes | #33, #37 |
| A11 | Two-week window cap from upstream | Kept after the window cap study: it binds only in the smallest pools, and a longer cap only costs nodes | no | #119 |
| A12 | Depths sized by Bitcoin's 6,048-share maturity | Payout depth 121,063 and prune depth 242,126 from the 100-share maturity | yes | #33 |
| A13 | BIP141 sigop cost for share transactions | SigChecks: 3,000 a transaction, 1,452 a share | yes | #33 |
| A14 | 25-level coinbase branch | 32 levels for share proofs; merge-mining proofs keep 25 | yes | #33 |
| A15 | SAFA's 800-byte branch check | Covenant moved to merge-mining v2, bounded by the 10,000-byte unlocking limit | no | #33 |
| A16 | Namecoin's merge-mining slot layout | Merge-mining common format v2, agreed with Pickaxe (its PR #70) | yes | #33 |
| A17 | Integer difficulty weights | Exact work, 2^256 / (target + 1) | yes | #33 |
| A18 | BIP141 witness commitment accepted in a BCH coinbase | Rejected | yes | #33 |
| A19 | No scalenet | Scalenet added (testnet4 still open) | no | #33 |
| A20 | Base58 payout metric labels | CashAddr | no | #33 |
| A21 | Bitcoin signet data in a BCH payout test | BCH regtest data | no | #33 |
| A23 | Bitcoin signet `config.sample.toml` | A chipnet sample; `network` derived from `bitcoin_cash_network` | no | #33 |
| A24 | ECDSA-only share trades | BCH Schnorr, checked against BCHN's vectors | no | #33 |
| A26 | Miner statistics keyed by address spelling | Keyed by the decoded script | no | #33 |
| A27 | Metric names with `bitcoin` and `btc` | `found_blocks_total`, `found_block_time_seconds`, `payout_address` (JSON names still open) | no | #33 |
| Docs | Pages and comments that described Bitcoin or the old cap | Rewritten | no | #33 |

## Fine as is

Bitcoin-mode only, kept for upstream behind `Chain::Bitcoin`:
`PayoutSplit::EveryPayee`, the 16-level Bitcoin proof limit, the taproot
share owner and bech32m share address, the BIP141 share and witness
commitments, the `segwit`/`signet` template rules, `libbitcoinconsensus`,
and Bitcoin's 6,048-share maturity and fixed 120,960-share window.

Already correct for Bitcoin Cash:

- The `getblocktemplate` path takes `sizelimit` from BCHN, ABLA included,
  and checks the final block with the real coinbase
  ([mining.cpp](https://gitlab.com/bitcoin-cash-node/bitcoin-cash-node/-/raw/master/src/rpc/mining.cpp)).
- `sigoplimit` is read as BCHN's SigChecks limit (`sizelimit / 141`).
- First-seen: the share transaction pool refuses a transaction that spends an
  outpoint another pool transaction spends, and its priority rule only
  decides which non-conflicting transaction a full pool keeps.
- `MAX_POOL_TRANSACTION_SIZE` (50 KiB) is P2Pool's own limit on share
  transactions, not BCHN's 100 KB policy.
- CTOR, the BIP34 height push, the 100-byte scriptSig, coinbase version 2,
  sequence `0xfffffffe` and lock time height - 1, BIP320 version rolling,
  SIGHASH_FORKID, BCH Schnorr, CashAddr, dust thresholds (546, 540, 576),
  the 100-block maturity of payout outputs, ASERT, and the window of three
  blocks of work.
- The midstate-and-tail share proof is constant in size whatever the
  coinbase size.

## Ruled out

- **Share transactions should follow CTOR.** A share is not a BCH block;
  topological order inside a share is a share-chain choice.
- **A peer can steal value through a witness-program payout.** On BCH an
  input script must be push-only and leave exactly one stack item (since
  November 2018), so a bare `OP_0 <program>` output cannot be spent at all:
  a peer that names one burns only its own payout.
- **`MAX_AUXPOW_COINBASE_SIZE` (65,685) limits BCH parents.** It is the Rust
  proof's transport bound; with the payout tree, merge-mining shares stay
  far below it.
- **Unused rate-limit settings** (`max_workbase_per_second` and others) are
  real but not a Bitcoin assumption.
- **The TLA+ specifications model upstream rules only.** A specification
  gap, not a Bitcoin assumption in running code.
- **The 10-second share period and the 600-second ASERT half-life** were
  weighed for BCH in decisions 11 and 12 of [bch-plan.md](bch-plan.md).

See also [What P2Pool v1 knew about Bitcoin Cash](p2pool-v1-bch-lessons.md).
