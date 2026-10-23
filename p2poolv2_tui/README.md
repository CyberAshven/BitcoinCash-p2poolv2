# p2poolv2_tui

A terminal UI for a Bitcoin Cash P2Poolv2 node and the BCHN it mines on.
It is the Bitcoin Cash port of upstream's PDM, the
[P2Poolv2 Deployment and Management Tool](https://github.com/p2poolv2/pdm),
a Rust [ratatui](https://ratatui.rs) program:

- BCHN over JSON-RPC instead of Bitcoin Core;
- the fork's node API: `/network`, `/hashrate`, `/miners`, `/miner`,
  `/found_blocks`, beside upstream's `/chain_info`, `/shares`, `/peers` and
  the `/ws` live feed;
- no Lightning.

Mainnet and chipnet run the same code. The one switch is the node config's
`stratum.bitcoin_cash_network`, and the tool warns when BCHN or the node's
API report a different network.

## Screens

| Screen | Shows | Read from |
|---|---|---|
| Home | the node config, network, API and BCHN, each with whether it answers, and the dashboard link | `/network`, BCHN |
| BCHN Status: Chain | network, BCHN version, blocks, headers, sync state, best block, difficulty, network hashrate, verification progress, size on disk, connections, BCHN's warnings | `getblockchaininfo`, `getnetworkinfo`, `getmininginfo` |
| BCHN Status: Peers | address, direction, synced blocks, ping, connected time, version | `getpeerinfo` |
| BCHN Status: Mempool | transactions, size, memory, minimum and relay fees | `getmempoolinfo`, `getnetworkinfo` |
| P2Pool Config | the node's config.toml, editable | the file |
| P2Pool Status: Overview | network, share chain and candidate tip, total work, pool, node and network hashrate, the last hour's pool hashrate, live feed state | `/network`, `/chain_info`, `/hashrate?window=3600` |
| P2Pool Status: Shares | the newest shares, live | `/shares?num=25`, `/ws` topic `shares` |
| P2Pool Status: Miners | miners on this node and their workers, then one miner's place in the PPLNS window | `/miners?page=1`, `/miner?address=` |
| P2Pool Status: Blocks | blocks the pool found | `/found_blocks` |
| P2Pool Status: Peers | share chain peers and live connects and disconnects | `/peers`, `/ws` topic `peers` |
| Settings | the node config file and where these settings are kept | `settings.toml` |

Each shown source is asked again every 10 seconds, and `r` asks now.

## Point it at a node and BCHN

Build it from the workspace root (it is not in the release archives yet):

```sh
cargo build --release -p p2poolv2_tui
./target/release/p2poolv2_tui --config /path/to/node/config.toml
```

On Windows the build needs the same environment as the rest of the
workspace for `bitcoin`'s secp256k1 (`CFLAGS=-DSECP256K1_STATIC
-DENABLE_MODULE_ELLSWIFT`); it does not build RocksDB.

Give it the node's own `config.toml`, with `--config` or by picking it in
P2Pool Config or Settings, which remembers it. Everything is read from that
file, as the node reads it, `P2POOL__` environment overrides included:

| Setting | Used for |
|---|---|
| `[stratum] chain = "bitcoin-cash"` | required; a Bitcoin config is refused |
| `[stratum] bitcoin_cash_network` | the network: `mainnet`, `chipnet` or `regtest` |
| `[api] hostname`, `port` | the API at `http://hostname:port`; `0.0.0.0` is reached on `127.0.0.1` |
| `[api] auth_user`, `auth_password` | the API login, sent as HTTP Basic auth and to `/ws` as `?token=` |
| `[bitcoinrpc] url`, `username`, `password` | BCHN's JSON-RPC |

So whatever the node can reach, the tool reaches: a BCHN on the same
machine (`http://127.0.0.1:48332` on chipnet, `:8332` on mainnet), or one
elsewhere, such as over Tailscale. BCHN needs `server=1` and an RPC login
the node config names. The tool calls only `getblockchaininfo`,
`getnetworkinfo`, `getmempoolinfo`, `getpeerinfo` and `getmininginfo`.

For a node on another machine, use a copy of its config with
`[api] hostname` set to that machine. `/peers` always needs the API login
when the node has one, also with `public_dashboard = true`.

Settings live in `p2poolv2_tui/settings.toml` under `$XDG_CONFIG_HOME` or
`~/.config` on Linux, `~/Library/Application Support` on macOS and
`%APPDATA%` on Windows, or in `--settings-dir` / `P2POOL_TUI_CONFIG_DIR`.

## Keys

| Key | Does |
|---|---|
| Up / Down | move through the sidebar (it switches screens), or through a focused list |
| Left / Right | switch tabs |
| Enter | focus a screen's list; edit a setting; apply an edit; show the selected miner |
| Esc | give the keyboard back to the sidebar; cancel an edit or the file picker |
| s | save the config (in the editor) |
| Backspace | clear a setting; go up a folder in the file picker |
| r | refresh now |
| q, Ctrl-C | quit (q types a q while editing) |

## The config editor

The editor works on the file itself: edits go into a document that keeps
its comments and layout, and settings it does not list stay as they are.
Before saving, the result is read with the node's own parser
(`p2poolv2_config`) and its address and key checks; a file the node would
refuse is not saved, and the screen says why. The file is written beside the
original with its permissions and renamed over it. Restart the node to use
the new settings.

PDM wrote a loaded configuration back, which would also write `P2POOL__`
environment overrides into the file. This editor never does.

Secrets (`bitcoinrpc.password`, `api.auth_password`,
`stratum.sv2_authority_secret_key`) are always shown as `********`, also
while typed. An edit of a secret starts empty, and leaving it empty keeps
the old one.

## Safety and limits

- No `unsafe`: both crate roots are `#![forbid(unsafe_code)]`.
- Bounded state: at most 50 live shares and 50 live peer events, 256 share
  chain peers, 256 BCHN peers, 200 found blocks and 4,096 entries a folder
  in the file picker; at most one request per source in flight; answers
  over 4 MiB and `/ws` frames over 1 MiB are refused; every request times
  out after 10 seconds; typed values stop at 1,024 characters.
- Credentials never reach the screen: URLs are shown without a login,
  request errors leave out the URL, and config parse errors leave out
  quoted values and the line they quote.

## Drawing the views from your own data

Without its default `client` feature the crate is the views and their
plain data only: no HTTP, WebSocket or config code, just `ratatui` and
`serde`. Another ratatui program can draw the screens from data it fetched
itself:

```toml
p2poolv2_tui = { git = "https://github.com/CyberAshven/BitcoinCash-p2poolv2", branch = "alpha", default-features = false }
```

```rust
use p2poolv2_tui::network::CashNetwork;
use p2poolv2_tui::remote::Remote;
use p2poolv2_tui::views::bchn::{self, BchnScreen};

bchn::render(
    frame,
    area,
    &BchnScreen { status: &Remote::Ready(status), expected: Some(CashNetwork::Chipnet), tab: 0, now },
);
```

The types in `model` deserialize the API's and BCHN's JSON.

## Tests

```sh
cargo nextest run -p p2poolv2_tui
cargo nextest run -p p2poolv2_tui --no-default-features
```

The views are checked against snapshots in `src/views/snapshots`
([insta](https://insta.rs)); the clients against mock servers. A check
against a running node and BCHN, which only reads, is skipped unless asked
for:

```sh
P2POOL_TUI_LIVE_CONFIG=/path/to/node/config.toml cargo nextest run -p p2poolv2_tui --run-ignored only --no-capture
```

## Not ported

- Lightning: upstream PDM had it on its roadmap and removed its screens;
  this fork has no Lightning at all. Shares are traded for BCH with
  covenant contracts instead (`p2poolv2_cli bch-trade`, see
  `docs/atomic-swap/bch-share-trading.md`).
- The share market screen: upstream removed it before it had content.
- Work still open upstream (installing P2Poolv2 and its systemd service,
  memory and storage use, Prometheus): it can follow once upstream lands it.

## Credits and licence

Ported from [p2poolv2/pdm](https://github.com/p2poolv2/pdm) at `83835ae`,
by the PDM Authors (Raunak Kumar, Francesco Micheli, pool2win and
contributors). Its app state, key handling, file picker, settings, status
bar, Bitcoin and P2Pool status views, API and WebSocket clients and config
editor are the base of the modules here; each file says what it ports.

This crate is licensed under the GNU Affero General Public License v3.0 or
later, as PDM is: see [LICENSE](LICENSE). Every source file carries its
SPDX header. The rest of the workspace stays MIT or Apache-2.0.
