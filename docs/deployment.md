# Docker, Ansible and Umbrel

Three ways to deploy a node besides the release binaries and systemd. The
full guides are in the repository:
[docker/README.adoc](https://github.com/CyberAshven/BitcoinCash-p2poolv2/blob/alpha/docker/README.adoc),
[ansible/README.adoc](https://github.com/CyberAshven/BitcoinCash-p2poolv2/blob/alpha/ansible/README.adoc)
and `packaging/umbrel/README.md`.

## Docker

`docker/docker-compose.yml` runs Bitcoin Cash Node 29.2.0 (the official
release tarball, pinned by version and SHA-256) and a P2Poolv2 node on the
network `NETWORK` names in `docker/.env`: `regtest`, `chipnet` or `mainnet`.

On regtest (the default) the stack mines by itself:

```sh
just setup-dev            # creates docker/.env from docker/.env.sample
cd docker
docker compose up -d --build
docker compose exec -T bchn bcli getblockcount    # grows past 101
```

One-shot services mine 101 blocks to BCHN's dev wallet and take a payout
address and share-owner key from it; a Rust CPU miner (mujina) then mines
through the node. On regtest every submitted hash also meets the block
target, so blocks reach BCHN within seconds; shares still need their full
difficulty, which takes a CPU hours.

For chipnet or mainnet:

- Copy `config-docker.sample.toml` to `config.toml` and set your addresses,
  `dial_peers` and BCHN's RPC port.
- Set `NETWORK`, `BCHN_RPC_PORT` and `P2POOL_CONFIG` in `docker/.env`, and
  let BCHN sync before starting the node.
- No RPC password ships: BCHN makes a random one on first start unless
  `BCHN_RPC_PASSWORD` in `docker/.env` sets one, keeps it in the `bchn_rpc`
  volume, and the node reads it as `P2POOL_BITCOINRPC__PASSWORD`. Nothing
  logs it ("BCHN's RPC password" in docker/README.adoc says how to read, set
  or replace it). Only regtest uses the public `p2pool` / `p2pool` login.

`docker/docker-compose.node.yml` adds more nodes, and
`docker/docker-compose.metrics.yml` runs Prometheus and Grafana. The
published image is `ghcr.io/cyberashven/bitcoincash-p2poolv2`. The node and
the miner use host networking, which Docker supports on Linux.

## Ansible

`ansible/playbook.yml` deploys one or more nodes to a Debian or Ubuntu host:

- **p2poolv2:** installs the release with its installer script, writes each
  instance's config (`chain = "bitcoin-cash"`, `bitcoin_cash_network`,
  CashAddr addresses, donation and fee, `dial_peers`, optional Stratum V2
  keys, `[api]`) and runs it under systemd.
- **nginx_p2poolv2:** a rate-limited TLS reverse proxy for the API and,
  optionally, Grafana.
- **monitoring_p2poolv2:** Prometheus and Grafana on loopback, with the
  repository's dashboards.

Secrets (BCHN's RPC login, the API login, the Stratum V2 authority key and
Grafana's admin password) live in an ansible-vault file that is never
committed.

## Umbrel

`packaging/umbrel` is an Umbrel community app store, ported from upstream's
`p2poolv2/umbrel-app`, with two apps on the published image:

- **P2Pool for Bitcoin Cash** mines on mainnet on the Bitcoin Cash Node app
  from Umbrel's own store, with no setup.
- **P2Pool for Bitcoin Cash (external node)** mines on a BCHN you run
  elsewhere, on chipnet by default.

Their ports differ from Umbrel's Bitcoin apps and the Bitcoin P2Poolv2 app.
Publishing, installing, BCHN options and ports are in
[packaging/umbrel/README.md](https://github.com/CyberAshven/BitcoinCash-p2poolv2/blob/alpha/packaging/umbrel/README.md).

## Grafana dashboards

Both deployments use the dashboards in `prometheus/grafana/dashboards/`.
Found blocks link to a Bitcoin Cash explorer for the node's network
(Blockchair on mainnet, chipnet.imaginary.cash on chipnet), coinbase amounts
are in BCH, and miners are listed by payout address (CashAddr).
