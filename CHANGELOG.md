# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Share trading for Bitcoin Cash (decision 13), and a trade now carries
value: nodes relay and mine share transactions, and a coinbase pays each
window share's weight to whoever holds the share's payout right. This
changes the share chain's rules, not Bitcoin Cash consensus: nodes start
from a new store, and Bitcoin Cash protocol strings carry `trade-htlc` and
then `payout-holder`.

A Bitcoin Cash coinbase now fits stock ASIC firmware: at most 11 payout
outputs in a coinbase of at most 784 bytes, the tail of small payees paid by
a draw weighted by their PPLNS weight. Another share-chain rule change:
protocol strings carry `payout-cap11` after `payout-holder`. The share
transaction pool ranks transactions by the share units they burn per byte.

Share-chain safety fixes from a review against upstream's design documents
(2026-10-23). An uncle may have only one nephew on a chain, header sync checks
share timestamps, a block spending an output a node never fetched is stored
instead of stalling the node, the reorg depth limit is `PAYOUT_DEPTH`, the
pending block buffer is bounded and stratum difficulty can no longer
overflow. The uncle rule and the header timestamp checks change the share
chain's rules on Bitcoin Cash and Bitcoin, not Bitcoin Cash consensus:
Bitcoin Cash protocol strings carry `uncle-once` after `payout-cap11`, and
the store format changes, so nodes start from a new store.

The approved Bitcoin Cash share-chain rule batch (owner's OK, 2026-10-23),
one flag day on a fresh share chain. It changes P2Pool's own share-chain
rules and the coinbase P2Pool builds, not Bitcoin Cash consensus. Builder A's
part: merge-mining common format v2 with the ASERT header check.

Synced with upstream P2Poolv2 through `61810c56` (its v0.16.2). The fork
keeps version `0.0.1-alpha`. The sync itself leaves Bitcoin Cash genesis
shares, protocol strings, the PPLNS window and maturity, the ASERT parent
anchor and the donation and fee cap as they were.

### Added

- Token-aware CashAddr payout addresses (types 2 and 3, `z...` and `r...`,
  from CashTokens) are accepted wherever a payout address is read: the
  Stratum username and SV2 identity, `[stratum]` addresses and
  `bch-share-address` (A10 a in `docs/bch-btc-assumptions-audit.md`). They
  lock with the same script as their plain form, so the payout output is
  unchanged; P2SH32 stays refused.
- A Bitcoin Cash node refuses to start unless `[bitcoinrpc]` is BCHN on
  `bitcoin_cash_network` (V15 in `docs/p2pool-v1-bch-lessons.md`). Before
  waiting for the sync, `getblockchaininfo` must report BCHN's chain name
  (`main`, `chip`, `regtest`); after it, the node's block at a height only
  Bitcoin Cash has must be BCHN's checkpoint (661,648 on mainnet, after the
  Bitcoin SV and eCash splits; 115,252 on chipnet, where it forked from
  testnet4). A Bitcoin, Bitcoin SV or eCash node, or BCHN on testnet4, is
  refused with an error saying what was found.

- A landing page in `website/`, ported from upstream's
  `p2poolv2/website` (MIT, whose notice it keeps): what the pool is, mining
  over Stratum V2 first with Stratum V1 supported, running a node, the
  dashboards and terminal UI, and what has been measured on chipnet and
  regtest. HTML and CSS only: Bootstrap's script is gone and the menu is a
  `<details>` dropdown. It wears the dashboard's stylesheets, Ubuntu font
  and Bitcoin Cash logo, now with their licence files beside them, and
  `style_guard` checks it. The `Docs` workflow stages it with the book under
  `docs/` and publishes both when `DOCS_PAGES` is `true`.
- `p2poolv2_tui`, a terminal UI for a Bitcoin Cash node and its BCHN,
  ported from upstream's PDM (`p2poolv2/pdm`, AGPL-3.0-or-later, which this
  crate keeps). BCHN status over JSON-RPC (chain, peers, mempool) replaces
  Bitcoin Core's; the node's status reads `/network`, `/chain_info`,
  `/hashrate`, `/shares`, `/miners`, `/miner`, `/found_blocks`, `/peers`
  and the `/ws` live feed; the config editor keeps the file's comments,
  never writes environment overrides into it and saves only what the node's
  own parser accepts. No Lightning. One node config points it at
  everything, and its `bitcoin_cash_network` is the only switch between
  mainnet and chipnet. Without its default `client` feature the crate is
  the views over plain data, for other ratatui programs. See
  `p2poolv2_tui/README.md`.
- An Umbrel community app store in `packaging/umbrel`, ported from
  upstream's `p2poolv2/umbrel-app` (Apache-2.0). `bch-p2pool` depends on
  the official store's Bitcoin Cash Node app (mainnet) and takes BCHN's RPC
  login and ZMQ endpoint from its exports; `bch-p2pool-external` mines on a
  BCHN elsewhere, chipnet by default, set on umbrelOS 2.0's settings page
  or in the app's `p2pool.toml`. Their ports stay clear of Umbrel's Bitcoin
  apps and the Bitcoin P2Poolv2 app. `p2poolv2_config/tests/umbrel_apps.rs`
  loads each app's config rendered from its defaults. See
  `packaging/umbrel/README.md`.
- Bitcoin Cash share trading: a miner sells share units for BCH with one
  hash time locked contract behind P2SH32 on each chain and no Lightning.
  The seller locks shares, the buyer locks BCH under the same secret hash,
  the seller's BCH claim reveals the secret and the buyer claims the shares
  with it; refunds use height locks. See
  `docs/atomic-swap/bch-share-trading.md`.
- `p2poolv2_cli bch-trade` (`contract`, `inspect`, `secret-hash`, `lock`,
  `claim`, `refund`, `extract-secret`) builds and signs both legs offline,
  with a WIF key read from a file or a hidden prompt and never generated.
- Bitcoin Cash share transaction pool: a node checks signed share
  transactions against the confirmed share chain under every share-chain
  rule, keeps at most 1,000 of them and 4 MiB (each at most 50 KiB, for 3
  hours, lowest priority evicted first, spending only confirmed unspent
  outputs),
  relays them to its peers as `Transaction` requests (peer transactions
  admitted at most 10 a second, bursts of 100), and mines them in the shares
  it builds, within 99 transactions and 64 KiB a share, with the share
  commitment covering them. A transaction leaves the pool once confirmed and
  returns if a reorg takes it off the confirmed chain. Bitcoin nodes keep
  building shares with the coinbase alone.
- `POST /share_transaction` (behind the API login) and
  `p2poolv2_cli transactions submit` hand a signed share transaction to a
  node, for example a `bch-trade` lock or claim.
- Nodes delete block bodies more than `PRUNE_DEPTH + PRUNE_INTERVAL`
  (254,376) shares below the confirmed tip, in a background task every 6
  hours while the node is current (from upstream). Headers and their merkle
  branches are kept, so pruned nodes still serve header sync from genesis,
  and the candidate chain refuses a reorg whose fork point is more than
  `PAYOUT_DEPTH` below the confirmed tip (see Changed). On Bitcoin Cash the PPLNS
  window reads only headers and reaches at most 121,063 shares deep (100
  maturity, the 120,960-share cap and uncle depth 3), inside `PAYOUT_DEPTH`
  (127,008); production builds assert this at compile time. The API's
  `/share?full=true` marks a pruned share with `"pruned": true`.
- Built-in DNS seeds as a fallback when no peer is connected, with a
  `network.dns_seed` option (from upstream; only Bitcoin testnet4 has seeds).
  They are always off on Bitcoin Cash, whatever `dns_seed` says: the seeds
  are Bitcoin P2Poolv2 nodes and there are no Bitcoin Cash seeds yet. A
  Bitcoin Cash node finds peers through `dial_peers` only.
- The `pplns-compare` feature of `p2poolv2_sim` builds `pplns_compare` with
  production window sizes; the `sim` feature now shortens them.
- `/miners` serves the miners on this node with their workers as JSON, paged
  and built like `/dashboard/miners`, and `/network` names the chain the node
  mines as the dashboard header does. Both open with `public_dashboard`.
- A docs site built with mdBook from `docs/` (`mdbook build docs`, CI job
  `Docs`), with new pages for requirements, miner options, small-miner
  arithmetic on Bitcoin Cash and known attacks.
- The pages of upstream's documentation site (`p2poolv2/docs` at `623e4c1`),
  ported to the docs book and adapted to Bitcoin Cash under "How P2Poolv2
  works": why P2Poolv2, the comparison with SV2 and DATUM, the P2P network,
  node components, the Stratum server, the share chain (commitments,
  validation, mining on the current chain, PPLNS accounting, share
  transactions, spending share outputs, pool size limits) and payouts
  (coinbase payouts, selling shares). They keep upstream's credit and MIT or
  Apache-2.0 notice; `docs/upstream-port-map.md` maps every upstream page and
  says why the rest were not ported. The coinbase payout cap is marked as an
  open design question, being redesigned as direct payouts plus a payout tree
  for shares that merge-mine tokens.
- Docker: a Bitcoin Cash Node 29.2.0 image from the release tarball, pinned
  by SHA-256, and a regtest stack that mines through the pool with a Rust CPU
  miner (mujina), checked by the `Docker regtest stack` CI job.
- Ansible: Bitcoin Cash keys in the node's config template and a
  `monitoring_p2poolv2` role for Prometheus and Grafana.

### Changed

