# Dashboard assets

These files are compiled into the node binary by `../assets.rs` and served
under `/dashboard/assets/`.

| File | Source | Licence |
|---|---|---|
| `dashboard.css` | CashDragon0's dashboard design, PR #8 | MIT OR Apache-2.0, as the rest of the repository |
| `pages.css` | Additions for the pages the node renders in Rust and for the Rust/WASM dashboard | MIT OR Apache-2.0 |
| `pico.min.css` | [Pico CSS](https://picocss.com) v2.1.1 | MIT |
| `ubuntu-400.woff2`, `ubuntu-700.woff2` | [Ubuntu font family](https://design.ubuntu.com/font) | Ubuntu Font Licence 1.0 |
| `logo-horizontal-wt.svg`, `logo-circle.svg` | Bitcoin Cash logo, as used in PR #8 | Bitcoin Cash community brand assets |

The pages use CSS and HTML only for presentation. All logic is Rust, and no
page loads a script.

## The only styling source

The Rust/WASM dashboard (`p2poolv2_web`) has trunk bundle these same files
from here; it has no stylesheet of its own. They are the repository's only
stylesheets, so the node's pages and the WASM app always look the same. A
rule either of them needs goes into `pages.css`.

`../style_guard.rs` fails the workspace tests when that stops being true: a
`.css` file anywhere else in the repository, a class the node's pages or the
WASM views use with no selector in `pico.min.css`, `dashboard.css` or
`pages.css` (a short list of exceptions is documented there), or a WASM
`index.html` link that does not point here. Change `ASSET_VERSION` in
`../assets.rs` with any file here, so browsers fetch the new copy.
