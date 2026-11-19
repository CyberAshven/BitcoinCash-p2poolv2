# Network simulation: what limits P2Pool on Bitcoin Cash

`p2poolv2_netsim` is a discrete-event simulator of how shares travel through
a Bitcoin Cash P2Poolv2 network. It answers one question with numbers: as
the pool grows, what limits it first, the share chain, the coinbase or
bandwidth? It also measures what the
[hybrid payout design](../architecture/bch-hybrid-payouts.md) asks the
network sim for in its section 8.1: propagation delay and stale rate against
coinbase size from 1 KB to 1 MB.

The model runs on one-minute Bitcoin Cash blocks (CHIP-2025-03, on chipnet
and on mainnet from 2027-05-15): a block every minute, a tenth of the
ten-minute reward and of the ABLA floors (3.2 MB on mainnet, 200 kB on
chipnet, 25.6 MB to start on scalenet). Where a ten-minute result differs, it
is given in parentheses. Every row and column of the one-minute tables is
in this file at commit 28baa432, and the ten-minute tables at commit
2972cfd2. `cap-11`, the payout rule the hybrid rule replaced (at most
11 payouts and a weighted draw for the tail, decision 14, `payout-cap11`),
appears only as a comparison, and so does its 200 KiB share body limit.
Earlier rules: [Rule history](../sharechain/rule-history.md).

## The short answer

- **The coinbase limits P2Pool first, as a bandwidth cost.** Every payee
  paid directly puts 34 bytes into every share sent whole and into every
  Stratum job. The hybrid payout rule pays a share's payees directly up to
  BCH's 1,000,000-byte transaction limit (29,402 P2PKH payees) and the rest
  through a payout tree, so BCH never rejects a block for its coinbase. With
  whole shares on the assumed links, stale shares climb with the direct
  payees: 3% at 1,000, 8% at 6,010, 11% at 10,000 and 25% at 29,402 (a 1 MB
  coinbase; 0.6% orphaned).
- **Sending shares without their payouts removes that cost**: about 1.3 KB a
  share whatever the payee count, and 2.6% stale at 29,402 payees. Every
  node rebuilds the payouts to validate a share anyway. The node relays a
  share this way once its outputs take 50,000 bytes or more.
- **For miners the coinbase stays a per-job cost.** A Stratum V1 job carries
  it in hex about every 4.6 seconds: 124 kbit/s per miner at 1,000 payees. A
  Stratum V2 extended job cannot carry more than 1,922 payees, and stock
  firmware is reported to take 12 to 461. Only Stratum V2 standard channels
  carry no coinbase.
- **The PPLNS window bounds the payees.** It holds three BCH blocks of the
  pool's work, about `18 / f` shares for a pool with fraction `f` of BCH's
  hashrate. A pool above 0.3% of BCH has fewer than about 6,000 payees; only
  below about 0.06% (0.6%) can a crowd of small miners reach the 1 MB limit
  and the tree. With merge-mined tokens the 3,072-byte threshold keeps the
  coinbase at 3,051 bytes for any number of payees (79 direct).
- **The share chain itself is not the limit at 10-second shares.** With
  small shares 2 to 3% go stale, nearly all named as uncles (paid 9/10). It
  grows with latency (10% at 400 ms one way), node count (3.8% at 1,000
  nodes), slow job switching (7% at 0.5 s) and faster shares (16% at
  1-second shares).
- **Large blocks load BCHN and templates, not the share chain.** A node's
  BCHN needs about 6.1 Mbit/s each way at full 25.6 MB one-minute blocks and
  240 Mbit/s at 1 GB just to relay transactions; full `getblocktemplate`
  JSON every 10 seconds adds 8.7 Mbit/s at 3.2 MB blocks, which light
  templates avoid. A 1 MB coinbase adds 1.06% BCH orphan risk to every
  block the pool finds (0.11%).
- **Upstream's limit of about 1,000 miners at 10-second shares (3.83% stale
  on 1,000 nodes) moves only with smaller shares.** Shares without payouts,
  forwarded after the header check, on 8 to 64 peers keep 1,000 nodes
  within that stale rate down to 5.3 to 4.0 second shares (5.0 to 3.5); the
  node does all three, with 16 outgoing peers. Weak shares gossiped beside
  the shares cost upload but no stale shares.
- **For pools paying tens of thousands of miners, validation bounds a faster
  share period.** Checking a share costs a node 6.4 ms with 1,000 miners in
  the window, 38 ms with 10,000 and 362 ms with 100,000; at 10-second shares
  100,000 miners keep a node 18% busy.

## Measured and estimated

Nothing here is measured on a running pool. Share, coinbase and message
sizes and the payee counts at each limit are counted exactly, from this
repository's encoders and the hybrid rule's byte counts. Stale, uncle and
orphan rates, propagation times and P2Pool's bytes per node are simulated
under assumed link distributions; the bandwidth assumptions audit plans the
real-world check (`bch-btc-assumptions-audit.md`, M2: chipnet nodes with
padded shares). BCHN's relay, compact block and template traffic, Stratum
job traffic, the size of a share sent without its payouts and the extra BCH
orphan risk of a large coinbase are estimated from formulas, for full
blocks of 226-byte transfers. What checking a share costs is measured in the
node's code ("Limits study").

