# Running P2Poolv2 on Bitcoin Cash

A short operator guide. Design details are in [bch-design.md](bch-design.md)
and status in [bch-plan.md](bch-plan.md). Bitcoin Cash support is not yet a
proven production pool: try it on regtest or chipnet first. Hardware, disk and
ports are in [requirements.md](requirements.md).

## 1. Run Bitcoin Cash Node

P2Poolv2 asks BCHN for templates over JSON-RPC and hears new blocks over ZMQ
(and, with `node_p2p_address`, on BCHN's P2P port). In `bitcoin.conf`:

```ini
server=1
rpcuser=p2pool
rpcpassword=<your password>
zmqpubhashblock=tcp://127.0.0.1:28332
```

- BCHN answers `getblocktemplate` with "Bitcoin is not connected!" until it
  has a peer, on regtest too. On regtest set `allowunconnectedmining=1` (the
  Docker dev stack does, `docker/README.adoc`) or connect a second BCHN.
- A new regtest chain holds only the 2011 genesis block, so BCHN reports
  `initialblockdownload` and P2Poolv2 waits (`Bitcoin node still syncing,
  checking again in 300s`). Mine one block first:
  `bitcoin-cli -regtest generatetoaddress 1 <bchreg: address>`.
- BCHN fills templates only to `-percentblockmaxsize` of the block size limit
  (50% on mainnet by default). Leave it below 100%: the pool adds a coinbase
  with one output per miner in the PPLNS window.

### Or run BCHD

BCHD works too (checked with BCHD 0.22.4 on regtest and chipnet). It has no
ZMQ, so set `node_p2p_address` to hear blocks on its P2P port. P2Poolv2's TLS
refuses BCHD's self-made `rpc.cert`, a CA certificate used as the server's own
(`CaUsedAsEndEntity`), so run BCHD on the same computer with TLS off, which
BCHD allows only on localhost. In `bchd.conf`:

```ini
rpcuser=p2pool
rpcpass=<your password>
rpclisten=127.0.0.1:8334
notls=1
; Leave room for the pool's coinbase.
blockmaxsize=16000000
```

and in P2Poolv2's config:

```toml
[stratum]
node_p2p_address = "127.0.0.1:8333"

[bitcoinrpc]
url = "http://127.0.0.1:8334"
```

- Without `blockmaxsize`, BCHD fills a template to 1,000 bytes below the
  limit, too full for the pool's coinbase; 16,000,000 is BCHN's 50% of 32 MB.
- BCHD needs no `miningaddr` (on regtest, its `generate` does), gives regtest
  templates without peers, and does not report `initialblockdownload`:
  P2Poolv2 waits until it gives a template.
- `zmqpubhashblock` can stay in the config; it never fires.

## 2. Get your share address

A share is owned by a P2PKH key from your own wallet. Take a receiving
address (`q...`) for that key, ideally not your payout address, and
re-encode it:

```sh
p2poolv2_cli bch-share-address --network mainnet bitcoincash:q...
# p2pool:q...
```

On test networks use `--network chipnet` (`bchtest:` to `tp2pool:`),
`--network scalenet` (`bchtest:` to `sp2pool:`) or `--network regtest`
(`bchreg:` to `rp2pool:`). The command takes no private key. Keep the wallet:
it signs if you sell shares for BCH with `p2poolv2_cli bch-trade`
([atomic-swap/bch-share-trading.md](atomic-swap/bch-share-trading.md)).

## 3. Configure the node

Start from `config.sample.toml`, a Bitcoin Cash chipnet node
(`config-bitcoin.sample.toml` is upstream's Bitcoin sample). For mainnet,
under `[stratum]`:

```toml
chain = "bitcoin-cash"
bitcoin_cash_network = "mainnet"   # or "chipnet", "scalenet", "regtest"
miner_address = "p2pool:q..."      # from step 2
bootstrap_address = "bitcoincash:q..."
```

A Bitcoin Cash config needs no `network`: the node uses the one
`bitcoin_cash_network` implies and ignores a `network` line.

Point `[bitcoinrpc]` at BCHN's RPC port and `zmqpubhashblock` at its ZMQ
endpoint. Optionally set `node_p2p_address` in `[stratum]` to BCHN's P2P port
(`127.0.0.1:48333` on chipnet, `127.0.0.1:8333` on mainnet) to also hear each
block as a P2P peer, as jtoomim's p2pool did; a node without ZMQ (BCHD, Knuth)
then gets a template at once instead of at the next 10-second poll. BCHN
accepts P2P connections by default, but not with `-listen=0`, or `-connect`
without `-listen=1`.

### Node checks at start

The node refuses to start, saying what it found, unless `[bitcoinrpc]` is a
BCHN on `bitcoin_cash_network`:

- `getblockchaininfo` must report the network's chain name: `main`, `chip`,
  `scale` or `regtest`. Testnet4 (`test4`) has chipnet's genesis and CashAddr
  prefix; only the name tells them apart.
- Once synced, BCHN's block at its checkpoint height must be the checkpoint:
  block 661,648 on mainnet (the first after the November 2020 upgrade, past
  the eCash and 2018 Bitcoin SV splits), 115,252 on chipnet (where it forked
  from testnet4) and 10,000 on scalenet (where scalenet is reorged back to).
  Bitcoin, Bitcoin SV and eCash nodes also say `main`. Regtest checks the
  name only.

### Block submission and peers

Found blocks go to this node and to every extra BCHN listed as
`[[submit_nodes]]` (`url`, `username`, `password`).

Under `[network]`, list share chain peers in `dial_peers` by IP address or
host name; a host name is looked up again on every dial, so dynamic DNS works:

```toml
dial_peers = ["/ip4/203.0.113.1/tcp/6884", "/dns4/pool.example.com/tcp/6884"]
```

A Bitcoin Cash node never uses DNS seeds (the built-in ones are Bitcoin
nodes): `dns_seed` is ignored and off on `chain = "bitcoin-cash"`. Without
`dial_peers` or [Nostr discovery](p2p-network.md#finding-peers-through-nostr)
the node only waits to be dialed. Once connected it dials peers it learns of
until it has `target_outgoing_peers` outgoing peers, 16 by default
([P2P network](p2p-network.md#outgoing-peers)).

`wait_for_chain_sync` in `[stratum]`: with `true`, the default, the Stratum
port opens once the share chain tip is under 5 minutes old. The first node of
a new share chain, and any node on regtest, must set `false` (the genesis
share is older), or the port never opens; `config.sample.toml` sets `false`.
Set `true` once `dial_peers` lists a node of a running share chain.

For a node others join: keep `listen_address` on all interfaces
(`/ip4/0.0.0.0/tcp/6884`), open TCP port 6884 in the firewall and router, and
publish `/dns4/<host name>/tcp/6884` or `/ip4/<public IP>/tcp/6884` for their
`dial_peers`.

### Donation and pool fee

- `donation` (basis points) applies to miners who choose none: 100 (1%) to
  `bitcoincash:qze95tc2dqrnltvxfa5yhunwx5952f34fvw4g6365k` by default, 0 for
  none. A miner's own `donation=<percent>` always wins
  ([Miner options](miner-options.md#donation)).
- `fee_address` and `fee` add an operator's cut: 100 basis points (1%) when
  `fee_address` is set without `fee`, taken from what the donation leaves.
- Each is at most 1,000 basis points (10%); peers reject shares above that.
  The node logs the donation in effect at startup.

How the cuts are taken: [Payout distribution](architecture/payout-distribution.md).

### Stratum V2

Set `sv2_port` to serve Stratum V2 (standard and extended channels, and Job
Declaration) beside Stratum V1. The identities are in
[Miner options](miner-options.md#stratum-v2). Without keys, frames are
unencrypted, which the specification allows only between local roles: keep
the port on localhost or a trusted LAN. To require Noise, set an authority key
pair made with an external tool (such as the Stratum V2 reference
implementation's key utility); miners pin the public key:

```toml
sv2_authority_public_key = "9b..."
# secret via env: P2POOL_STRATUM__SV2_AUTHORITY_SECRET_KEY=...
#   (or sv2_authority_secret_key in the file)
sv2_cert_validity_secs = 3600
```

### Light templates

`template_source = "rpc_light"` asks the `[bitcoinrpc]` node for
`getblocktemplatelight`: the node keeps the transactions and sends their
merkle branch, and a found block goes back as header and coinbase with
`submitblocklight`. BCHN gives light templates; BCHD and Knuth do not today.

- A template the node cannot give light (no `getblocktemplatelight`, no
  `merkle` in it, or no `miningblocksizelimit` in `getmininginfo`) comes from
  `getblocktemplate`, with a warning logged once.
- The node's mining size limit (`-blockmaxsize` or equivalent), the pool's
  coinbase and 89 bytes of header and transaction count must fit the block
  size limit, or the node withdraws work and logs why. BCHN's default leaves
  half the block on mainnet and regtest, none on chipnet: lower
  `-blockmaxsize` there.
- Not with `[[submit_nodes]]`: only the node that made a template can
  complete its block.
- BCHN writes each light job's transactions under `<datadir>/gbt/` and
  deletes them after `-gbtstoretime` (3600 s by default): with 5 MB templates
  every 10 s, about 1.9 GB an hour.

### Templates from a Stratum V2 Template Provider

For templates from a Template Provider, such as
[bchn-sv2-bridge](https://github.com/danhaus93-ops/bchn-sv2-bridge), Pickaxe's
template server or another node's Template Distribution service:

```toml
template_source = "sv2"
template_provider_address = "127.0.0.1:8442"
# Required unless the provider is on this computer:
# template_provider_authority_public_key = "9b..."
```

- Found blocks go back with `SubmitSolution`, or over RPC when the provider
  cannot take them. Keep `[bitcoinrpc]`: it checks each template's parent,
  validates shares and takes blocks.
- The node asks for the BCH large templates extension (`0x4C54`). A provider
  that grants it (log: `granted the large templates extension`) pushes every
  template's transactions, so only the block size limit bounds a template.
  Otherwise the node asks for each template's transactions: at most 65,535
  in one 16 MiB message.

#### A Template Provider over Tor or I2P

A provider behind a Tor onion service or an I2P server tunnel is reached
through that network's SOCKS5 proxy on this computer:

```toml
template_source = "sv2"
template_provider_address = "<56 characters>.onion:3335"
template_provider_authority_public_key = "9b..."   # always required here
template_provider_proxy = "127.0.0.1:9050"         # Tor's SOCKS port
```

For I2P, give the provider's `.b32.i2p` name and server tunnel port, and the
router's SOCKS proxy (i2pd: `127.0.0.1:4447`). The name goes to the proxy
unresolved; the node never connects directly, and retries the proxy. Each
connection has its own SOCKS username, so Tor gives it its own circuit, and a
`Reconnect` is followed through the proxy. Time limits: 20 seconds to connect
through the proxy and 15 per setup answer, 90 and 45 for an I2P name, 10 each
for a provider reached directly.

The provider's operator points an onion service
(`HiddenServicePort 3335 127.0.0.1:3335` in `torrc`) or an i2pd server tunnel
at its Template Distribution port. `[bitcoinrpc]` still names a node this
computer reaches directly.

## 4. Point miners at the node

Every option (`p2p=`, `d=`, `th=` and the Stratum V2 identity) is in
[miner-options.md](miner-options.md).

Stratum V1:

```
url:      stratum+tcp://<node>:3333
user:     <payout CashAddr>.<worker>      (the bitcoincash: prefix is optional)
password: x                                (or p2p=<share address> if miner_address is unset)
```

Stratum V2: `<node>:<sv2_port>`, user identity `<payout CashAddr>.<worker>`
(add `,p2p=<share address>` if `miner_address` is unset), and the authority
public key when Noise is on. Work selection with Job Declaration, in both
modes, is in [bch-job-declaration.md](bch-job-declaration.md).

Payouts arrive in the coinbase of each block the pool finds and are spendable
after 100 blocks, as for any Bitcoin Cash coinbase.

With no template for 60 seconds, the node withdraws work and closes the
miners' connections, so a miner with a backup pool fails over. The log says
`Withdrawing mining work: no template for 60 seconds` after `Error polling
block template` lines with the reason, and `The node gives templates again`
when work returns. With `template_source = "sv2"` the 60 seconds start when
the last Template Provider connection that published a template ends
(`Template Provider connection to … ended`).

## 5. Check two nodes on chipnet

The first run is recorded in
[bch-design.md](bch-design.md#chipnet-run-2026-10-15). It needs a machine
that reaches BCH peers on port 48333.

1. Run BCHN with `chipnet=1`, `server=1` and `zmqpubhashblock`, and let it
   sync (`bitcoin-cli -chipnet getblockchaininfo`).
2. Start two P2Poolv2 nodes with `bitcoin_cash_network = "chipnet"`, their
   own `store` paths and ports, and the second's `dial_peers` pointing at the
   first's `listen_address`; the first needs `wait_for_chain_sync = false`.
   Both logs show `Identified Peer ...` with the same protocol,
   `/p2pool/test/<genesis>/<rule tags>/1.0.0`.
3. Point a miner at the first node with a `bchtest:` username. A share needs
   about 2e10 hashes (stratum difficulty about 4.7); vardiff can go down to
   the sample's `minimum_difficulty` of 1 and never above the job's share
   difficulty (4 here). 24 CPU threads (about 150 MH/s) found a share every
   minute or two; a small ASIC finds them in seconds.
4. Pass: `curl http://<api>/chain_info` on both nodes shows the same
   `chain_tip_height` above 0, and neither log has `Chain-context validation
   failed`. A block found on chipnet appears in BCHN with the 1% donation
   output first.
