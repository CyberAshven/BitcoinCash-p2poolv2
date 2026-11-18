# Upstream port map

Where each page of upstream P2Poolv2's documentation site lives in this book,
or why it was not ported.

The source is [p2poolv2/docs](https://github.com/p2poolv2/docs) at commit
`623e4c1` (2026-10-22), AsciiDoc pages describing P2Poolv2 for Bitcoin at
upstream commit `dc5bb65` (2026-10-08), Copyright 2024-2026 P2Poolv2
Developers, MIT or Apache-2.0. Every ported page names its source and keeps
that notice (this fork is AGPL-3.0, plan decision 13), has its diagrams
redrawn as text, and states the Bitcoin Cash situation wherever upstream
rests on a Bitcoin constraint. The `docs/` folder of
[upstream's code repository](https://github.com/p2poolv2/p2poolv2) is a
different source, already in this book.

## Pages

| Upstream page | This book | Bitcoin Cash notes |
|---|---|---|
| `index.adoc` | [Introduction](introduction.md) | The fork's own. |
| `getting_started.adoc` | [Running P2Poolv2 on Bitcoin Cash](bch-quickstart.md) | No public node (upstream points to its testnet4 node). |
| `why_p2poolv2.adoc` | [Why P2Poolv2](why-p2poolv2.md) | Block space, fees, SegWit, Lightning, covenants, coinbase size. |
| `requirements.adoc` | [Requirements](requirements.md), "Design requirements" | This fork's status on each. |
| `p2p_network_of_miners.adoc` | [P2P network of miners](p2p-network.md) | One share chain per network, rule tags, no DNS seeds. |
| `node_components.adoc` | [Node components](node-components.md) | BCHN or a Template Provider for bitcoind. |
| `stratum.adoc` | [Stratum server and vardiff](stratum.md) | Miner options on [Miner options](miner-options.md). |
| `pool_difficulty_adjustment.adoc` | [Stratum server and vardiff](stratum.md), "The pool difficulty" | ASERT ([Share difficulty: ASERT](difficulty_adjustment/asert.md)). |
| `architecture/sharechain.adoc` | [Share chain](sharechain/README.md) | No witness commitment; payout rights. |
| `architecture/share_commitment.adoc` | [Share commitments](sharechain/share-commitments.md) | Share transaction root over txids; longer coinbase merkle branch. |
| `architecture/validation.adoc` | [Share validation](sharechain/validation.md) | Bitcoin Cash scripts, `uncle-once`, what BCHN checks. |
| `architecture/bitcoin_chain_validation.adoc` | [Mining on the current chain](sharechain/current-chain.md) | Built with an aserti3-2d header check; upstream's design is [Mine on the current Bitcoin chain](architecture/mine-on-bitcoin-chain.md). |
| `architecture/pplns_accounting.adoc` | [PPLNS accounting](sharechain/pplns-accounting.md) | Three blocks of work (at most 120,960 shares) behind a 100-share maturity, for 120,960 shares behind 6,048. |
| `architecture/transactions.adoc` | [Share transactions](sharechain/transactions.md) | CashAddr owners; fees burned as node policy. |
| `architecture/spending_utxos.adoc` | [Spending share outputs](sharechain/spending-outputs.md) | Earnings follow the payout right. |
| `architecture/pool_scalability_limits.adoc` | [Pool size limits](sharechain/pool-size.md) | A title only upstream. |
| `architecture/share_flow.adoc` | not ported | A sample page off upstream's site; see [Share processing pipeline](architecture/share-processing-pipeline.md). |
| `payouts/index.adoc` | [Payouts](payouts/README.md) | |
| `payouts/coinbase.adoc` | [Coinbase payouts](payouts/coinbase.md) | Donation, fee, payout tree. |
| `payouts/marketplace.adoc` | [Selling shares](payouts/selling-shares.md) | Covenants; a window of three blocks of work, not two weeks. |
| `payouts/onchain.adoc` | [Selling shares](payouts/selling-shares.md), "On-chain swaps" | A heading upstream; built here as [Trading shares for BCH](atomic-swap/bch-share-trading.md). |
| `payouts/l2_swaps.adoc` | [Selling shares](payouts/selling-shares.md), "Off-chain swaps" | Ark and Lightning "coming soon" upstream; not planned here (plan decision 13). |
| `payouts/marketplace_ux.adoc` | [Selling shares](payouts/selling-shares.md), "Marketplace application" | Headings only upstream; no application here. |
| `payouts/pricing_shares_as_options.adoc` | [Selling shares](payouts/selling-shares.md), "Pricing a share" | A heading upstream; an open question here, with an estimate. |
| `handling_known_attacks.adoc` | [Known attacks and threat model](known-attacks.md) | "Coming soon" upstream; adapted from the list in its comments. |
| `compare_datum_sv2.adoc` | [Comparison with SV2 and DATUM](compare-datum-sv2.md) | Job Declaration without an operator. |
| `dashboards.adoc` | not ported | A title only upstream. Here: the built-in dashboard, [Grafana dashboards](deployment.md#grafana-dashboards), [Metrics](architecture/metrics.md). |
| `stories/index.adoc`, `stories/single_miner.adoc` | not ported | Placeholders upstream. |

Not pages, not ported: `presentations/` (a Bitcoin conference talk); the
Jekyll build (`README.adoc`, `_config.yml`, `Gemfile`, `Rakefile`,
`.github/workflows/pages.yml`, `CNAME`; this book builds with mdBook,
`docs/book.toml`, in the `Docs` CI job); the theme (`_sass/`, `_includes/`,
`_plantuml/`, `assets/`); `_snippets/stratum-config.toml` (a `[stratum]`
snippet no page includes, with a Bitcoin testnet4 address); and notes for
upstream's docs repository (`AGENTS.md`, `CLAUDE.md`, `DOC_CODE_REVIEW.md`).

To bring a page up to date, diff `p2poolv2/docs` from `623e4c1` to its new
head, port the changes into the page named above, and move the commit named
in that page's header and at the top of this page.

## Code from upstream

Upstream's code comes in by `git merge` of `upstream/main` (first merged at
upstream `c0c70cdb`). Changes copied by hand before that (the four uncle
commits, the decoder fuzz harness, the libzmq fix) keep this fork's version.
Upstream's Electrum server crate (off unless `[electrum] enabled`), ansible
source install, node preflight check, per-host vaults, nix devShell and CI
runner updates came in; its `grafana_p2poolv2` ansible role stays unused
beside this fork's `monitoring_p2poolv2`.

The merge of upstream `c2287b28` (v0.17.0) brought these `docs/` pages up to
that commit:

| Upstream page | This book | Notes |
|---|---|---|
| `docs/architecture/async-flow.md` | [Async flow and concurrency](architecture/async-flow.md) | Monitoring broadcast and Electrum tip watcher rows, beside the share transaction relay row. |
| `docs/architecture/pruning.md` | [Pruning](architecture/pruning.md) | Script history and its compaction filter; on Bitcoin Cash the window is `BCH_PAYOUT_DEPTH`. |
| `docs/architecture/store-schema.md` | [Store schema](architecture/store-schema.md) | The `ScriptHistory` column family. |
| `docs/testnet5.md` | not ported | A Bitcoin test network; upstream's `Chain` config type for it collides with this fork's `chain` setting. |

The Electrum crate's `README.adoc` came with the code. Shell scripts upstream
changed under `load-tests/sim/` stay deleted here; their changes (the
promoted block log line, the BIP 320 version mask) are in `cargo sim`
([Tooling](tooling.md)).
