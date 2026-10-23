# Miner options

Everything a miner sets to mine on a P2Poolv2 Bitcoin Cash node, over
Stratum V1 or Stratum V2. The node's own settings that miners notice are at
the end.

A miner needs two addresses, on two different chains:

- a **payout address**: a CashAddr on the node's Bitcoin Cash network
  (`bitcoincash:`, `bchtest:` on chipnet, `bchreg:` on regtest). Every block
  the pool finds pays it in the coinbase.
- a **share address**: `p2pool:`, `tp2pool:`, `sp2pool:` or `rp2pool:`. It owns the
  miner's shares on the share chain, which can be sold for BCH
  ([share trading](atomic-swap/bch-share-trading.md)). Make it from a P2PKH
  receiving address of a wallet you control, ideally not the payout address:

  ```sh
  p2poolv2_cli bch-share-address --network chipnet bchtest:qq...
  # tp2pool:qq...
  ```

  The command takes an address, never a private key. If the node operator
  sets `miner_address`, that address owns every share and miners need none.

## Stratum V1

| Field | Value |
|---|---|
| URL | `stratum+tcp://<node>:3333` (the node's `[stratum] port`) |
| Username | `<payout CashAddr>.<worker>` |
| Password | options, see below; anything else (such as `x`) is ignored |

### Username

- The part before the first dot is the payout address. It must be a P2PKH or
  P2SH CashAddr for the node's network. The prefix may be left out
  (`qq...` is read as `bitcoincash:qq...` on mainnet); the checksum must
  still match the network's prefix. A token-aware CashAddr (`z...` or
  `r...`, which CashTokens wallets hand out) is accepted too, and is paid
  with the same P2PKH or P2SH output as its plain `q...` or `p...` form.
  P2SH32 addresses (32-byte hashes) are not accepted yet.
- Everything after the first dot is the worker name. It labels the worker
  in the dashboard and metrics and is optional.
- At most 128 characters in all.
- A username the node cannot read gets the error `Invalid username`; a second
  bad attempt on the same connection closes it.

### Password options

Options are `key=value` pairs separated by commas, spaces, tabs or
semicolons, in any order. Keys are case insensitive.

| Option | Effect |
|---|---|
| `p2p=<share address>` | The share address that owns this miner's shares. Needed unless the node sets `miner_address`. |
| `d=<difficulty>` | Start at this difficulty instead of the node's `start_difficulty`. A whole number above 0. |
| `th=<TH/s>` | Your hashrate in whole TH/s; the node starts you at the difficulty that gives one share every 3.3 seconds, about 776 per TH/s. `d=` wins if both are given. |

Rules:

- `d=` and `th=` are kept within the node's `minimum_difficulty` and
  `maximum_difficulty`. After the first shares, vardiff takes over and aims
  at one share every 3.3 seconds (0.3 shares a second) whatever was asked
  for.
- Your difficulty never goes above the share difficulty of the job you are
  mining, whatever `d=`, `th=`, `mining.suggest_difficulty`, vardiff or the
  node's `minimum_difficulty` say. A miner submits only hashes that meet its
  difficulty, so a higher one would throw away hashes that are valid shares
  and you would be paid for fewer hashes than you did. When a new job's share
  difficulty is below yours, the node sends `mining.set_difficulty` before
  the job. The share difficulty is rounded down to a whole number, so the
  mini chain's lowest share difficulty, 4.55, holds you at 4.
- `p2p=` must be a share address for the node's network: `p2pool:` on
  mainnet, `tp2pool:` on chipnet, `sp2pool:` on scalenet, `rp2pool:` on
  regtest.
- If the node sets `miner_address`, leave `p2p=` out or send that same
  address. A different one is refused: `Share address conflicts with the
  pool's configured address`.
- Without `p2p=` and without `miner_address`, authorize is refused: `This
  pool needs a Bitcoin Cash share address. Set the stratum password to
  p2p=<address>`.
- A worker suffix after the share address (`p2p=tp2pool:qq....rig1`, as some
  proxies append) is ignored.
- `p2p=` plus a share address is 53 characters (`tp2pool:` 54). Check that
  your firmware does not cut the password field: a cut address fails its
  checksum and is refused, it never pays someone else.

### Examples

Chipnet, own share address, starting difficulty 65,536:

