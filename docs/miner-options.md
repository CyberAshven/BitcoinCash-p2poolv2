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
| Username | `<payout CashAddr>.<worker>`, optionally followed by options after a comma |
| Password | options, see below; anything else (such as `x`) is ignored |

### Username

- The part before the first dot is the payout address. It must be a P2PKH or
  P2SH CashAddr for the node's network. The prefix may be left out
  (`qq...` is read as `bitcoincash:qq...` on mainnet); the checksum must
  still match the network's prefix. A token-aware CashAddr (`z...` or
  `r...`, which CashTokens wallets hand out) is accepted too, and is paid
  with the same P2PKH or P2SH output as its plain `q...` or `p...` form.
  P2SH32 addresses (32-byte hashes) are not accepted yet.
- The dashboard, `/metrics` and the store know a miner by its address
  written out in full: prefix, lower case, plain (`q...` or `p...`) form.
  Every spelling of one address is one miner, whichever way its workers
  type it.
- Everything after the first dot is the worker name. It labels the worker
  in the dashboard and metrics and is optional.
- Options may follow the worker name after a comma, space, tab or
  semicolon, in the password's format: `bchtest:qq....rig1,donation=0.5`.
  They count as if they came first in the password. Pickaxe writes its
  logins this way, so one login works at both kinds of pool.
- At most 128 characters in all, not counting the options.
- A username the node cannot read gets the error `Invalid username`; a second
  bad attempt on the same connection closes it.

### Options

Options are `key=value` pairs separated by commas, spaces, tabs or
semicolons, in any order, in the password or after a comma in the
username. Keys are case insensitive. Keys the node does not read, such as
Pickaxe's own `pickaxe-donation=`, are ignored (the node notes them in its
debug log); only a bad value of a key the node reads is refused.

