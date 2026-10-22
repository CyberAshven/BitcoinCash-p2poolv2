# p2poolv2_web

An interactive dashboard for a P2Poolv2 node, written in Rust with
[Leptos](https://leptos.dev) (client-side rendering) and compiled to
WebAssembly. There is no hand-written JavaScript; the only JavaScript is the
loader `wasm-bindgen` generates.

It looks and reads like the dashboard the node renders itself
(`/dashboard`): the same header, menu bar, search box, trail and footer, the
same KPI cards, panels, tables, pager and miners grid, and the same pages:
Dashboard, Blocks, Miners and Help, with block, miner and share pages behind
them. On top of that it takes new shares live from `/ws`, refreshes the
overview every 30 seconds, and its hashrate chart zooms (scroll, drag to
select, double-click to reset) and shows the values under the cursor. A
Settings page holds the API URL and login. Pages are addressed by the URL
fragment (`#/miners`, `#/miner?address=...`), so every page is a link that
works on any static server and never collides with the node's API paths.

## One stylesheet source

The app has no stylesheet of its own. `index.html` has trunk bundle the
node's own assets from `p2poolv2_api/src/api/dashboard/assets/`:
`pico.min.css`, CashDragon0's `dashboard.css` and `pages.css`, in the order
the node serves them; the Ubuntu fonts next to them, where `dashboard.css`'s
`url()`s find them; and the Bitcoin Cash logos. The views use the markup and
class names of `p2poolv2_api/src/api/dashboard/pages.rs`, and `src/format.rs`
formats numbers, times and hashes exactly as the pages do. Anything the app
needs that the pages do not, such as the chart's crosshair and zoom
selection, goes into `pages.css`, never into a file here.

`p2poolv2_api/src/api/dashboard/style_guard.rs` fails the workspace tests
when the two could drift apart: a `.css` file anywhere else in the
repository, a class either dashboard uses with no selector in the three
stylesheets, a computed class or inline style in these views, an
`index.html` link outside the assets directory, a stylesheet `url()` the
bundle cannot resolve, or formatting that differs from the pages'.

## Data

Everything comes from the node's JSON API. Numbers are computed in the node,
not the browser:

| Endpoint | Used for |
|---|---|
| `/network` | the chain badge, footer and tab title |
| `/hashrate?window=` | KPIs and chart, from the node's once-a-minute hashrate history (the same one `/dashboard` draws) |
| `/chain_info` | share chain height |
| `/shares?num=`, `/ws` (topic `shares`) | the first page of recent shares, then live updates |
| `/shares?to=&num=` | older pages of shares, and the newest 500 shares a miner's hashrate is estimated from |
| `/found_blocks` | found blocks, block pages, the block badge on shares, search |
| `/miners?page=`, `/miners?address=` | the miners on this node with their workers, and the online counts |
| `/miner?address=` | a miner's window shares and payout fraction |
| `/share?hash=` | share pages, search |

Memory stays bounded: the live share list keeps one page (25 shares), and
the chart keeps at most a day of samples (1,440) and draws at most 240
points per line, as the node's chart does.

## Build

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk --locked
cd p2poolv2_web
trunk build --release
```

The assets land in `p2poolv2_web/dist`, built to be served from `/`.

## Serve from the node

Point the node's `web_root` at the built directory and restart it:

```toml
[api]
web_root = "/path/to/p2poolv2_web/dist"
```

The app is then at `http://<api hostname>:<api port>/`; the node serves the
folder for every path its API does not use. With `public_dashboard = true`
the app and the share chain routes it reads open without a login. Otherwise
the API calls need the credentials from `auth_user` and `auth_token`. Enter
them under **Settings**; the user and API URL are remembered in local
storage, the password only for the browser tab.

## Serve it elsewhere

Any static file server works. Build for its path, for example
`trunk build --release --public-url /p2pool/`, and set the node's API URL in
Settings or with `?api=https://node.example:46884` in the page URL. The node
must then allow the page's origin: set `cors_allowed = true` under `[api]`.

## Develop

`trunk serve --config Trunk.dev.toml` serves on http://127.0.0.1:8080/ and
proxies the API paths to a node on `127.0.0.1:46884`, so no CORS setup is
needed. Unit tests for the formatting, routing, chart geometry and API
parsing run natively with `cargo nextest run -p p2poolv2_web`.
