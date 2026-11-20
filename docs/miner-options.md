# Miner options

Everything a miner sets to mine on a P2Poolv2 Bitcoin Cash node, over
Stratum V1 or Stratum V2, and the node settings miners notice.

A miner needs two addresses, on two different chains:

- a **payout address**: a CashAddr on the node's Bitcoin Cash network
  (`bitcoincash:`, `bchtest:` on chipnet, `bchreg:` on regtest). Every block
  the pool finds pays it in the coinbase.
- a **share address**: `p2pool:`, `tp2pool:`, `sp2pool:` or `rp2pool:`. It
  owns the miner's shares, which can be sold for BCH
  ([share trading](atomic-swap/bch-share-trading.md)). Make it from a P2PKH
  receiving address of a wallet you control, ideally not the payout address
  (the command takes an address, never a private key):

  ```sh
  p2poolv2_cli bch-share-address --network chipnet bchtest:qq...
  # tp2pool:qq...
  ```

  If the node sets `miner_address`, that address owns every share and miners
  need none.

## Stratum V1

| Field | Value |
|---|---|
| URL | `stratum+tcp://<node>:3333` (the node's `[stratum] port`) |
| Username | `<payout CashAddr>.<worker>`, optionally followed by options after a comma |
| Password | options, see below; anything else (such as `x`) is ignored |

### Username

- Before the first dot: the payout address, a P2PKH or P2SH CashAddr for the
  node's network. The prefix may be left out (`qq...` is read as
  `bitcoincash:qq...` on mainnet); the checksum must still match it. A
  token-aware CashAddr (`z...`, `r...`) is paid like its plain `q...` or
  `p...` form. P2SH32 addresses are not accepted yet.
- The dashboard, `/metrics` and the store know a miner by its address in
  full: prefix, lower case, plain form. Every spelling is one miner.
- After the first dot: the worker name, optional.
- Without a worker name, the node gives the login one: the device from the
  `mining.subscribe` user agent (`bitaxe/24.4.0` gives `bitaxe`, a second
  bitaxe `bitaxe2`), or a number (`worker1`, `worker2`, ...) when there is no
  user agent. Each login of an address then has its own dashboard row and
  metrics key. An explicit name is used as typed. A login is never refused
  for want of a name: with 1,000 names of one address live, it mines
  unnamed.
- Options may follow the worker name after a comma, space, tab or semicolon,
  as in the password: `bchtest:qq....rig1,donation=0.5`. They count as if
  they came first in the password. Pickaxe writes its logins this way.
- At most 128 characters, not counting the options.
- An unreadable username gets `Invalid username`; a second bad attempt closes
  the connection.

### Options

`key=value` pairs separated by commas, spaces, tabs or semicolons, in any
order, in the password or after a comma in the username. Keys are case
insensitive. Unknown keys, such as Pickaxe's `pickaxe-donation=`, are ignored
(noted in the debug log); only a bad value of a known key is refused.