## Run it

```sh
cargo netsim run                         # one scenario, as text
cargo netsim run --payees 29402 --relay without-payouts
cargo netsim run --tokens --miners 1000000 --pool-fraction 0.001
cargo netsim run --format json           # every number
cargo netsim sweep payees                # one sweep as a Markdown table
cargo netsim sweep all                   # every sweep in Results
cargo netsim sweep scaling --threads 10  # every sweep of the scaling study
cargo netsim interval-limit scaling --nodes 1000 --threads 10
cargo netsim sweep limits --threads 4    # every sweep of the limits study
cargo netsim limits                      # payee counts at each limit
```

`cargo netsim` is an alias for
`cargo run --release -p p2poolv2_netsim --bin netsim --`; `netsim run
--help` lists every option. A run is a pure function of its inputs and its
seed (`--seed`, default 1), so the tables below come out the same on any
machine, and each table's full columns come from its command.

## The model

It ports the idea of upstream's ns-3 simulation,
[p2poolv2/Ns3_simulation](https://github.com/p2poolv2/Ns3_simulation)
(`ns-3.44/scratch/p2pool_v2_with_share.cc`): nodes on a small-world network
mining and gossiping shares, counting uncles and orphans. This one is Rust,
copies no upstream code, and changes the model for Bitcoin Cash and for
scale:

- **Shares** are found pool-wide as a Poisson process (one every 10 s,
  `IDEAL_BLOCK_TIME`), each by a node picked by hashrate, on upstream's
  small-world graph (a ring of 4 neighbours a side, each link moved with
  probability 0.5).
- **Relay** follows `send_share_block_broadcast`: a node that has validated
  a share pushes it to every peer not known to have it, sharing its upload
  among them, and each peer answers with an `Ack`. A share waits for its
  parent and uncles, then takes the validation time (5 ms plus 20 ms per MB,
  an estimate). Miners switch to new work after 50 ms. A new share names up
  to three uncles as `Store::find_uncles` picks them.
- **Outcome:** a share on the best chain is fine, one named by it as an
  uncle is stale but paid 9/10, any other is orphaned.
- **Links** are fluid (no TCP slow start, no loss). BCHN's transaction relay
  and every BCH compact block share each node's link; a node that cannot
  carry BCHN's relay keeps relaying shares, but its miners move to other
  nodes.
- **Coinbase layouts:** `no-tokens` (the hybrid rule: payees direct while
  their outputs fit `1,000,000 - S`, `S` = 367 bytes, the rest in one
  P2SH32 payout tree output), `tokens-3072` (coinbase limit `T` = 3,072
  bytes), `cap-11` and `uncapped` (upstream's Bitcoin rule). Donation and
  fee come off each share's weight (`cuts-per-share`), so their addresses
  are payees.
- **Payees** are the miners expected in a window of `min(120,960, 3 * 6 /
  f)` shares (`BCH_PPLNS_WINDOW_BLOCKS` = 3), plus the donation key.

It leaves out share difficulty changes, header-first sync, share
transactions (`--extra-share-bytes` adds their bytes), node churn and
attacks.

## Inputs

| Input | Option | Default |
|---|---|---|
| Nodes, peers per node, rewiring | `--nodes`, `--peers`, `--rewire` | 100, 8, 0.5 |
| Hashrate over nodes | `--node-hashrate` | equal |
| Miners and their hashrate | `--miners`, `--miner-hashrate` | 1,000, equal |
| Payees, instead of the expected count | `--payees` | from miners and window |
| Pool's fraction of BCH's hashrate | `--pool-fraction` | 0.01 (window of 1,800 shares) |
| Share interval | `--share-interval` | 10 s |
| BCH block size, average transaction | `--block-bytes`, `--transaction-bytes` | 3.2 MB (mainnet's one-minute ABLA floor), 226 bytes |
| Payout rule, tokens, threshold | `--payout`, `--tokens`, `--token-threshold` | hybrid, no tokens, 3,072 bytes |
| How shares travel | `--relay` | `full` (whole shares) |
| Upload, download (Mbit/s) | `--upload-mbps`, `--download-mbps` | `lognormal:50:0.8`, `lognormal:200:0.8` |
| One-way latency (ms) | `--latency-ms` | `lognormal:50:0.5` |
| Validation, job switch | `--validation-seconds`, `--validation-seconds-per-megabyte`, `--work-switch-seconds` | 5 ms + 20 ms/MB, 50 ms |
| BCHN on its own link | `--separate-bchn-link` | shares the node's link |
| BCHN peers | `--bch-peers` | 8 (`MAX_OUTBOUND_CONNECTIONS`) |
| Stratum protocol | `--stratum` | `v1` |
| Duration, seed | `--duration`, `--seed` | one day (8,640 shares), 1 |
| Uncle cap | `--max-uncles` | 3 (`MAX_UNCLES`) |
| When a share is forwarded | `--forward`, `--header-check-seconds` | `after-validation`; 1 ms for `after-header-check` |
| Weak shares | `--weak-shares`, `--weak-share-bytes`, `--weak-share-carriage` | none; 800 bytes; `gossip-only` |

The link distributions are assumptions, a log-normal mix standing for home
links and servers together. A distribution is written `50` (fixed),
`uniform:10:100` or `lognormal:<median>:<sigma>`.

## Limits on the coinbase

From `cargo netsim limits`: the most P2PKH payees a coinbase can pay
directly under each limit, with every other field at its largest.

| Limit | Whose | Direct payees at most |
|---|---|---:|
| 200 KiB share body (`BLOCK_TXS_SIZE_LIMIT`), before the hybrid rule | P2Pool share-chain rule, historical on Bitcoin Cash | 6,010 |
| 1,204,800-byte share body (`BCH_SHARE_BODY_SIZE_LIMIT`, design section 8.1) | P2Pool share-chain rule, hybrid | 35,422 |
| 2,144 KiB P2P message (`MAX_P2P_MESSAGE_SIZE`), keeping 960 KiB for carried weak shares (`WEAK_BYTES`) | this node's implementation | 35,634 |
| 1,000,000-byte transaction (`MAX_TX_SIZE`, BCHN `src/consensus/tx_check.cpp`) | Bitcoin Cash consensus | 29,402 |
| 65,535-byte coinbase field of a Stratum V2 extended or declared job | Stratum V2 | 1,922 |
| 10,000-byte VM stack element, for a token claim hashing the coinbase (CHIP-2021-05) | Bitcoin Cash consensus | 284 |
| 784-byte OCEAN block on stock Antminer firmware (upstream #668) | evidence from another pool | 13 |
| DATUM gateway's per-device coinbase sizes: 755 bytes (older stock S19), 2,250 (stock S21), 6,500 (Whatsminer, Bitaxe), 16,000 (ePIC, vnish) | evidence from another pool's software | 12, 56, 181, 461 |

Under the hybrid rule a share without tokens pays up to 29,402 payees
directly and, past that, 29,400 beside the tree output, a 999,967-byte
coinbase. A share with tokens pays up to 80 directly and, past that, 79
beside the tree, a 3,051-byte coinbase. ABLA never raises the transaction
limit, and none of these limits depends on the block interval.

## Results

All tables come from `cargo netsim sweep <name>` with the defaults above,
seed 1 and a day of mining, and keep the columns that carry the result.
Stale counts uncles and orphans; "to all" is the mean seconds until every
node had each share ("never" if some node never got some share); "P2Pool
up" is the share-chain upload per node. Under eight seeds (`sweep seeds`)
the default's stale rate moves between 2.68% and 3.35%: differences under
about half a point are noise.

### Payees and the coinbase layout

`sweep payees`:

| Scenario | Coinbase bytes | Share bytes | Stale | Orphaned | To all (s) | P2Pool up, max (Mbit/s) | Stratum V1 per miner (kbit/s) |
|---|---:|---:|---:|---:|---:|---:|---:|
| no-tokens, 10 payees | 661 | 1,679 | 1.95% | 0.00% | 0.20 | 0.016 | 4.36 |
| no-tokens, 1,000 payees | 34,323 | 36,261 | 3.01% | 0.00% | 0.38 | 0.321 | 124 |
| no-tokens, 6,010 payees | 204,663 | 211,241 | 7.64% | 0.01% | 1.13 | 1.87 | 731 |
| no-tokens, 10,000 payees | 340,323 | 350,621 | 10.94% | 0.00% | 1.73 | 3.10 | 1,214 |
| no-tokens, 29,402 payees or more | 999,991 | 1,028,369 | 24.70% | 0.62% | 5.05 | 9.08 | 3,564 |
| tokens-3072, any payees | 3,051 | 4,109 | 2.05% | 0.00% | 0.22 | 0.037 | 12.9 |
| cap-11, 1,000,000 payees | 782 | 1,800 | 1.97% | 0.00% | 0.20 | 0.017 | 4.78 |
| uncapped, 100,000 payees | 3,400,325 | 3,494,463 | 51.70% | 15.77% | never | 30.8 | 12,113 |

Past 29,402 payees the hybrid rule's tree keeps the coinbase under 1 MB, so
100,000 and a million payees cost the same as 29,402. Without that limit
(`uncapped`) a 100,000-payee coinbase is 3.4 MB: BCH rejects the block, it
breaks the share body and P2P message limits, and a share takes longer to
cross the network than the share interval.

### Sending shares without their payouts

`sweep relay`:

| Payees | Coinbase bytes | Stale, whole | Stale, without payouts | To all, whole (s) | To all, without payouts (s) |
|---:|---:|---:|---:|---:|---:|
| 1,000 | 34,323 | 3.01% | 1.95% | 0.38 | 0.20 |
| 6,010 | 204,663 | 7.64% | 2.09% | 1.13 | 0.22 |
| 29,402 | 999,991 | 24.70% | 2.55% | 5.05 | 0.30 |

Without the payouts a share is 1,301 bytes at any payee count: the share
chain no longer cares how many miners are paid directly. The remaining
growth is validation: rebuilding a 1 MB coinbase is taken to cost 20 ms. The
coinbase itself still reaches the miners in their Stratum jobs and BCH in
the found block.

### Pool size: the window bounds the payees

`sweep pool-size`, a million equally sized miners:

| Pool's part of BCH | Payees | Coinbase bytes | Stale |
|---:|---:|---:|---:|
| 0.1% | 17,840 | 606,883 | 17.09% |
| 0.3% | 5,983 | 203,745 | 7.61% |
| 1% | 1,799 | 61,489 | 3.77% |
| 3% | 601 | 20,757 | 2.57% |
| 10% | 181 | 6,475 | 2.18% |
| 30% | 61 | 2,395 | 2.02% |

The window is three BCH blocks of the pool's work, so a growing pool pays
fewer, larger miners per block: at 10% of BCH a million miners fill at most
180 payee slots, 181 payees with the donation key. No pool in the sweep
reaches the 1 MB limit; that takes a pool below about 0.06% of BCH (0.6%).
Token shares stay at 3,051 bytes and 2.05% stale at every size. Uneven
hashrate helps: with a Pareto spread (`--miner-hashrate pareto:1.16`, 80% of
the hashrate on 20% of the miners), a million miners make 1,499 payees at 1%
of BCH and 12,624 at 0.1%.

### Block size

`sweep block-size`, the default share (836 payees, a 28,747-byte coinbase)
on mixed and on data-centre (1 Gbit/s) links:

| One-minute block | Stale, mixed links | Stale, data-centre links | BCHN relay each way (Mbit/s) | Full templates (Mbit/s) |
|---|---:|---:|---:|---:|
| 200 kB | 2.84% | 1.55% | 0.048 | 0.546 |
| 3.2 MB | 2.85% | 1.56% | 0.766 | 8.72 |
| 25.6 MB | 3.20% | 1.56% | 6.13 | 69.8 |
| 1 GB | 0.74% (99 of 100 nodes short of BCHN's relay) | 1.57% | 240 | 2,726 |
| 2 GB | 97.92% (every node short) | 1.61% | 479 | 5,451 |

The share chain does not care about block size: its merkle branch grows by
one 32-byte hash for each doubling. What grows is BCHN: the model moves the
miners of a node short of BCHN's relay to other nodes, so at 1 GB the one
node left finds every share, and at 2 GB every node keeps mining and 98% of
shares go stale. P2Pool adds the full `getblocktemplate` JSON it fetches
every 10 seconds and on each block, about three times the block, unless
BCHN runs on the same host; `getblocktemplatelight` and `submitblocklight`
send only the merkle branch.

### Bandwidth

`sweep bandwidth`, upload medians from 5 Mbit/s to 1 Gbit/s (download four
times the upload):

| Median upload | Stale, 3 KB coinbase (tokens) | Stale, 200 KiB (6,010 payees) | Stale, 1 MB (29,402 payees) |
|---:|---:|---:|---:|
| 5 Mbit/s | 3.69% | 43.02% | 78.66% |
| 20 Mbit/s | 2.23% | 15.17% | 44.78% |
| 50 Mbit/s | 2.05% | 7.64% | 24.70% |
| 200 Mbit/s | 1.92% | 3.53% | 8.99% |
| 1 Gbit/s | 1.88% | 2.38% | 3.92% |

A 200 KiB coinbase needs about 50 Mbit/s for stale shares to stay under 8%,
and a 1 MB coinbase needs data-centre links when shares travel whole.

### Latency, share interval and node count

`sweep interval`:

| Share interval | Stale, 3 KB coinbase | Orphaned | Stale, 200 KiB coinbase | Orphaned |
|---|---:|---:|---:|---:|
| 1 s | 16.02% | 0.07% | 45.81% | 9.70% |
| 2 s | 8.86% | 0.01% | 29.82% | 1.11% |
| 5 s | 3.52% | 0.00% | 14.00% | 0.08% |
| 10 s | 2.05% | 0.00% | 7.64% | 0.01% |
| 30 s | 0.91% | 0.00% | 2.87% | 0.00% |

Stale shares grow about linearly with how long a share takes to reach the
other miners compared with the share interval: latency, hops and the
share's own transfer time all add up. With the default share, `sweep
latency` gives 2.05% stale at 10 ms one way, 2.85% at 50, 4.80% at 150 and
9.56% at 400; `sweep nodes` gives 1.84% on 10 nodes, 2.48% on 50, 3.08% on
200 and 3.83% on 1,000 (0.52 s to reach all). Other delays matter as much
as the network: miners that take 0.5 s to get new work
(`--work-switch-seconds 0.5`) make it 6.82%, 100 ms validation
(`--validation-seconds 0.1`) 5.73%, and hashrate spread over nodes as
Pareto 1.16 (`--node-hashrate pareto:1.16`) 2.48%.

### Stratum jobs

Every miner gets a new job for each new share-chain tip (the payouts
change), each template poll and each BCH block: about one job every 4.6
seconds (5). A Stratum V1 job carries the coinbase twice over (hex), a V2
extended job once (binary), a V2 standard job only a merkle root:

| Coinbase | V1 job | V1 per miner | V2 extended per miner | V2 standard per miner |
|---|---:|---:|---:|---:|
| 3,051 bytes (tokens) | 7,416 bytes | 12.9 kbit/s | 6.4 kbit/s | 0.19 kbit/s |
| 28,747 bytes (836 payees) | 60,248 bytes | 104 kbit/s | 52 kbit/s | 0.19 kbit/s |
| 204,663 bytes (6,010 payees) | 421,720 bytes | 731 kbit/s | 365 kbit/s (does not fit its fields) | 0.19 kbit/s |

A node serving 10,000 V1 miners needs 129 Mbit/s for their jobs with the
3 KB coinbase, and 7.3 Gbit/s at 6,010 payees. For a pool of many small
miners this, not the share chain, is the largest load a node carries, and
no change to how shares travel reduces it.

### A found block's coinbase

The coinbase is the one transaction in a found block no other BCH node has,
so a compact block carries it whole. Over four BCH hops at the median 50
Mbit/s, a 1 MB coinbase adds about 0.64 s: 1.06% extra risk of the block
being orphaned on BCH (0.11%). The default 29 KB coinbase adds 0.018 s and
0.03%. A pool with a large coinbase loses about 1.7% of its blocks for every
second the coinbase adds, so sending found blocks to BCH nodes over fast
links, or keeping the coinbase small, matters.

## What this means

1. **Under the hybrid rule the coinbase never makes a block invalid; its
   cost is bandwidth.** The rule raised the 200 KiB share limit (it binds
   from 6,010 payees) to 1,204,800 bytes and the P2P message limit with it
   to carry such shares.
2. **Sending shares without their payouts makes direct payouts cheap for the
   share chain.** It is a change to how shares travel, with its own message
   (`ShareWithoutPayouts`), not a payout rule change; the node does it above
   `RELAY_WITHOUT_PAYOUTS_MIN_BYTES` (50,000 bytes of outputs). See
   [Shares relayed without their payouts](../architecture/share-processing-pipeline.md#shares-relayed-without-their-payouts).
3. **For miners, a large coinbase stays expensive.** The chipnet test with
   real devices (design, section 8.2; audit M2) has to settle what pool-built
   jobs can carry; Stratum V2 standard channels avoid the question.
4. **The window keeps most pools far from the limits**, more so with
   one-minute blocks.
5. **At large blocks, plan for the BCH node.** A one-minute block of 100 MB
   needs the 24 Mbit/s of relay a 1 GB ten-minute block did. Run BCHN on
   links that carry its relay with room to spare, and fetch
   [light templates](../bch-quickstart.md#light-templates)
   (`template_source = "rpc_light"`).
6. **A large coinbase costs BCH blocks**: send found blocks over fast links,
   or keep the coinbase small.

## Scaling propagation instead of capping it (study #85)

The [small-miners design](../sharechain/small-miners-design.md) asked for
weak-share traffic and four ways to scale propagation to be measured
against upstream P2Poolv2's stated limit of about 1,000 miners at 10-second
shares, read here as 1,000 nodes at today's 3.83% stale. The node now has
all but one:

| Idea | In the model | In the node |
|---|---|---|
| 1. Weak shares | 16, 32 or 64 a share, 800 bytes each, gossiped like shares, or also carried whole in every share | Built: [weak shares](../sharechain/small-miners-design.md#3-weak-shares), gossiped; a share lists its own by hash (`ShareWithWeakHashes`, 32 bytes each) |
| 2. Compact coinbase | shares without their payout outputs, 1,301 bytes, at a whole share's validation cost | Built above 50,000 bytes of outputs (`ShareWithoutPayouts`) |
| 3. Relay before full validation | forward once the header checks out (1 ms), validate the body after | Built for shares from peers ([Relay after the header check](../architecture/share-processing-pipeline.md#relay-after-the-header-check)) |
| 4. Fewer hops | 16, 32 or 64 peers instead of 8 | Built: 16 [outgoing peers](../p2p-network.md#outgoing-peers) by default |
| 5. Uncle cap | 6 or 12 uncles a share instead of 3 | Not changed: `MAX_UNCLES` stays 3 |
| 6. Combination of 2, 3 and 4 | 8 to 64 peers | Built, as above |

Each idea ran on 100 and 1,000 nodes at 10, 5, 2 and 1-second shares, the
rest at the defaults; the default coinbase grows from 836 payees at 10
seconds to 1,001 at 2, as the window holds more shares. `interval-limit`
then finds, for each idea, the shortest share interval at which 1,000 nodes
stay within today's stale rate at 10-second shares (to about 6% below 10
seconds and 2% above). 1,000-node runs mine for 8,640 share intervals.

```sh
cargo netsim sweep scaling --threads 10   # every grid: scaling-today, scaling-weak-shares,
                                          # scaling-compact-coinbase, scaling-header-check,
                                          # scaling-peers, scaling-uncle-cap, scaling-combined
cargo netsim interval-limit scaling --nodes 1000 --threads 10
cargo netsim interval-limit scaling-combined --nodes 1000 --seed 2 --threads 4
```

### Where each idea reaches the limit

From `cargo netsim interval-limit scaling --nodes 1000` (seed 1), no
orphans at any limit:

| Scenario | Shortest share interval (s) | Shares per second against 10-second shares | Stale there |
|---|---:|---:|---:|
| today | 10.00 | 1.00 | 3.83% |
| 16, 32 or 64 weak shares a share, gossip only | 10.00 | 1.00 | 3.83% |
| 16 weak shares a share, carried whole | 11.14 | 0.90 | 3.58% |
| 32 weak shares a share, carried whole | 12.69 | 0.79 | 3.71% |
| 64 weak shares a share, carried whole | 15.76 | 0.63 | 3.56% |
| 3 KB coinbase, whole shares | 6.68 | 1.50 | 3.66% |
| 200 KiB coinbase, whole shares | 29.54 | 0.34 | 3.67% |
| 29 KB coinbase, without payouts | 6.31 | 1.59 | 3.82% |
| 200 KiB coinbase, without payouts | 6.68 | 1.50 | 3.67% |
| forwarded after the header check | 9.44 | 1.06 | 3.77% |
| 16 peers | 10.44 | 0.96 | 3.82% |
| 32 peers | 12.97 | 0.77 | 3.65% |
| 64 peers | 15.76 | 0.63 | 3.67% |
| uncle cap 6 or 12 | 10.00 | 1.00 | 3.83% |
| without payouts, header check, 8 peers | 5.30 | 1.89 | 3.82% |
| without payouts, header check, 16 peers | 4.46 | 2.24 | 3.36% |
| without payouts, header check, 32 peers | 4.21 | 2.37 | 3.43% |
| without payouts, header check, 64 peers | 3.98 | 2.51 | 3.64% |

Under seeds 2 and 3, each with its own ceiling (3.93% and 3.84%), the
combination's limits move by up to 19% (8 peers: 5.30, 5.62 and 6.31
seconds; 16 peers: 4.46, 4.46 and 5.01), so a limit within about 20% of 10
seconds is not a measured change. Read that way:

- **Weak shares, gossip only, do not move the limit**; they raise the mean
  upload per node from 0.17 to 0.52 Mbit/s at 64 a share. Carried whole,
  they move it to longer shares, as each share grows by their bytes. The
  node lists a share's weak shares by hash, 32 bytes each, which is close
  to gossip only.
- **The compact coinbase moves it to 6.3 seconds** (5.6) and makes the
  coinbase size irrelevant: with a 200 KiB coinbase, from 29.5 seconds to
  6.7, the same as whole shares with a 3 KB coinbase.
- **Relay before full validation alone barely moves it** (9.4 seconds):
  validation is 5.6 ms of a hop that a 31 KB push and 50 ms of latency
  dominate.
- **More peers alone move it the wrong way**: with whole shares a push to
  more peers takes longer than the hops it saves (1.55 Mbit/s mean upload
  at 64 peers against 0.17 at 8).
- **A larger uncle cap does not move it**: no share ever had a fourth uncle
  to name. Smaller caps orphan more: 1,000 nodes at 1-second shares are
  0.62% orphaned with a cap of 3, 0.66% with 2 and 1.93% with 1. The header
  bytes a syncing node fetches per chain share (305 bytes a header, 32 an
  uncle hash) grow with the share rate whatever the cap: 318 at 10-second
  shares, 455 at 1-second.
- **Today's design gains nothing from shorter shares** (8.91 seconds with
  ten-minute blocks): with one-minute blocks its whole share grows as the
  window holds more shares.
- **The combination moves it to 5.3, 4.5, 4.2 and 4.0 seconds** at 8, 16,
  32 and 64 peers: 1.9 to 2.5 times the share rate of 10-second shares (5.0
  to 3.5 seconds). Once shares are small, more peers help, with little gain
  past 32. At 10-second shares it keeps 1,000 nodes at 2.00% stale on 16
  peers and 2.30% on 8; at 2-second shares at 10.65% on 8 peers and 7.77%
  on 64, against today's 17.88%, with the busiest node under 1 Mbit/s up.

The 800-byte weak share and 1 ms header check are estimates; the model
validates without a queue (the limits study adds one), and its fork choice
is by height and hash, so uncles never change which chain wins.

## Limits study: upstream's practical objections

Upstream's reply to the scaling study: a pool always has an upper bound, and
five of the ideas have practical limits.

1. A share needs enough data to check how it paid out.
2. Relaying early probably won't move the needle much.
3. More connections are limited by link bandwidth: a 100 Mbit/s peer can't
   send a 250 kB share to more than 10 peers in under 250 ms.
4. A dynamic uncle cap could raise latency.
5. A faster share rate builds a fatter DAG and loads every node.

The scaling study guessed validation and let a node validate any number of
shares at once, which hid objection 5. This study measures validation in
the node's own code and queues it.

### What a node pays to check a share

`validation_cost_at_bitcoin_cash_window_sizes` in `pplns_window.rs` (an
ignored test, run by hand in release mode) builds a 120,960 share chain and
times what a node does for each share it receives: walk the PPLNS window
from the share's parent, then build the expected coinbase outputs (the level
draw, the hybrid split and the payout tree) and compare them. A weak share
pays only the build: its walk is cached by parent and paid block. Median of
seven, on one core of a 2026 desktop:

| Miners in the window | Payout level | Share | Paid by | Walk | Build | A share | A weak share |
|---:|---:|---|---|---:|---:|---:|---:|
| 1,000 | 5 | no tokens | 1,000 coinbase outputs | 6.1 ms | 0.3 ms | 6.4 ms | 0.3 ms |
| 10,000 | 5 | no tokens | 10,000 coinbase outputs | 34 ms | 3.8 ms | 38 ms | 3.8 ms |
| 100,000 | 5 | no tokens | 17,883 coinbase outputs (the draw) | 317 ms | 45 ms | 362 ms | 45 ms |
| 100,000 | 0 | no tokens | 29,400 coinbase outputs, 70,600 in the tree | 305 ms | 133 ms | 438 ms | 133 ms |
| 1,000 | 5 | tokens | 79 coinbase outputs, 921 in the tree | 6.1 ms | 1.2 ms | 7.4 ms | 1.2 ms |
| 10,000 | 5 | tokens | 79 coinbase outputs, 9,921 in the tree | 34 ms | 10.5 ms | 45 ms | 10.5 ms |
| 100,000 | 5 | tokens | 79 coinbase outputs, 17,804 in the tree | 317 ms | 59 ms | 376 ms | 59 ms |

The scaling study's guess matched at 1,000 miners, was 3 times too low at
10,000 and 15 times too low at 100,000. At level 5 a 3.125 BCH block pays at
most 17,886 outputs, so 100,000 miners cost a third of what they cost at
level 0, which a share may no longer carry (`MIN_PAYOUT_LEVEL` is 1). The
costs use a ten-minute block's 3.125 BCH and a 120,960 share window: a
one-minute block pays at most about 1,790 level-5 outputs, and only a pool
under 0.015% of BCH has a window that long, so the 100,000-miner rows are an
upper bound until the timing tests are run again.

A window whose shares carry 32 weak shares each
(`validation_cost_with_weak_share_credits`) takes 14 ms to walk for 1,800
shares and 5,100 payees, 174 ms with 50,100 payees, 211 ms for 18,000 shares
and 51,000 payees, and 160 ms at the cap of 120,960 shares with 6,000
payees: about 3.5 microseconds a payee, whatever the credits.

The node validates up to one share per CPU at once (`validation_worker.rs`),
but the walk needs write access to the one shared window
(`Arc<RwLock<PplnsWindow>>`), so walks run one at a time; the model's
`ValidationQueue::Serial` does the same. After the first weak share on a
parent and template, the node's `WeakPayoutCache` leaves only the coinbase
rebuild and merkle check: 0.08 ms with 1,000 miners, 1.0 ms with 10,000 and
1.4 ms with 100,000 (`validation_cost_of_a_cached_weak_share`).

`netsim sweep limits` (or one of `limits-coinbase`, `limits-relay-early`,
`limits-fanout`, `limits-uncles`, `limits-period`, `limits-network`) runs
each objection on 1,000 nodes at the measured level-5 costs, one validation
at a time, on 50 Mbit/s median upload and 50 ms median latency, mining 8,640
share intervals (1,080 for the period and network sweeps, which need
`--threads 4` to fit a desktop's memory).

### 1. Enough data to check the payout

Every node already rebuilds a share's payouts to check it, so a share sent
without them leaves out nothing a peer can't rebuild, and only the node that
finds a block needs the whole coinbase. A token share is already small:
4,109 bytes on the wire whatever it pays. Stale rate, `limits-coinbase`:

| Miners in the window | Share interval | Whole | Without payouts | Token share |
|---:|---:|---:|---:|---:|
| 1,000 | 10 s | 4.15% | 2.52% | 2.69% |
| 10,000 | 10 s | 16.46% | 3.87% | 4.24% |
| 100,000 | 10 s | 32.56% | 16.37% | 17.03% |
| 10,000 | 2 s | 50.02% | 17.24% | 18.62% |

At 100,000 miners validation itself (362 ms at each hop) keeps every form at
16% or more.

### 2. Relaying before full validation

Upstream is right: forwarding after the header check takes 0.2 to 2.1
points off the stale rate at 1,000 and 10,000 miners. It matters when
validation is slow: 32.56% to 24.68% at 100,000 miners.

### 3. More connections against upload bandwidth

Upstream is right for whole shares: at 10,000 miners (a 350 kB share) more
peers raise the stale rate, from 31% at 8 peers to 82% at 64 on a 20 Mbit/s
upload, and from 11% to 28% on 100 Mbit/s. Only with shares sent without
payouts (1.3 kB) do more peers help, from 3.9% to 2.9% on 20 Mbit/s and 3.8%
to 2.5% on 100.

### 4. The uncle cap

Caps of 6 and 12 change neither the stale rate nor propagation: a chain
share names 0.2 uncles at 10-second shares and 1.38 to 1.41 at 1-second. The
cap only decides which stale shares are paid; it adds no latency, since
every uncle was validated when it arrived and naming one costs 32 bytes. Not
worth changing on this evidence.

### 5. Share period and each node's load

`sweep limits-period`: how much of the run the busiest node's validation
worker was busy, with 32 weak shares a share mined on a template a node
("per node") or on one template for the whole pool ("one"). Every weak
share on a template of its own is within a few points of "per node".

| Scenario | 10 s | 5 s | 2 s | 1 s |
|---|---:|---:|---:|---:|
| 1,000 miners, per node | 0.15% | 0.30% | 0.74% | 1.49% |
| 10,000 miners, no weak shares | 0.40% | 0.79% | 1.95% | 3.97% |
| 10,000 miners, per node | 1.58% | 3.15% | 7.84% | 15.65% |
| 10,000 miners, nodes 5x slower, per node | 7.89% | 15.73% | 39.14% | 78.06% |
| 100,000 miners, no weak shares | 3.80% | 7.54% | 18.64% | 37.89% |
| 100,000 miners, per node | 17.79% | 35.47% | 87.72% | 165.23% |
| 100,000 miners, one | 4.62% | 9.18% | 24.40% | 53.60% |
| 100,000 miners, token shares, per node | 22.48% | 44.84% | 107.65% | 207.26% |
| 100,000 miners, level 0, per node | 45.96% | 91.44% | 210.37% | 407.87% |

Above 100% the queue grows without end and nearly every share goes stale;
below it weak shares barely change the stale rate. One template for the
whole pool halves the load at 10,000 miners, and at 100,000 no share period
overflows (43.93% at 1-second shares on token shares, 91.06% at level 0).
Without weak shares these runs (1,080 share intervals, so noisier) are 4.9%
stale at 10-second shares and 32% at 1-second with 1,000 miners, 17% to 68%
with 10,000 and 34% to 84% with 100,000.

So a faster share period is bounded by propagation for small pools and by
validation, mostly of weak shares mined on different templates, for pools
paying tens of thousands of miners. At the 10-second period and the default
level, 100,000 miners fit with room.

### 6. A bigger network

`sweep limits-network`, 10,000 miners: from 1,000 to 3,000 and 10,000 nodes
the stale rate of shares sent without payouts goes from 5.00% to 5.62% and
5.89%, and of whole shares from 17.48% to 19.54% and 21.14%. Hops grow with
the logarithm of the node count, so the share's size and its validation at
each hop matter far more than the size of the network. Weak shares do not
change the stale rate at 1,000 or 3,000 nodes; they add 0.2 Mbit/s of
upload a node.

The study did not run real nodes at these sizes (the costs are measured,
the network is modelled), did not measure deep forks, header sync of a wide
DAG or uncles deeper than 3, and takes a small node as 5 times slower than
the desktop without measuring one.

## Code

The simulator is `p2poolv2_netsim/src/`: `constants.rs` (constants and their
sources), `sizes.rs` (coinbase layouts, share sizes, limits), `payees.rs`,
`topology.rs`, `engine.rs` (the event simulation), `traffic.rs` (BCHN,
template and Stratum traffic), `scenario.rs`, `sweep.rs`, `report.rs`,
`scaling.rs` (the scaling study), `limits.rs` (the limits study) and
`bin/netsim.rs`. The timing tests are in
`p2poolv2_lib/src/accounting/payout/sharechain_pplns/pplns_window.rs` and
`pplns_window/weak_walk_cost.rs`.

The crate is new code for the Bitcoin Cash fork, under AGPL-3.0-or-later
(decision 13). Upstream's ns-3 repository has no licence file, so its idea is
credited here and in the crate's README, and none of its code is used.