```
url:      stratum+tcp://pool.example.org:3333
user:     bchtest:qr3rryn6ugv8qpk82ccaqzk4ras92vpluvr2yclquu.rig1
password: p2p=tp2pool:qq...,d=65536
```

Mainnet, a 200 TH/s machine, prefix left out:

```
user:     qq....s21
password: th=200 p2p=p2pool:qq...
```

A node whose operator set `miner_address` (every share belongs to the node):

```
user:     bitcoincash:qq....bitaxe
password: x
```

### Other Stratum V1 methods

| Method | What the node does |
|---|---|
| `mining.configure` with `version-rolling` | Answers with the node's `version_mask` (default `1fffe000`, BIP 320's general purpose bits). The node keeps only the submitted version bits inside the mask, so a miner that rolls others submits headers that do not meet its difficulty. |
| `mining.suggest_difficulty` | Like `d=`: kept within the node's minimum and maximum and at or below the share difficulty, then vardiff. |
| `mining.extranonce.subscribe` | Acknowledged. The node never changes a session's extranonce. |
| `mining.authorize` before `mining.subscribe` | Accepted, as some firmware sends it. |

## Stratum V2

| Field | Value |
|---|---|
| Endpoint | `<node>:<sv2_port>` (the node opens no SV2 port unless `sv2_port` is set) |
| Authority public key | the node's `sv2_authority_public_key`, when it requires the Noise handshake |
| `user_identity` | `<payout CashAddr>.<worker>`, then a separator and options, as the SV1 username and password |

- Standard (header-only) and extended channels are served, up to 32 channels
  per connection. Job Declaration is served in both modes; see
  [bch-job-declaration.md](bch-job-declaration.md).
- The options use the SV1 password format after a comma, space, tab or
  semicolon. Only `p2p=` applies; the same rules as SV1 decide the share
  owner. Example on chipnet:
  `bchtest:qr3rryn6ugv8qpk82ccaqzk4ras92vpluvr2yclquu.rig1,p2p=tp2pool:qq...`
- `d=` and `th=` are not used on SV2. A channel starts at the difficulty its
  `nominal_hash_rate` calls for (within the node's minimum and maximum, or
  `start_difficulty` when the rate is 0), capped by the channel's
  `max_target`; vardiff and `UpdateChannel` move it after that. As on SV1, a
  channel's difficulty never goes above the share difficulty of its job: a
  channel that would open above it opens at it, and a channel above a new
  job's share difficulty gets a `SetTarget` before the job.
- A refused channel gets `OpenMiningChannel.Error` with
  `invalid-user-identity` (unreadable payout address),
  `invalid-share-address` (bad or conflicting `p2p=`) or
  `share-address-required` (no `p2p=` and no `miner_address`).
- `version_rolling_allowed` is set when the node's `version_mask` is not 0;
  rolled bits must stay inside it.
- Without authority keys the node speaks SV2 in plaintext, which the
  specification allows only between local roles: use it on localhost or a
  trusted LAN. Remote miners should get the authority public key from the
  operator and pin it.
- Stratum V2 proxies that append `.<worker>` to the whole identity are
  handled: the share address ends at its first dot.

## Node settings miners notice

Set by the operator under `[stratum]` (see `config.sample.toml`):

| Setting | Effect on miners |
|---|---|
| `port`, `sv2_port` | Where miners connect. |
| `start_difficulty` | Difficulty before vardiff has data, when the miner asks for none. |
| `minimum_difficulty`, `maximum_difficulty` | Bounds for `d=`, `th=`, `mining.suggest_difficulty` and vardiff. Public nodes should set `maximum_difficulty`. The share difficulty caps them both: a `minimum_difficulty` above it is not applied, and the node logs a warning once. |
| `miner_address` | Owns every share; `p2p=` becomes optional and must match. |
| `version_mask` | Version bits miners may roll. |
| `sv2_authority_public_key`, `sv2_authority_secret_key` | Turn on the Noise handshake for SV2. |
| `max_connections` | Concurrent Stratum connections. |
| `wait_for_chain_sync` | When true, Stratum opens only once the share chain is current. |
| `donation`, `fee_address`, `fee` | Cut taken from each block before the PPLNS window: 1.5% donation by default, plus any pool fee, 10% at most together. |

Each block's coinbase pays the miners in the PPLNS window directly. The
outputs can be spent after 100 Bitcoin Cash blocks, as for any coinbase. How
much a small miner can expect, and how often, is in
[small-miners.md](small-miners.md).
