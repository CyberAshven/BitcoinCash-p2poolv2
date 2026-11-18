# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Share-chain rule changes on Bitcoin Cash, listed under Breaking, are flag
days: every node upgrades together and starts from a fresh share-chain store
(delete the store directory and restart). Each changes a segment of the
Bitcoin Cash protocol string, so nodes on different rules do not peer.
Bitcoin Cash consensus is unchanged: the pool's blocks are ordinary Bitcoin
Cash blocks. `docs/sharechain/rule-history.md` lists every segment, store
format and genesis re-pin.

### Breaking

- Shares follow Bitcoin Cash's one-minute blocks (CHIP-2025-03) as BCHN does:
  each known BCH header carries its ticks and next interval, and the nBits a
  header must carry, the payout window and the token ASERT band follow them
  (`bch-prev-1off-ticks`, `mm2-roots-asert8-ticks`, store format 12, #136).
- Chipnet switched to one-minute blocks after block 332,450; mainnet,
  testnet, scalenet and regtest switch at median time past 1810382400
  (2027-05-15 12:00 UTC), as in BCHN (#136).
- The weak root also commits the summary of the weak shares a share carries,
  so whole weak shares are pruned one payout depth below the tip and served
  as their summary (segment suffix `-summary`, store format 11, #129).
- Weak shares follow the asks: while more than one in 64 of the weak shares
  a share carries ask for an easier target, the weak ratio rises (up to 4096)
  and a share may carry up to 640 of them; a new chain starts at a weak ratio
  of 32 (`weak4-640-100-48-256-320-64-4096-64-32`, store format 10, #118).
- A carried weak share is held to its carrier's time, not each node's clock,
  so nodes whose clocks differ agree on every share (`weak3-64-100-48-256`,
  store format 9, #103).
- Payout level 0 is no longer valid, as its 546 satoshis fell below dust in
  the payout tree: levels are 1 to 7, and `payout_level = 0` is refused
  (`levels-546x2-k1`, store format 8, #102).
- Weak-share constants from study #85: `WEAK_DEPTH` 6 to 48 shares,
  `INCLUDER_CUT` 5% to 1%, `R_MAX` 4096 to 256 (`weak2-64-100-48-256`, store
  format 7, #98).
- A share's weak root commits each carried weak share's whole encoding, so a
  relaying peer can no longer alter one and get an honest share marked
  invalid (`weak2-64-500-6`, store format 6, #91).
- Weak shares (issue #79): a share carries easier proofs of work on jobs at
  the minimum, and the PPLNS window credits every such proof at the weak
  work, less a cut for the carrying share (`weak-64-500-6`, store format 5,
  #84).
- Payout levels (issue #79): each share header carries its miner's payout
  level, and a payee below `546 * 2^k` satoshis is paid by a draw among the
  slices at its level (`levels-546x2`, store format 4, #83).
- One share chain per network, the former mini chain, with a difficulty
  band: a share may be up to 64 times harder than the minimum, and the next
  share's ASERT anchors at the minimum (`band-c64`, store format 3, #82).
- The genesis share is paid only when nothing else in the window is; it had
  taken a part of every block found in a chain's first hours
  (`genesis-last-resort`, store format 2, #68).
- Mining on the current chain (V16): share messages carry the BCH headers a
  peer needs, and a coinbase pays a share only for work on the paying share's
  BCH chain or one block off it (`bch-prev-1off`, #37).
- Chain work counts the work of each uncle a share names (upstream
  `604fe3ce`), and the store records its format: an older store is refused at
  start-up (store format 1, #37).
- Hybrid payouts replace the 11-payout cap of #30: a share pays its payees
  directly in the coinbase (a token share within 3,072 bytes) and the rest
  through a P2SH32 payout tree (`payout-hybrid-t3072-f500-r1to3-z8-cov1`, #33).
- The donation and operator fee come off each window share's own weight and
  are paid as ordinary payees (`cuts-per-share`, #33). Each has its own 10%
  cap, where the two shared one (#33).
- A share body may hold a 1,000,000-byte coinbase beside 200 KiB of share
  transactions, and payout scripts must be P2PKH, P2SH20 or P2SH32
  (`body-1000000-204800`, `payee-p2pkh-p2sh20-p2sh32`, #33).
- Coinbase merkle branches may have 32 levels, up from 25, and a
  `ShareHeaders` response carries at most 600 headers (`merkle32`, #33).
- Depths come from Bitcoin Cash's 100-share maturity: the payout depth is
  121,063 shares instead of 127,008 (`depths-m100`, #33).
- Share transactions are budgeted by SigChecks, at most 3,000 a transaction
  and 1,452 a share (`sigchecks3000-1452`, #33).
- A coinbase with a SegWit witness commitment makes its share invalid
  (`no-segwit`, #33).
- Shares and uncles are weighted by exact work, not rounded-down difficulty
  (`weight-work`, #33).
- Merge-mining common format v2: every coinbase carries the `CTMM` commitment
  as output 0, and a token share's nBits must sit in the ASERT band, an
  eighth to 8 times the target on test networks (`mm2-roots-asert8`, #33).
- Share-version vote (V17): each share header carries its rule version and
  the version its node desires, so a later rule change can activate on the
  live chain (`version-vote`, #33).
- An uncle may have only one nephew on a chain, and header sync applies the
  timestamp rules of full validation (`uncle-once`, Bitcoin too, #32).
- A coinbase pays each window share's weight to the holder of its payout
  right, so a sold share pays its buyer (`payout-holder`, #26).
- Share transactions may spend the share-trading contract, their outputs
  must be P2PKH or P2SH32 above dust, and they must be final at the share's
  height (`trade-htlc`, #20).

### Added

- The dashboard, server-rendered and WASM, gains a Sharechain chart tab
  (served as JSON at `/sharechain`), income tiles and weak share detail on
  the miner pages. The miner report shows each share's weak count; payouts
  are unchanged (#146).
- A node keeps its peer ID across restarts: its libp2p key is kept in
  `p2p_node_key` beside the store, or at `[network] node_key_path`;
  `ephemeral_node_key = true` makes a new one at every start (#142).
- Peer discovery through Nostr: a node with an `external_address` announces
  it on public Nostr relays every hour, and a node with fewer than 8 peers
  dials the newest announcements for its network and rules (#143).
- `[network] nostr_relays` (six public relays by default; empty turns Nostr
  off) and `nostr_announce` (default true). `dial_peers` is no longer
  required (#143).
- A restarted node remembers its peers: they are saved to `p2p_peers.json`
  beside the store, and the ones it held over outgoing connections are
  dialed first on the next start (#144).
- A node keeps at least 16 outgoing peers (`[network] target_outgoing_peers`),
  at most one per IPv4 /16 or IPv6 /32; the sample configs raise
  `max_established_incoming` from 50 to 128 (#122).
- `store.keep_whole_weak_shares = true` keeps whole weak shares below the
  payout depth, for an archive (#129).
- `template_provider_proxy`: a Stratum V2 Template Provider behind Tor or
  I2P, reached only through a SOCKS5 proxy, each connection on its own Tor
  circuit (#133).
- Both Umbrel apps run Prometheus and Grafana on the Bitcoin Cash dashboards,
  Grafana on port 3342 (`bch-p2pool`) or 3343 (`bch-p2pool-external`) (#139).
- The terminal UI's System tab starts, stops and restarts the
  `p2poolv2@<instance>` systemd user service, from upstream's PDM through
  `b7d97c34` (#138).
- Share relay metrics (`p2p_share_relay_*`) and weak share fill metrics
  (`weak_shares_filled_from_pool_total` and others) (#121).
- Weak share metrics on `/metrics`: found, relayed, carried, expired and
  refused by reason, the pool's size, and the tip's weak ratio and band cap
  (#90).
- The dashboard's miner page and `/miner` show a miner's expected income and
  payments a month and its monthly swing (#90).
- Miners choose a payout level at login with `min-payout=<satoshis>` or
  `fee-tolerance=<percent>`; without either the node's `[stratum]
  payout_level` applies, level 5 by default (#83).
- Payout simulator tables for the small-miners design (`scale`,
  `scale-levers`, `band-step`, `window-cap`, `storage`, `r-max-mixes`,
  `weak-asks-*`) and a limits study in `docs/simulation/network-sim.md`
  (#98, #100, #107, #116, #117, #119, #124).
- The network and payout simulators model one-minute Bitcoin Cash blocks
  (#135, #140).
- A test loads the dashboard in headless Chrome and follows every link a
  visitor could; CI keeps a screenshot of each page (#108).
- Stratum V2 Worker-Specific Hashrate Tracking (`0x0002`): a proxy can name
  the machine behind each share, shown as a worker of the channel's payout
  address (#77).
- The Stratum V2 Template Provider client asks for the large templates
  extension `0x4C54`, so it takes any template the block size limit allows
  (#76).
- BCHD support: templates ask for `coinbasevalue`, a node that does not
  report `initialblockdownload` is asked for a template instead, and BCHD's
  network names pass the start check (#73).
- Regtest tests of the per-share donation against BCHN, run nightly (#67).
- `VerifyPayoutOutputs` (`0x04` in extension `0x5052`): a Job Declaration
  client can ask a second node to recompute its payout outputs (#57).
- Extension `0x5052`, Requested Payout Outputs: a standard SV2 Job
  Declaration client gets a job's exact coinbase outputs on its Job
  Declaration connection, without `/jd_prepare` (#47).
- Full-Template Job Declaration of any template the Bitcoin Cash block size
  limit allows, through the experimental extension `0x4C54` (#63).
- Electrum `blockchain.transaction.get` and `blockchain.block.header` for the
  share chain, ported from upstream, with Bitcoin Cash depths (#54, #56).
- `node_p2p_address`: hear new blocks on the Bitcoin Cash node's P2P port,
  for nodes without ZMQ such as BCHD and Knuth (#42).
- `template_source = "rpc_light"`: light templates from BCHN
  (`getblocktemplatelight`, `submitblocklight`), with full templates as the
  fallback (#41).
- Payouts to P2SH32 addresses, which contracts and CashTokens covenants use
  (#37).
- A cargo-fuzz harness for the P2P message decoder: a 60-second run on pushes
  and pull requests and an hour nightly (#37).
- `cargo safa claimer` follows a SAFA vault and verifies each claim in the
  BCH 2026 VM before broadcasting; the tSAFA test token is deployed on
  chipnet (#37).
- Offline PHOTON payout studies and the `p2poolv2_photon_payout` prototype
  (`cargo photon-payout-sim`, `photon-lean-measure`, `photon-payout-measure`),
  with no node integration (#37).
- Merge-mined tokens: `[stratum] merge_mining_tokens` names a JSON file of
  tokens, and each pool-built job commits its miner's leaves, with the
  donation and the node's fee in each win's split (#33).
- SAFA vaults in the token file: a job's leaf takes the claim target the
  covenant computes at the job's height (#33).
- Leaf and win disclosure in Pickaxe's formats: SV2 extension `0x4D4D`, SV1
  `pickaxe-merge`, and JSON line logs beside the token file (#33).
- A Full-Template declaration whose output 0 is a token root discloses its
  leaves in `excess_data`, each paying the channel's miner (#33).
- A Sponsor button, and a donate section on the landing page with the
  donation address and its QR codes (#33).
- Each miner chooses its own donation with `donation=<percent>` (0 to 10, in
  steps of 0.5) in its username, SV2 identity or SV1 password; the node's
  `donation` is the default. See `docs/miner-options.md` (#33).
- Bitcoin Cash scalenet (`bitcoin_cash_network = "scalenet"`), with its own
  share chain (#33).
- Token-aware CashAddr payout addresses (`z...`, `r...`) are accepted wherever
  a payout address is read (#33).
- A Bitcoin Cash node refuses to start unless its node is on
  `bitcoin_cash_network`, checked by chain name and a block only Bitcoin Cash
  has (#33).
- A landing page in `website/`, ported from upstream's website, in HTML and
  CSS only (#33).
- `p2poolv2_tui`, a terminal UI for a Bitcoin Cash node and its BCHN, ported
  from upstream's PDM (#29).
- An Umbrel community app store in `packaging/umbrel`: `bch-p2pool` on the
  official Bitcoin Cash Node app, `bch-p2pool-external` on a BCHN elsewhere
  (#33).
- Bitcoin Cash share trading: a miner sells share units for BCH with a hash
  time locked contract on each chain, built and signed offline by
  `p2poolv2_cli bch-trade` (#20).
- A share transaction pool: nodes check, relay and mine signed share
  transactions, handed in by `POST /share_transaction` or `p2poolv2_cli
  transactions submit` (#26).
- Block bodies are pruned below `PRUNE_DEPTH + PRUNE_INTERVAL`; headers are
  kept for header sync (from upstream, #19).
- Built-in DNS seeds and `network.dns_seed` (from upstream), always off on
  Bitcoin Cash, which has no seeds (#19).
- The `pplns-compare` feature of `p2poolv2_sim` (#19).
- `/miners` serves this node's miners and workers as JSON, and `/network`
  names the chain the node mines (#25).
- A docs site built with mdBook from `docs/`, with upstream's documentation
  pages ported and adapted to Bitcoin Cash (#31, #33).
- Docker: a BCHN 29.2.0 image and a regtest stack that mines through the
  pool (#31).
- Ansible: Bitcoin Cash config keys and a `monitoring_p2poolv2` role (#31).

### Changed

- The header rules of the weak shares a share carries are checked on several
  cores, one fewer than the node's CPUs; the verdict is unchanged (#127).
- A new share with 50,000 bytes or more of coinbase outputs is relayed
  without them, and peers rebuild them from their PPLNS window (message
  segment `without-payouts`; nodes restart together, no store change, #131).
- A new share lists the weak shares it carries by hash, sending whole only
  the ones peers probably lack (message segment `weak-by-hash`; nodes restart
  together, no store change, #121).
- A share a peer sent is relayed once its header checks out, before its body
  is validated, and uncles come only from fully validated shares (#128).
- A Stratum V2 Job Declaration client stays on the pool as the share chain
  moves: declared jobs and job tokens are kept per connection, and a tip
  change answers `stale-chain-tip` or `template-unavailable` (#115).
- The Docker image runs the node as `p2pool` (uid 1000); a `p2poolv2_data`
  volume from an earlier image must be removed or given to uid 1000 (#109).
- The Umbrel apps' dashboards move from ports 46890 and 46891 to 36890 and
  36891, out of the 40000 to 49999 range umbrelOS 2.0 keeps for Umbrel
  Machines (#137).
- A node's default never gives a miner more than the minimum's work over the
  ceiling's, so in a pool too small for 10-second shares at the ceiling every
  miner mines at the minimum (#94).
- A node lowers a miner's payout level when it would expect fewer than 3
  payments a month at it (#83).
- A node gives a miner that would make more than 4 shares per pool block, or
  300 a month, a target up to `C` times harder, so big miners make fewer,
  heavier shares (#82).
- The weak pool remembers at most 1,024 peers' rate limits, a flood of held
  weak shares pushes out its sender's own first, and unknown parents are
  asked for at most once a second (#90, #113).
- Weak shares are checked more cheaply: one whose parent is too deep is
  dropped first, and window weights are cached per parent and BCH block
  (#90).
- The PPLNS window finds a payee's key with one lookup instead of a scan, so
  large windows load in seconds instead of minutes (#101).
- A Bitcoin Cash node writes upstream's script history index only while it
  serves Electrum (#88).
- Coinbase-Only Job Declaration is taken only from `[stratum]
  sv2_coinbase_only_from`, by default this computer; `["0.0.0.0/0", "::/0"]`
  restores the old behaviour (#64).
- `public_dashboard` also opens `GET /peers` and a tokenless `/ws` (#55).
- A node withdraws mining work after 60 seconds without a template from its
  node or its Template Provider, so miners can fail over (#69, #71).
- The configs, packaging, terminal UI and docs offer only P2Pool mode;
  Hydrapool's code stays as upstream has it (#53).
- The SAFA tools refuse a one-thread deployment (#48).
- A node remembers candidate reorgs it refused for depth, so a repeat costs
  a lookup (#39).
- A share naming a parent or uncle the node has no header for is dropped and
  its headers fetched, instead of buffered (#38).
- A Bitcoin Cash node learns its node's headers, for up to 5 minutes, before
  it dials peers (#38).
- A peer's share body is stored only once its BCH prev is known; share
  headers never wait (#37).
- A share counts as a found block only once BCHN accepts it (#37).
- The Template Provider reserves 0 SigChecks for the coinbase instead of
  1,922 sigops (#37).
- A job that does not fit BCHN's block size limit withdraws work instead of
  stopping the node (#37).
- A declared token job must pass the token-share ASERT band before BCHN
  checks it (`invalid-token-asert`, #37).
- The SAFA covenant counts CHIP-2025-03 ticks for its band, claim lock and
  target easing. Chipnet needs a new tSAFA deployment, and mainnet's
  activation block is not pinned yet (#141).
- Node-local one-minute changes: payout trees spend 60,000 ticks after their
  block, the found-block verifier counts 6,000, and `/hashrate` reports
  `block_interval_secs` (#136).
- A new store learns its BCH tip region first, in chunks of 2,000 headers,
  and a walk that stops is finished in later rounds (#136).
- Synced with upstream P2Poolv2 through `75c4d7ea`: header sync serves every
  share at a height and pages inside one, in place of #74's order (message
  segment `headers-resume`, nodes restart together; testnet5 left out, #152).
- Synced with upstream P2Poolv2 through `fdbf85df` (v0.18.0 and the
  Whatsminer change): host names in `dial_peers`, and a `d=` or `th=`
  difficulty that `mining.suggest_difficulty` no longer overrides; testnet5
  is left out (#120, #134).
- Synced with upstream through `c2287b28` (v0.17.0): Electrum header and
  script-hash subscriptions, libp2p 0.57, the coverage gate and the nix crane
  build; testnet5 is left out (#88).
- Merged upstream main `c0c70cdb` with git: an Electrum server for the share
  chain, Ansible installs from source and a nix devShell (#37).
- Synced with upstream through `61810c56` (v0.16.2); the fork keeps version
  `0.0.1-alpha` (#19).
- `network.dial_peers` is optional, Hydrapool shares go to their own PPLNS
  worker, Bitcoin genesis shares load from `.dat` files, and the sim build
  compiles short depths (from upstream, #19).
- A found block's payout tree is kept before the block is submitted and
  stays open until every transaction is confirmed; a tail too small for its
  fee goes to the largest payee (#33).
- Stratum V2 extended channels refuse a coinbase prefix or suffix over 65,535
  bytes; standard channels and Stratum V1 carry the whole coinbase (#33).
- Share transactions are ranked by the share units they burn per byte, and a
  full pool evicts only for a transaction that outranks the lowest (#30, #33).
- The SAFA covenant tool is Rust (`p2poolv2_safa`, `cargo safa`), on the BCH
  2026 VM of libauth-rs, and verifies merge-mining format v2 with the token
  rules agreed with Pickaxe (#33).
- `p2poolv2_netsim` treats the hybrid payout as the Bitcoin Cash payout rule
  (#33).
- The organise channel and pending buffer are bounded in bytes, 64 and 128
  MiB, as well as by count (#33).
- Bitcoin Cash defaults: a 1% donation instead of 1.5%, and a 1% operator
  fee when `fee_address` is set without `fee` (#33).
- Prometheus names drop Bitcoin: `found_blocks_total`,
  `found_block_time_seconds` and the `payout_address` label. Dashboards of
  your own need the same renames (#33).
- Miners are keyed by the canonical text of their payout script, so every
  spelling of one CashAddr is one miner (#33).
- `bch-trade` signs with Bitcoin Cash Schnorr instead of ECDSA (#33).
- `config.sample.toml` is a Bitcoin Cash chipnet node; upstream's Bitcoin
  sample is `config-bitcoin.sample.toml` (#33).
- The developer tools are Rust (`p2poolv2_tools`: `cargo sim`, `cargo
  mock-bitcoind` and the log analysers); see `docs/tooling.md` (#33).
- The sim swarm and its nightly run Bitcoin Cash against BCHN (#33).
- The candidate chain refuses a reorg forking more than `PAYOUT_DEPTH` below
  the confirmed tip (#32).
- Stratum `maximum_difficulty` defaults to 2^32, and a session's difficulty
  is at least 1 (#32).
- The Docker stacks run BCHN on regtest, chipnet or mainnet, and the Ansible
  role installs this fork's release (#31).
- The Grafana dashboards link found blocks to Bitcoin Cash explorers and
  show amounts in BCH (#31).
- The sample configs offer BIP 320's version rolling mask, `1fffe000` (#31).
- README rewritten for the Bitcoin Cash fork (#31).
- The Rust/WASM dashboard looks and behaves like the node's own and takes
  its stylesheets from the node's assets (#25).

### Removed

- The `bch_share_chain` setting: a config that still sets it is refused at
  start-up, and the Docker, Umbrel and Ansible store folders are named after
  the network alone (#82).
- The Node SAFA tool `tools/safa-auxpow` and its v1 regtest harness, replaced
  by `p2poolv2_safa` (#33).
- The decoded copy of every template transaction kept for Stratum V2 (#33).
- The `store.background_task_frequency_hours` option (from upstream);
  configs that set it still load (#19).
- The `[sim] coinbase_maturity` option (#19).

### Fixed

- Templates from a node without long polling, such as Knuth, parse again: a
  missing `longpollid` reads as empty (#123).
- Requests to the Bitcoin Cash node time out after 30 seconds without data,
  or 10 to connect, so a dead connection no longer freezes templates (#114).
- A syncing node no longer marks an honest share invalid over a weak share
  paid from shares it never fetched: the body prune depth adds `WEAK_DEPTH`
  (#132).
- A weak share carried by a share that becomes an uncle is carried again
  (#90).
- A weak share's proof of work is remembered only once it passes its checks,
  so an altered copy cannot keep the real one out (#90).
- A weak share whose parent is unknown is held while the parent's headers
  are fetched, instead of dropped (#90).
- A new node syncs a share chain with more than 20 shares at one height
  (#74).
- CTOR sorting hashes each transaction once (#70).
- RPC calls no longer fail on a connection BCHN just closed (#59).
- A job a miner declares over Stratum V2 no longer becomes the node's latest
  job (#58).
- A Stratum V2 share for a job on the BCH block a new tip just replaced is
  credited again (#65).
- The Job Declaration server handles `PushSolution`: it rebuilds the block
  and submits it, instead of closing the connection (#62).
- `config.sample.toml` sets `wait_for_chain_sync = false`, so the first node
  of a new share chain opens its Stratum port (#61).
- The sim harness runs on Windows (#52).
- A node no longer stops when a share transaction's stored rows are missing
  (#50).
- A Hydrapool-mode node starts again after the share-chain genesis changes
  (#51).
- `safa claimer` no longer stops for good when chipnet replaces the block it
  last checked while the vault is unspent (#46).
- The node's Docker image builds again (#43).
- A candidate reorg no longer takes confirmed shares off the confirmed chain
  (#40).
- The Apple Silicon macOS release build runs on `macos-15` (#72).
- A block spending an output the node does not hold is stored with
  `CoinbaseRoot::Unknown` instead of stalling the node; the stored output
  format changed (#32).
- The SAFA covenant's chipnet ASERT band is an eighth to 8 times the computed
  target, so backwards timestamp skew passes; deployments refuse 64 or more
  threads (#33).
- A miner's stratum difficulty never goes above the share difficulty of its
  job (#33).
- A found block no longer depends on one RPC call: it is sent again for up
  to two minutes, and `[[submit_nodes]]` lists extra nodes it also goes to
  (#33).

### Security

- `libz-sys` 1.1.33 closes RUSTSEC-2026-0544, a heap overflow in zlib's
  `gz_vacate` that RocksDB links. `chrono`, declared unmaintained
  (RUSTSEC-2026-0543), is replaced by `time`; timestamps print as before,
  and the CLI now sends `/pplns_shares` time filters as
  `2025-10-17T19:40:00Z` instead of `+00:00` (#145).
- `/metrics` escapes its label values, so a worker name or payout address
  can no longer break the scrape or add samples (#78).
- The Docker stack's chipnet and mainnet BCHN make a random RPC password on
  first start instead of shipping `p2pool` (#33).
- Cleared three cargo-deny advisories: tokio 1.54.1, zstd-sys 2.1.2, and
  crypto-bigint ignored until k256 0.14 (#33).
- The buffer of blocks waiting for their parent is bounded: 1,024 blocks,
  32 MiB, 10 minutes each (#32).
- A huge requested difficulty no longer overflows the difficulty adjuster
  (#32).
- Updated `rustls-webpki` to 0.103.16 (from upstream, #19).

## [v0.0.1-alpha] - 2026-10-18, refreshed 2026-10-19 and 2026-10-22

First alpha of P2Poolv2 for Bitcoin Cash, synced with upstream P2Poolv2
through `2dabcdd2`. Alpha software: run it on chipnet or regtest first.
The pre-release is refreshed in place with `main` until the beta. The
2026-10-19 and 2026-10-22 refreshes change share-chain rules (the payout
window and the ASERT anchor, under Changed): nodes start from a new store.

### Added

- Bitcoin Cash: `stratum.chain = "bitcoin-cash"` with `bitcoin_cash_network`
  mines from BCHN templates, pays CashAddr scripts, records P2PKH share owners
  and starts from a main or mini Bitcoin Cash genesis. See `docs/bch-plan.md`
  and `docs/bch-design.md`.
- Bitcoin Cash donation: 1.5% of each block a node finds goes to the P2Poolv2
  donation address by default. `donation` raises or lowers it and 0 opts out;
  `fee_address` and `fee` remain the operator's own cut.
- `p2poolv2_cli bch-share-address` prints the Bitcoin Cash share address for a
  P2PKH CashAddr.
- A local Stratum V2 listener (`sv2_port`) with both Job Declaration modes,
  standard and extended channels, and optional Noise encryption.
- Bitcoin Cash share transfers are checked by a Bitcoin Cash interpreter
  (P2PKH with `SIGHASH_FORKID` ECDSA or Schnorr).
- `docs/architecture/payout-distribution.md` describes the whole payout
  scheme, including the maturity window.
- The Template Distribution client and a template refresh after a found
  block (2026-10-19).
- `GET /miner?address=...` returns one miner's shares, weight and part of
  the next payout from the PPLNS window (2026-10-19).
- The dashboard is rendered in Rust by the node, without scripts, building
  on the design in PR #8 by CashDragon0 (2026-10-22).
- `[api] web_root` serves a self-hosted frontend, and `public_dashboard`
  opens the dashboard and read-only routes for a public node (2026-10-22).
- The Rust/WASM dashboard (`p2poolv2_web`) with `/hashrate` and
  `/found_blocks`, and `/jd_prepare` for external SV2 job declarators
  (2026-10-22).

### Removed

- The JavaScript dashboard (`p2poolv2_api/static`), its `dashboard.tar.gz`
  release archive and `P2POOL_STATIC_DIR`.

### Fixed

- A Stratum V2 channel starts at the difficulty its declared
  `nominal_hash_rate` settles on (#21).
- A Bitcoin Cash share address in `p2p=` ends at its first dot, so the SV2
  translator's `.<worker>` suffix no longer gets a channel refused (#17).
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
  instead of genesis, so an outage no longer leaves shares at the easiest
  difficulty; Bitcoin keeps the genesis anchor (`asert-parent`, 2026-10-22).
- The Bitcoin Cash PPLNS window is sized by the paying block's difficulty
  behind a 100-share maturity (`pplns-k3-m100`, 2026-10-19).
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
