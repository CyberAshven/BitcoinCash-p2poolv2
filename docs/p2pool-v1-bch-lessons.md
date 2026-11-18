# What P2Pool v1 knew about Bitcoin Cash

A study of [jtoomim/p2pool](https://github.com/jtoomim/p2pool) at `ece15b0`
(2022-06-16, the last commit) and the original
[p2pool/p2pool](https://github.com/p2pool/p2pool) by Forrest Voight at
`53c438b` (2018-09-19), against this fork. Both v1 repositories were read,
not run. Where a lesson backs an item of the
[Bitcoin assumptions audit](bch-btc-assumptions-audit.md), the item is named
(A1 to A27, M1 to M9).

P2Poolv2 is better than P2Pool v1 almost everywhere: uncles instead of lost
orphans, ASERT instead of a 200-share retarget, Rust instead of Python 2, a
real store, share transactions. Its weak spots on Bitcoin Cash were the
limits it inherited from Bitcoin. P2Pool v1 ran on Bitcoin Cash from
December 2017 (jtoomim commit `dcf9b5b`) with 32 MB blocks, CashAddr and
BCH-specific parameters; this page lists what it learned there.

## Licences

forrestv/p2pool and jtoomim/p2pool are GPL-3.0. This fork is AGPL-3.0
(decision 13 in [bch-plan.md](bch-plan.md)), and GPLv3 section 13 allows
GPL-3.0 code to be combined into an AGPL-3.0 work, so v1 logic may be ported
into Rust, not only its ideas. Ported code waits until the workspace
metadata (`Cargo.toml`, still `MIT OR Apache-2.0`) switches to AGPL, and
carries a notice naming the
source file and commit and crediting Forrest Voight, Jonathan Toomim and the
other v1 contributors. The lessons taken below were reimplemented in Rust,
not ported (decision 10).

## Parameters side by side

Line numbers are jtoomim's at `ece15b0` unless marked "forrestv".

| Parameter | P2Pool v1 on BCH | This fork on BCH |
|---|---|---|
| Share period | 60 s (`p2pool/networks/bitcoincash.py:10`) | 10 s (decision 11) |
| Window | `SPREAD` = 3 blocks of work, from the share's own BCH header (`networks/bitcoincash.py:14`, `data.py:254-257`) | `K` = 3, the same sizing |
| Window cap | 4,320 shares, 3 days (`networks/bitcoincash.py:11-12`) | 120,960 shares, 14 days |
| Payout delay | 1 share: the window starts at the grandparent (`data.py:254`; forrestv `76b297b`) | 100 shares |
| Share that crosses the window edge | Paid in part (`data.py:683-689`) | Paid whole |
| Weight | Expected hashes, 2^256 / (target + 1) (`data.py:672-674`) | Exact work, the same |
| Stale shares | Lost | Uncles at 9/10 weight, up to 3 per share |
| Finder bonus | 0.5% of the block (`data.py:268-270`) | None (decision 11) |
| Donation and fee | Donation by per-share weight; operator fee by crediting a random fraction of shares to the operator (`work.py:212-213`) | 1% donation and an operator fee, each capped at 10%, taken off each share's own weight (decisions 9, 13) |
| Share difficulty | Per share, up to 30 times the pool minimum (`data.py:170`); minimum retargeted over 200 shares (`data.py:162-168`) | One chain, ASERT anchored at the parent (decision 12), heavier shares inside the difficulty band |
| Lowest share difficulty | 1 (`networks/bitcoincash.py:19`) | The chain's ceiling, about 4.55 (`0x1c384bd7`) |
| Stratum difficulty | Never above the share difficulty (`bitcoin/stratum.py:86, 136-150`) | ckpool-style vardiff, capped at the share difficulty |
| Block size | Hard-coded 32 MB (`networks/bitcoincash.py:36`) | From BCHN |
| BCH transactions in shares | None since share version 34 (2019) | None |
| Coinbase in shares | Not carried; rebuilt by peers, tied by a SHA-256 midstate (`data.py:24-42`) | Carried, but relayed without its outputs once they pass 50,000 bytes |
| Payout outputs | Up to 4,000 (`data.py:285-286`) | Every payee direct within 1,000,000 bytes, or 3,072 bytes with tokens, then a payout tree (decision 14) |
| Small payouts | A small miner's share difficulty raised until one share pays 0.001 BCH a block (`work.py:330-339`) | Payout levels: a slice below its miner's level, at least 1,092 sat, is paid by a draw within the block |

## Lessons

"Rule" says whether the lesson changes P2Pool's share-chain rules. None
changes BCH consensus rules.

| # | Lesson from v1 | v1 source | This fork | Rule |
|---|---|---|---|---|
| V1 | Window of three BCH blocks of work, sized from the paying share's own header | `networks/bitcoincash.py:14`; `data.py:254-257`; forrestv `data.py:189-192` | **Done** (decision 11, PR #4) | yes |
| V2 | Window capped at 3 days of shares | `networks/bitcoincash.py:11-12` | **Decided against**: the 14-day cap stays (PR #119); with one-minute blocks it binds only under about 0.015% of BCH | yes |
| V3 | Payouts delayed by exactly one share | `data.py:254`; forrestv `76b297b` | **Decided against**: 100 shares, for reorg safety | yes |
| V4 | The share crossing the window edge is paid in part | `data.py:666-702` (`WeightsSkipList`) | **Not now**: paid whole; the overshoot is at most one share | yes |
| V5 | Shares weighted by expected hashes | `data.py:672-674` | **Done**: exact work on BCH (A17, PR #33) | yes |
| V6 | A node raises its miners' share difficulty until one share pays at least 0.001 BCH a block | `work.py:330-339`; `data.py:170` | **Done differently**: payout levels (PRs #83, #102) | yes |
| V7 | Heavier shares for big miners, and a pool minimum that counts shares at minimum work, so the minimum is easier for small miners | `work.py:197-202, 320-326`; `data.py:162-170, 888-898` | **Done differently**: one share chain with a difficulty band (PR #82) | yes |
| V8 | The node never sets a miner's stratum difficulty above the share difficulty | `bitcoin/stratum.py:80-86, 148`; `work.py:396-397` | **Done**: the session difficulty is capped at the job's share difficulty, SV1 and SV2, minimum 1 (PR #33) | no |
| V9 | A found block goes to the node over its P2P port and over RPC, each retried | `bitcoin/helper.py:143-171`; `util/deferral.py:28-54` | **Partly**: RPC retries for two minutes, extra BCHN nodes in `[[submit_nodes]]`, `submitblocklight` for light templates (PRs #33, #41). **Open**: submission over the node's P2P port | no |
| V10 | Every node rebuilt a block found by a peer and submitted it to its own node (dropped in share version 34) | `node.py:282-300`; `data.py:629-633` | **Open**: a peer's block-grade share is only counted. An optional relay of the found block to share-chain peers, a node message, would help home nodes with slow uplinks | no |
| V11 | Shares carry no BCH transactions. v1 forwarded them and raised the limits at each block size step, a share-protocol hard fork each time, then removed it all | `p2p.py:30-33, 195-255`; `data.py:123-126` | **Have**. Never put BCH transactions or the BCH block size in share rules | no |
| V12 | The coinbase itself was never sent: peers rebuilt it, and a SHA-256 midstate tied it to the proof of work | `data.py:24-42, 361-372, 552-557` | **Done for relay**: a share whose coinbase outputs pass 50,000 bytes is relayed without them, and the receiver rebuilds them from its window (PR #131) | no |
| V13 | The 16-level merkle branch limit was removed for BCH | `e2bad3e` | **Done**: 32 levels for share proofs (A14, PR #33) | yes |
| V14 | Block size hard-coded per network, the template cut by position to fit | `networks/bitcoincash.py:36-37`; `data.py:200-203` | **Have**: the limit comes from BCHN. Never cut a template by position: in CTOR a child can sort before its parent | no |
| V15 | At start, the node must have the 2017 BCH fork block and report the right chain | `bitcoin/networks/bitcoincash.py:17-21`; `ee0d7ca` | **Done**: the chain name and a BCH-only checkpoint (mainnet 661,648, chipnet 115,252) are checked at start (PR #33); regtest has none | no |
| V16 | The best chain is chosen by work per real BCH time, using the BCH heights the shares built on; a chain built on unknown BCH blocks scores lowest | `data.py:866-878`; `bitcoin/height_tracker.py:83-125` | **Open**: plain BCH shares are not checked against the real BCH ancestry they claim | a rule once designed |
| V17 | Rules change on a live chain by vote: 60% of work in the oldest tenth for the new version to follow, nodes switch at 95% | `data.py:491-510, 880-886, 947-956`; `work.py:304-318` | **Done** (`shares/share_version.rs`, PR #33); a second version has not yet activated on a live chain | yes |
| V18 | Each share says whether its miner's last share went stale; the node shows pool hashrate with stales and each miner's efficiency | `data.py:121`; `work.py:143-156, 354-358`; `web.py:94-184` | **Open**: `/miner` counts a miner's uncles; no pool uncle rate or efficiency. M6 needs the first | no |
| V19 | `/current_payouts`: every address's amount in the next block | `web.py:210-212`; `data.py:934-939` | **Open**: `/miner` gives a fraction, not satoshis | no |
| V20 | A share claiming more than subsidy plus fees was refused and orphaned | `data.py:524-549, 605-611, 823-846` | **Open**: log and count such shares; make it a rule only if it is seen | no as a log |
| V21 | A CashAddr padding bug paid miners whose hash starts with a zero byte to a wrong address | `0825661`; `5174de1` | **Done**: payouts are script bytes, and a test keeps the leading zero byte | no |
| V22 | Coinbase size evidence: up to 4,000 payees, a warning near 226 outputs for the Antminer S9 | `data.py:285-288`; `7d31661`, `7875cfe` | Covered by A1 (the cap went) and M1 | no |
| V23 | Per-connection vardiff, `user.worker` names, version rolling | `bitcoin/stratum.py:52-66, 136-150`; `2bcdd45` | **Have** | no |

## v1 practices not to copy

- **Mining an empty block on a peer's header.** v1 switched to an empty block
  on any BCH header a P2Pool peer sent, before its own node had the block
  (`work.py:104-125`, `node.py:224-247`). BCHN is the authority: mine on
  what BCHN has validated, signalled by its ZMQ `hashblock` or its block
  `inv` on the P2P port.
- **Paying the operator for unknown usernames.** A username v1 could not
  parse mined to the node's own address (`work.py:221-229`). This fork
  refuses it.
- **A fee by redirecting shares** (`work.py:212-213`). This fork pays the fee
  as a payee, capped at 10%.
- **Hard-coded block limits and transaction forwarding** (V11, V14).

## Why P2Pool v1 faded

1. **CPU and memory.** v1 is Python 2 on Twisted; much of jtoomim's work is
   speed and memory (`9692d6e`, `f223000`, `1161221`, `3308181`, `b67f27a`),
   and job building grows with block size.
2. **Transaction forwarding.** Missing transactions dropped peers
   (`p2p.py:336-338`), and raising the limits as blocks grew took a
   share-protocol hard fork (`e48be1f`). Removed in 2019.
3. **Splits.** Bitcoin's v1 network split into jtoomim's and forrestv's
   chains in 2017, each with less hashrate.
4. **Too little hashrate.** In February 2018 jtoomim's Bitcoin chain found a
   block about every 25 days and forrestv's every 108 (README line 95).
   Variance that long drives miners away (inferred).
5. **Lost stale shares.** v1 has no uncles.
6. **Device quirks.** The S9 coinbase output limit, a Braiins OS version
   rolling bug (`335369c`), rental services (`af1f8b1`).
7. **Maintainers.** jtoomim's last merge is from 2022. His code still mines
   BCH as a solo pool: block 841,340 (2024-04-15) carries "Toomim Bros solo"
   (third-party explorer:
   [blockchain.com](https://www.blockchain.com/explorer/blocks/bch/841340)).

P2Poolv2 answers 1, 2 and 5; the share-version vote (V17) and the small
miners' design (V6, V7) address 3 and 4.

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
- This fork: [Bitcoin assumptions audit](bch-btc-assumptions-audit.md);
  [Hybrid payouts on Bitcoin Cash](architecture/bch-hybrid-payouts.md);
  [Share chain redesign](sharechain/small-miners-design.md);
  [Plan and decisions](bch-plan.md); [Known attacks](known-attacks.md).