| Option | Effect |
|---|---|
| `p2p=<share address>` | The share address that owns this miner's shares. Needed unless the node sets `miner_address`. |
| `d=<difficulty>` | Start at this difficulty instead of the node's `start_difficulty`. A whole number above 0. |
| `th=<TH/s>` | Your hashrate in whole TH/s; the node starts you at the difficulty that gives one share every 3.3 seconds, about 776 per TH/s. `d=` wins if both are given. |
| `donation=<percent>` | Your own P2Pool donation: 0 to 10 in steps of 0.5, such as `donation=0`, `donation=0.5` or `donation=1`. Without it the node's `donation` applies, 1% by default. See [Donation](#donation). |
| `min-payout=<satoshis>` | The smallest payment you want, in whole satoshis: your payout level becomes the smallest level at or above it, at most level 7 (69,888 satoshis). See [Payout level](#payout-level). |
| `fee-tolerance=<percent>` | The most of a payment you accept to spend when you spend it, 0 to 100 with at most two decimals, such as `fee-tolerance=1`: your payout level becomes the smallest level whose spend costs at most that share of it. `min-payout=` wins if both are given. See [Payout level](#payout-level). |

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
  share chain's lowest share difficulty, 4.55, holds you at 4.
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

### Donation

Each share you earn records what fraction of its future BCH payouts goes
to the P2Pool project's fixed Bitcoin Cash address. With `donation=` each
miner chooses that fraction for themselves, on any P2Pool node:

- The value is a percentage from 0 to 10 in steps of 0.5: `0`, `0.5`, `1`,
  `1.5` ... `10`, with at most two decimals (`1.50` is 1.5) and no `%`
  sign. `donation=0` donates nothing.
- The node builds every job and share for you with your choice. Every pool
  block that pays your share applies that share's recorded choice only to
  its own part of the reward. Finding a block does not charge your donation
  to other miners. The node's own `donation` is only the default
  for miners who choose none: it cannot raise, or lower, your choice.
- For each share, the donation is taken from its weighted entitlement
  first. The operator's fee recorded on that share, if any, is taken from
  what its donation leaves and paid to that share's fee address; the rest
  goes to its payout-right holder. The donation and fee each have their own
  10% cap. Donation and fee recipients are combined by payout script with
  all other payees and follow the same direct or payout-tree rules. Integer
  rounding remainders go to the largest payee.
- On a node that merge-mines tokens, your choice and the operator's fee
  also apply to each token win: your token leaves carry them as the
  split's donation and fee slots
  ([Auxiliary work proofs](bch-auxpow.md#merge-mining-tokens-on-a-public-node)).
  Firmware that asks for `pickaxe-merge` in `mining.configure`, or an SV2
  client that negotiates extension `0x4D4D`, is sent its leaves and the
  proofs of its wins.
- This is P2Pool's donation. Pickaxe's donation is Pickaxe's own, set with
  its own key, and P2Pool ignores that key.
- A value off the 0.5 steps, above 10, empty or otherwise unreadable is
  refused at authorize with a message saying why; the node never falls back
  to its own donation. The same value given twice is accepted; two different
  values are refused.
- Only a Bitcoin Cash node can honour the choice. A Bitcoin node refuses a
  login that carries `donation=`; leave it out to mine there.

### Payout level

Each share you earn records your payout level `k`, from 0 to 7, the smallest
payment you want: `546 * 2^k` satoshis.

| Level | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 |
|---|---|---|---|---|---|---|---|---|
| Satoshis | 546 | 1,092 | 2,184 | 4,368 | 8,736 | 17,472 | 34,944 | 69,888 |

When a block pays you less than your level, your slice is pooled with the
slices of the other miners at the same level and paid by a draw in that
same block: fewer, larger payments, nothing held and nothing owed. On
average you are paid what you earned
([Coinbase payouts](payouts/coinbase.md#payout-levels)).

- `min-payout=10000` asks for payments of at least 10,000 satoshis: level 5
  (17,472). Any whole number of satoshis is accepted; above 69,888 it is
  level 7.
- `fee-tolerance=1` asks for payments that cost at most 1% to spend. With
  the node's `spend_cost` of 148 satoshis (a P2PKH input at 1 sat/byte) that
  is level 5: 148 satoshis are 0.85% of 17,472, while at level 4 they would
  be 1.7% of 8,736. A tolerance no level meets gives level 7.
- `min-payout=` wins when both are given. Without either the node's
  `payout_level` applies, level 5 by default.
- The node lowers your level when you would expect fewer than 3 payments a
  month at it, from your hashrate, the pool's and the block reward, and
  logs a warning once per connection. This is the node's choice, not a
  share-chain rule; any level from 0 to 7 is valid.
- An empty, signed, fractional or otherwise unreadable `min-payout=`, or a
  `fee-tolerance=` above 100 or with more than two decimals, is refused at
  authorize with a message saying why. The same value given twice is
  accepted; two different values are refused.
- Only a Bitcoin Cash node has payout levels. A Bitcoin node refuses a
  login that carries either option.

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
  semicolon. `p2p=`, `donation=`, `min-payout=` and `fee-tolerance=` apply,
  by the same rules as on SV1; other keys are ignored. Each channel mines
  with its own identity's donation and payout level. Example on chipnet:
  `bchtest:qr3rryn6ugv8qpk82ccaqzk4ras92vpluvr2yclquu.rig1,donation=0.5,p2p=tp2pool:qq...`
- With Job Declaration, give `AllocateMiningJobToken.user_identifier` the
  same `donation=` as the channel's `user_identity` (or leave it out of
  both): the token's terms carry it, and a channel whose identity chose
  another donation refuses the custom job with `donation-mismatch`. The
  token's terms carry the payout level `user_identifier` chooses with
  `min-payout=` or `fee-tolerance=`, or the node's; the node does not lower
  a declared job's level. The protocol has no error reply to
  `AllocateMiningJobToken`, so a bad `donation=`, `min-payout=` or
  `fee-tolerance=` there closes the connection, with the reason in the
  node's log.
- A client of the requested payout outputs extension gives
  `AllocateMiningJobToken.user_identifier` the channel's whole identity:
  the payout CashAddr, then `p2p=` and any `donation=`. The node pays the
  outputs it answers to that payout address and share owner
  ([Payout outputs on the Job Declaration connection](bch-job-declaration.md#payout-outputs-on-the-job-declaration-connection)).
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
  `share-address-required` (no `p2p=` and no `miner_address`),
  `invalid-donation` (a bad `donation=`, or one on a node that cannot
  honour it) or `invalid-payout-level` (a bad `min-payout=` or
  `fee-tolerance=`, or one on a node that cannot honour it).
- `version_rolling_allowed` is set when the node's `version_mask` is not 0;
  rolled bits must stay inside it.
- Without authority keys the node speaks SV2 in plaintext, which the
  specification allows only between local roles: use it on localhost or a
  trusted LAN. Remote miners should get the authority public key from the
  operator and pin it.
- Stratum V2 proxies that append `.<worker>` to the whole identity are
  handled: the share address ends at its first dot. Behind such a proxy put
  `p2p=` last: a suffix after `donation=` makes its value unreadable, and
  the channel is refused.
- A proxy that puts many machines on one extended channel can name the
  machine behind each share with the extension Worker-Specific Hashrate
  Tracking (`0x0002`). It asks for it with `RequestExtensions` right after
  `SetupConnection.Success`, before opening a channel, as the extensions
  negotiation (`0x0001`) orders; a later request is not granted. It then
  ends each `SubmitSharesExtended` with a TLV field: extension type
  `0x0002` (`U16`), field type `0x01` (`U8`), the length (`U16`) and the
  name, at most 32 bytes of UTF-8. The header is little-endian, as every
  SV2 field is; the extension's own example prints it big-endian.
  - The node's dashboard, `/miners` and Prometheus metrics list each named
    machine as a worker of the channel's payout address, active until the
    channel closes, as they list SV1 workers. Only the label changes: the
    channel's payout address and share owner still own every share.
  - A share without a name counts as the channel's own worker. A name over
    32 bytes, or not UTF-8, refuses that share with `invalid-share`. TLV
    fields the node does not use are skipped, and bytes after the message
    that are not whole TLV fields are ignored, as before.
  - A channel's shares bring at most 1,024 names; a new name past that
    counts as the channel's own worker. A proxy with more machines opens
    more channels.
  - The names stay on this node, like SV1 worker names: they are never
    stored in a share, sent to a peer or put on the share chain.
  - The miner's side decides: the node grants `0x0002` when asked and never
    requires it. On a connection that did not ask, TLV fields are ignored.
- The node answers `RequestExtensions` with `RequestExtensions.Success`
  naming the extensions it grants (on a Mining connection, `0x4D4D` and
  `0x0002`), or with `RequestExtensions.Error` listing them all as
  unsupported when it grants none of them.

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
| `donation` | The donation of miners whose login chooses none: 1% by default. It never replaces a miner's own `donation=`. |
| `fee_address`, `fee` | The operator's fee, taken from what the donation leaves before the PPLNS window: 1% once `fee_address` is set, unless `fee` says otherwise; at most 10%. |
| `payout_level` | The payout level of miners whose login chooses none, 0 to 7: level 5 (17,472 satoshis) by default. |
| `spend_cost` | What spending one payment costs, in satoshis, for `fee-tolerance=` and the node's level policy: 148 by default. |

Each block's coinbase pays the miners in the PPLNS window directly. The
outputs can be spent after 100 Bitcoin Cash blocks, as for any coinbase. How
much a small miner can expect, and how often, is in
[small-miners.md](small-miners.md).
