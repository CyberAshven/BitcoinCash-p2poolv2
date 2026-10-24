# Upstream port map

Where each page of upstream P2Poolv2's documentation site lives in this book,
or why it was not ported.

The source is [p2poolv2/docs](https://github.com/p2poolv2/docs) at commit
`623e4c1` (2026-10-22), a Jekyll site of AsciiDoc pages describing P2Poolv2
for Bitcoin at upstream commit `dc5bb65` (2026-10-08). It is Copyright
2024-2026 P2Poolv2 Developers, distributed under the MIT or Apache-2.0
license. Every ported page names its source and keeps that notice; this fork
is distributed under AGPL-3.0 (plan decision 13). The pages are rewritten in
Markdown for mdBook, with PlantUML and Graphviz diagrams redrawn as text, and
adapted to Bitcoin Cash: where upstream rests on a Bitcoin constraint, the page
states the Bitcoin Cash situation and its source instead.

This is a different source from upstream's code repository
([p2poolv2/p2poolv2](https://github.com/p2poolv2/p2poolv2)), whose `docs/`
folder this book already carried (the [Architecture](architecture/README.md)
pages, difficulty, simulation and atomic swap pages).

## Pages

| Upstream page | This book | Notes |
|---|---|---|
| `index.adoc` | [Introduction](introduction.md) | The fork's own introduction; its "How it works" list links the ported pages. |
| `getting_started.adoc` | [Running P2Poolv2 on Bitcoin Cash](bch-quickstart.md) | Upstream points to its README and its public testnet4 node. This fork lists no public node. |
| `why_p2poolv2.adoc` | [Why P2Poolv2](why-p2poolv2.md) | Adds what differs on Bitcoin Cash: block space, fees, SegWit, Lightning, covenants, coinbase size. |
| `requirements.adoc` | [Requirements](requirements.md), "Design requirements" | Each requirement with this fork's status. |
| `p2p_network_of_miners.adoc` | [P2P network of miners](p2p-network.md) | Main and mini share chains, protocol strings with rule tags, no DNS seeds on Bitcoin Cash. |
| `node_components.adoc` | [Node components](node-components.md) | BCHN or a Stratum V2 Template Provider in place of bitcoind. |
| `stratum.adoc` | [Stratum server and vardiff](stratum.md) | Its miner options table is on [Miner options](miner-options.md), with Bitcoin Cash addresses. |
| `pool_difficulty_adjustment.adoc` | [Stratum server and vardiff](stratum.md), "The pool difficulty" | ASERT is also Bitcoin Cash's own block algorithm; details on [Share difficulty: ASERT](difficulty_adjustment/asert.md). |
| `architecture/sharechain.adoc` | [Share chain](sharechain/README.md) | No witness commitment; payout rights. |
| `architecture/share_commitment.adoc` | [Share commitments](sharechain/share-commitments.md) | Share transaction root over txids; longer coinbase merkle branch under ABLA. |
| `architecture/validation.adoc` | [Share validation](sharechain/validation.md) | Bitcoin Cash script interpreter, `uncle-once`, what BCHN checks. |
| `architecture/bitcoin_chain_validation.adoc` | [Mining on the current chain](sharechain/current-chain.md) | Not built on Bitcoin Cash yet; needs an aserti3-2d header check. The full design is [Mine on the current Bitcoin chain](architecture/mine-on-bitcoin-chain.md). |
| `architecture/pplns_accounting.adoc` | [PPLNS accounting](sharechain/pplns-accounting.md) | Window of three Bitcoin Cash blocks of work behind a 100-share maturity, in place of 120,960 shares behind 6,048. |
| `architecture/transactions.adoc` | [Share transactions](sharechain/transactions.md) | CashAddr share owners, fees burned as node policy instead of paid to the miner. |
| `architecture/spending_utxos.adoc` | [Spending share outputs](sharechain/spending-outputs.md) | Earnings follow the payout right, not the root height. |
| `architecture/pool_scalability_limits.adoc` | [Pool size limits](sharechain/pool-size.md) | Upstream's page is a title only. Written for Bitcoin Cash: the limits that apply, and the hybrid payout. |
| `architecture/share_flow.adoc` | not ported | A sample page upstream excludes from its site; its diagrams are illustrative. The real flow is [Share processing pipeline](architecture/share-processing-pipeline.md). |
| `payouts/index.adoc` | [Payouts](payouts/README.md) | |
| `payouts/coinbase.adoc` | [Coinbase payouts](payouts/coinbase.md) | Bitcoin Cash outputs, donation and fee; the hybrid payout and its payout tree. |
| `payouts/marketplace.adoc` | [Selling shares](payouts/selling-shares.md) | Covenants exist on Bitcoin Cash; the window is three blocks of work, not two weeks. |
| `payouts/onchain.adoc` | [Selling shares](payouts/selling-shares.md), "On-chain swaps" | Upstream's page is a heading only. Built here: [Trading shares for BCH](atomic-swap/bch-share-trading.md). |
| `payouts/l2_swaps.adoc` | [Selling shares](payouts/selling-shares.md), "Off-chain swaps" | Upstream's Ark and Lightning swaps are "coming soon". Not planned on Bitcoin Cash (plan decision 13). |
| `payouts/marketplace_ux.adoc` | [Selling shares](payouts/selling-shares.md), "Marketplace application" | Upstream's page is headings only; no application here either. |
| `payouts/pricing_shares_as_options.adoc` | [Selling shares](payouts/selling-shares.md), "Pricing a share" | Upstream's page is a heading only. Marked as an open question, with an estimate for Bitcoin Cash. |
| `handling_known_attacks.adoc` | [Known attacks and threat model](known-attacks.md) | Upstream's page shows "coming soon" and keeps its list in comments; this fork adapted that list earlier. |
| `compare_datum_sv2.adoc` | [Comparison with SV2 and DATUM](compare-datum-sv2.md) | Adds how this fork uses Stratum V2 Job Declaration without an operator. |
| `dashboards.adoc` | not ported | A title with no text upstream. This fork's dashboards: the node's built-in dashboard, [Grafana dashboards](deployment.md#grafana-dashboards) and [Metrics](architecture/metrics.md). |
| `stories/index.adoc`, `stories/single_miner.adoc` | not ported | Placeholders upstream ("For now this page is a placeholder"). |

## Not pages

| Upstream file | Why not ported |
|---|---|
| `presentations/` (talk slides and notes, a Grafana screenshot) | A Bitcoin conference talk, excluded from upstream's own site. |
| `README.adoc`, `_config.yml`, `Gemfile`, `Rakefile`, `.github/workflows/pages.yml`, `CNAME` | Build and publishing of the Jekyll site. This book builds with mdBook (`docs/book.toml`, the `Docs` CI job). |
| `_sass/`, `_includes/`, `_plantuml/`, `assets/` (fonts, logo, favicon) | Upstream's site theme and brand, with Bitcoin colours; mdBook's theme is used here. |
| `_snippets/stratum-config.toml` | A `[stratum]` snippet no page includes yet, with a Bitcoin testnet4 share address; this fork's settings are in `config.sample.toml` and [Miner options](miner-options.md). |
| `AGENTS.md`, `CLAUDE.md`, `DOC_CODE_REVIEW.md` | Contributor and review notes for upstream's docs repository; this repository has its own `AGENTS.md`. |

## Keeping up with upstream

To bring a page up to date, diff `p2poolv2/docs` from `623e4c1` to its new
head, port the changes into the page named above, and move the commit named
in that page's header and at the top of this page.
