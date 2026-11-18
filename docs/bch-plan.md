# P2Poolv2 for Bitcoin Cash: plan and decisions

The goal of the Bitcoin Cash fork, the decisions behind it, what is done
and what is still open. How each part works is in `docs/architecture/` and
the pages linked below; dated regtest and chipnet runs are in
[bch-design.md](bch-design.md), [bch-job-declaration.md](bch-job-declaration.md)
and the [changelog](../CHANGELOG.md). Read [AGENTS.md](../AGENTS.md) before
changing code.

## Goal

A peer-to-peer Bitcoin Cash mining pool built from P2Poolv2: miners
coordinate without an operator, and every block's coinbase pays the miners
directly. Pickaxe will run it for its users the way Gupaxx runs P2Pool for
Monero: the user's own node plus a local share chain, behind one setup
screen. Pool operators may use the same software too, while the aim is to
move most BCH hashrate to the share chain.

## Decisions so far

1. **Base.** Fork P2Poolv2 (Rust) and port the Bitcoin specifics to BCH.
   Take what is needed from P2Pool v1 for BCH
   ([jtoomim/p2pool](https://github.com/jtoomim/p2pool) and
   [BitcoinCash1/p2poolBCH](https://github.com/BitcoinCash1/p2poolBCH));
   their licence is settled in decision 13. What v1 taught is in
   [p2pool-v1-bch-lessons.md](p2pool-v1-bch-lessons.md).
2. **Miner protocol: Stratum V2 first.** Encryption with an authenticated
   server key, binary framing, standard and extended channels and Job
   Declaration, beside SV1 as ckpool added it. SV1 stays for firmware that
   cannot speak SV2 (Pickaxe already translates SV1 devices). Prior art:
   [average-gary/sv2-p2pool](https://github.com/average-gary/sv2-p2pool).
3. **Job Declaration: both modes**, Full-Template and Coinbase-Only. On BCH,
   declared jobs must keep canonical transaction order (CTOR), or they
   declare invalid blocks.
4. **Payouts: direct in the coinbase, no ledger, no custody.**
   - PPLNS: while a share is in the sliding window it earns a slice of
     every block found; then it slides out. Nothing is held or expired.
   - Payouts are spendable after 100 BCH blocks (BCH consensus).
   - Small miners' variance: first a main and a mini share chain; since
     issue #79 one chain with a difficulty band, weak shares and payout
     levels ([sharechain/small-miners-design.md](sharechain/small-miners-design.md)).
   - A keyless covenant payout tree in one coinbase output pays the payees
     that do not fit a share's coinbase limit (decision 14).
5. **Merge mining.** One coinbase commitment output whose hash covers every
   token the miner chose to merge-mine; each token's covenant checks a short
   proof up to it. The share chain fixes the payouts; each share's token
   choice is free within a size budget, which lets Pickaxe users choose
   their tokens. Format: [bch-auxpow.md](bch-auxpow.md).
6. **Templates from the miner's own node.** BCHN over local JSON-RPC
   (`getblocktemplate`, or `getblocktemplatelight` with `submitblocklight`),
   BCHD or Knuth, or a Stratum V2 Template Provider. Any local
   inter-process link works; Cap'n Proto is not required.
7. **Interface with Pickaxe: Stratum V2 is the boundary.** The P2Pool node
   acts as an SV2 pool and job declarator; Pickaxe connects the devices and
   the user's node, and can start the P2Pool node in the background.
   Pickaxe's default will be the local share chain plus the user's own node.
8. **Not adopted: DATUM-style designs**, where an operator sets the coinbase
   outputs and holds small balances ([compare-datum-sv2.md](compare-datum-sv2.md)).
9. **Donation and pool fee.** Every Bitcoin Cash node donates 1% of each
   block it finds (1.5% until 2026-10-24), from the coinbase, to the project
   address `bitcoincash:qze95tc2dqrnltvxfa5yhunwx5952f34fvw4g6365k` unless
   its operator changes it: `donation` sets the basis points and 0 opts out.
   The address is fixed in the share chain's rules, so a donation cannot be
   redirected. Public pool operators set their own `fee_address` and `fee`
   (1% if only the address is set), taken from what the donation leaves.
   Each is capped at 10% by the share chain (decision 13).
10. **Rust stays the implementation.** Ideas from P2Pool v1, CashStratum and
    other BCH software are reimplemented in Rust, never ported back as Python
    or C.
11. **PPLNS window: three BCH blocks of work behind a 100-share maturity**
    (PR #4, a share-chain rule; Bitcoin keeps upstream's fixed window).
    Details in [architecture/payout-distribution.md](architecture/payout-distribution.md),
    "Bitcoin Cash window".
    - **Rule.** Below a 100-share maturity skip, a coinbase pays the shares
      up to the first one that brings the window's work (shares, uncles and
      weak shares) to `K` = 3 times the difficulty of the paying share's own
      BCH header, at most 120,960 shares. The window spans about three of
      this pool's blocks at any pool size: with one-minute blocks about 60
      shares at 30% of BCH, 1,800 (5 hours) at 1%, and the full 120,960
      below about 0.015%.
    - **Difficulty source.** The paying share's header bits, never a node's
      own BCHN or config: BCH accepts a block only with the correct ASERT
      bits, and every node reads the same bits, so payouts stay identical.
    - **`K` = 3** (`BCH_PPLNS_WINDOW_BLOCKS`) is P2Pool v1's `SPREAD` on BCH,
      reimplemented. A share is paid by a Poisson number of blocks with mean
      `K`: with 3 it misses every block 5% of the time, against 13.5% with 2.
    - **Minimum difficulty.** A paying header at the network's proof-of-work
      limit (chipnet's 20-minute blocks, every regtest block) pays the capped
      120,960 shares.
    - **Maturity: 100 shares** (`BCH_COINBASE_MATURITY`), like BCH's
      100-block coinbase maturity: a block's payout can differ between forks
      only if they split more than 100 shares below the paying share, while
      uncles are at most 3 deep. P2Pool v1 skipped one share (its window
      starts at the paying share's grandparent). `PAYOUT_DEPTH` and pruning
      derive from it.
    - **Cap.** 14 days of shares; the window cap study (PR #119) found no
      reason to lengthen it.
    - **Share period: 10 seconds.** Uncles are paid 9/10 and headers relay
      compactly, so the short period costs little; where small miners need
      more credit, more weak shares a share beat a shorter period (PR #116).
    - **Finder bonus: not now.** It leaves incomes fair and variation
      unchanged, and costs a new consensus output; worth it only if block
      withholding shows up.
12. **Share difficulty: ASERT anchored at each share's parent** (PR #6, a
    share-chain rule). The rule is chosen by chain, never by Bitcoin Cash
    network, so mainnet and chipnet run the same rules. Upstream anchors at
    genesis, which counts every second since the genesis time: the chipnet
    chain restarted on 2026-10-19 began 1.55 million shares behind schedule
    and sat at its ceiling, and any outage does the same. BCH's own aserti3
    with the parent as anchor drops the time spent behind schedule. A later
    genesis time or an anchor at the first share fix one start but not
    outages. Details in [difficulty_adjustment/asert.md](difficulty_adjustment/asert.md).
13. **Scope (2026-10-22).**
    - **Mainnet readiness.** Mainnet, chipnet and regtest share one code
      path; only the network setting differs. Work finished and tested on
      chipnet counts as mainnet ready.
    - **Cap, per item.** The share chain caps the donation and the pool fee
      at 10% each, public pool hosts included; there is no combined cap. A
      10% donation and a 10% fee take 10% and 9% of the coinbase value.
    - **Share trading stays on Bitcoin Cash.** Shares trade through BCH
      covenants, every share output above the dust limit, no Lightning
      Network ([atomic-swap/bch-share-trading.md](atomic-swap/bch-share-trading.md)).
      A coinbase pays each window share's weight to the holder of its payout
      right (`payout-holder`).
    - **SAFA.** Experimental, tested in the BCH VM (`p2poolv2_safa`), with a
      test token on chipnet.
    - **Licence.** P2Pool v1's code is held under AGPL-3.0, so copying it is
      allowed. The fork is distributed under AGPL-3.0, and upstream P2Poolv2
      files keep their MIT/Apache-2.0 notices.
    - **Independent clients.** Stratum V2 and Job Declaration tests against
      other software and devices are run by the developers.
14. **Coinbase payouts: the hybrid payout** (PRs #30 and #33, a share-chain
    rule). It replaced the 11-payout cap of 2026-10-23, a Bitcoin assumption
    ([architecture/bch-hybrid-payouts.md](architecture/bch-hybrid-payouts.md);
    audit items A1 to A3).
    - Each window share's own donation and fee come off its own weight; the
      donation key and fee addresses are payees like any other.
    - A share whose merge-mining commitment is empty pays every PPLNS payee
      directly, within BCH's 1,000,000-byte `MAX_TX_SIZE` (about 29,400
      P2PKH payees). A share that merge-mines tokens keeps its coinbase
      within `T` = 3,072 bytes (79 P2PKH payees), so a token covenant can
      hash it; the payees past the limit go to one P2SH32 payout tree
      output. Payees are in canonical order, ascending weight, ties by
      locking bytecode; nobody is split.
    - The tree: keyless covenant nodes of 500 outputs, paid from the tree's
      own value at 1 to 3 sat/byte; tree payees below dust merge into the
      largest. Every node keeps the tree of each found block, broadcasts it
      after the coinbase's 100 confirmations and serves it at
      `/payout_tree`.
    - The share body carries a coinbase of up to 1,000,000 bytes beside
      200 KiB of share transactions; payout scripts must be P2PKH, P2SH20 or
      P2SH32; PPLNS weights are exact work. An SV2 extended job carries at
      most 65,535 bytes of coinbase on each side of the extranonce.
15. **Share transaction pool priority** (node policy, PR #30). Priority is
    the share units a transaction burns per byte, then arrival; a full pool
    evicts the lowest only for a transaction that outranks every one it
    evicts. Not upstream's planned depth-scaled fee: on Bitcoin Cash units
    carry no payout (the right does), so a depth scale would tax honest
    sellers without raising an attacker's cost.
16. **Each miner's own donation** (node policy, PR #33). A miner sets
    `donation=<percent>`, 0 to 10 in steps of 0.5, in its SV1 username or
    password or its SV2 identity; the node's `donation` applies otherwise
    and cannot override the miner. No share-chain rule changes: the share
    header already carries each share's donation. See
    [miner-options.md](miner-options.md).

## Done

| What | PRs |
|---|---|
| The port: BCHN templates, CashAddr payouts, the donation and fee, P2PKH share owners, BCH script checks of share transactions, SV1 and SV2 with both Job Declaration modes and Noise, the Template Provider client (`template_source = "sv2"`), merge-mining proofs | #1 |
| By-address `/miner` query of the PPLNS window | #2 |
| Template refresh right after a found block (the first chipnet block, 328,451, was found on 2026-10-18) | #3 |
| Window sized by BCH difficulty, 100-share maturity (decision 11) | #4 |
| ASERT anchored at the parent (decision 12) | #6 |
| Upstream syncs | #19, #88, #120, #134 |
| Share trading for BCH, with value and BCHN regtest runs (decision 13) | #20, #26, #60 |
| Terminal UI, the port of upstream's PDM | #29, #138 |
| Share-chain safety gaps from the upstream review | #32 |
| Hybrid payouts, share pool priority, each miner's own donation, the Bitcoin assumptions removed (decisions 14 to 16) | #30, #33 |
| tSAFA on chipnet; SAFA on one-minute blocks | #37, #141 |
| Light templates (`template_source = "rpc_light"`) and block announcements on the node's P2P port | #41, #42 |
| Payout outputs for standard SV2 Job Declaration clients (`0x5052`) | #47, #57 |
| Genesis share paid only as the last resort | #68 |
| Templates from BCHD | #73 |
| Large templates over SV2 (`0x4C54`), for Job Declaration and from a Template Provider | #63, #76 |
| One share chain, the band, payout levels and weak shares (issue #79) | #82, #83, #84 |
| Study #85's weak share constants | #98 |
| Weak shares that follow the asks | #118, #129 |
| A Template Provider behind Tor or I2P | #133 |
| One-minute blocks (CHIP-2025-03) | #136 |

## Open

- **Independent clients and devices.** Stock ASIC firmware, other Job
  Declaration clients, Noise against a real Template Provider, Pickaxe's
  provider and Knuth's have not run against this node. The real coinbase
  limit of stock Antminer firmware is audit measurement M1; device limits
  are to be handled per connection, never as a share-chain cap.
- **Share trading on chipnet.** Trades have run in process, on BCHN regtest
  and between live sim nodes, not yet on chipnet. Also open: an
  introspection template (a claim anyone can complete, a cancel the buyer
  alone signs) and selling part of a share's payout right (a right is
  whole).
- **The payout tree on chipnet.** It has run in a Rust BCH script machine
  with the 2025 VM limits and end to end on BCHN regtest, not on chipnet or
  at scale.
- **A signed payout binding over SV2.** A miner names its share address
  with `p2p=` in `user_identity`, as the SV1 password does; a signed binding
  a miner could prove is open (sv2-p2pool lists it as its ADR 0002).
- **SAFA.** Final token policy and a complete independent covenant review
  ([p2poolv2_safa](../p2poolv2_safa/README.md)).
- **Stratum V2 features not built**, because a multi-device client works
  without them: group channels and `SetGroupChannel`,
  `SetExtranoncePrefix`, sending `Reconnect` or `ChannelEndpointChanged` to
  miners, and more than 32 channels on one connection.
- **The terminal UI in the release archives** (`dist = false` for now).
- **Pool operators** running this software beside the share chain.
- The measurements and partly fixed items of the
  [Bitcoin assumptions audit](bch-btc-assumptions-audit.md).

## Sources

Stratum V2:

- Specification: [protocol overview](https://stratumprotocol.org/specification/03-protocol-overview/),
  [Mining Protocol](https://stratumprotocol.org/specification/05-mining-protocol/),
  [Job Declaration](https://stratumprotocol.org/specification/06-job-declaration-protocol/).
- Reference implementation: https://github.com/stratum-mining/stratum ,
  applications https://github.com/stratum-mining/sv2-apps , Umbrel UI
  https://github.com/stratum-mining/sv2-ui .
- Coinbase payouts extension: https://github.com/stratum-mining/sv2-spec/pull/203 ;
  closed alternatives https://github.com/stratum-mining/sv2-spec/pull/202 and
  https://github.com/stratum-mining/sv2-spec/pull/195 .
- Start9 packages: https://github.com/Start9Labs/stratum-v2-startos ,
  https://github.com/Start9Labs/stratum-v2-pool-startos .
- ckpool (pool, solo, proxy and Job Declaration server):
  https://github.com/ckolivas/ckpool .

Stratum V2 on BCH:

- bchn-sv2-bridge: https://github.com/danhaus93-ops/bchn-sv2-bridge ;
  LoneStrike BCH apps: https://github.com/danhaus93-ops/umbrel-bch-apps ;
  LoneStrike ckpool image: https://github.com/danhaus93-ops/lonestrike-ckpool .
- Knuth: https://github.com/k-nuth/kth (Stratum V2 from
  https://github.com/k-nuth/kth/pull/534 ; JSON-RPC:
  https://github.com/k-nuth/kth/blob/master/docs/json-rpc.md); package:
  https://github.com/BitcoinCash1/knuth-bch-startos ; site: https://kth.cash .
- SoloFury: https://solofury.com/blog/stratum-v2-bitcoin-cash-solo-mining/ .
- CashStratum (GPL-3.0), skaisser's BCH ckpool fork with its own C Stratum V2
  code including Job Declaration: https://github.com/cashstratum/cashstratum .
  Its SV2 certificate uses format version 1, which the specification forbids:
  https://github.com/cashstratum/cashstratum/issues/3 .
- ViaBTC BCH (SV1 endpoints):
  https://support.viabtc.com/hc/en-us/articles/7207458561679-BCH-Mining .
- Bitaxe ESP-Miner: https://github.com/bitaxeorg/ESP-Miner .

P2Pool and decentralized pools:

- P2Poolv2: https://github.com/p2poolv2/p2poolv2 ; docs
  https://github.com/p2poolv2/docs (`compare_datum_sv2.adoc`, `stratum.adoc`).
- sv2-p2pool: https://github.com/average-gary/sv2-p2pool ; design notes
  https://github.com/average-gary/wiki/tree/main/topics/sv2-p2pool-integration .
- Braidpool: https://github.com/braidpool/braidpool .
- P2Pool v1: https://github.com/jtoomim/p2pool ; for BCH
  https://github.com/BitcoinCash1/p2poolBCH (from
  https://github.com/frstrtr/p2poolBCH).
- Monero P2Pool: https://github.com/SChernykh/p2pool ; Gupaxx:
  https://github.com/Cyrix126/gupaxx .
- OCEAN DATUM: https://ocean.xyz/docs/datum .
- Pickaxe: https://github.com/CyberAshven/pickaxe-miner ; its architecture page with
  the decided mining destinations: https://github.com/CyberAshven/pickaxe-miner/pull/39 .

BCH pools, nodes and tokens:

- ASICseer pool: https://github.com/ASICseer/asicseer-pool ; BCHN
  `getblocktemplatelight`: https://gist.github.com/cculianu/89805c9cf525f314f46ea75e5b103d29 .
- SAFA: https://bitcoincashresearch.org/t/safas-a-sha256-asic-minable-automated-token-market/2123 ;
  BTOP: https://bitcoincashresearch.org/t/block-tops-btop-a-minable-cashtoken/1703 .
- Faster blocks: CHIP-2025-03
  https://bitcoincashresearch.org/t/chip-2025-03-faster-blocks-for-bitcoin-cash/1513 .
- Coinbase size RFC (2025-06, not a CHIP):
  https://bitcoincashresearch.org/t/rfc-limit-coinbase-tx-size/1602 .
- BCHN releases: https://download.bitcoincashnode.org/releases/ .
