# What P2Pool v1 knew about Bitcoin Cash

Status: study of [jtoomim/p2pool](https://github.com/jtoomim/p2pool) at
`ece15b0` (2022-06-16, the last commit) and the original
[p2pool/p2pool](https://github.com/p2pool/p2pool) by Forrest Voight at
`53c438b` (2018-09-19), compared with `feat/rust-only-tooling` at `0e269f05`
on 2026-10-23. Both v1 repositories were read, not run. Nothing on this page
was measured on a live pool; the dust figures are arithmetic and say so.
Implementation status for exact work and hybrid payouts was updated on
2026-10-24 for PR #33; the original comparison remains the study baseline.
The dated status below refreshes V1 to V23 from source at `7ebc0892`.

P2Poolv2 is better than P2Pool v1 almost everywhere: uncles instead of lost
orphans, ASERT instead of a 200-share retarget, Rust instead of Python 2, a
real store, share transactions. Its weak spots on Bitcoin Cash are the limits
it inherited from Bitcoin. P2Pool v1 ran on Bitcoin Cash from December 2017
(jtoomim commit `dcf9b5b`) with 32 MB blocks, CashAddr and BCH-specific
parameters. This page lists what v1 and jtoomim's fork learned there that
P2Poolv2's Bitcoin focus dropped, and what this fork should take back.

It complements the [Bitcoin assumptions audit](bch-btc-assumptions-audit.md):
where a lesson backs an audit item, the item is named (A1 to A27, M1 to M9)
instead of repeating it.

## In plain words

- **The window rule matches.** This fork's `K` = 3 is v1's `SPREAD` = 3, sized
  the same way from the paying share's own BCH header. v1 capped the window at
  3 days of shares; this fork caps it at 14 days.
- **Small direct payouts are paid as they are.** With
  the payout cap gone (the owner's decision), the smallest payout is one
  share's slice of a block: the reward divided by the window cap. That is
  about 2,583 sat today. It falls under BCHN's 546-sat dust threshold after
  the 2036 halving, or now if BCH moves to 1-minute blocks (258 sat). The
  implemented [hybrid payout rule](architecture/bch-hybrid-payouts.md) pays such
  outputs as they are, which BCH consensus rules allow, and v1 did too. The real
  limit is value: with 1-minute blocks one share pays less than it costs to
  spend after the 2028 halving. A 3-day cap keeps one share above dust in
  both cases at today's subsidy; v1 also let a small miner's node mine harder
  shares.
- **Miner difficulty is capped.** SV1 and SV2 jobs cap the session difficulty
  at the share difficulty, rounded down with a minimum of 1 (V8).
- **Found blocks are retried.** RPC submission retries transient failures and
  time-outs for two minutes. Operators can add other RPC nodes. Share-peer
  relay, BCH P2P submission and light submission remain open (V9, V10).
- **Version votes have machinery.** Shares carry rule versions and work-weighted
  votes. This build supports only version 1. A real second-version upgrade
  remains unverified (V17).
- **Startup checks the BCH chain.** The node checks the RPC chain name and a
  BCH-only checkpoint after syncing. Regtest has no checkpoint. This checks
  chain identity, rather than proving which software serves RPC (V15).
- **BCH chain eligibility remains open.** Plain BCH shares are not checked
  against the real BCH ancestry they claim. The token-only ASERT format check
  does not fill that gap (V16).
- **Already learned:** shares carry no BCH transactions (v1 got there in 2019
  after years of forwarding problems), block size comes from BCHN, payouts are
  carried as scripts, unknown usernames are refused, and stale shares earn as
  uncles.

## Status update: 2026-10-30

These statuses come from source at `7ebc0892`. "Fixed" means the requested
code path exists. It does not mean a fresh live-pool or device check passed.
The original study and recommendations below remain the baseline, except
for the corrected merkle and stratum comparisons.

Paths in the evidence column use `L/` for `p2poolv2_lib/src/`, `C/` for
`p2poolv2_config/src/`, `A/` for `p2poolv2_api/src/`, and `N/` for
`p2poolv2_node/src/`. Line numbers refer to that source snapshot.

| Item | Status | Source evidence and remaining work |
|---|---|---|
| V1: Three BCH blocks of work | Fixed | `L/accounting/payout/sharechain_pplns/pplns_window.rs:74,817-827` selects three blocks of work. `L/shares/validation/mod.rs:1510-1516` sizes it from the paying header. Header authenticity remains V16. |
| V2: Three-day cap | Open | `L/accounting/payout/sharechain_pplns/pplns_window.rs:50-53` still caps the production window at 120,960 shares, or 14 days at 10 seconds per share. |
| V3: One-share payout delay | Decided against | The adopted BCH maturity is 100 shares at `L/accounting/payout/sharechain_pplns/pplns_window.rs:128-134`. The historical one-share delay is not this fork's rule. |
| V4: Fractional edge share | Decided against for now | `L/accounting/payout/sharechain_pplns/pplns_window.rs:1433-1445` credits the whole final entry before stopping. The study below defers fractional credit. |
| V5: Expected-hash weights | Fixed | `L/accounting/payout/sharechain_pplns/pplns_window.rs:824-827,1217-1223` selects exact work for BCH payout weights. |
| V6: Harder shares for tiny payouts | Decided against in the adopted design | `L/accounting/payout/coinbase_budget.rs:17-29,192-193` pays direct sub-dust outputs unchanged and sends the overflow to the hybrid payout tree. It does not raise a miner's share difficulty. |
| V7: Variable share difficulty | Open | `L/shares/validation/mod.rs:1822-1838` requires the calculated pool target. Comparing variable difficulty with the present main/mini chains remains a study. |
| V8: Cap miner difficulty | Fixed | `L/stratum/difficulty_adjuster/mod.rs:61-71,423-429` computes and applies the ceiling. SV1 uses it at `L/stratum/server.rs:861-879`; SV2 uses it at `L/stratum/sv2/mining.rs:795-843`. The minimum session difficulty is 1. |
| V9: Robust found-block submission | Partly fixed | `L/stratum/block_submit.rs:48-52,150-180,197-212` supplies retries, time-outs and extra RPC nodes. BCH P2P and light submission remain open. |
| V10: Relay found blocks through share peers | Open | Extra RPC nodes are configured destinations, not share peers. `L/node/organise_worker.rs:825-837` records block-grade peer shares for effort accounting; it does not relay a full BCH block to BCHN. |
| V11: No BCH transaction list in shares | Fixed | `L/shares/share_block/mod.rs:490-510` carries share transactions, the coinbase and its branch. It does not carry the BCH template's transaction list. |
| V12: Compact coinbase transport | Open | The midstate proof exists at `L/shares/coinbase_proof.rs:6-20`. The full `bitcoin_coinbase` still travels at `L/shares/share_block/mod.rs:501-510`, and validators rebuild it at `L/shares/validation/mod.rs:1534-1553`. |
| V13: Deeper merkle branches | Fixed to the adopted bound | `L/shares/coinbase_proof.rs:36-59` permits 32 levels for BCH share proofs. Token merge-mining proofs retain their separate 25-level bound. |
| V14: Node-derived block size | Partly fixed | `L/stratum/work/block_template.rs:190-203` checks the final block against the node's size limit. `L/stratum/work/prepared_notify.rs:934-960` trims only share transactions. Recovery when the final BCH coinbase exceeds template room remains A8. |
| V15: Startup BCH chain check | Fixed | `N/preflight.rs:91-149` checks the chain name and BCH-only checkpoint. `C/lib.rs:310-321` supplies the mainnet and chipnet checkpoints. Regtest skips the checkpoint. RPC vendor identity is not proved. |
| V16: Work on the real BCH chain | Open | `L/shares/validation/mod.rs:839-854` checks BCH header target format only for token shares. Payout sizing still trusts the paying header's bits at `L/shares/validation/mod.rs:1510-1516`. The carried-header eligibility design is not implemented. |
| V17: Work-weighted version votes | Partly fixed | `L/shares/share_version.rs:14-63,225-276` implements 60% acceptance and a 95% builder switch. Only version 1 is supported. A second-rule activation has not been demonstrated here. |
| V18: Uncle rate and miner efficiency | Open | `A/api/endpoints/miner.rs:58-83` exposes miner share and uncle counts. `A/api/dashboard/history.rs:157-164` derives pool hashrate from confirmed shares alone. The requested pool uncle rate and miner efficiency are absent. |
| V19: Current payout amount | Open | `A/api/endpoints/miner.rs:74-83` exposes a payout fraction, without the requested satoshi amount. |
| V20: Inflated-reward warning | Open | `L/shares/validation/mod.rs:412-421,1534-1544` bounds the coinbase total by the money supply, then distributes it. It has no requested subsidy-plus-fees warning or counter. Local template fees must not become a share-chain validity rule. |
| V21: Zero-prefixed CashAddr regression | Open test | `C/cashaddr.rs:180-198,220-225` preserves decoded bytes. The existing tests beginning at `C/cashaddr.rs:233` do not cover the requested zero-prefixed hash. No current padding bug was found in this source review. |
| V22: Real coinbase and device evidence | Partly fixed | `L/accounting/payout/coinbase_budget.rs:50-122` removes the old cap. Current ASIC limits and large-pool propagation remain unmeasured; historical v1 observations do not establish them. |
| V23: Vardiff, workers and version rolling | Fixed | Vardiff is at `L/stratum/difficulty_adjuster/mod.rs:74-82`; worker parsing at `L/stratum/validate_username.rs:160-176`; version-rolling negotiation at `L/stratum/message_handlers/configure.rs:35-50`, using `C/lib.rs:20`. |

## Licences

forrestv/p2pool and jtoomim/p2pool are GPL-3.0 (`COPYING` in both
repositories). This fork is AGPL-3.0 under decision 13, and the owner holds an
AGPL fork of P2Pool v1. GPLv3 section 13 allows GPL-3.0 code to be combined
into an AGPL-3.0 work, so v1 logic may be ported into Rust, not only its
ideas.

Two conditions apply:

- The workspace metadata still says `MIT OR Apache-2.0` (`Cargo.toml:22`)
  until the owner's AGPL switch lands. Ported code waits for that switch.
- Ported code carries a notice naming the source file and commit and crediting
  Forrest Voight, Jonathan Toomim and the other contributors in the v1
  history, under GPL-3.0, used under AGPL-3.0.

The lessons table marks each recommendation **Port** (with the v1 file a port
would translate) or **Idea** (reimplement from the description). This page
ports no code. Decision 10 still applies: all code here is Rust.

## Parameters side by side

Line numbers are jtoomim's at `ece15b0` unless marked "forrestv".

| Parameter | P2Pool v1 on BCH | This fork on BCH |
|---|---|---|
| Share period | 60 s (`p2pool/networks/bitcoincash.py:10`); Bitcoin used 30 s | 10 s (decision 11) |
| Window | `SPREAD` = 3 blocks of work, from the share's own BCH header (`networks/bitcoincash.py:14`, `data.py:254-257`) | `K` = 3, the same sizing (`pplns_window.rs:74`) |
| Window cap | 4,320 shares, 3 days (`networks/bitcoincash.py:11-12`) | 120,960 shares, 14 days (`pplns_window.rs:51`); A11 |
| Payout delay | 1 share: the window starts at the grandparent (`data.py:254`; forrestv commit `76b297b`, 2013) | 100 shares (`pplns_window.rs:128`) |
| Share that crosses the window edge | Paid in part, so the window is exactly `K` blocks of work (`data.py:683-689`) | Paid whole |
| Weight | Expected hashes, 2^256 / (target + 1) (`data.py:672-674`) | Exact work since PR #33; A17 |
| Stale shares | Lost; counted for statistics only | Uncles at 9/10 weight, up to 3 per share |
| Finder bonus | 0.5% of the block (`data.py:268-270`) | None (decision 11) |
| Donation and fee | Donation by per-share weight, default 0 since `75b9fdc`; operator fee by crediting a random fraction of shares to the operator (`work.py:212-213`) | 1% donation by default and an operator fee, each capped at 10% (decisions 9, 13), taken off each share's own weight as v1 took its donation (`cuts-per-share`); the donation key and fee addresses are payees, with no separate outputs |
| Share difficulty | Per share, up to 30 times the pool minimum (`data.py:170`); minimum retargeted over 200 shares, at most 10% per share (`data.py:162-168`) | One per chain, ASERT anchored at the parent (decision 12); bits must equal it (`validation/mod.rs:1443`) |
| Lowest share difficulty | 1 (`MAX_TARGET`, `networks/bitcoincash.py:19`) | Mini chain ceiling 4.55 (`0x1c384bd7`) |
| Stratum difficulty | Per connection; the node's own choice never above the share difficulty (`bitcoin/stratum.py:86, 136-150`) | ckpool-style vardiff, capped at the share difficulty since PR #33, with a minimum of 1 (V8) |
| Block size | Hard-coded 32 MB (`networks/bitcoincash.py:36`) | From BCHN (A4, A5) |
| BCH transactions in shares | None since share version 34 (2019) | None |
| Coinbase in shares | Not carried; rebuilt by peers, tied by a SHA-256 midstate (`data.py:24-42`) | Carried whole (`share_block/mod.rs:404-413`); A2 |
| Payout outputs | Up to 4,000 (`data.py:285-286`) | Hybrid payout since PR #33: every payee direct within 1,000,000 bytes, or 3,072 bytes with tokens, then a payout tree (A1) |
| Dust | Small miners' share difficulty raised until one share pays 0.001 BCH a block (`work.py:330-339`); outputs of any size above zero (`data.py:334-336`) | Direct sub-dust payouts paid as they are; tree payees below dust merged into the largest (hybrid payout section 3.5) |

**Is `SPREAD` matched?** Yes. Both size the window as three blocks of work at
the difficulty in the paying share's own BCH header, which BCH checks when
the block is found. The differences are the cap, the payout delay and the
edge share, rows V2 to V4 below. Both now weight BCH shares by expected
hashes (V5).

## Dust: one share's slice of a block

With every miner paid directly, a miner with one share in the window gets
one share's slice. The slice is smallest when the window is at its cap: the
block reward divided by the cap. Above the pool size where the cap binds,
the slice grows with the pool's share of BCH hashrate.

Arithmetic, not measurement. Subsidy only (fees are small on BCH), before the
donation and fee cuts. Uncles earn 9/10 of these. BCHN's dust threshold for
a P2PKH output is 546 sat (`bch_script.rs:214`).

| Subsidy per block | 120,960-share cap, 10-min blocks | 120,960 cap, 1-min blocks | 25,920 cap (3 days), 10-min | 25,920 cap, 1-min |
|---|---|---|---|---|
| 3.125 BCH (now) | 2,583 sat | **258 sat** | 12,056 sat | 1,206 sat |
| 1.5625 BCH (about 2028) | 1,292 sat | **129 sat** | 6,028 sat | 603 sat |
| 0.78125 BCH (about 2032) | 646 sat | **65 sat** | 3,014 sat | **301 sat** |
| 0.390625 BCH (about 2036) | **323 sat** | **32 sat** | 1,507 sat | **151 sat** |

Bold is below 546 sat. The 1-minute columns assume CHIP-2025-03 is adopted
and cuts the subsidy per block tenfold, as it proposes; its outcome is open
(`bch-plan.md`, open questions).

The cap binds when the pool's share of BCH hashrate is below
`K` x block time / (share time x cap): 0.15% with 10-minute blocks and the
current cap, 0.015% with 1-minute blocks. Under 1-minute blocks and the
current cap, a pool below about 0.031% of BCH hashrate pays one-share miners
less than dust; at the 508 G difficulty the audit uses (about 3.6 EH/s,
estimated), that is any pool under about 1.1 PH/s, which a new pool is at
its start.

Below dust is not invalid. A coinbase output may hold any value under BCH
consensus rules; dust is relay policy for transactions that create outputs. The
[hybrid payout rule](architecture/bch-hybrid-payouts.md) (section 3.5)
pays sub-dust coinbase outputs as they are, as v1 did
(`data.py:334-336`); its estimate that this starts around 2036 is the
10-minute column. What matters for a miner is value: spending a P2PKH output
takes an input of about 148 bytes (`bch_script.rs:83`), about 148 sat at
BCHN's default 1 sat/byte. Under 1-minute blocks and the current cap, one
share pays 258 sat today and 129 sat after the 2028 halving, by when a
one-share payout is worth less than spending it.

v1's answer (V6) worked on the miner, not the window: a node whose miners
earned less than 0.001 BCH a block made their shares harder, so each share
paid at least that much. This fork has one share difficulty per chain, so it
would need V7's variable difficulty first. The simpler lever is the cap: with
A11's 3-day cap one share stays above dust with 10-minute blocks through the
2036 halving, and with 1-minute blocks until the 2032 halving.

## Lessons

"Rule" says whether the recommendation changes P2Pool's share-chain rules,
which needs the owner's agreement. No recommendation changes BCH consensus rules.
The table keeps the original comparison and its later PR #33 notes. Use the
dated status update above for current implementation status.

| # | Lesson from v1 | v1 source | This fork | Recommendation | Rule | Port or idea |
|---|---|---|---|---|---|---|
| V1 | Window of three BCH blocks of work, sized from the paying share's own header | `networks/bitcoincash.py:14`; `data.py:254-257`; forrestv `data.py:189-192` | **Have**: `pplns_window.rs:74`, `bitcoin_block_difficulty` (decision 11) | Keep | Already a rule | Done |
| V2 | Window capped at 3 days of shares | `networks/bitcoincash.py:11-12` | **Differently**: 14 days, `pplns_window.rs:51` | Adopt A11's 3-day BCH cap (25,920 shares); it also lifts the dust floor 4.7 times | Yes | Idea |
| V3 | Payouts delayed by exactly one share | `data.py:254`; forrestv `76b297b` | **Differently**: 100 shares, `pplns_window.rs:128` | Keep 100 for reorg safety; correct decision 11's "P2Pool v1 ran with no skip at all" | No | None |
| V4 | The share crossing the window edge is paid in part | `data.py:666-702` (`WeightsSkipList`) | **Differently**: paid whole | No action now: the overshoot is at most one share, 0.17% of a 600-share window (arithmetic). Revisit with V5 | Yes if adopted | Port: `data.py:666-702` |
| V5 | Shares weighted by expected hashes | `data.py:672-674` | **Have** since PR #33: BCH shares use exact work, 2^256 / (target + 1), for both the window and payout weights (A17, `75821ce5`) | Keep; V7 can use the same weights | Yes | Done |
| V6 | A node raises its miners' share difficulty until one share pays at least the dust threshold (0.001 BCH) per block | `work.py:330-339`; `bitcoin/networks/bitcoincash.py:32`; `data.py:170` | **Differently**: one difficulty per chain (`validation/mod.rs:1443`); the hybrid payout pays direct sub-dust outputs as they are and merges sub-dust tree payees into the largest | The approved hybrid payout decides small payouts (section 3.5). If v1's way is considered later, derive the floor from BCHN's dust threshold or the spending cost, not a fixed 0.001 BCH | Yes | Port: `work.py:330-339` |
| V7 | Heavier shares for big miners: a node makes at most about 1.67% of shares (`work.py:320-326`), miners may ask for harder shares (`work.py:197-202`), and the pool minimum counts shares at minimum work (`data.py:162-170, 888-898`), so heavy shares make the minimum easier for small miners | as cited; README line 125 | **Differently**: one difficulty per chain plus a mini chain (decision 4) | Compare with the mini chain in the BCHN simulation before choosing. One chain with variable difficulty keeps every miner in one window | Yes | Idea |
| V8 | The node never sets a miner's stratum difficulty above the share difficulty; only a miner's own fixed `+difficulty` can be | `bitcoin/stratum.py:80-86, 148`; `work.py:396-397` | **Lack**: session difficulty is bounded only by 1 and `maximum_difficulty` (2^32); `d=`, `th=`, `mining.suggest_difficulty` and `minimum_difficulty` act as floors (`difficulty_adjuster/mod.rs:354-382`). A hash that meets the share target but not the session difficulty is never submitted, so the miner is credited one share for more work | Cap each job's session difficulty at that job's share difficulty (`share_commitment.bits`) and warn when a configured floor is above it. Once ASERT has settled, vardiff's own choice stays about three times below the share difficulty (estimate: one submission per 3.3 s per connection against one share per 10 s for the whole pool), so the cap binds mainly on explicit settings. **Done on PR #33** (`share_difficulty_ceiling`, SV1 and SV2) | No | Idea |
| V9 | A found block is sent to the node over its P2P port and over RPC, each retried on errors up to 10 times, 10 s apart (`util/deferral.py:28-54`) | `bitcoin/helper.py:143-171`; `work.py:450-455` | **Lack**: one `submitblock` RPC, no retry (`submit.rs:365-385`; `bitcoindrpc/src/lib.rs:375-385`) | Retry transport errors and time-outs (not rejections) for about a minute. For large blocks, `submitblocklight` (A6). **Done on PR #33** (`stratum/block_submit.rs`): two minutes of retries, `duplicate` and `inconclusive` read as the node holding the block, and `found_block_submissions_total`. Not done: `submitblocklight` and the P2P path | No | Idea |
| V10 | Every node rebuilt a block found by a peer and submitted it to its own node | `node.py:282-300`; `data.py:629-633`; dropped in share version 34 (`5ba9eba`, merged `473afc4`) | **Lack**, as v1 since 2019: a peer's block-grade share is only counted (`organise_worker.rs:792`); `known-attacks.md`, block withholding | Optional found-block relay: the finder sends the whole block once to its share-chain peers, which submit it to their own BCHN. A node-to-node message, not a share rule. Helps home nodes with slow uplinks. **Partly done on PR #33**: an operator can list extra BCHN nodes in `[[submit_nodes]]`, which every found block also goes to. The relay to peers is not built | No | Idea |
| V11 | Shares carry no BCH transactions. v1 forwarded transactions between nodes and raised the limits at each block size step (new transactions per share 50 kB to 1 MB, `643768f`; remembered transactions 2.5 MB to 25 MB, a share-protocol hard fork, `e48be1f`; messages 1 MB to 8 MB to 32 MB, `e8d182a`, `72cbbb2`), then removed it all | forrestv `data.py:74`, forrestv `p2p.py:30, 33`; `p2p.py:30-33, 195-255`; `data.py:123-126` | **Have**: header, coinbase proof and coinbase merkle branch (`share_block/mod.rs:393-414`) | Keep. Never put BCH transactions or the BCH block size in share rules | No | None |
| V12 | The coinbase itself was never sent: peers rebuilt it from share data, and a SHA-256 midstate tied it to the proof of work | `data.py:24-42, 361-372, 552-557` | **Partly**: the midstate exists (`coinbase_proof.rs`) and validators rebuild the coinbase (`validation/mod.rs:1198-1267`), but the whole coinbase is still carried (A2) | Take A2's compact option, which the hybrid payout design also lists as a follow-up: carry only the scriptSig, extranonce and aux fields. v1 ran this way with up to 4,000 payees | Yes | Idea |
| V13 | The 16-level merkle branch limit was removed for BCH | `e2bad3e` | **Have**: BCH share proofs allow 32 levels (`coinbase_proof.rs:36-59`); token proofs keep their separate 25-level bound | Keep the adopted bound (A14) | Already a rule | Done |
| V14 | Block size hard-coded per network: each BCH increase needed a v1 release (8 MB to 32 MB, `df7409d`). The template was cut by position to fit | `networks/bitcoincash.py:36-37`; `data.py:200-203` | **Have**: limit from BCHN (A4, A5) | Keep. If a coinbase ever outgrows BCHN's 1,000-byte reserve (A8), never cut a template by position: BCH blocks are in canonical order (CTOR), where a child can sort before its parent, so a cut can keep a child without its parent. This is read from v1's code; it never happened live because BCH blocks were never full | No | None |
| V15 | At start, the node must have the 2017 BCH fork block and report the right chain | `bitcoin/networks/bitcoincash.py:17-21`; `ee0d7ca` | **Lack**: preflight checks only initial block download (`p2poolv2_node/src/preflight.rs:41-49`); `GetBlockchainInfo` reads one field (`bitcoindrpc/src/lib.rs:100-104`) | Check `getblockchaininfo` `chain` against `bitcoin_cash_network`, and that a BCH-only block after the 2018 BSV and 2020 eCash splits is in the node's chain; refuse to start otherwise. **Done on PR #33** (`preflight.rs`): the chain name before the sync wait, BCHN's checkpoint 661,648 (mainnet) or 115,252 (chipnet) after it | No | Idea |
| V16 | The best chain is chosen by work per real BCH time, using the BCH heights the shares built on, checked against the node's own bitcoind; a chain built on unknown BCH blocks scores lowest | `data.py:866-878`; `bitcoin/height_tracker.py:83-125` | **Lack**: "mine on the current chain" is not built (`sharechain/current-chain.md`) | Use as one input when that rule is designed | Fork choice; a rule once that design lands | Port: `data.py:866-878` |
| V17 | Rules change on a live chain by vote: shares carry `desired_version`; the new version may follow the old once 60% of work in the oldest tenth of the chain votes for it; nodes switch at 95% and then drop old peers | `data.py:491-510, 880-886, 947-956`; `work.py:304-318` | **Have** since the 2026-10-24 rule batch: each BCH share carries its version and desired version; the next version may follow once 60% of the work in the oldest tenth of 25,920 shares desires it, and nodes switch at 95% (`shares/share_version.rs`, reimplemented, not ported) | Ship the next rule change as version 2 | Yes | Done |
| V18 | Stale accounting: each share says whether its miner's last share was orphaned or dead on arrival; the node shows pool hashrate including stales and each miner's efficiency | `data.py:121`; `work.py:143-156, 354-358`; `web.py:94-184` | **Partly**: `/miner` counts a miner's uncles (`endpoints/miner.rs:70-73`); pool hashrate leaves uncles out; no pool uncle rate or efficiency | Add the pool uncle rate and each miner's efficiency (its uncle rate against the pool's) to the API and dashboard. M6 needs the first | No | Port: `web.py:164-165` (formula) |
| V19 | `/current_payouts`: every address's amount in the next block, in BCH | `web.py:210-212`; `data.py:934-939` | **Partly**: `/miner` gives one address's `payout_fraction` (`endpoints/miner.rs:78-81`) | Add the amount in satoshis (template value less donation and fee, times the fraction) | No | Idea |
| V20 | A share claiming more than subsidy plus fees was "naughty": nodes refused to build on it and mined low-difficulty shares to orphan it | `data.py:524-549, 605-611, 823-846`; `21b31d9`, `c9e55fe`, `10f324f` | **Lack**, as v1 since 2019: the coinbase value is taken from the coinbase (`validation/mod.rs:1198`). A share claiming too much is an invalid block if it wins, which costs the pool the block as withholding does | Log and count shares whose value exceeds the subsidy for their `bitcoin_height` plus the node's own template fees by a margin. Make it a rule only if it is seen | No as a log; yes as a rule | Idea |
| V21 | A CashAddr padding bug paid miners whose hash starts with a zero byte to a wrong address, fixed by share version 35 | `0825661`; `5174de1` | **Have**: payouts carried as script bytes (`share_block/mod.rs:126-162`); CashAddr decodes to bytes (`p2poolv2_config/src/cashaddr.rs`) | Add a test with a zero-prefixed hash | No | None |
| V22 | Coinbase size evidence: v1 paid up to 4,000 payees and warned near 226 outputs for Antminer S9 | `data.py:285-288`; `7d31661`, `7875cfe` | Covered by A1 (the cap goes) and M1 | Nothing new | None | None |
| V23 | Per-connection vardiff, `user.worker` names, BIP 310 version rolling | `bitcoin/stratum.py:52-66, 136-150`; `2bcdd45` | **Have**: ckpool-style vardiff, worker names, BIP 320 bits | None | No | None |

## v1 practices not to copy

- **Mining an empty block on a peer's header.** v1 switched to an empty block
  on any BCH header with valid proof of work that a P2Pool peer sent, before
  its own node had the block (`work.py:104-125`, `node.py:224-247`). BCHN is
  the authority: mine on what BCHN has validated, signalled by its ZMQ
  `hashblock`.
- **Paying the operator for unknown usernames.** A username v1 could not parse
  mined to the node's own address (`work.py:221-229`, README line 122). This
  fork refuses it (`stratum/validate_username.rs`).
- **A fee by redirecting shares.** v1's operator fee credited a random
  fraction of shares to the operator (`work.py:212-213`). This fork pays a fee
  output, capped with the donation at 10%.
- **Hard-coded block limits and transaction forwarding** (V11, V14).

## Why P2Pool v1 faded

From the v1 history; where a cause is inferred rather than stated, it says
so.

1. **CPU and memory.** v1 is Python 2 on Twisted. jtoomim's README recommends
   PyPy, and much of his work is speed and memory: `--bench` (`9692d6e`),
   faster packing (`f223000`, `d2c6edc`), a txid cache (`1161221`), "35 ms
   faster" job building (`3308181`), smaller memory (`b67f27a`, `5d5a6b4`),
   and branches named `jtoomim_performance`, `lowmem` and `memory`. Job
   building grows with block size; this fork's version of that cost is A6.
2. **Transaction forwarding.** Missing transactions dropped peers
   (`p2p.py:336-338`), shares could not be sent without them (`cd0b24a`), and
   raising the limits as blocks grew took a share-protocol hard fork
   (`e48be1f`). Removed in 2019.
3. **Splits.** Bitcoin's v1 network split into jtoomim's and forrestv's
   chains in 2017 (README lines 76-95, `1f5448b`, `fe9179e`), each with less
   hashrate.
4. **Too little hashrate.** In February 2018 jtoomim's Bitcoin chain found a
   block about every 25 days and forrestv's every 108 (README line 95).
   Variance that long drives miners away, which lowers hashrate further
   (inferred).
5. **Lost stale shares.** v1 has no uncles; an orphaned share earned nothing.
6. **Device quirks.** The S9 coinbase output limit, a Braiins OS version
   rolling bug (`335369c`), rental services (`af1f8b1`).
7. **Maintainers.** jtoomim set the author fee to 0 "as forrestv has been
   inactive for a few years" (`75b9fdc`, 2019); jtoomim's last merge is from
   2022. His code still mines BCH as a solo pool: block 841,340 (2024-04-15)
   carries "Toomim Bros solo: http://toom.im (mined with BCHN and p2pool)"
   (third-party explorer:
   [blockchain.com](https://www.blockchain.com/explorer/blocks/bch/841340)).

P2Poolv2 already answers 1, 2 and 5. Lessons V17 (live upgrades) and V7 or
the mini chain (small miners' variance) address 3 and 4.

## Top recommendations

1. **Cap stratum difficulty at the share difficulty (V8).** A miner set above
   the share difficulty throws away valid shares and is paid for fewer hashes
   than it did; v1's node never set one that high. Node policy, Idea.
   **Done on PR #33.**
2. **Submit found blocks robustly (V9, then V10).** A block is worth 3.125 BCH
   and one failed RPC call loses it; v1 retried over two paths, and before
   2019 every peer re-sent it. Node policy, Idea. **Retries and extra BCHN
   nodes done on PR #33**; the relay to peers is not.
3. **Settle small payouts inside the hybrid payout design (V6, V2, V7).**
   With every miner paid directly, the smallest payout is the reward divided
   by the window cap; with 1-minute blocks that is 258 sat now and less than
   its spending cost after 2028. The 3-day cap keeps it at 1,206 sat now, and
   v1's harder shares for small miners are the fallback. Share-chain rule,
   Port possible from `work.py:330-339`.
4. **Add a share-version vote before mainnet (V17).** Every rule change so far
   restarted the chain, which on mainnet would reset the miners' window; v1
   changed rules on a live chain by vote. Done in the 2026-10-24 rule batch
   (`shares/share_version.rs`, the idea reimplemented in Rust): later rule
   changes ship as version 2 and on.
5. **Check at start that the node is BCHN on the right network (V15).**
   Pointed at a Bitcoin, BSV or eCash node, the pool would mine for the wrong
   chain; v1 checked for a BCH-only block first. Node policy, Idea.
   **Done on PR #33.**

Also strengthened by v1's history, already in the audit: carry a compact
coinbase (V12, A2), the 3-day cap (V2, A11), weights by work (V5, A17) and
the merkle branch limit (V13, A14).

## Corrections to other pages

- `bch-plan.md`, decision 11, maturity: "P2Pool v1 ran with no skip at all".
  v1 skipped one share: its window starts at the paying share's grandparent
  (`data.py:254`), a delay forrestv added in 2013 (`76b297b`, "changed new
  share to delay payouts 1 share"). The conclusion (keep 100) is unaffected.

## Sources

- jtoomim/p2pool, master at
  [`ece15b03`](https://github.com/jtoomim/p2pool/tree/ece15b03fc2367d02e730b2b85b666217fa8985e):
  `p2pool/networks/bitcoincash.py`, `p2pool/bitcoin/networks/bitcoincash.py`,
  `p2pool/data.py`, `p2pool/work.py`, `p2pool/node.py`, `p2pool/p2p.py`,
  `p2pool/web.py`, `p2pool/bitcoin/stratum.py`, `p2pool/bitcoin/helper.py`,
  `p2pool/bitcoin/height_tracker.py`, `README.md`, and its commit history
  (commits named by short hash above, for example
  [`5ba9eba`](https://github.com/jtoomim/p2pool/commit/5ba9eba)).
- p2pool/p2pool (Forrest Voight), master at
  [`53c438bb`](https://github.com/p2pool/p2pool/tree/53c438bbada06b9d4a9a465bc13f7694a7a322b7):
  `p2pool/data.py`, `p2pool/p2p.py`, `p2pool/networks/bitcoin.py`, and commit
  [`76b297b`](https://github.com/p2pool/p2pool/commit/76b297b).
- This fork: the files cited in the tables, at `0e269f05`;
  [Bitcoin assumptions audit](bch-btc-assumptions-audit.md);
  [Hybrid payouts on Bitcoin Cash](architecture/bch-hybrid-payouts.md);
  [Plan and decisions](bch-plan.md); [Known attacks](known-attacks.md).