| Option | Effect |
|---|---|
| `p2p=<share address>` | The share address that owns this miner's shares. Needed unless the node sets `miner_address`. |
| `d=<difficulty>` | Start at this difficulty instead of the node's `start_difficulty`. A whole number above 0. |
| `th=<TH/s>` | Your hashrate in whole TH/s; you start at the difficulty that gives one share every 3.3 seconds, about 776 per TH/s. `d=` wins if both are given. |
| `donation=<percent>` | Your own P2Pool donation, 0 to 10 in steps of 0.5. Without it the node's `donation` applies, 1% by default. See [Donation](#donation). |
| `min-payout=<satoshis>` | The smallest payment you want, in whole satoshis: your payout level becomes the smallest level at or above it, at most level 7 (69,888 satoshis). See [Payout level](#payout-level). |
| `fee-tolerance=<percent>` | The most of a payment you accept to spend when spending it, 0 to 100 with at most two decimals: your payout level becomes the smallest level whose spend costs at most that share. `min-payout=` wins if both are given. |

Rules:

- `d=` and `th=` are kept within `minimum_difficulty` and
  `maximum_difficulty`. After the first shares, vardiff aims at one share
  every 3.3 seconds (0.3 shares a second).
- Your difficulty never goes above your job's share difficulty, whatever
  `d=`, `th=`, `mining.suggest_difficulty`, vardiff or `minimum_difficulty`
  say: a higher one would discard hashes that are shares. Before a job with a
  lower share difficulty the node sends `mining.set_difficulty`. The cap is
  rounded down: the lowest share difficulty, 4.55, holds you at 4.
- `p2p=` must be a share address for the node's network: `p2pool:` on
  mainnet, `tp2pool:` on chipnet, `sp2pool:` on scalenet, `rp2pool:` on
  regtest.
- If the node sets `miner_address`, leave `p2p=` out or send that address;
  another is refused: `Share address conflicts with the pool's configured
  address`.
- Without `p2p=` and `miner_address`, authorize is refused: `This pool needs
  a Bitcoin Cash share address. Set the stratum password to p2p=<address>`.
- A worker suffix after the share address (`p2p=tp2pool:qq....rig1`) is
  ignored.
- `p2p=` plus a share address is 53 characters (`tp2pool:` 54). Check that
  your firmware does not cut the password: a cut address fails its checksum
  and is refused; it never pays someone else.

### Donation

Each share you earn records the fraction of its BCH payouts that goes to the
P2Pool project's Bitcoin Cash address. `donation=` sets it, on any P2Pool
node:

- 0 to 10 in steps of 0.5 (`0`, `0.5`, `1`, `1.5` ... `10`), at most two decimals
  (`1.50` is 1.5), no `%` sign.
- It applies only to your shares' part of each reward. The node's `donation`
  is only the default for miners who choose none and never overrides yours.
  The operator's fee is taken from what the donation leaves; each has its own
  10% cap ([Payout distribution](architecture/payout-distribution.md)).
- On a node that merge-mines tokens, your choice and the operator's fee also
  apply to each token win
  ([Auxiliary work proofs](bch-auxpow.md#merge-mining-tokens-on-a-public-node)).
  Firmware that asks for `pickaxe-merge` in `mining.configure`, or an SV2
  client that negotiates extension `0x4D4D`, is sent its leaves and win
  proofs.
- Pickaxe's own donation key is Pickaxe's; P2Pool ignores it.
- A value off the 0.5 steps, above 10, empty or unreadable is refused at
  authorize with the reason; the node never falls back to its own donation.
- A Bitcoin node refuses a login that carries `donation=`.

### Payout level

Each share you earn records your payout level `k`, from 1 to 7: the smallest
payment you want, `546 * 2^k` satoshis.

| Level | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 |
|---|---|---|---|---|---|---|---|---|
| Satoshis | 546 | 1,092 | 2,184 | 4,368 | 8,736 | 17,472 | 34,944 | 69,888 |

When a block pays you less than your level, your slice is pooled with the
other miners' at that level and paid by a draw in the same block: fewer,
larger payments, nothing held or owed, and on average what you earned
([Coinbase payouts](payouts/coinbase.md#payout-levels)).

- `min-payout=10000` gives level 5 (17,472). Any whole number of satoshis is
  accepted; above 69,888 it is level 7.
- `fee-tolerance=1` asks that spending cost at most 1%. With the node's
  `spend_cost` of 148 satoshis (a P2PKH input at 1 sat/byte) that is level 5:
  0.85% of 17,472, where level 4 would be 1.7% of 8,736. A tolerance no level
  meets gives level 7.
- Without either option, the node's `payout_level` applies, level 5 by
  default.
- The node lowers your level when you would expect fewer than 3 payments a
  month at it (from your hashrate, the pool's and the block reward), and logs
  a warning once per connection. That is node policy, not a share-chain rule.
- An empty, signed, fractional or unreadable `min-payout=`, or a
  `fee-tolerance=` above 100 or with more than two decimals, is refused at
  authorize with the reason.
- A Bitcoin node refuses a login that carries either option.

For `donation=`, `min-payout=` and `fee-tolerance=`, the same value twice is
accepted and two different values are refused.

### Examples

Chipnet, own share address, starting difficulty 65,536:

```
url:      stratum+tcp://pool.example.org:3333
user:     bchtest:qr3rryn6ugv8qpk82ccaqzk4ras92vpluvr2yclquu.rig1
password: p2p=tp2pool:qq...,d=65536
```

Mainnet, a login shared with Pickaxe that donates 0.5% to P2Pool:

```
user:     bitcoincash:qq....rig1,pickaxe-donation=0,donation=0.5
password: p2p=p2pool:qq...
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
| `mining.configure` with `version-rolling` | Answers with the node's `version_mask` (default `1fffe000`, BIP 320's general purpose bits). Bits outside the mask are dropped, so rolling them misses the difficulty. |
| `mining.suggest_difficulty` | Like `d=`: within the node's bounds and the share difficulty, then vardiff. After a login with `d=` or `th=`, that difficulty stays and is sent again (Whatsminer's firmware suggests one right after authorizing). |
| `mining.extranonce.subscribe` | Acknowledged. The node never changes a session's extranonce. |
| `mining.authorize` before `mining.subscribe` | Accepted, as some firmware sends it. |

## Stratum V2

| Field | Value |
|---|---|
| Endpoint | `<node>:<sv2_port>` (no SV2 port unless `sv2_port` is set) |
| Authority public key | the node's `sv2_authority_public_key`, when it requires the Noise handshake |
| `user_identity` | `<payout CashAddr>.<worker>`, then a separator and options, as the SV1 username and password |

- Standard (header-only) and extended channels, up to 32 per connection, and
  Job Declaration in both modes
  ([bch-job-declaration.md](bch-job-declaration.md)).
- `p2p=`, `donation=`, `min-payout=` and `fee-tolerance=` apply by the SV1
  rules; other keys are ignored. Each channel mines with its own identity's
  donation and payout level. Example on chipnet:
  `bchtest:qr3rryn6ugv8qpk82ccaqzk4ras92vpluvr2yclquu.rig1,donation=0.5,p2p=tp2pool:qq...`
- With Job Declaration, give `AllocateMiningJobToken.user_identifier` the
  channel's `donation=`, or leave it out of both: otherwise the channel
  refuses the custom job with `donation-mismatch`. The token carries the
  payout level its `user_identifier` chooses (or the node's), which the node
  does not lower. A bad option there closes the connection (the protocol has
  no error reply), with the reason in the node's log.
- A client of the requested payout outputs extension gives `user_identifier`
  the channel's whole identity
  ([Payout outputs on the Job Declaration connection](bch-job-declaration.md#payout-outputs-on-the-job-declaration-connection)).
- `d=` and `th=` are not used. A channel starts at the difficulty its
  `nominal_hash_rate` calls for (within the node's bounds, or
  `start_difficulty` when the rate is 0), capped by its `max_target`; vardiff
  and `UpdateChannel` move it after that. It never goes above the job's share
  difficulty: a channel gets a `SetTarget` before a job whose share
  difficulty is lower.
- A refused channel gets `OpenMiningChannel.Error` with
  `invalid-user-identity` (unreadable payout address),
  `invalid-share-address` (bad or conflicting `p2p=`),
  `share-address-required` (no `p2p=` and no `miner_address`),
  `invalid-donation` or `invalid-payout-level` (a bad option, or one on a
  node that cannot honour it).
- `version_rolling_allowed` is set when `version_mask` is not 0.
- Without authority keys SV2 is plaintext, which the specification allows
  only between local roles: keep it on localhost or a trusted LAN. Remote
  miners pin the operator's authority public key.
- Proxies that append `.<worker>` to the whole identity are handled: the
  share address ends at its first dot. Behind one, put `p2p=` last; a suffix
  after `donation=` makes it unreadable and the channel is refused.
- `RequestExtensions` gets `RequestExtensions.Success` naming what the node
  grants (on a Mining connection, `0x4D4D` and `0x0002`), or
  `RequestExtensions.Error` listing them all when it grants none.

### Worker names on one channel (`0x0002`)

A proxy with many machines on one extended channel can name the machine
behind each share with Worker-Specific Hashrate Tracking (`0x0002`):

- Ask with `RequestExtensions` right after `SetupConnection.Success`, before
  opening a channel, as the extensions negotiation (`0x0001`) orders; a later
  request is not granted. The node never requires it.
- End each `SubmitSharesExtended` with a TLV field: extension type `0x0002`
  (`U16`), field type `0x01` (`U8`), length (`U16`) and the name, at most 32
  bytes of UTF-8. The header is little-endian, as every SV2 field; the
  extension's own example prints it big-endian.
- The dashboard, `/miners` and Prometheus list each name as a worker of the
  channel's payout address until the channel closes. The channel's payout
  address and share owner still own every share; names never leave the node.
- A share without a name counts as the channel's own worker. A name over 32
  bytes or not UTF-8 refuses the share with `invalid-share`. Other TLV fields
  and trailing partial fields are ignored, as are all TLV fields on a
  connection that did not ask.
- A channel takes at most 1,024 names; later new names count as the
  channel's own worker.

## Node settings miners notice

Set by the operator under `[stratum]` (see `config.sample.toml`):

| Setting | Effect on miners |
|---|---|
| `port`, `sv2_port` | Where miners connect. |
| `start_difficulty` | Difficulty before vardiff has data, when the miner asks for none. |
| `minimum_difficulty`, `maximum_difficulty` | Bounds for `d=`, `th=`, `mining.suggest_difficulty` and vardiff. Public nodes should set `maximum_difficulty`. The share difficulty caps both: a `minimum_difficulty` above it is not applied, with a warning logged once. |
| `miner_address` | Owns every share; `p2p=` becomes optional and must match. |
| `version_mask` | Version bits miners may roll. |
| `sv2_authority_public_key`, `sv2_authority_secret_key` | Turn on the Noise handshake for SV2. |
| `max_connections` | Concurrent Stratum connections. |
| `wait_for_chain_sync` | When true, Stratum opens only once the share chain is current. |
| `donation` | The donation of miners whose login chooses none: 1% by default. |
| `fee_address`, `fee` | The operator's fee, taken from what the donation leaves: 1% once `fee_address` is set, unless `fee` says otherwise; at most 10%. |
| `payout_level` | The payout level of miners whose login chooses none, 1 to 7: level 5 (17,472 satoshis) by default. |
| `spend_cost` | What spending one payment costs, in satoshis, for `fee-tolerance=` and the node's level policy: 148 by default. |

Each block's coinbase pays the miners in the PPLNS window directly; the
outputs can be spent after 100 Bitcoin Cash blocks. What a small miner can
expect: [small-miners.md](small-miners.md).
