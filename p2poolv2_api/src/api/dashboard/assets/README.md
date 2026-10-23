# Dashboard assets

These files are compiled into the node binary by `../assets.rs` and served
under `/dashboard/assets/`.

| File | Source | Licence |
|---|---|---|
| `dashboard.css` | CashDragon0's dashboard design, PR #8 | MIT OR Apache-2.0, as the rest of the repository |
| `pages.css` | Additions for the pages the node renders in Rust, for the Rust/WASM dashboard and for the landing page in `website/`. Its landing page rules are adapted from the [P2Poolv2 website](https://github.com/p2poolv2/website) (MIT, see `website/LICENSE-MIT`) | MIT OR Apache-2.0 |
| `pico.min.css` | [Pico CSS](https://picocss.com) v2.1.1 | MIT, in `PICO-LICENSE.txt` |
| `ubuntu-400.woff2`, `ubuntu-700.woff2` | [Ubuntu font family](https://design.ubuntu.com/font) | Ubuntu Font Licence 1.0, in `UBUNTU-FONT-LICENCE.txt` |
| `logo-horizontal-wt.svg`, `logo-circle.svg` | Bitcoin Cash logo, as used in PR #8 | Bitcoin Cash community brand assets |

The pages use CSS and HTML only for presentation. All logic is Rust, and no
page loads a script.

## The only styling source

The Rust/WASM dashboard (`p2poolv2_web`) has trunk bundle these same files
from here; it has no stylesheet of its own. The landing page
(`website/index.html`) links them from here too, and the Pages workflow
copies them beside it, with the two licence files. They are the
repository's only stylesheets, so the node's pages, the WASM app and the
landing page always look the same. A rule any of them needs goes into
`pages.css`; the landing page's rules all start at `.landing`, the class of
its `<body>`, so no dashboard page matches them.

`../style_guard.rs` fails the workspace tests when that stops being true: a
`.css` file anywhere else in the repository, a class the node's pages, the
WASM views or the landing page use with no selector in `pico.min.css`,
`dashboard.css` or `pages.css` (a short list of exceptions is documented
there), a WASM `index.html` link that does not point here, or a landing page
link to a local file that is not here. Change `ASSET_VERSION` in
`../assets.rs` with any file here, so browsers fetch the new copy.