- Merge-mining common format v2 (share-chain rule, audit A16 and A15): every
  Bitcoin Cash coinbase carries the 53-byte `OP_RETURN "CTMM" 02 root h salt`
  commitment as output 0, the exact empty commitment (zero root, `h = 0`,
  salt 0) on pool-built jobs. The share header carries a declared job's
  root, `h` and salt, and peers rebuild output 0 byte for byte. Format
  checks refuse `h` above 16 and a zero root with any other `h` or salt.
  `shares::auxpow` builds the salted SHA-256 tree (at most 16 tokens) and
  encodes and verifies `AuxProof` v2 (coinbase branch `d ≤ 25`). P2Pool
  matches Pickaxe's golden vectors byte for byte (pickaxe-miner PR #70).
  `share_has_token_commitment` tells token shares from no-token shares. The
  old `fabe6d6d` output and Namecoin slots are gone; `p2poolv2_cli
  bch-auxpow` builds and verifies the new format. The experimental SAFA
  covenant still verifies v1 proofs and must be ported before issuance.
- ASERT header check (share-chain rule for token shares): `shares::bch_asert`
  computes BCH's aserti3-2d exactly as BCHN's `pow.cpp` does, with BCHN's
  mainnet and chipnet/testnet4 anchors. A token share's bitcoin header, and
  a proof's parent header, must carry nBits within `computed / 2` to
  `computed × 1.05` of ASERT's target at its BIP34 height on mainnet, and
  `computed / 8` to `computed × 1.05` or the proof-of-work limit on test
  networks. Tested on all 14,000 lines of the aserti3-2d specification's
  vectors and on real mainnet and chipnet headers.
- Coinbase merkle branches between Bitcoin Cash share-chain nodes may have
  32 levels, up from 25 (share-chain rule, audit A14): BCHN's 2 GB guard is
  temporary and Bitcoin Cash has no fixed block size maximum. Merge-mining
  proofs keep `d ≤ 25`. A `ShareHeaders` response carries at most 600
  headers, down from 650, so the worst case (1,584 bytes a header) still
  fits the 1 MiB frame.
- Coinbase budget on Bitcoin Cash (share-chain consensus, decision 14): a
  coinbase pays at most 11 PPLNS payout outputs (`BCH_MAX_COINBASE_PAYOUTS`)
  holding at most 374 bytes (`BCH_MAX_PAYOUT_OUTPUT_BYTES`), and no payout
  below BCHN's dust threshold, so stock Antminer firmware, which upstream
  reports takes "just over a dozen outputs", can mine every job, over
  Stratum V1 directly or behind a Stratum V2 translator. Payees that fit
  are paid as before. Past the budget the largest payees (by weight after
  the payout-holder rule) are paid their exact share and everything else
  goes to one of the remaining payees, drawn by weight from the payout
  anchor, so each one's expected income is unchanged and the coinbase sums
  exactly. The donation, the fee and the 10% cap are untouched. Job builders
  refuse a coinbase over `BCH_COINBASE_BYTE_BUDGET` (784 bytes; the largest
  the rule allows is 782). Protocol strings carry `payout-cap11`. Bitcoin and
  Hydrapool pay every payee. See `docs/architecture/payout-distribution.md`,
  "Bitcoin Cash coinbase budget".
- Bitcoin Cash share transaction pool priority (node policy, decision 15):
  a transaction's priority is the share units it burns (inputs less
  outputs) per byte, then its arrival. Shares carry the highest first; a
  full pool evicts the lowest ones only for a transaction that outranks them
  all, and otherwise refuses it ("the pool is full ..."), instead of
  evicting the oldest. No validity change: there is still no consensus fee.
  `p2poolv2_cli bch-trade --fee-rate` sets the bid on the share chain. See
  `docs/atomic-swap/bch-share-trading.md`, "Priority".
