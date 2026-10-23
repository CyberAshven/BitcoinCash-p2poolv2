# Landing page

The landing page for P2Poolv2 for Bitcoin Cash: what the pool is, how to
mine on it (Stratum V2 first, Stratum V1 supported), how to run a node, what
has been measured so far, and links to the source, the docs, the releases,
the dashboard and the terminal UI.

```
index.html    the page: HTML only, no script
LICENSE-MIT   the upstream website's licence notice, which index.html keeps
README.md     this file
```

The page has no stylesheet, font or logo of its own. It links the node
dashboard's files in `p2poolv2_api/src/api/dashboard/assets/`: Pico CSS,
`dashboard.css`, `pages.css`, the Ubuntu font and the Bitcoin Cash logo. Its
own rules are in `pages.css`, each starting at `.landing`, the class of the
page's `<body>`, so no dashboard page matches them. That keeps one styling
source for the node's pages, the Rust/WASM dashboard and this page, as
`p2poolv2_api/src/api/dashboard/assets/README.md` describes.

## Working on it

The links are relative (`../p2poolv2_api/src/api/dashboard/assets/...`), so
the page renders from a checkout. Serve the repository root with any static
file server, for example `miniserve .` (a Rust program), and open
`/website/index.html`. Opening the file directly also works in browsers that
let a `file://` page load fonts. The `docs/` links only resolve on the
published site.

Check a change at phone width (390px) as well as on a desktop. Below 768px
the menu bar's links move into a Pico dropdown, a `<details>` element that
opens without a script.

The workspace tests check the page (`p2poolv2_api/src/api/dashboard/style_guard.rs`):
it loads the three shared stylesheets in the node's order and nothing of its
own, it has no script, `<style>`, `style=` or event handler attribute, every
class it names has a selector, and every link is an anchor on the page, an
`https` URL, the mdBook's front page (`docs/`), or a shared asset that
exists. They also check that the Pages workflow still copies the assets and
the book to where the page looks for them. Run
`cargo nextest run -p p2poolv2_api style_guard`.

## Publishing

`.github/workflows/docs.yml` builds the site on every pull request and push
that touches the docs, this directory or the shared assets. It stages:

- `index.html`, with the asset prefix rewritten to `assets/`;
- `assets/`: the shared stylesheets, fonts and logos, with
  `PICO-LICENSE.txt` and `UBUNTU-FONT-LICENCE.txt`;
- `LICENSE-MIT`;
- `docs/`: the mdBook from `docs/`;
- `404.html`: the mdBook's not-found page, whose links are absolute
  (`site-url` in `docs/book.toml`), so they resolve from the site root.

The step fails if the staged page still links a file outside the site, or
an `assets/` file it did not copy.

It publishes to GitHub Pages only on a push to `main`, and only when the
repository variable `DOCS_PAGES` is `true` and Settings, Pages, Source is
"GitHub Actions". Until the owner sets both, nothing is published. The site
is then `https://cyberashven.github.io/BitcoinCash-p2poolv2/`, with the docs
under `/docs/`.

To build the same site locally, run `mdbook build docs` and then the
workflow's "Stage the site" commands from the repository root; they write
`target/site`.

## Keeping the claims true

Every statement about the pool comes from the repository's own docs, and
goes stale when they change. Re-check these when the protocol or the status
moves:

| On the page | Source |
|---|---|
| A share every ten seconds; mini 256 times easier than main | `docs/bch-design.md` |
| Up to three uncles, paid nine tenths, the nephew one tenth | `docs/architecture/payout-distribution.md`, `MAX_UNCLES` |
| Donation 1.5% by default; window of three blocks' work, at most 120,960 shares, behind a 100-share maturity | `docs/architecture/payout-distribution.md`, `docs/bch-design.md` |
| At most 11 coinbase payouts, the tail drawn by weight | decision 14 in `docs/bch-plan.md` |
| Share trading tested in-process, not yet through BCHN | `docs/bch-plan.md` |
| Stratum ports 3333 and 3336, `p2p=`, share address prefixes | `config.sample.toml`, `docs/bch-quickstart.md` |
| The dated chipnet and regtest results, and the "Not yet" list | `docs/bch-plan.md`, `docs/bch-design.md` |

Keep "Measured" for things that ran, with their dates, and put everything
else under "Not yet". The page says the code is alpha and runs on chipnet;
do not soften that without the owner's say.

## What changed from the upstream page

`index.html` is adapted from the P2Poolv2 website,
https://github.com/p2poolv2/website at commit `1199458`, Copyright (c) 2026
P2Poolv2, under the MIT License in `LICENSE-MIT`, which stays with it. The
Bitcoin Cash changes are distributed with this fork under AGPL-3.0 (decision
13 in `docs/bch-plan.md`).

- Bootstrap's CSS and JavaScript bundle are gone. The navbar collapse, the
  only thing the script did, is a `<details>` dropdown.
- Inter, JetBrains Mono and the orange brand pack are replaced by the
  dashboard's Ubuntu font, Bitcoin Cash green and logo. The share chain
  figure, its one animation and the terminal blocks are kept, recoloured.
- The Bitcoin content is gone: testnet4, signet, bech32m share addresses,
  the Lightning and Ark swap steps, the small-miner probability worked out
  for S19 Pros, and the upstream pool's Grafana, Matrix and X links. In
  their place are the Bitcoin Cash share chain, CashAddr and P2PKH share
  addresses, Stratum V2 with Job Declaration, the coinbase budget, and the
  chipnet status.
- The social card image (`og.png`) is not carried over; the page has
  Open Graph text only.
