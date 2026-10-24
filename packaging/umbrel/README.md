# Umbrel app store for P2Poolv2 on Bitcoin Cash

This folder is an [Umbrel](https://umbrel.com) community app store with two
apps that run a P2Poolv2 node for Bitcoin Cash. It is a port of upstream's
[p2poolv2/umbrel-app](https://github.com/p2poolv2/umbrel-app), which runs
the Bitcoin version on Umbrel's Bitcoin Node app.

| App | Bitcoin Cash Node | Network | Stratum V1 | Share chain P2P | Dashboard |
|---|---|---|---|---|---|
| P2Pool for Bitcoin Cash (`bch-p2pool`) | Umbrel's Bitcoin Cash Node app | mainnet | 3340 | 6890 | 46890 |
| P2Pool for Bitcoin Cash (external node) (`bch-p2pool-external`) | BCHN on another computer | chipnet by default, or mainnet | 3341 | 6891 | 46891 |

Both apps run the fork's image,
`ghcr.io/cyberashven/bitcoincash-p2poolv2:v0.0.1-alpha` (amd64 and arm64),
with the same code and the same rules. Only the node they mine on and its
network differ.

This is alpha software. The pool runs on chipnet; mainnet has not started.
Try the external node app on chipnet first.

## How the apps reach Bitcoin Cash Node

- **Umbrel ships BCHN.** The official Umbrel App Store has had a
  [Bitcoin Cash Node](https://github.com/getumbrel/umbrel-apps/tree/master/bitcoin-cash-node)
  app (`bitcoin-cash-node`, by LoneStrike Labs) since July 2026. It runs BCHN
  on mainnet; it has no network option.
- **Apps declare dependencies by app id.** An app lists the apps it needs
  under `dependencies` in its `umbrel-app.yml`. Umbrel offers to install them
  first, and every time the app starts it sources each dependency's
  `exports.sh`, so the app's `docker-compose.yml` can use their variables.
  The Bitcoin Cash Node app exports:

  | Variable | Value |
  |---|---|
  | `APP_BITCOIN_CASH_NODE_NODE_IP` | `10.21.21.50`, BCHN's address on Umbrel's network |
  | `APP_BITCOIN_CASH_NODE_RPC_PORT` | `8332` |
  | `APP_BITCOIN_CASH_NODE_ZMQ_HASHBLOCK_PORT` | `28332` |
  | `APP_BITCOIN_CASH_NODE_RPC_USER` | `umbrel` |
  | `APP_BITCOIN_CASH_NODE_RPC_PASS` | derived from your Umbrel's secret seed |

  `bch-p2pool` depends on `bitcoin-cash-node` and passes these to the node as
  `P2POOL_BITCOINRPC__URL`, `__USERNAME`, `__PASSWORD` and
  `P2POOL_STRATUM__ZMQPUBHASHBLOCK`, so there is nothing to type in.
- **A dependency cannot be optional.** Because the Bitcoin Cash Node app is
  mainnet only, chipnet, and any BCHN that runs elsewhere, need the second
  app. `bch-p2pool-external` declares no dependency and takes BCHN's address,
  login and ZMQ endpoint from its settings page or its `p2pool.toml`. The
  [BCHN options](#running-on-a-bchn-elsewhere) it needs are below.

## Install

### 1. Publish the store

Umbrel reads a community store from the root of a Git repository's default
branch: `umbrel-app-store.yml` there, and one app per folder. This folder
sits inside the P2Poolv2 repository, so publish it as a repository of its
own, for example:

```sh
git subtree split --prefix packaging/umbrel -b umbrel-store
git push git@github.com:<you>/bch-p2pool-umbrel.git umbrel-store:main
```

Push again after each change here. Umbrel fetches the store again from
time to time and offers an app update when an app's `version` changes.

### 2. Add the store to Umbrel

In Umbrel's App Store, open the menu, choose **Community App Stores**, paste
the repository's URL (`https://github.com/<you>/bch-p2pool-umbrel`) and add
it. On umbrelOS 2.0 only the owner account can add stores. The apps appear
under **Bitcoin Cash P2Pool**.

### 3. Install an app

- **P2Pool for Bitcoin Cash** offers to install Bitcoin Cash Node first. BCHN
  then downloads the whole Bitcoin Cash chain, which takes a while. The P2Pool
  node waits, logging "Bitcoin node still syncing", and opens its Stratum port
  once BCHN has synced.
- **P2Pool for Bitcoin Cash (external node)** starts with the placeholder
  `bchn.example` as its BCHN, so it stops with a lookup error and Umbrel
  restarts it until you set BCHN's address
  ([below](#running-on-a-bchn-elsewhere)).

### 4. Settings

On umbrelOS 2.0 and later, open the app's settings, then **Advanced**,
**Environment variables**. The settings this app suggests are listed there;
a value you save overrides the app's default and its `p2pool.toml`.

| Setting | App | What it sets |
|---|---|---|
| `P2POOL_STRATUM__MINER_ADDRESS` | both | The share address (`p2pool:q...` on mainnet, `tp2pool:q...` on chipnet) that owns every share this node mines. Make it from a key in your own wallet with `p2poolv2_cli bch-share-address`. Leave it unset when each miner sends `p2p=<share address>` in its Stratum password. |
| `P2POOL_NETWORK__EXTERNAL_ADDRESS` | both | `/ip4/<public IP>/tcp/<P2P port>`, the address other nodes should dial, when you forward the P2P port on your router. |
| `RUST_LOG` | both | Log level: `error`, `warn`, `info` (default), `debug` or `trace`. |
| `P2POOL_BITCOINRPC__URL` | external | BCHN's JSON-RPC, `http://<address>:48332` on chipnet, `http://<address>:8332` on mainnet. |
| `P2POOL_BITCOINRPC__USERNAME` | external | `rpcuser` in BCHN's `bitcoin.conf`. |
| `P2POOL_BITCOINRPC__PASSWORD` | external | `rpcpassword` in BCHN's `bitcoin.conf`. |
| `P2POOL_STRATUM__ZMQPUBHASHBLOCK` | external | BCHN's ZMQ hashblock endpoint, `tcp://<address>:28332`. |

On older umbrelOS, which has no settings page, set the matching keys in the
app's `p2pool.toml` (`miner_address`, `external_address`, `[bitcoinrpc]`
and `zmqpubhashblock`); the log level then stays at `info`.

### 5. Point your miners at it

```
url:      stratum+tcp://umbrel.local:3340      (3341 for the external node app)
user:     <payout CashAddr>.<worker>           (the bitcoincash: prefix is optional)
password: x                                    (or p2p=<share address> if miner_address is unset)
```

Every option is in
[miner-options.md](https://github.com/CyberAshven/BitcoinCash-p2poolv2/blob/main/docs/miner-options.md). The
dashboard opens from the app's icon on Umbrel's home screen, behind your
Umbrel login.

## p2pool.toml

Each app keeps its node's config in `p2pool.toml`. Umbrel copies it into the
app's folder when you install the app and keeps your edits across app
updates. Edit it from Umbrel's terminal or over SSH, then restart the app:

```sh
nano ~/umbrel/app-data/bch-p2pool/p2pool.toml           # or bch-p2pool-external
```

The app's `docker-compose.yml` sets some keys with `P2POOL_*` variables, which
override the file, so they are not in it: `stratum.chain`, the ports
(`stratum.port`, `network.listen_address`, `api.port`), and for `bch-p2pool`
also the network and BCHN's RPC and ZMQ. Values on the settings page override
both.

Other things to set in the file (umbrelOS 2.0's custom variables can also
set a single value as `P2POOL_<SECTION>__<KEY>`, but not a list):

- `dial_peers`, the share chain nodes to join, as `/ip4/<address>/tcp/<port>`.
  Bitcoin Cash has no DNS seeds yet, so without one the node only waits for
  peers to dial it. When you list a node of a running share chain, also set
  `wait_for_chain_sync = true`, so miners connect only once this node has
  caught up.
- `bch_share_chain`: `mini` (the default, a share target 256 times easier,
  for small miners) or `main`. Change `[store] path` with it, to
  `/p2poolv2/data/<network>-<share chain>`: the node refuses a store that
  holds another share chain.
- `donation` (default 100 basis points, 1%; 0 opts out), and `fee_address`
  and `fee` for a public pool's own cut (`fee` defaults to 100 basis points,
  1%, once `fee_address` is set). The donation and the fee may each be at
  most 1,000 basis points.

The share chain store and the node's stats live in the app's `data/` folder.
To start again from an empty store, stop the app, delete
`data/data/<network>-<share chain>` in the app's folder, and start it.

## Running on a BCHN elsewhere

`bch-p2pool-external` mines on a Bitcoin Cash Node you run on another
computer. BCHN needs these options in its `bitcoin.conf`. For chipnet:

```ini
chipnet=1
server=1
# The pool needs templates, not a wallet.
disablewallet=1

[chip]
rpcuser=p2pool
rpcpassword=<a long random password>
# Listen where the Umbrel can reach BCHN, and let only the Umbrel in.
rpcbind=0.0.0.0
rpcallowip=<the Umbrel's LAN address>/32
# P2Pool waits for new blocks here.
zmqpubhashblock=tcp://0.0.0.0:28332
```

For mainnet, leave out `chipnet=1` and put the same lines under `[main]`
instead of `[chip]`. BCHN's RPC port is 48332 on chipnet and 8332 on
mainnet; the ZMQ port is whatever `zmqpubhashblock` names. Then set
`P2POOL_BITCOINRPC__URL`, `__USERNAME`, `__PASSWORD` and
`P2POOL_STRATUM__ZMQPUBHASHBLOCK` on the app's settings page, or
`[bitcoinrpc]` and `zmqpubhashblock` in its `p2pool.toml`, and restart the
app.

- The app reaches BCHN from the Umbrel's own LAN address (Docker translates
  the container's address), so that is the address `rpcallowip` names.
- RPC is unencrypted and ZMQ has no login. Keep both on your LAN or a VPN
  such as Tailscale, never open to the internet.
- BCHN must run the network `bitcoin_cash_network` names, and finish its
  sync before P2Pool can mine. A pruned BCHN works (`prune=2000` keeps about
  2 GB): P2Pool only asks for templates and submits blocks.
- BCHN answers `getblocktemplate` only once it has at least one peer.

The external node app starts on chipnet. To move it to mainnet, change three
values in its `p2pool.toml` together, then point it at a mainnet BCHN and
restart:

```toml
[store]
path = "/p2poolv2/data/mainnet-mini"

[stratum]
bitcoin_cash_network = "mainnet"
bootstrap_address = "bitcoincash:qze95tc2dqrnltvxfa5yhunwx5952f34fvw4g6365k"
```

`bootstrap_address` only has to parse for the network: a share chain node
never pays it (only Hydrapool mode does). The sample value is the P2Poolv2
Bitcoin Cash donation address.

## Ports

| Port | Used by |
|---|---|
| 3340, 6890, 46890 | `bch-p2pool`: Stratum V1, share chain P2P, dashboard |
| 3341, 6891, 46891 | `bch-p2pool-external`: Stratum V1, share chain P2P, dashboard |
| 3333, 6884, 46884 | P2Poolv2 for Bitcoin (`p2poolv2/umbrel-app`) |
| 2100, 8332 to 8335, 28332 to 28336 | Bitcoin Node |
| 2105, 8334, 9332, 9333, 9335, 48332 to 48336 | Bitcoin Knots |
| 3330, 8335 | Bitcoin Cash Node (dashboard, P2P); its RPC 8332 and ZMQ 28332 stay on Umbrel's network |
| 2018, 2019, 2021 | Public Pool |
| 21000, 23334 | DATUM |
| 21420 to 21422 | Gobrrr Pool |

The two apps here use ports none of the others take, so any of them can run
side by side. They were also checked against every port the apps in the
official Umbrel App Store published on 2026-10-23, and
`p2poolv2_config/tests/umbrel_apps.rs` checks them against the table. Inside the
container the node listens on the same Stratum and P2P ports it publishes;
its API stays on 46884 and only Umbrel's app proxy reaches it. Forward the
P2P port on your router (6890 or 6891) and set the external address if other
nodes should dial yours. Stratum V2 is not published yet.

## Updating the image

The fork refreshes `v0.0.1-alpha` in place for every alpha build until the
beta. Umbrel pulls an app's image only when it installs or updates the app,
so to run a refreshed alpha, pull it on the Umbrel (terminal or SSH) and
restart the app:

```sh
sudo docker pull ghcr.io/cyberashven/bitcoincash-p2poolv2:v0.0.1-alpha
```

## Trying it without an Umbrel

As upstream's README describes, `getumbrel/umbrel` has a development
environment that runs umbrelOS in one Docker container on your computer. It
needs Docker and Node.js 22 or later, and its first build takes hours:

```sh
git clone https://github.com/getumbrel/umbrel.git
cd umbrel
npm install
npm run dev start
```

Open the container's address (`docker inspect -f
'{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' umbrel-dev`) in a
browser, finish the setup, and add the store as above.

## Status

- Not yet installed on a real umbrelOS. What is checked: the test
  `p2poolv2_config/tests/umbrel_apps.rs` renders each app's config the way
  Umbrel runs it (its `p2pool.toml` under the compose file's variables, with
  the Bitcoin Cash Node app's exports filled in), loads it with
  `p2poolv2_config`, and checks the network, the BCHN wiring, the store
  folder, the settings and the ports.
- The app settings need umbrelOS 2.0 or later; on older versions the apps
  ignore them and run with `p2pool.toml`.
- The icon is upstream's P2Poolv2 artwork, unchanged. Umbrel loads it from
  this repository's `main` branch, so it shows once this folder is released
  there.
- Stratum V2 is not published.

## Credits and licence

Ported from [p2poolv2/umbrel-app](https://github.com/p2poolv2/umbrel-app) at
`5b1a3ae`, by Juwon Ogunseye and the P2Poolv2 developers, under the Apache
License 2.0; [LICENSE](LICENSE) is upstream's licence file. Changes from
upstream: Bitcoin Cash in place of Bitcoin (the fork's image and config
keys, the Bitcoin Cash Node app in place of Bitcoin Node), a second app for a
BCHN elsewhere, ports apart from the Bitcoin apps, a persistent
`p2pool.toml`, settings for umbrelOS 2.0, and this README. The icons are
upstream's.