- Share-chain consensus, Bitcoin Cash and Bitcoin: an uncle may have only
  one nephew on a chain. A share naming an uncle that its parent or
  grandparent already names is invalid (upstream's `validation.adoc` rule,
  which upstream's code does not enforce yet). Before, such a share was
  accepted and the PPLNS window paid the uncle twice, with two nephew
  bonuses. Siblings on different branches may still name the same uncle.
  Bitcoin Cash protocol strings carry `uncle-once`.
- Share-chain consensus, Bitcoin Cash and Bitcoin: header sync applies the
  timestamp rules of full validation to every header before it is organised:
  later than the median time of its 11 ancestors (read from the batch and the
  store) and at most 120 seconds ahead of the local clock. Blocks below the
  full-validation zone had unchecked times, and on Bitcoin Cash each share's
  ASERT target comes from its parent's time. Block receipt checks the same
  rules before it stores a block.
- A block spending an output the node does not hold is stored, with
  `CoinbaseRoot::Unknown` stamped on its outputs, instead of failing to
  store. On a syncing node such a block (spending an output below its prune
  height) sat on the best chain unstorable since share transactions shipped.
  Spending an `Unknown` output is rejected (`UnknownCoinbaseRoot`), and
  confirmation no longer asks whether a source the node never held is
  confirmed, which would have marked that valid block Invalid. A fully
  validated block spending a missing output is still Invalid. The store
  format changes (one byte per output).
- The candidate chain refuses a reorg forking more than `PAYOUT_DEPTH`
  (127,008 shares, about 14.7 days) below the confirmed tip, down from the
  prune target (254,376). Share transactions need it: a deeper fork could
  spend an output the node never fetched or stamped `Unknown`. The refusal
  logs `branch forks more than MAX_REORG_DEPTH below the confirmed tip`.
- Stratum `maximum_difficulty` defaults to 2^32 (about 5.5 EH/s at one share
  every 3.33 seconds) when unset, and a session's difficulty is at least 1.
- The Docker stacks run Bitcoin Cash: BCHN on regtest, chipnet or mainnet
  instead of Bitcoin Core on signet, `chain = "bitcoin-cash"` in the image's
  default config, and Prometheus and Grafana pinned by version and digest.
- The Ansible role installs this fork's release and writes Bitcoin Cash
  configs. Optional instance keys no longer render `default(omit)`
  placeholders into the config.
- The Grafana dashboards link found blocks to Blockchair (mainnet) or
  chipnet.imaginary.cash (chipnet) instead of mempool.space, show coinbase
  amounts in BCH and list miners by payout address.
- The sample configs offer miners BIP 320's version rolling mask, `1fffe000`,
  the code default, instead of `1fffffe0`.
- README rewritten for the Bitcoin Cash fork.

- Share-chain consensus on Bitcoin Cash: the interpreter also accepts a
  claim or refund of the share-trading contract; every output of a
  non-coinbase share transaction must be P2PKH or P2SH32 at or above BCHN's
  dust threshold (546 or 576 units), so `OP_RETURN`, P2SH20 and dust outputs
  invalidate a share; and a share transaction must be final at its share's
  height. Bitcoin keeps upstream's rules.
- Payout rule on Bitcoin Cash: a coinbase pays each PPLNS window share's own
  weight to the key hash holding the share's payout right at the paying
  share, instead of always to its `miner_bitcoin_address`. The right starts
  on the share's coinbase output and moves from a spending transaction's
  input `i` to its output `i`; a P2SH32 output keeps the previous holder, so
  a share locked in a trade contract pays the seller until the buyer's claim.
  While the miner holds its own right, it is paid at `miner_bitcoin_address`
  as before; uncles are always paid to their miner. Protocol strings carry
  `payout-holder`. Bitcoin keeps upstream's payee. See
  `docs/atomic-swap/bch-share-trading.md`, "Payout rights".
- `network.dial_peers` is optional and defaults to empty, and a node without
  a `listen_address` still dials its `dial_peers` (from upstream).
- Hydrapool shares go to a dedicated PPLNS share worker and are stored only
  when their hash meets the difficulty they are credited at, the session
  difficulty on Stratum V1 and the channel target on Stratum V2 (from
  upstream). Only P2Poolv2 mode emits shares to the share chain, and a share
  whose coinbase proof cannot be built is no longer emitted.
- Bitcoin genesis shares load from raw `.dat` files, and the Bitcoin testnet4
  genesis moves to a new block and timestamp (from upstream). Bitcoin Cash
  genesis shares are built by `bch_genesis` and do not change.
- The sim build compiles short depths (`COINBASE_MATURITY` 20,
  `PPLNS_WINDOW_SHARES` 80, `PRUNE_DEPTH` 200, `PRUNE_INTERVAL` 20, background
  tasks every 30 s) instead of the `[sim] coinbase_maturity` option (from
  upstream). Bitcoin Cash's 100-share maturity is the same in every build.
- The Rust/WASM dashboard (`p2poolv2_web`) looks and behaves like the one the
  node renders: the same markup, menu, search, trail and pages (Dashboard,
  Blocks, Miners, Help, block, miner and share pages), addressed by URL
  fragment. Its own `style.css` is gone; trunk bundles the node's
  stylesheets, fonts and logos from `p2poolv2_api/src/api/dashboard/assets/`,
  the only styling source, and tests fail if a stylesheet appears elsewhere
  or a class either dashboard uses has no selector there. It keeps live
  shares over `/ws`, the zoomable hashrate chart and the API login settings.
- The developer tools are Rust: the `p2poolv2_tools` crate replaces the sim
  scripts (`cargo sim run-swarm`, `observe`, `metrics`, `failures`,
  `nightly`, `maturity-check`, `plot-metrics`, `stop-swarm`), the JavaScript
  mock bitcoind (`cargo mock-bitcoind`), the Python log analysers
  (`cargo share-lifecycle`, `cargo share-latency`) and the JMeter and
  diagnosis scripts. The nightly workflow runs `cargo sim nightly`. Same
  environment variables and run files; `plot-metrics` now writes SVG. See
  `docs/tooling.md`, which also maps each old script for upstream merges.
  `tools/safa-auxpow` stays JavaScript until a Rust BCH VM exists.
- The sim swarm and its nightly run Bitcoin Cash against BCHN instead of
  Bitcoin against Bitcoin Core. `p2poolv2_sim` reads `chain =
  "bitcoin-cash"` configs with a `bchreg:` payout address and an
  `rp2pool:` share address through the node's own parsers; the harness
  writes such configs, `MINER_ADDRESS` defaults to a `bchreg:` address, and
  the nightly starts BCHN with `-allowunconnectedmining`. CI installs BCHN
  29.2.0 from its release tarball, checked against the SHA-256 sums in
  `docker/bchn/Dockerfile`. Bitcoin Cash keeps its 100-share maturity in
  the sim build. `cargo mock-bitcoind` answers as BCHN on regtest by
  default; `--node bitcoin-core` keeps the Bitcoin signet answers the JMeter
  comparison with CKPool needs.

### Removed

- The `store.background_task_frequency_hours` option (from upstream). Existing
  configs that set it still load.
- The `[sim] coinbase_maturity` option.

### Fixed

- A miner's stratum difficulty never goes above the share difficulty of the
  job it mines (V8 in `docs/p2pool-v1-bch-lessons.md`). `d=`, `th=`,
  `mining.suggest_difficulty` or `[stratum] minimum_difficulty` could hold a
  miner above it, so it threw away hashes that were valid shares and was
  paid for fewer hashes than it did. Before a job with a lower share
  difficulty, an SV1 session gets `mining.set_difficulty` and an SV2 channel
  `SetTarget`; a `minimum_difficulty` above the share difficulty is logged
  once. Node policy, not a share-chain rule: peers see nothing new.
- A found block no longer depends on one RPC call (V9 and V10 in
  `docs/p2pool-v1-bch-lessons.md`). BCHN gets it again after a connection
  error, a time-out or a busy node, for up to two minutes; its answer is
  final and is not sent again. `duplicate` (BCHN already has the valid
  block) and `inconclusive` (stored without a verdict) now count as BCHN
  holding the block, and the template refresh follows them too. New
  optional `[[submit_nodes]]` tables list extra BCHN nodes that every found
  block also goes to, each from its own task with its own retries. New
  metrics `found_block_submissions_total{node,outcome}` and
  `found_block_submit_retries_total{node}`. A JSON-RPC error sent with HTTP
  500 is now read as the node's RPC error, not as an HTTP failure.

### Security

- The buffer of blocks waiting for their parent is bounded: at most 1,024
  blocks and 32 MiB, the oldest evicted first, and a block that waited 10
  minutes is dropped. It had no cap and no expiry, and a share naming a
  parent that never arrives waited for ever.
- A miner asking for `d=18446744073709551615` (or suggesting it) no longer
  overflows the difficulty adjuster, which panicked in debug builds and
  wrapped in release: its sums saturate, and the default maximum caps the
  request. Property tests run the adjuster over extreme start difficulties,
  bounds and share runs.
- Updated `rustls-webpki` to 0.103.16 (from upstream).

## [v0.0.1-alpha] - 2026-10-18, refreshed 2026-10-19 and 2026-10-22

First alpha of P2Poolv2 for Bitcoin Cash, synced with upstream P2Poolv2
through `2dabcdd2`. Alpha software: run it on chipnet or regtest first.
The pre-release is refreshed in place with `main` until the beta.

- The 2026-10-19 refresh adds the Template Distribution client, the
  template refresh after a found block, the `/miner` API, and the BCH payout
  window sized by block difficulty with a 100-share maturity. That is a
  share-chain rule change: nodes start from a new store.
- The 2026-10-22 refresh adds:
  - the dashboard rendered in Rust by the node;
  - `[api] web_root` for a self-hosted frontend and `public_dashboard` for
    a public node;
  - the Rust/WASM dashboard (`p2poolv2_web`) with `/hashrate` and
    `/found_blocks`;
  - `/jd_prepare` for external SV2 job declarators;
  - BCH share difficulty anchored at each share's parent. This changes the
    share chain's rules, not Bitcoin Cash consensus: nodes start from a new
    store.

### Added

- The dashboard is rendered in Rust by the node itself: an overview with
  pool, node and network hashrate, a one-day hashrate chart drawn as SVG,
  found blocks and recent shares, plus miner (`/dashboard/miner`) and share
  (`/dashboard/share/<hash>`) pages. Pages carry no scripts and sit behind
  the API's authentication unless `public_dashboard` is set; the node keeps
  the hashrate history, one sample a minute for a day. Builds on the
  dashboard design in PR #8 by CashDragon0.
- `[api] web_root` serves a self-hosted frontend (TypeScript, Rust/WASM or
  any static build) for every path the API does not use, beside the JSON API
  it reads. Without one, `/` leads to the dashboard.
- `[api] public_dashboard` opens the dashboard, the `web_root` frontend and
  the read-only share chain routes to anyone for a public node, while
  `/metrics`, `/peers`, `/blocked_ips` and `/jd_prepare` keep the API login.
- Bitcoin Cash: `stratum.chain = "bitcoin-cash"` with `bitcoin_cash_network`
  mines from BCHN templates, pays CashAddr scripts, records P2PKH share owners
  and starts from a main or mini Bitcoin Cash genesis. See `docs/bch-plan.md`
  and `docs/bch-design.md`.
- Bitcoin Cash donation: 1.5% of each block a node finds goes to the P2Poolv2
  donation address by default. `donation` raises or lowers it and 0 opts out;
  `fee_address` and `fee` remain the operator's own cut.
- `p2poolv2_cli bch-share-address` prints the Bitcoin Cash share address for a
  P2PKH CashAddr.
- Local Stratum V2 listener (`sv2_port`) with both Job Declaration modes and
  standard and extended mining channels that share the SV1 job tracker,
  vardiff, block submission and share chain, with optional Noise encryption
  signed by an operator-supplied authority key.
- Bitcoin Cash share transfers are checked by a Bitcoin Cash interpreter
  (P2PKH with `SIGHASH_FORKID` ECDSA or Schnorr).
- `docs/architecture/payout-distribution.md` describes the whole payout
  scheme, including the maturity window.
- API `GET /miner?address=...` returns one miner's shares and uncles in the
  share chain PPLNS window, its summed weight, its share of the window and
  its fraction of the next block's PPLNS payout, so explorers and miner
  software need not index the chain. It takes a payout address (CashAddr
  with or without prefix on Bitcoin Cash) or a share chain address, and is
  computed by the payout code's own `PplnsWindow`.

### Removed

- The JavaScript dashboard (`p2poolv2_api/static`), its `dashboard.tar.gz`
  release archive and `P2POOL_STATIC_DIR`. The Docker image, systemd unit
  and Ansible role no longer install dashboard files.

### Fixed

- A Stratum V2 channel starts at the difficulty its declared
  `nominal_hash_rate` settles on, within the node's minimum and maximum,
  as the SV2 spec asks. Before, every channel started at `start_difficulty`,
  which left a small device days from its first share. A channel that
  declares no rate still starts at `start_difficulty`.
- A Bitcoin Cash share address in `p2p=` ends at its first dot, so the
  `.<worker>` the Stratum V2 reference translator appends to a channel's
  `user_identity` no longer makes the node refuse the channel.
- A share header with a donation or fee above 10,000 basis points no longer
  panics the validating node; such shares are rejected.
- Stratum usernames on a Bitcoin Cash node accept CashAddr payout addresses.
- Bitcoin Cash nodes derive `stratum.network` from `bitcoin_cash_network`, so
  nodes on one network no longer split over a leftover Bitcoin value.
- Stratum V2: Full-Template declarations are matched per connection,
  `coinbase_outputs` is a counted output list, and the listener accepts host
  names and stops the node when it cannot bind.
- Bitcoin Cash shares mined from a BCHN template are no longer rejected by
  every node because of BCHN's empty `coinbaseaux` flags string.
- A young Bitcoin Cash mini share chain can be synced over header batches.

### Changed

- On Bitcoin Cash, share difficulty anchors ASERT at each share's parent
  instead of genesis, the same on mainnet, chipnet and regtest. A late start
  or an outage no longer leaves shares at the easiest difficulty, where
  PPLNS paid by submission count instead of work. Bitcoin keeps the genesis
  anchor. This changes only the share chain, not Bitcoin Cash consensus. It
  is a share chain consensus change: Bitcoin Cash protocol strings carry
  `asert-parent`, and existing chipnet and regtest nodes restart together on
  a new store path.
- On Bitcoin Cash, peers reject shares whose donation plus fee exceed 10%, or
  whose donation pays another address.
- A found block's coinbase now pays only shares that are mature for it. The
  PPLNS walk skips the shares less than `COINBASE_MATURITY` (6,048) below the
  paying share, the same depth spending a share coinbase needs, then pays the
  next `PPLNS_WINDOW_SHARES`. The first `COINBASE_MATURITY` shares of the chain
  are always mature for payout, so a young chain pays its shares and the paid
  set never shrinks; spending them still needs the full depth. This is a
  consensus change and needs a chain reset.
- Share chain outputs now expire at `PAYOUT_DEPTH` (`COINBASE_MATURITY +
  PPLNS_WINDOW_SHARES`, 127,008) instead of `PPLNS_WINDOW_SHARES`, so an output
  stays spendable for a full window after it matures. This is a consensus
  change.
- The full-validation zone, the PPLNS window cache and `PRUNE_DEPTH` grow with
  the maturity skip: the zone is `PAYOUT_DEPTH`, the cache holds
  `PAYOUT_DEPTH` plus its 1% buffer, and `PRUNE_DEPTH` is `2 * PAYOUT_DEPTH`
  (254,016). Nodes retain about 5% more block bodies. No store migration is
  needed.

## [v0.15.3] - 2026-09-21

### Changed

- Relicensed from AGPL-3.0-or-later to the dual license `MIT OR Apache-2.0`.
  `LICENSE` is replaced by `LICENSE-MIT` and `LICENSE-APACHE`, and every source
  file now carries an SPDX header instead of the GPL notice. Downstream users
  may take the project under either license at their option.

### Fixed

- Block fetching no longer degrades to quadratic time on a large initial sync.
  Duplicate detection across the pending and backlog queues used nested linear
  scans, so fetching the whole chain could spend a long time enqueuing before it
  dispatched a single request. It now uses a hash-set membership index and
  begins requesting block bodies immediately, while the queues keep their
  request order.
- `p2poolv2_cli db cleanup-dense-heights` now collapses a dense height to its
  real chain block. It invalidates off-chain `BlockValid` siblings (shares this
  node mined onto a losing fork), not only `HeaderValid` ones, and processes
  heights top-down so flood siblings that merely reference each other as uncles
  are removed as well; genuine uncles referenced by a served block are kept.
  Previously such heights stayed dense and could still stop peers syncing past
  them.

### Security

- Updated `rustls` to 0.23.45, resolving RUSTSEC-2026-0285: version 0.23.43 could
  accept a TLS 1.3 handshake message packed into the same record as a preceding
  key-changing message, contrary to RFC 8446 section 5.1. The handshake
  transcript remains authenticated, so this could not alter or complete a
  handshake.

## [v0.15.2] - 2026-09-06

### Fixed

- Increase docker action timeout on github

### Removed

- x86_64 macOS (Intel) release binaries
- linux/arm/v7 docker images

## [v0.15.1] - 2026-09-06

### Breaking

- Environment variable overrides now use `__` between a config section and its
  key: `P2POOL_STORE__PATH`, not `P2POOL_STORE_PATH`. A single underscore could
  not address a field whose own name contains one, so
  `P2POOL_STORE_PPLNS_TTL_DAYS` resolved to `store.pplns.ttl.days` and was
  silently discarded. Operators setting any `P2POOL_*` override must add the
  second underscore. `P2POOL_CONFIG` and `P2POOL_STATIC_DIR` are unaffected;
  they are read directly rather than through the config crate.
- RocksDB engine moves from 10.4.2 to 11.8.1 with the `rocksdb` 0.25 upgrade,
  and libp2p moves from 0.53 to 0.56. Both are format-relevant and land
  together with the planned chain reset.
- docker-compose now requires `NETWORK` in `.env` and refuses to start without
  it. It selects bitcoind's config file and p2poolv2's share chain and store
  directory, so one value keeps them on the same chain. It overrides `network`
  in a mounted `config.toml`, which is ignored under compose.
- The libp2p protocol strings now carry the share chain genesis hash as well as
  the bitcoin network: `/p2pool/<network>/<genesis prefix>/1.0.0`, used for the
  request-response protocol, Kademlia, Identify and the Noise prologue. Nodes on
  different share chains no longer negotiate a protocol, so a node on an older
  release cannot connect to one on this release. The genesis hash is derived in
  code rather than taken from config, so a chain cannot be joined by mistyping a
  network name.
- The testnet4 share chain genesis moves to bitcoin block
  `0000000000a57b6dd9b7340a2e2cbb7069a0ccdf28f3db69070ea89d7287444b` at height
  151197, resetting that share chain. Existing testnet4 stores are on the old
  chain and must be deleted.

### Added

- `cargo-deny` in CI, checking advisories, licences, duplicate versions and
  source registries, with a `deny.toml` policy and `just deny`. Licence, ban
  and source violations block; advisories run non-blocking with a daily
  schedule, so a newly published advisory does not stop an unrelated merge.
- Dependabot for cargo, GitHub Actions and Docker base images, weekly.
  Semver-compatible cargo updates arrive as one grouped PR. Majors are left to
  Dependabot except for libp2p, rocksdb and bitcoinconsensus, and bitcoin is
  also held at minor because 0.32 to 0.33 changes consensus serialization.
- Known-answer tests for `password_to_hmac` and `build_basic_auth_header`,
  pinning the values that operator `auth_token` config depends on.
- Characterisation tests for environment-variable overrides, including the
  exact keys docker-compose sets.
- Startup check that the genesis this node is built for matches the one the
  store was built on. A store holding another share chain now fails with
  `GenesisMismatch` naming both hashes and telling the operator to delete the
  store directory, instead of quietly keeping two chains in one database and
  serving the heavier old one to peers on the new chain. The check reads the
  share header, which pruning never removes.

### Changed

- Dependencies updated across the tree. Packages with known advisories drop
  from 25 to 3, and the lockfile from 574 packages to 542. The three that
  remain are blocked upstream in libp2p and are recorded in `deny.toml` with
  the conditions for removing them.
- `reqwest` 0.13 changes what `default-tls` means, so TLS moves from OpenSSL to
  rustls. `openssl`, `openssl-sys`, `native-tls` and `hyper-tls` leave the tree
  entirely. Certificate trust is unchanged: `rustls-platform-verifier` still
  uses the platform trust store.
- Container base images move to Debian 13 (trixie) for both build and runtime
  stages, and the runtime image now installs `ca-certificates`, which rustls
  requires when constructing an HTTPS client.
- CI lint runs `cargo clippy --workspace --all-targets --all-features
  -- -D warnings`, matching what AGENTS.md already required.
- GitHub Actions in `rust.yml` are pinned by commit SHA, as `docker.yml`
  already did.
- The node logs its P2P protocol string at startup, so an operator can see which
  share chain a node will peer on.
- CI runs coverage through `cargo nextest`, pins the remaining action hashes,
  builds on a newer Rust docker image, and skips the codecov upload on
  dependabot PRs, where the token is not available.

### Removed

- `void` and `rust_decimal` dependencies. libp2p 0.55 replaced `void::Void`
  with `std::convert::Infallible`, and `rust_decimal` had no references in any
  source file.

### Fixed

- `P2POOL_NETWORK_LISTEN_ADDRESS` in docker-compose silently did nothing,
  because it could not reach a field whose name contains an underscore. It now
  applies, with the same default the shipped config already used.
- docker-compose hardcoded the bitcoind endpoint to `127.0.0.1`, contradicting
  the `http://bitcoind:38332` in the shipped config. It is now passed through
  only when set, so platforms that inject it (Umbrel and similar) can, while
  everyone else keeps the value from their `config.toml`. Neither this nor the
  chain override had any effect before, because both named a `bitcoin` config
  section that does not exist.
- `publish = false` had no effect on eight of the nine crates. Workspace
  package keys apply only where a member opts in, so those crates were
  publishable despite the workspace declaring otherwise.
- `OutgoingConnectionError` logged every failed outbound connection twice.

## [v0.14.1] - 2026-09-04

### Fixed

- Docker image build to handle new p2poolv2_address crate.

## [v0.14.0] - 2026-09-04

### Added

- Share chain addresses: a new `p2poolv2_address` crate encoding a taproot
  output key as bech32m under a P2Poolv2 human readable part (`p2pool`,
  `tp2pool`, `sp2pool`, `rp2pool`). Witness version 1 only; testnet3 is
  unsupported.
- `ShareHeader.miner_address`, the share chain owner of a share's coinbase
  output, alongside the existing `miner_bitcoin_address` that receives the
  bitcoin payout. The two are never derived from one another. It is stored as
  the taproot witness program an address encodes, not as the address, because
  an address also names a network and the network is not consensus data.
- `p2p=<address>` in the stratum password, naming the share chain address for
  a miner's shares. Combines with the existing `d=` and `th=` hints.
- Any well formed P2TR share chain address is accepted, including one whose
  output key is not a curve point. Bitcoin lets anyone pay to such an output
  and it is unspendable at the owner's own cost, while rejecting it would have
  put an elliptic curve operation on every share header decode.
- Optional `[stratum] miner_address`. When set it owns every share the pool
  mines, so miners need no password change; a miner sending a different
  address is rejected.
- Prometheus pool metrics and Grafana dashboards for the pool, node, and
  users, with provisioning and import-ready public variants.
- Pool-wide block fund tracking, with effort reset only on a pool-wide
  block find rather than on every bitcoin block.
- Grafana dashboard setup instructions in the README.
- `p2poolv2_cli address`, which encodes a taproot output key as a share chain
  address. It takes the `witness_program` field of `bitcoin-cli
  getaddressinfo` on stdin or as an argument, applies no tweak because the
  wallet already did, and checks the key is a curve point so a typo cannot
  produce an address whose shares nobody can spend. It needs no running node
  and no config file, and it never sees a private key.
- A security policy file.

### Changed

- **BREAKING (consensus):** share chain restart on all networks. `ShareHeader`
  gained `miner_address` and the share coinbase now pays it, so every share
  block hash changes, genesis included. The commitment hash embedded in the
  bitcoin coinbase scriptSig changes with it. `miner_address` is encoded as a
  witness version byte, a length byte and the program, rather than as an
  address string. An address string would have carried the human readable
  part, which names a network and is covered by `block_hash`, so one owner
  could be spelled four ways and each spelling would have been a distinct
  share block carrying the same proof of work. The version travels in the
  encoding, so accepting a further witness version later is a policy change
  rather than a format break, and validation rejects anything but P2TR.
- **BREAKING (consensus):** the share coinbase carries the weak block hash in
  its scriptSig, and is no longer covered by the share commitment. Every share
  a miner produced was previously byte identical -- null prevout, empty
  scriptSig, and a payout derived only from the address -- so one miner's
  shares all shared a txid and their share units collided on a single
  outpoint. A miner with ten thousand shares held one. The weak block hash is
  unique per share by construction, because `prev_share_blockhash` is in the
  commitment, so a replay under a different parent changes the commitment, the
  bitcoin coinbase, and therefore the hash.

  This required the commitment to stop covering the coinbase, which would
  otherwise be circular: the commitment is fixed before the miner hashes,
  while the weak block hash exists only afterwards. `ShareCommitment` drops
  `merkle_root` and gains `non_coinbase_root`, the merkle root over the share
  transactions excluding the coinbase, which is all zeros while share blocks
  carry nothing else. It also now digests `miner_address` directly: that used
  to be committed only through the coinbase that pays it, so with the coinbase
  outside the commitment it is the sole binding between a share's owner and
  its proof of work.

  `ShareHeader` is unchanged in shape. `merkle_root` keeps bitcoin's meaning,
  the root over all share transactions with the coinbase at leaf zero, and is
  computed at assembly from the final coinbase rather than copied from the
  commitment. `validate_share_coinbase` rebuilds the coinbase from the header
  and compares, which proves it is the canonical one rather than merely well
  shaped.
- **BREAKING (stratum):** in p2poolv2 mode, authorize is rejected unless a
  share chain address is available, either from `[stratum] miner_address` or
  from `p2p=<address>` in the miner's password. Hydrapool mode is unaffected,
  and rejects `miner_address` at startup.
- In p2poolv2 mode, config parsing rejects a network that has no share chain
  human readable part, and a configured `miner_address` is re-checked against
  the pool's network when a miner authorizes. Without the startup check the
  node ran and rejected every authorize forever, for a reason no miner could
  fix.
- **BREAKING (JSON API):** `miner_address` on the `/share`, `/shares`,
  `/share_headers`, `/candidates` and `/dag` endpoints, in `db-query` output
  and in WebSocket `Share` events now carries the share chain address. The bitcoin payout
  address moved to a new `miner_bitcoin_address` field. The key kept its name
  and changed meaning, so consumers must be updated rather than warned by a
  missing field.
- Share chain addresses are parsed with the same segwit decoder bitcoin uses
  for its own addresses, minus the restriction on the human readable part, so
  the BIP350 pairing of witness version to checksum applies: version 0
  requires bech32 and later versions bech32m. The 90 character segwit limit
  applies too.
- `db-query` renders a share chain owner as its witness program hex when no
  `--config` is given, since without one there is no network to choose a
  human readable part with. Pass `--config` to see the address form; the API
  and the WebSocket feed always have the configured network and always render
  the address.
- The dashboard shows both addresses, truncated client side.
- The `share_sync` test fixture is regenerated from a live signet node, with
  bitcoin headers carrying real proof of work mined under the two-address
  commitment and the per-share coinbase.
- `impl Encodable for ShareCommitment` is removed. It was a second, unused
  definition of the commitment byte layout that had already drifted from the
  one `hash()` uses, so it could only ever produce a digest no peer would
  reconstruct.
- The notify path no longer computes a share merkle root. The commitment does
  not cover the coinbase, so there is nothing per-miner left to compute there;
  the root was previously recomputed for every miner on every notify despite
  being constant for the life of a session.
- **BREAKING (JSON API):** the `status` field is renamed to
  `validation_status` on the `/share` and `/dag` endpoints and in
  `db-query` output; read the sibling `chain` field for chain position.
- Block metadata `Status` now tracks validation state only, with chain
  position moved to a separate `chain` field.
- `ValidationError` carries a failure kind (`Consensus`, `StoreAccess`,
  `Recoverable`) so a transient store failure no longer shuts the node down.
- Ingest-time prevout validation no longer judges whether a prevout's source
  is confirmed or whether the prevout is already spent. Both answers change
  with a reorg, so recording them as a permanent `Invalid` barred a
  higher-work fork from ever being adopted when the same transaction appeared
  on both sides of the fork. Confirmation re-checks them with the reorg
  overlay applied, and marks the block `Invalid` there.
- A template that cannot be prepared now stops the notifier, which shuts the
  node down, instead of being logged and skipped. The watch channel hands its
  last value to every connected and newly connecting miner, so skipping left
  the pool hashing a previousblockhash bitcoin had moved past, with nothing to
  age the job out.
- An empty sharechain PPLNS window and a zero PPLNS total difficulty are both
  errors rather than fallbacks onto the bootstrap address, so they reach the
  notifier and shut the node down. Paying a whole coinbase to one address at
  the moment the payout data is known to be wrong is worse than stopping.
  Neither is a bootstrapping state: genesis is on the confirmed index, so a new
  chain has it in the window, and the total difficulty is at least 1 on every
  network unless `difficulty_multiplier` is configured below 1.0, which
  truncates to zero when cast.
- Every block a promotion confirms is now reported at its own height, instead
  of only the block that triggered the promotion being reported at the new
  confirmed tip's height. A promotion confirms a whole prefix whenever
  confirmation was stalled and then catches up, and the rest of that prefix was
  missing from the share feed and from the pool effort numerator entirely.
- The mining base search keeps a validated block it has already found when it
  hits its search bound, instead of discarding it and falling back to the
  confirmed tip. Discarding it defeated the search in the case it exists for: a
  subtree above the confirmed tip large enough to exhaust the budget, which a
  peer can produce cheaply at minimum pool difficulty.
- The organise worker's pending buffer no longer holds one entry per delivery
  of the same block. A stored `HeaderValid` block is re-validated and
  re-emitted every time a peer sends it, so replaying one observed share filled
  the buffer and started dropping live blocks -- which are already stored with
  their bodies, so nothing re-requests them and confirmation stops at their
  height.
- The DoS gate now checks the header's merkle root against the transactions
  carried with the block. A block's identity is its header hash, so a peer
  could attach arbitrary transactions -- up to the 200 KB size limit, and even
  in reply to a block requested by hash -- and have them buffered and stored
  under a hash that said nothing about them.
- Uncle block bodies are required only for blocks inside the PPLNS zone. The
  BlockReceiver admits a block whose uncle sits below `prune_height`, because
  those bodies are never fetched, but validation required them in both tiers --
  so such a block was stored and then failed validation forever on data nothing
  would request. Uncle acceptance is now a pure function of chain shape, with
  body presence checked separately which runs during block validation in pplns
  zone
- A failed `mark_invalid` is no longer reported as an invalidation. The batch
  is dropped uncommitted, so the block keeps its `HeaderValid` status and
  candidate membership and confirmation still stops at its height; both the
  chain-context path and the `InvalidBlock` event path acted as though the
  candidate chain had been rebuilt. The block is now re-buffered for a retry
  where it is available, the confirmed-chain follow-up is skipped where it is
  not, and a closed store writer channel is fatal rather than logged.
- The validation tier is recorded on `OrganiseEvent::Block` instead of being
  re-derived by the organise worker. Both stages read `check_pplns_zone`
  independently and the candidate tip can shorten between them, so a block
  tiered below the PPLNS zone at stage 1 could be marked `BlockValid` back
  inside the zone with its merkle root, coinbase, witness commitment and
  scripts never checked. Those checks now run before promotion when that flip
  happens.
- `ValidationError` gains a fourth failure kind, `Unresolvable`: a check that
  needs data this node no longer retains and that no retry can decide. The
  organise worker drops such a block without recording a verdict, rather than
  buffering it. A PPLNS anchor deeper than the retained window is the first
  case; it was `Recoverable`, which buffered the share under a parent height
  that drains never revisit, so the entry held its slot until the buffer began
  dropping live blocks.
- `difficulty_multiplier` is validated at config parse: it must be a finite
  whole number of at least 1. Every consumer reads it through an `as u128`
  cast, so a smaller value truncated to zero and a fractional one silently
  rounded down, changing the payout window for the whole pool.
- Invalidating the last candidate above the confirmed tip now leaves the
  candidate top at the confirmed tip instead of deleting it. An absent top
  candidate height means the candidate chain has never been written, which
  stopped `organise_block` from promoting and let any header become the
  candidate tip with no parent, contiguity or work check.
- Coinbase maturity is measured from the stored `coinbase_root_height` to the
  spending block's own height, so the verdict no longer depends on which
  branch this node has confirmed. `check_prevouts_and_find_coinbase` is now
  `check_prevouts` and enforces existence, the payout window, and maturity in
  one batch read; `find_immature_coinbase_prevout` is removed.
- PPLNS payout is anchored on `prev_share_blockhash` instead of the live tip.
- The PPLNS window scan is bounded relative to its anchor.
- The PPLNS window retains an extra 1% of shares.
- Uncle selection requires at least `HeaderValid` status and excludes
  ancestors, nephews, and blocks beyond `MAX_UNCLES_DEPTH`.
- The mining base is loaded from confirmed tip descendants rather than the
  confirmed tip itself.
- The organise worker's pending block buffer capacity is reduced to bound
  memory use.
- The fallback confirmation path is removed.
- The unused `OrganiseEvent::Header` variant is removed; header
  organisation was already a direct store call.
- CI runs the workspace test suite both with and without the `sim` feature.
- The share blockhash is computed once per stratum submit, and the
  extranonce only when an emission needs it.

### Fixed

- Reject blocks whose transactions exceed `BLOCK_TXS_SIZE_LIMIT` before
  buffering them.
- Buffer a block until its parent and uncle bodies are available, so
  validation no longer fails on missing ancestor transactions.
- Re-check prevouts when extending and when reorging the confirmed chain,
  using an in-batch overlay so blocks in the same batch are visible.
- Use parent height plus one when checking coinbase maturity.
- Reorg the candidate chain onto the best surviving branch when a block is
  invalidated, instead of stalling confirmation at its height.
- Stop adding `Invalid` blocks to the candidate chain or confirming them.
- Mark a block `BlockValid` only once its parent is `BlockValid`.
- Mark prune-window blocks `BlockValid` so confirmation can pass them.
- Set the candidate top correctly on reorg.
- Drain descendants of every removed block on reorg, not just the tip.
- Re-buffer and retry a block that failed with a recoverable error instead
  of leaving the chain stalled.
- Validate share timestamps against chain structure.
- Remove a TOCTOU race between reading the chain tip and its height.
- Propagate errors when building the output distribution instead of
  falling back to a default.
- Fix a flaky tip-currency test that depended on a one-second clock tick.
- Store a locally mined block and organise its header atomically, matching
  the received-share path and fixing a sync failure under the sped-up
  nightly simulation.
- Record a miner's share even when it misses pool difficulty, fixing
  under-reported per-miner hashrate as pool difficulty rose.
- Publish self-contained multi-arch release manifests, with cleanup of
  per-arch temporary tags on both success and failure.

## [v0.13.0] - 2026-07-14

### Added

- Network isolation for P2P: the bitcoin network name is now embedded
  in the libp2p protocol strings (request-response, Kademlia, and
  Identify) and in the Noise handshake prologue. Nodes on different
  networks (main, testnet4, signet, regtest) derive different protocol
  strings and fail to negotiate. This results in so cross-network
  connections being dropped during the Noise handshake before any
  P2Poolv2 messages are exchanged or peer slots are allocated.

- Ansible deployment: Add playbook and roles to deploy P2Poolv2 nodes.
  Supports multiple instances (e.g. mainnet + testnet4) on the same
  host with per-instance configs, systemd services, and data
  directories. Includes nginx reverse proxy role with rate limiting,
  WebSocket support, Let's Encrypt certificates, and optional Grafana
  proxy. Credentials managed via ansible-vault. See ansible/README.adoc
  for setup instructions.

- Pruning support: Add two-zone block body validation, PPLNS zone and
  Prune Zone. Full block bodies are required for PPLNS zone at sync
  time. We fetch and retain only headers in the prune zone. All blocks
  below the prune zone will be deleted in a future PR. See wiki page
  on Pruning and/or the LLM architecture docs in docs/architecture for
  more details.
  - Block bodies are only fetched within PRUNE_DEPTH of the candidate
    tip. Blocks below this are validated header-only with PoW at sync
    time.
  - `validate_below_pplns_depth`: lightweight validation for
    prune-zone blocks, including PoW, uncles check, block size, tx
    count.
  - `coinbase_root_height` stored in StoredTxOut: tracks oldest
    coinbase ancestor. Outputs deeper than PPLNS_DEPTH from tip are
    unspendable.
  - Confirmation pipeline promotes prune-zone blocks without body
    data.  SpendsIndex skipped for these blocks as outputs are
    unspendable anyway.
  - `verify_chain` tool respects prune boundary. There are no body
    checks below it.
  - Constants: `PPLNS_DEPTH=120960`, `PRUNE_DEPTH=241920`,
    `PRUNE_INTERVAL=360`.

- ShareBlock relay: blocks are now pushed directly as full ShareBlocks
  instead of the Inv-then-fetch protocol, reducing propagation latency.
  Blocks are relayed on validation and broadcast only when the chain is
  current.

- Kademlia peer discovery with periodic bootstrap. libp2p identity is
  now configured from the node key, routable IPv6 address detection is
  improved, and the node advertises its external address to peers.

- Local block validation: locally mined blocks are enqueued for
  validation before being broadcast, matching the same path as
  remotely received blocks. Blocks are always broadcast after
  successful validation.

- Simulation framework: no-PoW share emitter, sim_overrides module,
  separate sim binary crate, log-based metrics with target rate
  auto-detect, nightly integration test, and simulation runbook.

- TLA+ pruning specification with two-window (Spend and Prune) model
  and blockchain transaction specs.

- Build script that embeds the git version; version is printed in info
  log on startup.

- Docker image builds for release tags. Podman-specific commands added
  to docker compose setup.

- Nightly sim script with configurable dial fanout, lagging node
  detection, and clean starts between runs.

### Changed

- RocksDB bloom filters enabled for column families using point
  queries, improving read performance.

- Limit transient dependencies to only those required, reducing
  compile times and binary size.

- PPLNS window rebuild no longer evicts the cache, avoiding redundant
  recalculations during sync.

- Candidates with available body are now preferred when selecting
  uncles.

- Do not candidatize a block that is already confirmed. Update
  candidates when a local block is confirmed.

- Use fresh timestamp when building work instead of reusing stale
  values.

- Send getheaders when a received block has a missing parent, resuming
  sync without waiting for the next periodic retry.

- Refactor node main into separate build and runner functions. Move
  sim to a separate binary crate with sim_overrides module localising
  pool difficulty, payout distribution, genesis, PoW, and emission
  overrides.

- Remove network parameter from Emission Worker.

- Use Bitcoin Core 31.0 for regtest in sim.

- Docker compose uses net mode host to avoid NAT issues.

- Run docker checks and builds only on release branches.

- Use debug logging level for nightly runs.

### Fixed

- Block receiver no longer early-returns on pending blocks, which
  could cause missed block processing.

- Sim uncle rate metric no longer over-counts under reorg churn by
  counting distinct blocks instead of all appearances.

- Do not advertise observed address with ephemeral source port to
  peers, which caused unreachable address entries in peer routing
  tables.

- Reject inbound messages whose advertised payload length exceeds the
  maximum message size before allocating the payload buffer. Closes a
  DoS/OOM vector where a malicious peer could advertise a large length
  prefix to exhaust memory.

## [v0.12.0] - 2026-06-12

### Changed

- PPLNS payout implementation is selected based on pool mode from
  stratum config. Simple PPLNS uses shares stored in rocksdb for
  hydrapool standalone PPLNS mode. Sharechain PPLNS uses shares from
  rocksdb stored for the chain DAG.

## [v0.11.2] - 2026-06-12

### Fixed

- Fix high-hashrate miners causing 90%+ rejection rate for
  low-hashrate miners in Hydrapool (standalone PPLNS) mode. The share
  chain ASERT difficulty acts as a global floor for all shares. When a
  powerful miner (e.g. Antminer S21 at 200TH) joined a pool of
  NerdAxe miners (~30TH total), ASERT raised the pool difficulty to
  match the combined hashrate, making it impossible for low-hashrate
  miners to produce shares meeting the pool target. In Hydrapool mode
  the ASERT pool difficulty check on submitted shares is now skipped,
  so per-worker vardiff is the sole difficulty control.

### Added

- Pool operating mode config field (`mode` in `[stratum]` section).
  Defaults to `p2poolv2` (full share chain with ASERT enforcement).
  Set to `hydrapool` for standalone PPLNS pools where the ASERT pool
  difficulty check is skipped and the share commitment is omitted from
  the coinbase to save space.

## [v0.11.0] - 2026-05-25

### Fixed

- Fix confirmed chain divergence during sync. Blocks are now confirmed
  strictly in candidate chain order instead of by block body arrival
  order. The candidate chain resolves historical forks via cumulative
  work (ASERT breaks equal-work ties at the next height through
  different parent timestamps), and the confirmed chain follows it.
  Previously, `build_direct_candidates` confirmed whichever block body
  arrived first at each height, which could diverge from the peer's
  confirmed chain, causing PPLNS distribution mismatches and merkle
  root validation failures.

- Add uncle body availability check before block confirmation. Both
  `should_extend_confirmed` and `reorg_confirmed` now verify that all
  uncle bodies referenced by candidate blocks are available in the
  store before promoting them. This prevents missing uncle data in the
  PPLNS window which caused incorrect payout distributions.

- Add fallback confirmation path. When the candidate chain cannot be
  advanced (e.g. stuck on a fork with missing block data), any block
  at confirmed_height + 1 that is a child of the confirmed tip with
  all block and uncle data available can be confirmed. This allows
  locally mined blocks to advance the confirmed chain.

- Trigger confirmed chain advancement when blocks are buffered by the
  organise worker. After a candidate chain reorg, the block at
  `confirmed_tip + 1` may already have its body stored but no
  OrganiseEvent was sent for it. The organise worker now calls
  organise_block when buffering blocks to pick up such blocks.

- Downgrade uncle-on-confirmed-chain validation log from error to
  debug. This validation failure is expected during sync when the
  confirmed chain has not yet settled and is harmless because the
  block body is still stored and confirmed via the candidate chain
  path.

- Cap the candidate chain scan in `find_promotable_candidates` to
  `FETCH_BATCH_SIZE` entries beyond the confirmed tip. Previously
  scanned the entire gap to the candidate tip (up to 50k+ entries),
  causing ~25ms delays per call during sync.

## [v0.10.15] - 2026-05-19

### Fixed

- Gracefully handle bad submit responses from broken ASICs

## [v0.10.15] - 2026-05-17

### Fixed

- Fix worker stats reset on reconnect. We used to replace existing
  worker entries, resetting `shares_valid_total` to zero. Changed to
  preserve accumulated stats across reconnections.

- Add grace period for stats persistence. Inactive workers are kept
  for 6 hours and users for 3 days before being removed from stats,
  allowing reconnections to preserve accumulated history.

## [v0.10.14] - 2026-05-16

### Fixed

- Fix user and worker `shares_valid_total` Prometheus counter overflow.
  The TWO32 multiplication used saturating_mul on u64, capping at
  u64::MAX for users with accumulated difficulty above ~4.29 billion.
  Switched to f64 arithmetic which handles the full range correctly.

## [v0.10.13] - 2026-05-15

### Changed

- Use precomputed merkle branches in share validation instead of
  re-parsing all transaction IDs from hex on every mining.submit.
  The branches are computed once when the block template arrives and
  reused across all miners, eliminating ~30% CPU overhead from hex
  parsing and full merkle tree rebuilds. Submit latency reduced by
  ~33% average and ~51% at p99, throughput increased ~40%.

- Parameterize JMeter stratum test plan so host, port, thread count,
  ramp-up, duration and submit delay can be overridden from the
  command line via JMeter properties.

### Added

- Standalone JMeter load generator script (run_remote_load.sh) for
  driving stratum traffic against a remote server without needing a
  local build or mock-bitcoind.

## [v0.10.12] - 2026-05-14

### Added

- Max stratum connections limit to prevent resource exhaustion from
  too many concurrent miner connections.

- Support for `extranonce.subscribe` stratum extension, allowing
  miners to request extranonce updates.

- Support for user-provided start difficulty with d and th params in
  password field.

- Per-user share valid total prometheus metric for tracking individual
  miner contribution rates.

- Test coverage for max connections limit, empty authorize requests,
  non-hex nonce in submit params, NotSubscribed and InvalidJobId
  stratum error paths.

### Changed

- Stratum error handling refactored to use a dedicated
  `StratumErrorCode` module with a minimal set of error codes,
  replacing the ckpool/blitzpool codes that were not used in
  production. Validation failures now pass the error reason in the
  response message.

- Set coinbase locktime and sequence per BIP54 and rename coinbase
  builder functions for clarity.

- Use Vec for share duplicate detection instead of HashMap.

- Feature gate hydrapool PPLNS share storage so it is only compiled
  when the hydrapool pplns storage feature is enabled.

- Use non-blocking compact console logging and quieter info-level
  logging to reduce log noise.

- Use debug log level for max connections reached events.

- Use share count for all users with non-zero shares in metrics.

- Simplify finding height from coinbase and remove extract height
  from coinbase dead code.

- Update mainnet genesis block.

### Fixed

- Remove unwraps from submit param parsing and diff validator to
  prevent panics on malformed miner input.

- Check for none version masks to avoid panics when version rolling
  is not configured.

- Fix docker signet bootstrap script.

- Fix typos across the codebase.

## [v0.10.11] - 2026-05-09

### Changed

- Share header sync now uses height-based DAG walking instead of
  confirmed-chain walk + uncle reference chasing. The sender collects
  ALL valid blocks at each height from the height index, producing a
  complete topologically sorted subgraph. This fixes sync failures
  caused by missing fork block ancestors that the old approach never
  included.

- Locator matching relaxed from confirmed-only to any valid block
  status (HeaderValid, Candidate, Confirmed, BlockValid). This
  prevents follow-up batch failures when the last header in a batch
  was a fork block.

- Receiver supports multiple chain anchors per batch, allowing
  height-based batches with blocks from different branches to
  validate correctly.

### Fixed

- Fork block ancestors missing from header sync response. The old
  confirmed-chain walk + `collect_uncle_chain` missed fork blocks whose
  `prev_share_blockhash` pointed to non-confirmed blocks, causing
  "parent not in batch or store" errors during testnet4 sync.

- Out-of-order header arrival no longer creates phantom entries at
  height 1 in the height index. `initialise_new_header` now returns an
  error when the parent is missing instead of silently defaulting.

- Missing external parents in a share headers batch now trigger a
  retry with a deeper locator instead of dropping the batch.
  HeaderSyncError enum distinguishes retryable errors (missing
  parent/uncle) from non-retryable errors (ASERT mismatch,
  insufficient work). Retry depth is computed from the lowest anchor
  height in the failed batch.

- `build_locator` and `send_getheaders` accept a depth parameter so
  the retry locator can start further back than the confirmed tip.

## [v0.10.10] - 2026-05-06

### Fixed

- Locator response now starts the confirmed chain walk from
  `anchor_height` - `MAX_UNCLES_DEPTH`, ensuring uncle blocks
  referenced by shares near the anchor are included in header
  batches. Previously the walk started at anchor_height + 1, causing
  "Declared uncle not delivered in batch and not in store" errors that
  broke sync between nodes.

- Block fetcher now dispatches body requests to the announcing peer
  (the inv/headers source) when the chain is current. Previously
  round-robin selection often picked a peer that hadn't received the
  body yet, resulting in NotFound responses and 2-7 second lag on
  non-mining nodes.

- Node now retries header sync every 60 seconds when the confirmed
  tip is stale (older than 300 seconds). Previously a single failed
  sync attempt during handshake left the node permanently stuck with
  no recovery path.

- Locator response now chases transitive uncle references
  (uncle-of-uncle chains) so the receiver has all declared uncle
  bodies available. Previously only direct uncles of confirmed blocks
  were included, causing "Declared uncle not delivered in batch"
  errors when an uncle itself referenced another uncle.

## [v0.10.9] - 2026-05-02

### Fixed

- Ignore verify_chain binary from cargo dist release binaries

## [v0.10.8] - 2026-05-02

### Changed

- Streamline header organisation by simplifying the candidate chain
  extension path and removing redundant lookups during header sync.

- Block fetcher now fetches missing blocks in batches instead of
  one at a time, reducing round-trips during sync.

- Buffer out-of-order blocks in the BlockReceiver actor until their
  parent and uncle dependencies are ready, then validate ASERT
  difficulty and commit atomically. Cascading descendants are driven
  iteratively when ancestors arrive.

- Remove `schedule_dependents` from the organise worker.
  `drain_pending_blocks` and the normal block pipeline already handle
  chain advancement, so scheduling dependents caused 40% redundant
  validation work during sync.

- Refactor `get_candidate_blocks_missing_data` into smaller functions
  for readability: `missing_data_scan_start` and
  `scan_heights_for_missing_blocks`.

### Fixed

- Use confirmed tip (not candidate) in `is_current()` to fix slow
  initial sync. The candidate tip is always recent during sync (each
  newly-received header has a fresh timestamp), which prevented inv
  suppression. Using the confirmed tip correctly identifies the node
  as not-current during initial sync, allowing bulk header-first sync
  to run in batches of 2000 instead of one block per inv message.

- BlockReceiver now checks uncle block bodies via `share_block_exists`
  and fetches missing uncles, preventing confirmation stalls when uncle
  bodies were never retrieved.

## [v0.10.7] - 2026-04-28

### Fixed

- Increase `MAX_TIP_AGE_SECS` from 60s to 300s to prevent chain stall
  where inventory messages were ignored after sync. The chance of a
  60s gap is about 1 in 400. At 5min we have a 1 in trillion chance
  now.

## [v0.10.6] - 2026-04-24

### Fixed

- Fix uncle sync errors by skipping uncles when looking for anchor
  and committing new headers so new branch lookups can see them.

- Respond to getdata block for all known blocks, not just confirmed
  ones.

- Update locator handling to use confirmed chain only and remove
  pre-seeding headers when handling headers messages.

- Initialise new header only if it doesn't exist, avoiding
  overwrites during sync.

- Include uncles when building block fetch list during sync.

- Clear block fetcher for any received block to avoid stale
  in-flight requests.

- Include details in Message::NotFound response.

### Changed

- Follow candidate chain for payout distribution.

- Change the testnet4 genesis to today. This is our testnet4 daa
  anchor. Genesis timestamps are now in genesis data.

- Stop clamping asert daa to bitcoin difficulty.

- Refactor extend and reorg candidate functions for clarity.

- Dashboard: show chain tip and height in page title, show
  difficulty, and fix layout for mobile.

### Added

- Add `debug_tools/share_latency.py` for computing share propagation and
  confirmation latencies across nodes from log files. Supports glob
  patterns, per-pair and aggregate statistics, and --include-uncles flag
  to separately track uncle vs confirmed chain latencies.

## [v0.10.5] - 2026-04-20

### Fixed

- Fix shared headers to always include uncles at chain boundary. This
  bug was causing sync to fail when this corner case was reached.

- Send getheaders in response to Inv blockhash for proper sync. We do
  not immediately send ShareBlock now, instead we let getheaders and
  headers sync drive the chain sync.

- Don't emit BlockFetch for ancestors on BlockReceive, reducing
  redundant fetches. Related to the fix in the previous point.

- Fallback to confirmed tip only on NotFound, propagating real store
  errors instead of silently masking them.

### Changed

- Per-peer rate limiting with dedicated service tasks. Each connected
  peer now gets its own spawned task with an independent rate limiter,
  so one flooding peer cannot block requests from others. Requests are
  forwarded via bounded channels with two-tier overload protection:
  channel full (instant disconnect) and rate limit timeout (sustained
  flood disconnect). Peer service tasks are spawned on connection and
  cleaned up on disconnect.

- Raise default max_requests_per_second from 1 to 100 to match sample
  config and support legitimate sync traffic.

- Remove rate limit window config option. The rate limit config uses
  per second semantics, so the window option was unnecessary.

- Check candidate chain (not just confirmed) for is_current

- Send Inv messages with peer block knowledge for protocol correctness

- Remove bitcoin transactions from ShareBlock to reduce message
  size. We will deal with building bitcoin blocks for submitting from
  other pool peers in a later version.

- Ignore Request ShareBlock messages during sync. This avoids block
  fetch storms during initial sync from both sides of the chain,
  genesis and tip.

- Clean up unused peer id from ShareBlockReceived as we don't respond
  to share block received to the same peer now.

## [v0.10.4] - 2026-04-18

## Added

- cargo dist artefact for dashboard static files

## [v0.10.3] - 2026-04-18

- Dashboard: add favicon
- Dashboard: switch to pico dark mode
- Dashboard: add P2Poolv2 logo in nav bar with mission tagline
- Dashboard: add link to p2poolv2.org landing page
- Dashboard: change logout to link style for consistency
- Dashboard: show difficulty alongside bits in share details

## [v0.10.2] - 2026-04-18

- Add testnet4 genesis block

## [v0.10.1] - 2026-04-16

### Changed

- Update version in Cargo.toml

## [v0.10.0] - 2026-04-16

### Added

- Input value sufficiency validation: non-coinbase share transactions
  are rejected if total input value is less than total output value,
  and total input value must not exceed `Amount::MAX_MONEY`
  
- Sigop cost validation: share blocks are rejected when the aggregate
  BIP141 sigop cost exceeds `MAX_BLOCK_SIGOPS_COST` (80000), using
  `Transaction::total_sigop_cost` with the collected spent outputs

- BIP141 witness commitment on the share coinbase: the coinbase now
  carries a second output containing `SHA256d(witness_root ||
  reserved_value)` over wtxids of the share transactions (coinbase
  wtxid replaced with all-zeros per BIP141), and the coinbase input
  carries the 32-byte witness reserved value on its witness stack

- `validate_share_witness_commitment` validator recomputes the
  witness root and compares it against the embedded commitment

- `StoredTxIn` wrapper persists TxIn witness data in the Inputs CF
  (bitcoin's `TxIn::consensus_encode` does not include witness, since
  BIP144 serializes witnesses at the Transaction level)

### Changed

- **Breaking database change**: the Inputs CF now stores TxIn bytes
  followed by the witness encoding via `StoredTxIn`. Existing
  databases populated by earlier versions cannot be read; users must
  resync from genesis. `rm -rf store.db` and then start again.

- `create_coinbase_transaction` now takes the other share
  transactions so it can compute the BIP141 witness commitment

- `validate_share_coinbase` expects two coinbase outputs (payout +
  witness commitment) instead of one

- `validate_scripts` refactored to `validate_scripts_values_and_sigops`
  which collects spent outputs once per transaction and runs script,
  value, and sigop validation in a single pass

### Fixed

- `test_three_nodes_share_sync` ignored: the
  `test_data/share_sync/share_blocks.json` fixture predates the
  BIP141 share coinbase format and needs regeneration

## [v0.9.1] - 2026-04-13

### Changed

- Updated version in Cargo.toml
- Updated release notes to sync with version

## [v0.9.0] - 2026-04-13

### Added

- Coinbase maturity validation: prevouts spending coinbase outputs are
  rejected unless the containing block is at least 6048 blocks deep
  (70% of blocks-per-day at 10s block time)

- StoredTxOut struct marking outputs with `is_coinbase` flag in Outputs
  CF

- Batch txid-to-blockhash lookups via `get_blockhashes_for_all_txids`

- Updated setup instructions to use prebuilt binaries

### Changed

- Replaced `is_missing_any_prevout` with
  `check_prevouts_and_find_coinbase` combining existence check and
  coinbase detection in a single batch read

- Optimised `are_all_txids_confirmed` to use two batch calls instead
  of per-txid individual lookups

- Deduplicate blockhashes before looking up metadata

## [v0.8.0] - 2026-04-12

### Added

- Sharechain PPLNS accounting with running payout distribution,
  reorg handling, fork buffer, and u128 difficulty (#402, #422, #425,
  #426, #429, #430, #433, #434)
- Share validation pipeline: context-free checks, header validation
  during sync, script validation via libbitcoinconsensus, coinbase
  commitment verification, coinbase payout validation, timestamp
  validation, and transaction prevout spend validation (#414, #417,
  #418, #419, #443, #454, #455, #468, #469)
- WebSocket API support and simple dashboard (#399, #400)
- Batch queries for API requests and API request tracing (#445, #453)
- Donation fees in share commitment with required pairing of
  donation/amount config (#438, #439)
- Prepared notify params as first step toward remote mode (#406, #407)
- Solo mode stratum support (#408)

### Changed

- Switched to address-based identification instead of pubkey (#403)
- Improved header and block storage layout (#415)
- Shared PPLNS window with notifier and validation workers (#436)
- Simplified uncles emissions and payout distribution trait (#401, #428)
- Optimised address map and payout distribution collection (#431)
- Changed coinbase nsecs to u64 and network magic (#456, #435)
- Extracted height computation and subsidy calculation (#442)
- Send notify when chain is extended (#421)
- Build system switched to just; brought back jmeter load testing
  (#471, #472)

### Fixed

- Write atomicity during sync (#466)
- Chain sync reliability and block fetching on reconnects (#459, #463)
- Wait for parent share blocks during normal runs (#464)
- Connection health monitoring improvements (#460)
- Docker setup fixes (#470)
- Task failure handling in main.rs (#437)
- Integration test compatibility with validation (#457)

### Removed

- Unused build_notify and extraneous config files (#440, #458)

## [v0.7.0] - 2026-01-24

### Changed

- Performance Optimizations - Replaced JobTracker actor pattern with
  optimized sync primitives, added TCP nodelay, preallocated hashmaps,
  implemented lazy transaction deserialization, and reused HTTP
  clients for Bitcoin RPC calls

- Graceful Shutdown & Signal Handling - Implemented proper exit signal
  handlers with cross-platform support (Unix + Windows), added
  shutdown reason tracking for correct exit codes, and automatic
  metrics saving on shutdown

- Storage Layer Enhancements - Added new database indexes (uncles,
  transactions, candidates/confirmed, spends tracking), implemented
  RocksDB merge operators for thread-safe updates, and reorganized
  store module architecture

- TLA+ Formal Specification - Created formal specifications for share
  chain candidate/confirmation/reorg logic, added fairness properties,
  and documented uncle chain handling

- Share Chain & Reorg Support - Implemented chain reorganization
  support, added uncle block handling with work contribution to
  chain_work, and improved share commitment mechanism linking miner
  shares to Bitcoin coinbase

- Stratum Protocol Improvements - Added duplicate share detection,
  configurable difficulty handling for testing, optional address
  validation, and two-strike policy for authorization failures

- Metrics & Monitoring Fixes - Fixed user/worker counting, improved
  Prometheus exposition for accepted/rejected shares, added coinbase
  reward split metrics, and filtered inactive users from reporting

- Standalone/Hydrapool Mode - Added support for running without chain
  maintenance, optional accounting bypass for 100% donation mode, and
  configurable miner pubkey coinbase building

## Pre v0.7.0

We used tags like hydrapool.v0.x.0 and we didn't keep a changelog.

[Unreleased]: https://github.com/p2poolv2/p2poolv2/compare/v0.15.2...HEAD
[v0.15.2]: https://github.com/p2poolv2/p2poolv2/compare/v0.15.1...v0.15.2
[v0.15.1]: https://github.com/p2poolv2/p2poolv2/compare/v0.14.1...v0.15.1
[v0.14.1]: https://github.com/p2poolv2/p2poolv2/compare/v0.14.0...v0.14.1
[v0.14.0]: https://github.com/p2poolv2/p2poolv2/compare/v0.13.0...v0.14.0
[v0.13.0]: https://github.com/p2poolv2/p2poolv2/compare/v0.12.0...v0.13.0
[v0.12.0]: https://github.com/p2poolv2/p2poolv2/compare/v0.11.2...v0.12.0
[v0.11.2]: https://github.com/p2poolv2/p2poolv2/compare/v0.11.0...v0.11.2
[v0.11.0]: https://github.com/p2poolv2/p2poolv2/compare/v0.10.15...v0.11.0
[v0.10.15]: https://github.com/p2poolv2/p2poolv2/compare/v0.10.14...v0.10.15
[v0.10.14]: https://github.com/p2poolv2/p2poolv2/compare/v0.10.13...v0.10.14
[v0.10.13]: https://github.com/p2poolv2/p2poolv2/compare/v0.10.12...v0.10.13
[v0.10.12]: https://github.com/p2poolv2/p2poolv2/compare/v0.10.11...v0.10.12
[v0.10.11]: https://github.com/p2poolv2/p2poolv2/compare/v0.10.10...v0.10.11
[v0.10.10]: https://github.com/p2poolv2/p2poolv2/compare/v0.10.9...v0.10.10
[v0.10.9]: https://github.com/p2poolv2/p2poolv2/compare/v0.10.8...v0.10.9
[v0.10.8]: https://github.com/p2poolv2/p2poolv2/compare/v0.10.7...v0.10.8
[v0.10.7]: https://github.com/p2poolv2/p2poolv2/compare/v0.10.6...v0.10.7
[v0.10.6]: https://github.com/p2poolv2/p2poolv2/compare/v0.10.5...v0.10.6
[v0.10.5]: https://github.com/p2poolv2/p2poolv2/compare/v0.10.4...v0.10.5
[v0.10.4]: https://github.com/p2poolv2/p2poolv2/compare/v0.10.3...v0.10.4
[v0.10.3]: https://github.com/p2poolv2/p2poolv2/compare/v0.10.2...v0.10.3
[v0.10.2]: https://github.com/p2poolv2/p2poolv2/compare/v0.10.1...v0.10.2
[v0.10.1]: https://github.com/p2poolv2/p2poolv2/compare/v0.10.0...v0.10.1
[v0.10.0]: https://github.com/p2poolv2/p2poolv2/compare/v0.9.1...v0.10.0
[v0.9.1]: https://github.com/p2poolv2/p2poolv2/compare/v0.9.0...v0.9.1
[v0.9.0]: https://github.com/p2poolv2/p2poolv2/compare/v0.8.0...v0.9.0
[v0.8.0]: https://github.com/p2poolv2/p2poolv2/compare/v0.7.0...v0.8.0
[v0.7.0]: https://github.com/p2poolv2/p2poolv2/compare/v0.1.0...v0.7.0
