# Dashboard assets

These files are compiled into the node binary by `../assets.rs` and served
under `/dashboard/assets/`.

| File | Source | Licence |
|---|---|---|
| `dashboard.css` | CashDragon0's dashboard design, PR #8 | MIT OR Apache-2.0, as the rest of the repository |
| `pages.css` | Additions for the pages the node renders in Rust | MIT OR Apache-2.0 |
| `pico.min.css` | [Pico CSS](https://picocss.com) v2.1.1 | MIT |
| `ubuntu-400.woff2`, `ubuntu-700.woff2` | [Ubuntu font family](https://design.ubuntu.com/font) | Ubuntu Font Licence 1.0 |
| `logo-horizontal-wt.svg`, `logo-circle.svg` | Bitcoin Cash logo, as used in PR #8 | Bitcoin Cash community brand assets |

The pages use CSS and HTML only for presentation. All logic is Rust, and no
page loads a script.
