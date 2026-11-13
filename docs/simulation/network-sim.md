# Network simulation: what limits P2Pool on Bitcoin Cash

`p2poolv2_netsim` is a discrete-event simulator of how shares travel through
a Bitcoin Cash P2Poolv2 network. It answers one question with numbers: as
the pool grows, what limits it first, the share chain, the coinbase or
bandwidth? It also measures what the
[hybrid payout design](../architecture/bch-hybrid-payouts.md) asks the
network sim for in its section 8.1: propagation delay and stale rate against
coinbase size from 1 KB to 1 MB.

The hybrid payout rule is the Bitcoin Cash payout rule on PR #33 and on
chipnet. The earlier rule it replaced, at most 11 payouts and a weighted
draw for the tail (decision 14, `payout-cap11`), appears below only as a
historical comparison (`cap-11`), and so does the 200 KiB share body limit
that came with it.

## The short answer

- **The coinbase limits P2Pool first, as a bandwidth cost.** Every payee
  paid directly puts 34 bytes into every share and into every Stratum job.
  Under the hybrid payout rule a share without tokens pays payees
  directly up to BCH's 1,000,000-byte transaction limit (29,402 P2PKH
  payees) and the rest through a payout tree, so BCH never rejects a block
  for its coinbase. But with whole shares on the wire, as nodes send them,
  stale shares climb with the direct payees on the assumed mix of
  home and data-centre links: 3% at 1,000 payees, 8% at 6,010, 11% at
  10,000 and 25% at 29,402 (a 1 MB coinbase; 0.8% orphaned). With 1 MB shares
  only data-centre links keep it near 4%.
- **Sending shares without their payouts removes that cost.** Every node
  rebuilds the payouts to validate a share anyway, as the design points
  out. Without them a share is about 1.4 KB whatever the payee count, and
  stale shares at 29,402 payees fall from 25% to 2.7%.
- **For miners the coinbase stays a per-job cost.** A Stratum V1 job carries
  the coinbase in hex about every 5 seconds: 116 kbit/s per miner at 1,000
  payees and 681 kbit/s at 6,010. A Stratum V2 extended job cannot carry a
  coinbase of more than 1,922 payees at all (65,535-byte fields), and the
  device evidence from other pools puts stock firmware between 12 and 461
  payees. Only Stratum V2 standard channels carry no coinbase.
- **The PPLNS window bounds the payees.** On BCH it holds three BCH blocks
  of the pool's work, about `180 / f` shares for a pool with fraction `f`
  of BCH's hashrate. A pool above 3% of BCH has fewer than about 6,000
  payees whatever its miner count; only below about 0.6% can a crowd of
  small miners reach the 1 MB limit and the tree.
- **With merge-mined tokens the 3,072-byte threshold keeps the coinbase at
  3,051 bytes** for any number of payees (79 direct, the rest in the tree),
  and stale shares at about 2%, like the old 11-payout cap.
- **The share chain itself is not the limit at 10-second shares.** With
  small shares 2 to 3% of shares go stale, nearly all named as uncles (paid
  9/10), and a share reaches 90% of 100 nodes in 0.2 to 0.3 s. It grows
  with latency (10% at 400 ms one way), node count (4.2% at 1,000 nodes),
  slow job switching (7% if miners take 0.5 s to get new work) and faster
  shares (16% at 1-second shares).
- **Raw bandwidth binds through BCHN and templates at multi-gigabyte
  blocks, not through the share chain.** A node's BCHN needs about 24
  Mbit/s each way at full 1 GB blocks and 48 Mbit/s at 2 GB just to relay
  transactions; on the assumed link mix 14 of 100 nodes cannot carry that
  at 1 GB and 52 at 2 GB. On data-centre links block size leaves the stale
  rate at 1.6% from 2 MB to 2 GB. Fetching a full `getblocktemplate` every
  10 seconds costs 76 Mbit/s between node and BCHN at full 32 MB blocks and
  4.75 Gbit/s at 2 GB; BCHN's `getblocktemplatelight` avoids it.
- **Upstream's limit of about 1,000 miners at 10-second shares (4.23% stale
  on 1,000 nodes) moves only with smaller shares.** Sending shares without
  their payouts, forwarding them after the header check and giving each
  node 8 to 64 peers keeps 1,000 nodes within 4.23% stale down to 5.0 to 3.5
  second shares. Each alone does less: shares without payouts reach 5.6
  seconds, header-check forwarding and larger uncle caps change nothing
  measurable, and more peers with whole shares make it worse. Weak shares
  gossiped beside the shares cost upload but no stale shares; carried whole
  in every share they push the limit out to 11 to 16 seconds (section
  "Scaling propagation instead of capping it").

The rest of this page says how the model works, what it assumes and what
each sweep showed.

## Measured and estimated

Nothing here is measured on a running pool.

- **Counted exactly:** share, coinbase and message sizes, from this
  repository's encoders and the hybrid rule's byte counts (P2PKH payees, the
  donation key among them, every other coinbase field at its largest), and
  the payee counts at which each limit is reached.
- **Simulated:** stale, uncle and orphan rates, propagation times and
  P2Pool's bytes per node, under assumed link distributions. They are as
  good as those assumptions. The bandwidth assumptions audit plans the
  real-world check (`bch-btc-assumptions-audit.md`, M2: chipnet nodes with
  padded shares).
- **Estimated from formulas:** BCHN's relay, compact block and template
  traffic, Stratum job traffic, the size of a share sent without its
  payouts (its format is not designed yet) and the extra BCH orphan risk of
  a large coinbase. They assume full blocks of 226-byte transfers.

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
cargo netsim limits                      # payee counts at each limit
```

`cargo netsim` is an alias for
`cargo run --release -p p2poolv2_netsim --bin netsim --`. Every run is a pure
function of its inputs and its seed (`--seed`, default 1), with its own
seeded xoshiro256** generator, so the tables below come out the same on any
machine. The whole `sweep all` took under a minute on the 32-thread machine
it was run on.

## The model

It ports the idea of upstream's ns-3 simulation,
[p2poolv2/Ns3_simulation](https://github.com/p2poolv2/Ns3_simulation)
(`ns-3.44/scratch/p2pool_v2_with_share.cc`): nodes on a small-world network
mining and gossiping shares, counting uncles and orphans. Upstream's
simulation has 15 nodes with 8 peers each on 100 Mbit/s links with 10 us
latency, a normally distributed mining interval per node and string-encoded
shares. This one is Rust, has no ns-3, copies no upstream code, and changes
the model for Bitcoin Cash and for scale:

- **Shares** are found pool-wide as a Poisson process (one every 10 s by
  default, `IDEAL_BLOCK_TIME`), each by a node picked by hashrate.
- **The graph** is upstream's small world: a ring where each node links to
  its 4 nearest neighbours on each side, then each ring link moved to a
  random node with probability 0.5 (Watts and Strogatz).
- **Relay** follows `send_share_block_broadcast` in `p2poolv2_lib`: a node
  that has validated a share pushes the whole `ShareBlock` to every peer not
  known to have it, and each peer answers with an `Ack`. A node knows a peer
  has a share once it sent it to or received it from that peer, so most
  nodes receive several copies (about 6.9 copies per share per node at 8
  peers). With `--relay without-payouts` the share travels without the
  payout outputs, as the design suggests; the receiver still spends the
  time to rebuild and check them.
- **Acceptance:** a share waits at a node until its parent and its uncles
  are accepted there, then takes the validation time (5 ms plus 20 ms per MB
  of the whole share, an estimate) before it is accepted and relayed.
- **Mining:** a node mines on its best share, the highest, with the lowest
  hash breaking a tie, as the mining base search does. Its miners switch to
  the new work after a job delay (50 ms, an estimate). A new share names up
  to three uncles picked as `Store::find_uncles` picks them: children of the
  base's parent, grandparent and great-grandparent that are not on its chain
  and not already named.
- **Outcome:** after mining stops, propagation runs until every share has
  reached every node. The best chain decides: a share on it is fine, a share
  named by it as an uncle is stale but paid 9/10, any other share is
  orphaned and paid nothing. Shares from the last 10 share intervals are not
  counted, since later shares could still name them.
- **Links:** each node has an upload and a download rate drawn from a
  distribution, and each link a one-way latency. A push to `k` peers shares
  the upload, so each copy needs `k` times the share's bytes over the upload
  rate, after whatever the node was already sending; it also needs the
  share's bytes over the receiver's download and the latency. A fluid
  approximation: no TCP slow start and no loss.
- **BCHN on the same link** (the default): BCHN's transaction relay is taken
  from each link first, and every BCH block (one every 10 minutes on
  average) takes the link for its compact block, once down and once up, so
  shares wait behind it. A node whose link cannot carry BCHN's relay at all
  cannot keep up with BCH: it still relays shares, but its miners are moved
  to the other nodes.
- **Coinbase layouts:**
  - `no-tokens`: the hybrid payout rule for a share without token
    commitments. Payees are paid
    directly while their outputs fit the budget `1,000,000 - S`, where
    `S` = 367 bytes is everything else at its worst case; the straddler and
    the rest go to one P2SH32 payout tree output. Each share's donation and
    fee come off its own weight (`cuts-per-share`), so the donation key and
    fee addresses are payees and the coinbase has no output of its own for
    them.
  - `tokens-3072`: the same rule with the coinbase limit `T` = 3,072 bytes,
    for a share that carries token commitments, whether the job is
    pool-built or declared.
  - `cap-11`: the earlier share-chain rule (decision 14, `payout-cap11`),
    at most 11 payouts and a weighted draw for the tail, which the owner
    rejected and the hybrid rule replaced on PR #33 and on chipnet; kept
    only as a historical comparison.
  - `uncapped`: every payee direct with no limit, upstream's Bitcoin rule;
    for comparison.

  The hybrid rule pays the smallest payees directly and the largest through
  the tree. With every payee on P2PKH that changes who is in the tree, not
  the sizes, so the model does not need the order.
- **Payees** are the miners expected to hold a share in the window, for a
  window of `min(120,960, 3 * 60 / f)` shares (`BCH_PPLNS_WINDOW_BLOCKS` = 3
  at 10-second shares), plus the donation key that every share's default
  donation pays (under `cap-11` the donation was an output of its own), or
  a count given directly with `--payees`. The model's nodes take no
  operator fee; each fee address would add one payee.

It leaves out: share difficulty changes (every share has the same work),
header-first sync of nodes that fall far behind, share transactions (they
can be added with `--extra-share-bytes`), node churn, attacks, and the BCH
network itself beyond its traffic.

## Inputs

| Input | Option | Default |
|---|---|---|
| Nodes, peers per node, rewiring | `--nodes`, `--peers`, `--rewire` | 100, 8, 0.5 |
| Hashrate over nodes | `--node-hashrate` | equal |
| Miners and their hashrate | `--miners`, `--miner-hashrate` | 1,000, equal |
| Payees, instead of the expected count | `--payees` | from miners and window |
| Pool's fraction of BCH's hashrate | `--pool-fraction` | 0.01 (window of 18,000 shares) |
| Share interval | `--share-interval` | 10 s |
| BCH block size, average transaction | `--block-bytes`, `--transaction-bytes` | 32 MB (mainnet's ABLA floor), 226 bytes |
| Payout rule, tokens, threshold | `--payout`, `--tokens`, `--token-threshold` | hybrid, no tokens, 3,072 bytes |
| How shares travel | `--relay` | `full` (as nodes send them) |
| Upload, download (Mbit/s) | `--upload-mbps`, `--download-mbps` | `lognormal:50:0.8`, `lognormal:200:0.8` |
| One-way latency (ms) | `--latency-ms` | `lognormal:50:0.5` |
| Validation, job switch | `--validation-seconds`, `--validation-seconds-per-megabyte`, `--work-switch-seconds` | 5 ms + 20 ms/MB, 50 ms |
| BCHN on its own link | `--separate-bchn-link` | shares the node's link |
| BCHN peers | `--bch-peers` | 8 (`MAX_OUTBOUND_CONNECTIONS`) |
| Stratum protocol | `--stratum` | `v1` |
| Duration, seed | `--duration`, `--seed` | one day (8,640 shares), 1 |
| Uncle cap (studied) | `--max-uncles` | 3 (`MAX_UNCLES`) |
| When a share is forwarded (studied, not built) | `--forward`, `--header-check-seconds` | `after-validation`; 1 ms for `after-header-check` |
| Weak shares (studied, not built) | `--weak-shares`, `--weak-share-bytes`, `--weak-share-carriage` | none; 800 bytes; `gossip-only` |

The link distributions are assumptions: a log-normal mix whose median node
has 50 Mbit/s up and 200 Mbit/s down, with a long tail both ways, standing
for home links and servers together. A distribution is written `50`
(fixed), `uniform:10:100` or `lognormal:<median>:<sigma>`.

## Limits on the coinbase

From `cargo netsim limits`: the most P2PKH payees a coinbase can pay
directly under each limit, with every other field at its largest.

| Limit | Whose | Direct payees at most |
|---|---|---:|
| 200 KiB share body (`BLOCK_TXS_SIZE_LIMIT`), before the hybrid rule | P2Pool share-chain rule, historical on Bitcoin Cash | 6,010 |
| 1,204,800-byte share body (`BCH_SHARE_BODY_SIZE_LIMIT`, design section 8.1) | P2Pool share-chain rule, hybrid | 35,422 |
| 1.25 MiB P2P message (`MAX_P2P_MESSAGE_SIZE`, 1 MiB before the hybrid rule) | this node's implementation | 38,525 |
| 1,000,000-byte transaction (`MAX_TX_SIZE`, BCHN `src/consensus/tx_check.cpp`) | BCH consensus | 29,402 |
| 65,535-byte coinbase field of a Stratum V2 extended or declared job | Stratum V2 | 1,922 |
| 10,000-byte VM stack element, for a token claim hashing the coinbase (CHIP-2021-05) | BCH consensus | 284 |
| 784-byte OCEAN block on stock Antminer firmware (upstream #668) | evidence from another pool | 13 |
| DATUM gateway's per-device coinbase sizes: 755 bytes (older stock S19), 2,250 (stock S21), 6,500 (Whatsminer, Bitaxe), 16,000 (ePIC, vnish) | evidence from another pool's software | 12, 56, 181, 461 |

Under the hybrid rule a share without tokens pays up to 29,402 payees directly
and, past that, 29,400 directly beside the tree output, a 999,967-byte
coinbase. A share with tokens pays up to 80 directly and, past that, 79 beside
the tree, a 3,051-byte coinbase, well inside one VM stack element. The old
11-payout cap gives a 782-byte coinbase. ABLA sets the block size limit
(32 MB floor on mainnet, 256 MB to start on scalenet, 2 GB guard in BCHN) and
never raises the transaction limit.

## Results

All tables are `cargo netsim sweep <name>` with the defaults above, seed 1
and a day of mining. Columns: payees per coinbase; coinbase bytes and the
bytes of a share on the wire; stale shares (uncles and orphans) and orphans,
as a share of all shares; mean seconds until 90% and all of the nodes had
each share ("never" when some node had not got some share a day after
mining stopped); P2Pool's share-chain upload per node per second of mining,
mean and busiest node; BCHN's transaction relay each way; full template
JSON from BCHN; Stratum V1 job bytes per miner; and the rule and protocol
limits broken (device evidence is left out of that column). The
"pre-hybrid 200 KiB share body" there is the limit before the hybrid rule,
listed for comparison; the hybrid rule's own share body limit is 1,204,800
bytes.

The tables were regenerated on 2026-11-12 from commit 5b67ca11. The earlier
tables came from before commit 7e1c2707 ("Pick uncles from the mining branch
only", 2026-10-30), which made the model pick uncles as upstream's
`find_uncles` does: a node skips only shares already named as uncles by its
own branch, not by any share it holds. A stale share named on a losing branch
can now still be named on the winning one, so some shares that were orphans
are now uncles. Orphan rates fell, most at short share intervals and on slow
links (1-second shares with a 200 KiB coinbase: 11.07% to 9.79%); stale rates
are the same except in three rows, which moved by at most 0.06 points.

### Payees and the coinbase layout

| Scenario | Payees | Coinbase bytes | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | BCHN relay (Mbit/s) | Templates (Mbit/s) | Stratum per miner (kbit/s) | Limits broken |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| no-tokens, 10 payees | 10 | 661 | 1,807 | 1.98% | 0.00% | 0.17 | 0.20 | 0.011 / 0.017 | 0.766 | 76.0 | 4.49 | none |
| no-tokens, 100 payees | 100 | 3,721 | 4,947 | 2.08% | 0.00% | 0.19 | 0.22 | 0.028 / 0.044 | 0.766 | 76.0 | 14.7 | none |
| no-tokens, 1,000 payees | 1,000 | 34,323 | 36,389 | 3.08% | 0.01% | 0.33 | 0.38 | 0.202 / 0.320 | 0.766 | 76.0 | 116 | none |
| no-tokens, 6,010 payees | 6,010 | 204,663 | 211,369 | 7.81% | 0.01% | 0.98 | 1.12 | 1.18 / 1.86 | 0.766 | 76.0 | 681 | SV2 extended 64 KiB field |
| no-tokens, 10,000 payees | 10,000 | 340,323 | 350,749 | 10.92% | 0.01% | 1.51 | 1.72 | 1.96 / 3.08 | 0.766 | 76.0 | 1,131 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, 29,402 payees | 29,402 | 999,991 | 1,028,497 | 24.51% | 0.84% | 4.47 | 5.03 | 5.76 / 9.02 | 0.766 | 76.0 | 3,317 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, 100,000 payees | 100,000 | 999,967 | 1,028,473 | 24.51% | 0.84% | 4.47 | 5.03 | 5.76 / 9.02 | 0.766 | 76.0 | 3,317 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, 1,000,000 payees | 1,000,000 | 999,967 | 1,028,473 | 24.51% | 0.84% | 4.47 | 5.03 | 5.76 / 9.02 | 0.766 | 76.0 | 3,317 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| tokens-3072, 100 payees | 100 | 3,051 | 4,237 | 2.07% | 0.00% | 0.19 | 0.22 | 0.024 / 0.038 | 0.766 | 76.0 | 12.5 | none |
| tokens-3072, 10,000 payees | 10,000 | 3,051 | 4,237 | 2.07% | 0.00% | 0.19 | 0.22 | 0.024 / 0.038 | 0.766 | 76.0 | 12.5 | none |
| tokens-3072, 1,000,000 payees | 1,000,000 | 3,051 | 4,237 | 2.07% | 0.00% | 0.19 | 0.22 | 0.024 / 0.038 | 0.766 | 76.0 | 12.5 | none |
| cap-11, 1,000,000 payees | 1,000,000 | 782 | 1,928 | 2.00% | 0.00% | 0.17 | 0.20 | 0.011 / 0.018 | 0.766 | 76.0 | 4.95 | none |
| uncapped, 100,000 payees | 100,000 | 3,400,325 | 3,494,591 | 51.69% | 15.58% | never | never | 19.5 / 30.7 | 0.766 | 76.0 | 11,275 | BCH 1 MB transaction, hybrid 1,204,800-byte share body, 1.25 MiB P2P message, pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |

Each payee paid directly adds 34 bytes to every share, and pushing whole
shares to every peer multiplies that: at 6,010 payees the busiest node
uploads 1.9 Mbit/s and 7.8% of shares go stale; at 29,402 (a 1 MB coinbase)
a quarter do and 0.8% are orphaned. Past 29,402 the hybrid rule's tree keeps the
coinbase under 1 MB, so 100,000 and a million payees cost the same as
29,402. Without that limit (`uncapped`) a 100,000-payee coinbase is 3.4 MB:
BCH rejects the block, and on these links a share takes longer to cross the
network than the share interval, so 52% go stale. With tokens, or under
the old cap, the coinbase is fixed and nothing grows with the payees.

### Sending shares without their payouts

The same coinbases, sent whole and sent without the payout outputs every
peer rebuilds:

| Scenario | Payees | Coinbase bytes | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | BCHN relay (Mbit/s) | Templates (Mbit/s) | Stratum per miner (kbit/s) | Limits broken |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| no-tokens, 1,000 payees, full relay | 1,000 | 34,323 | 36,389 | 3.08% | 0.01% | 0.33 | 0.38 | 0.202 / 0.320 | 0.766 | 76.0 | 116 | none |
| no-tokens, 6,010 payees, full relay | 6,010 | 204,663 | 211,369 | 7.81% | 0.01% | 0.98 | 1.12 | 1.18 / 1.86 | 0.766 | 76.0 | 681 | SV2 extended 64 KiB field |
| no-tokens, 29,402 payees, full relay | 29,402 | 999,991 | 1,028,497 | 24.51% | 0.84% | 4.47 | 5.03 | 5.76 / 9.02 | 0.766 | 76.0 | 3,317 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, 1,000 payees, without-payouts relay | 1,000 | 34,323 | 1,429 | 1.98% | 0.00% | 0.17 | 0.20 | 8.4e-3 / 0.013 | 0.766 | 76.0 | 116 | none |
| no-tokens, 6,010 payees, without-payouts relay | 6,010 | 204,663 | 1,429 | 2.10% | 0.00% | 0.19 | 0.22 | 8.3e-3 / 0.013 | 0.766 | 76.0 | 681 | SV2 extended 64 KiB field |
| no-tokens, 29,402 payees, without-payouts relay | 29,402 | 999,991 | 1,429 | 2.65% | 0.00% | 0.26 | 0.30 | 7.8e-3 / 0.013 | 0.766 | 76.0 | 3,317 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |

Without the payouts a share is about 1.4 KB at any payee count, and stale
shares stay between 2.0% and 2.7%: the share chain no longer cares how many
miners are paid directly. The remaining growth is validation: rebuilding a
1 MB coinbase is taken to cost 20 ms. The coinbase itself still reaches the
miners in their Stratum jobs (below) and BCH in the found block.

### Pool size: the window bounds the payees

A million equally sized miners, for pools from 0.1% to 30% of BCH:

| Scenario | Payees | Coinbase bytes | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | BCHN relay (Mbit/s) | Templates (Mbit/s) | Stratum per miner (kbit/s) | Limits broken |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| no-tokens, pool 0.1% of BCH | 113,932 | 999,967 | 1,028,473 | 24.51% | 0.84% | 4.47 | 5.03 | 5.76 / 9.02 | 0.766 | 76.0 | 3,317 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, pool 0.3% of BCH | 58,236 | 999,967 | 1,028,473 | 24.51% | 0.84% | 4.47 | 5.03 | 5.76 / 9.02 | 0.766 | 76.0 | 3,317 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, pool 1% of BCH | 17,840 | 606,883 | 624,629 | 16.89% | 0.21% | 2.58 | 2.93 | 3.49 / 5.50 | 0.766 | 76.0 | 2,014 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, pool 3% of BCH | 5,983 | 203,745 | 210,451 | 7.77% | 0.01% | 0.98 | 1.12 | 1.18 / 1.85 | 0.766 | 76.0 | 678 | SV2 extended 64 KiB field |
| no-tokens, pool 10% of BCH | 1,799 | 61,489 | 64,275 | 3.77% | 0.00% | 0.44 | 0.50 | 0.357 / 0.562 | 0.766 | 76.0 | 206 | none |
| no-tokens, pool 30% of BCH | 601 | 20,757 | 22,423 | 2.65% | 0.01% | 0.27 | 0.31 | 0.125 / 0.197 | 0.766 | 76.0 | 71.1 | none |
| tokens-3072, pool 0.1% of BCH | 113,932 | 3,051 | 4,237 | 2.07% | 0.00% | 0.19 | 0.22 | 0.024 / 0.038 | 0.766 | 76.0 | 12.5 | none |
| tokens-3072, pool 0.3% of BCH | 58,236 | 3,051 | 4,237 | 2.07% | 0.00% | 0.19 | 0.22 | 0.024 / 0.038 | 0.766 | 76.0 | 12.5 | none |
| tokens-3072, pool 1% of BCH | 17,840 | 3,051 | 4,237 | 2.07% | 0.00% | 0.19 | 0.22 | 0.024 / 0.038 | 0.766 | 76.0 | 12.5 | none |
| tokens-3072, pool 3% of BCH | 5,983 | 3,051 | 4,237 | 2.07% | 0.00% | 0.19 | 0.22 | 0.024 / 0.038 | 0.766 | 76.0 | 12.5 | none |
| tokens-3072, pool 10% of BCH | 1,799 | 3,051 | 4,237 | 2.07% | 0.00% | 0.19 | 0.22 | 0.024 / 0.038 | 0.766 | 76.0 | 12.5 | none |
| tokens-3072, pool 30% of BCH | 601 | 3,051 | 4,237 | 2.07% | 0.00% | 0.19 | 0.22 | 0.024 / 0.038 | 0.766 | 76.0 | 12.5 | none |

The window is three BCH blocks of the pool's work, so a growing pool pays
fewer, larger miners per block: at 10% of BCH the window is 1,800 shares
and a million miners fill at most 1,798 payee slots, 1,799 payees with the
donation key (a 61 KB coinbase). At 3% the coinbase has 5,983 payees
(204 KB), at 1% 17,840 (607 KB), and at 0.3% and below it reaches the 1 MB
limit, with the rest in the tree. Uneven hashrate helps: with a Pareto
spread (shape 1.16, 80% of the hashrate on 20% of the miners;
`--miner-hashrate pareto:1.16`), a million miners make 12,624 payees at 1%
of BCH (430 KB) and 66,700 at 0.1% (29,400 direct, 37,300 in the tree).

Whether a million miners is realistic depends on BCH's hashrate. As an
illustration only: if BCH had 3 EH/s, a million 1 TH/s miners would be a
third of it, where the window is 540 shares.

### Block size

The default share (a thousand miners and the donation key, 1,001 payees, no
tokens) on mixed and on data-centre (1 Gbit/s) links, from chipnet's 2 MB
to BCHN's 2 GB guard:

| Scenario | Payees | Coinbase bytes | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | BCHN relay (Mbit/s) | Templates (Mbit/s) | Stratum per miner (kbit/s) | Limits broken |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| 2 MB blocks, mixed links | 1,001 | 34,357 | 36,295 | 3.03% | 0.01% | 0.32 | 0.37 | 0.202 / 0.319 | 0.048 | 4.75 | 116 | none |
| 32 MB blocks, mixed links | 1,001 | 34,357 | 36,423 | 3.09% | 0.01% | 0.33 | 0.38 | 0.202 / 0.320 | 0.766 | 76.0 | 116 | none |
| 256 MB blocks, mixed links | 1,001 | 34,357 | 36,519 | 3.40% | 0.01% | 0.38 | 0.43 | 0.203 / 0.322 | 6.13 | 608 | 117 | none |
| 1 GB blocks, mixed links | 1,001 | 34,357 | 36,583 | 5.77% | 0.94% | 1.62 | 10.86 | 0.204 / 0.321 | 24.0 | 2,375 | 117 | BCHN relay over the link on 14 of 100 nodes |
| 2 GB blocks, mixed links | 1,001 | 34,357 | 36,615 | 8.09% | 1.77% | 3.53 | 173.99 | 0.203 / 0.324 | 47.9 | 4,750 | 117 | BCHN relay over the link on 52 of 100 nodes |
| 2 MB blocks, data-centre links | 1,001 | 34,357 | 36,295 | 1.62% | 0.00% | 0.12 | 0.14 | 0.197 / 0.317 | 0.048 | 4.75 | 116 | none |
| 32 MB blocks, data-centre links | 1,001 | 34,357 | 36,423 | 1.62% | 0.00% | 0.12 | 0.14 | 0.198 / 0.319 | 0.766 | 76.0 | 116 | none |
| 256 MB blocks, data-centre links | 1,001 | 34,357 | 36,519 | 1.62% | 0.00% | 0.12 | 0.14 | 0.198 / 0.319 | 6.13 | 608 | 117 | none |
| 1 GB blocks, data-centre links | 1,001 | 34,357 | 36,583 | 1.62% | 0.00% | 0.12 | 0.14 | 0.199 / 0.320 | 24.0 | 2,375 | 117 | none |
| 2 GB blocks, data-centre links | 1,001 | 34,357 | 36,615 | 1.62% | 0.00% | 0.12 | 0.14 | 0.199 / 0.320 | 47.9 | 4,750 | 117 | none |

The share chain does not care about block size: its merkle branch grows by
one 32-byte hash for each doubling, and on data-centre links the stale rate
is 1.6% at every size. What grows is BCHN: to relay a full 1 GB block's
transactions a node needs about 24 Mbit/s each way, 48 Mbit/s at 2 GB, and a
2 GB compact block is 53 MB. On the mixed links 14 of 100 nodes (1 GB) and
52 of 100 (2 GB) cannot carry that; their miners are moved to the others and
the shares that cross them wait. That is a cost of running a BCH full node
at that size, P2Pool or not. P2Pool adds the block templates it fetches from
BCHN: a full `getblocktemplate` every 10 seconds and on each block is about
three times the block in JSON, 76 Mbit/s at full 32 MB blocks and 4.75
Gbit/s at 2 GB. It stays inside one machine when BCHN runs on the same host,
but not when BCHN is reached over a network, and BCHN has to build that JSON
each time. BCHN's `getblocktemplatelight` and `submitblocklight` send only
the merkle branch.

### Bandwidth

Upload medians from 5 Mbit/s to 1 Gbit/s (download four times the upload),
for a 3 KB coinbase (tokens), a 200 KiB one (6,010 payees) and a 1 MB one
(29,402):

| Scenario | Payees | Coinbase bytes | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | BCHN relay (Mbit/s) | Templates (Mbit/s) | Stratum per miner (kbit/s) | Limits broken |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| tokens-3072, 1,000 payees, upload 5 Mbit/s | 1,000 | 3,051 | 4,237 | 3.97% | 0.02% | 0.44 | 0.50 | 0.024 / 0.038 | 0.766 | 76.0 | 12.5 | none |
| tokens-3072, 1,000 payees, upload 20 Mbit/s | 1,000 | 3,051 | 4,237 | 2.34% | 0.00% | 0.22 | 0.26 | 0.024 / 0.038 | 0.766 | 76.0 | 12.5 | none |
| tokens-3072, 1,000 payees, upload 50 Mbit/s | 1,000 | 3,051 | 4,237 | 2.07% | 0.00% | 0.19 | 0.22 | 0.024 / 0.038 | 0.766 | 76.0 | 12.5 | none |
| tokens-3072, 1,000 payees, upload 200 Mbit/s | 1,000 | 3,051 | 4,237 | 1.93% | 0.00% | 0.17 | 0.20 | 0.024 / 0.038 | 0.766 | 76.0 | 12.5 | none |
| tokens-3072, 1,000 payees, upload 1000 Mbit/s | 1,000 | 3,051 | 4,237 | 1.87% | 0.00% | 0.16 | 0.19 | 0.024 / 0.038 | 0.766 | 76.0 | 12.5 | none |
| no-tokens, 6,010 payees, upload 5 Mbit/s | 6,010 | 204,663 | 211,369 | 43.51% | 12.19% | never | never | 1.16 / 1.83 | 0.766 | 76.0 | 681 | SV2 extended 64 KiB field |
| no-tokens, 6,010 payees, upload 20 Mbit/s | 6,010 | 204,663 | 211,369 | 15.24% | 0.12% | 2.25 | 2.56 | 1.18 / 1.87 | 0.766 | 76.0 | 681 | SV2 extended 64 KiB field |
| no-tokens, 6,010 payees, upload 50 Mbit/s | 6,010 | 204,663 | 211,369 | 7.81% | 0.01% | 0.98 | 1.12 | 1.18 / 1.86 | 0.766 | 76.0 | 681 | SV2 extended 64 KiB field |
| no-tokens, 6,010 payees, upload 200 Mbit/s | 6,010 | 204,663 | 211,369 | 3.54% | 0.00% | 0.40 | 0.46 | 1.17 / 1.84 | 0.766 | 76.0 | 681 | SV2 extended 64 KiB field |
| no-tokens, 6,010 payees, upload 1000 Mbit/s | 6,010 | 204,663 | 211,369 | 2.45% | 0.01% | 0.23 | 0.27 | 1.15 / 1.84 | 0.766 | 76.0 | 681 | SV2 extended 64 KiB field |
| no-tokens, 29,402 payees, upload 5 Mbit/s | 29,402 | 999,991 | 1,028,497 | 78.38% | 56.95% | never | never | 5.21 / 8.23 | 0.766 | 76.0 | 3,317 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, 29,402 payees, upload 20 Mbit/s | 29,402 | 999,991 | 1,028,497 | 44.60% | 11.68% | never | never | 5.77 / 9.06 | 0.766 | 76.0 | 3,317 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, 29,402 payees, upload 50 Mbit/s | 29,402 | 999,991 | 1,028,497 | 24.51% | 0.84% | 4.47 | 5.03 | 5.76 / 9.02 | 0.766 | 76.0 | 3,317 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, 29,402 payees, upload 200 Mbit/s | 29,402 | 999,991 | 1,028,497 | 9.40% | 0.01% | 1.21 | 1.38 | 5.68 / 8.98 | 0.766 | 76.0 | 3,317 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, 29,402 payees, upload 1000 Mbit/s | 29,402 | 999,991 | 1,028,497 | 4.00% | 0.00% | 0.47 | 0.53 | 5.52 / 8.84 | 0.766 | 76.0 | 3,317 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |

Small shares go stale 4% of the time even when the median node has 5 Mbit/s
up. A 200 KiB coinbase needs about 50 Mbit/s for stale shares to stay under
8%, and a 1 MB coinbase needs data-centre links (9.4% at 200 Mbit/s, 4.0% at
1 Gbit/s) when shares travel whole.

### Latency, share interval and node count

One-way latency, median of each link:

| Scenario | Payees | Coinbase bytes | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | BCHN relay (Mbit/s) | Templates (Mbit/s) | Stratum per miner (kbit/s) | Limits broken |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| latency 10 ms | 1,001 | 34,357 | 36,423 | 2.20% | 0.00% | 0.19 | 0.22 | 0.200 / 0.317 | 0.766 | 76.0 | 116 | none |
| latency 50 ms | 1,001 | 34,357 | 36,423 | 3.09% | 0.01% | 0.33 | 0.38 | 0.202 / 0.320 | 0.766 | 76.0 | 116 | none |
| latency 150 ms | 1,001 | 34,357 | 36,423 | 5.08% | 0.00% | 0.63 | 0.73 | 0.204 / 0.322 | 0.766 | 76.0 | 116 | none |
| latency 400 ms | 1,001 | 34,357 | 36,423 | 9.85% | 0.02% | 1.34 | 1.57 | 0.205 / 0.322 | 0.766 | 76.0 | 116 | none |

Share interval, for a 3 KB and a 200 KiB coinbase:

| Scenario | Payees | Coinbase bytes | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | BCHN relay (Mbit/s) | Templates (Mbit/s) | Stratum per miner (kbit/s) | Limits broken |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| tokens-3072, 1,000 payees, 1 s shares | 1,000 | 3,051 | 4,237 | 15.89% | 0.10% | 0.19 | 0.22 | 0.239 / 0.379 | 0.766 | 76.0 | 68.1 | none |
| tokens-3072, 1,000 payees, 2 s shares | 1,000 | 3,051 | 4,237 | 8.68% | 0.01% | 0.19 | 0.22 | 0.120 / 0.190 | 0.766 | 76.0 | 37.2 | none |
| tokens-3072, 1,000 payees, 5 s shares | 1,000 | 3,051 | 4,237 | 3.84% | 0.00% | 0.19 | 0.22 | 0.048 / 0.076 | 0.766 | 76.0 | 18.6 | none |
| tokens-3072, 1,000 payees, 10 s shares | 1,000 | 3,051 | 4,237 | 2.07% | 0.00% | 0.19 | 0.22 | 0.024 / 0.038 | 0.766 | 76.0 | 12.5 | none |
| tokens-3072, 1,000 payees, 30 s shares | 1,000 | 3,051 | 4,237 | 0.52% | 0.00% | 0.19 | 0.22 | 8.0e-3 / 0.013 | 0.766 | 76.0 | 8.34 | none |
| no-tokens, 6,010 payees, 1 s shares | 6,010 | 204,663 | 211,369 | 45.81% | 9.79% | 676.33 | 676.48 | 11.8 / 18.6 | 0.766 | 76.0 | 3,719 | SV2 extended 64 KiB field |
| no-tokens, 6,010 payees, 2 s shares | 6,010 | 204,663 | 211,369 | 29.50% | 1.11% | 1.10 | 1.25 | 5.93 / 9.35 | 0.766 | 76.0 | 2,031 | SV2 extended 64 KiB field |
| no-tokens, 6,010 payees, 5 s shares | 6,010 | 204,663 | 211,369 | 14.09% | 0.13% | 1.00 | 1.14 | 2.38 / 3.75 | 0.766 | 76.0 | 1,018 | SV2 extended 64 KiB field |
| no-tokens, 6,010 payees, 10 s shares | 6,010 | 204,663 | 211,369 | 7.81% | 0.01% | 0.98 | 1.12 | 1.18 / 1.86 | 0.766 | 76.0 | 681 | SV2 extended 64 KiB field |
| no-tokens, 6,010 payees, 30 s shares | 6,010 | 204,663 | 211,369 | 2.74% | 0.00% | 0.98 | 1.11 | 0.394 / 0.621 | 0.766 | 76.0 | 456 | SV2 extended 64 KiB field |

Number of nodes:

| Scenario | Payees | Coinbase bytes | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | BCHN relay (Mbit/s) | Templates (Mbit/s) | Stratum per miner (kbit/s) | Limits broken |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| 10 nodes | 1,001 | 34,357 | 36,423 | 2.09% | 0.00% | 0.18 | 0.19 | 0.207 / 0.237 | 0.766 | 76.0 | 116 | none |
| 50 nodes | 1,001 | 34,357 | 36,423 | 2.76% | 0.00% | 0.29 | 0.34 | 0.203 / 0.293 | 0.766 | 76.0 | 116 | none |
| 200 nodes | 1,001 | 34,357 | 36,423 | 3.47% | 0.00% | 0.38 | 0.44 | 0.202 / 0.375 | 0.766 | 76.0 | 116 | none |
| 1000 nodes | 1,001 | 34,357 | 36,423 | 4.23% | 0.00% | 0.47 | 0.56 | 0.202 / 0.378 | 0.766 | 76.0 | 116 | none |

Stale shares grow about linearly with how long a share takes to reach the
other miners compared with the share interval: latency, hops (more nodes)
and the share's own transfer time all add up. Ten-second shares keep that
small; 1-second shares would make 16% of small shares stale, and 46% with a
200 KiB coinbase.

Other delays matter as much as the network. Single runs from the default:

| Change from the default | Stale | Orphaned |
|---|---:|---:|
| none | 3.09% | 0.01% |
| miners take 0.5 s to get new work (`--work-switch-seconds 0.5`) | 7.19% | 0.01% |
| validation takes 100 ms (`--validation-seconds 0.1`) | 6.09% | 0.01% |
| hashrate on nodes Pareto 1.16 (`--node-hashrate pareto:1.16`) | 2.74% | 0.00% |

### Stratum jobs

Every miner gets a new job for each new share-chain tip (the payouts
change), each template poll and each BCH block: about one job every 5
seconds. A Stratum V1 job carries the coinbase twice over (hex), a V2
extended job once (binary), a V2 standard job only a merkle root:

| Coinbase | V1 job | V1 per miner | V2 extended per miner | V2 standard per miner |
|---|---:|---:|---:|---:|
| 3,051 bytes (tokens) | 7,724 bytes | 12.5 kbit/s | 6.1 kbit/s | 0.18 kbit/s |
| 34,357 bytes (1,001 payees) | 72,056 bytes | 116 kbit/s | 58 kbit/s | 0.18 kbit/s |
| 204,663 bytes (6,010 payees) | 421,988 bytes | 681 kbit/s | 340 kbit/s (does not fit its fields) | 0.18 kbit/s |

A node serving 10,000 V1 miners needs 125 Mbit/s for their jobs with the
3 KB coinbase, and 6.8 Gbit/s at 6,010 payees. For a pool of many small
miners this, not the share chain, is the largest load a node carries, and
no change to how shares travel reduces it.

### Noise

The default under eight seeds (each seed also draws a different graph and
different links):

| Scenario | Payees | Coinbase bytes | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | BCHN relay (Mbit/s) | Templates (Mbit/s) | Stratum per miner (kbit/s) | Limits broken |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| seed 1 | 1,001 | 34,357 | 36,423 | 3.09% | 0.01% | 0.33 | 0.38 | 0.202 / 0.320 | 0.766 | 76.0 | 116 | none |
| seed 2 | 1,001 | 34,357 | 36,423 | 2.97% | 0.00% | 0.35 | 0.40 | 0.202 / 0.465 | 0.766 | 76.0 | 116 | none |
| seed 3 | 1,001 | 34,357 | 36,423 | 3.19% | 0.00% | 0.33 | 0.39 | 0.206 / 0.355 | 0.766 | 76.0 | 116 | none |
| seed 4 | 1,001 | 34,357 | 36,423 | 2.87% | 0.00% | 0.33 | 0.38 | 0.200 / 0.346 | 0.766 | 76.0 | 116 | none |
| seed 5 | 1,001 | 34,357 | 36,423 | 3.47% | 0.00% | 0.37 | 0.43 | 0.203 / 0.378 | 0.766 | 76.0 | 116 | none |
| seed 6 | 1,001 | 34,357 | 36,423 | 2.90% | 0.00% | 0.31 | 0.36 | 0.202 / 0.349 | 0.766 | 76.0 | 116 | none |
| seed 7 | 1,001 | 34,357 | 36,423 | 3.26% | 0.00% | 0.34 | 0.40 | 0.202 / 0.349 | 0.766 | 76.0 | 116 | none |
| seed 8 | 1,001 | 34,357 | 36,423 | 2.89% | 0.00% | 0.33 | 0.38 | 0.202 / 0.320 | 0.766 | 76.0 | 116 | none |

The stale rate moves between 2.9% and 3.5% from seed to seed: differences
smaller than about half a point between two rows of a table are noise.

### A found block's coinbase

The coinbase is the one transaction in a found block no other BCH node has,
so a compact block carries it whole. Over four BCH hops at the median 50
Mbit/s, a 1 MB coinbase adds about 0.64 s and 0.11% extra risk of the block
being orphaned on BCH. Small next to the costs above.

## What this means

1. **Under the hybrid rule the coinbase never makes a block invalid; its
   cost is bandwidth.** A share without tokens pays up to 29,402 payees
   directly. The rule raised the 200 KiB share limit (it binds from 6,010
   payees) to 1,204,800 bytes and the P2P message limit to 1.25 MiB to
   carry such shares. Sent whole, they make 8% of shares stale at 6,000
   payees and 25% at 29,402 on the assumed links.
2. **Sending shares without their payouts is what makes direct payouts
   cheap for the share chain**: 2.0 to 2.7% stale from 1,000 to 29,402
   payees. It is a change to how shares travel, with its own protocol tag
   as the design says, not a payout rule change, and nothing here builds
   it.
3. **For miners, a large coinbase stays expensive.** Stratum V1 sends it to
   every miner about every 5 seconds; Stratum V2 extended and declared jobs
   cannot carry more than 1,922 payees; and stock firmware is reported to
   take between 12 and 461. The chipnet test with real devices (design,
   section 8.2; audit M2) has to settle what pool-built jobs can carry, and
   Stratum V2 standard channels avoid the question.
4. **The window keeps most pools far from the limits.** Above 3% of BCH a
   coinbase has fewer than about 6,000 payees; only a small pool with very
   many small miners reaches the 1 MB limit and the tree.
5. **The share chain at 10-second shares is not the bottleneck.** Stale
   shares stay at 2 to 4% for small shares, almost all paid as uncles; fast
   job switching and quick validation keep it there.
6. **At multi-gigabyte blocks, plan for the BCH node.** Run BCHN on links of
   at least 50 to 100 Mbit/s each way, and fetch templates with
   `getblocktemplatelight` rather than full `getblocktemplate`.

This page only measures. The payout rule, the share limit and the relay
format are the owner's decisions.

## Scaling propagation instead of capping it (study #85)

The small-miners design
([small-miners-design.md](../sharechain/small-miners-design.md), "Sync and
propagation cost" and "Scaling propagation instead of capping it") asks for
weak-share traffic and four ways to scale propagation to be measured here,
before and after, against upstream P2Poolv2's stated limit of about 1,000
miners at 10-second shares. None of the ideas is built; each is this model's
version of it.

### What was measured

Upstream's limit is read here as 1,000 nodes at 10-second shares with
today's design: 4.23% stale and 0.00% orphaned (the "Number of nodes" table
above). Each idea was run on 100 and 1,000 nodes at 10, 5, 2 and 1-second
shares, with everything else at the defaults (1,000 miners, a 34 KB coinbase
of 1,001 payees, the mixed links). Then, for each idea, a search found the
shortest share interval at which 1,000 nodes stay at or below 4.23% stale:
how many shares a second the idea lets 1,000 nodes carry for the stale rate
they have today at 10-second shares.

- **Today:** whole shares, forwarded after validation, 8 peers, at most 3
  uncles, no weak shares.
- **1. Weak shares:** 16, 32 or 64 weak shares per share, found as a Poisson
  process by nodes picked by hashrate, on their mining tip. Each is an
  800-byte message (904 bytes on the wire) that travels like a share: held
  until its parent share is accepted, validated in a share's validation
  time, pushed to every peer not known to have it on the same upload queue
  as the shares, and answered with an Ack. Weak shares never extend a chain
  and are never named as uncles. Two cases: gossip only, and gossip with
  every share also carrying its weak shares whole in its body, as the design
  has it (each share carries the average count; 64 add 51,200 bytes).
- **2. Compact coinbase:** shares sent without the payout outputs, as in
  "Sending shares without their payouts" above: 1,429 bytes on the wire at
  32 MB blocks, the design's "about 1 KB" plus an 18-hash merkle branch.
  Validation still costs the whole share, since the receiver rebuilds the
  payouts. Compared with whole shares carrying a 3 KB coinbase (tokens,
  1,000 payees), today's 34 KB one and a 200 KiB one (6,010 payees).
- **3. Relay before full validation:** a node forwards a share once its
  proof of work and header check out (1 ms, an estimate), which needs the
  parent's and uncles' headers checked there, and validates the body after.
  It mines on, and names as an uncle, only a share it has fully validated.
- **4. Fewer hops:** 16, 32 or 64 peers per node instead of 8, in the same
  small-world graph. A push to `k` peers still shares the node's upload, so
  more peers mean fewer hops but longer pushes.
- **5. Uncle cap:** 6 or 12 uncles per share instead of 3, each still at
  most 3 shares below its nephew. The header-sync work is counted from the
  final chain: the headers a syncing node fetches per chain share (the chain
  share's and those of the uncles it names, 305 bytes each, plus 32 bytes
  per uncle hash they carry).
- **6. Combination of 2, 3 and 4:** shares without payouts, forwarded after
  the header check, on 8, 16, 32 or 64 peers.

### How it was run

```sh
cargo netsim sweep scaling --threads 10                  # every table below
cargo netsim sweep scaling-combined                       # one idea's table
cargo netsim interval-limit scaling --nodes 1000 --threads 10
cargo netsim interval-limit scaling-combined --nodes 1000 --seed 2 --threads 4
cargo netsim run --nodes 1000 --share-interval 1 --duration 8640 --max-uncles 2
```

The sweeps are `scaling-today`, `scaling-weak-shares`,
`scaling-compact-coinbase`, `scaling-header-check`, `scaling-peers`,
`scaling-uncle-cap` and `scaling-combined`; `sweep scaling` runs them all.
100-node runs mine for a day, like every table above, so a row that repeats
an earlier scenario repeats its numbers. 1,000-node runs mine for 8,640
share intervals, the share count of a day at 10-second shares, because a day
of 1-second shares on 1,000 nodes would hold about 2.8 GB per run; at
10-second shares that is still a day, so today's 1,000-node row is the
"Number of nodes" table's. `--threads` only limits how many runs are held in
memory at once; it does not change any number. On a 32-thread machine also
running a CPU miner, the sweeps took 69 minutes with 10 threads and the
search 181 minutes.

`interval-limit` first runs today's design on the given node count at
10-second shares: its stale rate is the ceiling. For each scenario it then
runs 10 seconds, then 0.25 or 40 seconds depending on the side the ceiling
is on, then halves that bracket six times on a log scale, and reports the
shortest interval tried that stayed within the ceiling (to about 6% below 10
seconds and 2% above).

### Where each idea reaches the limit

From `cargo netsim interval-limit scaling --nodes 1000` (seed 1): the
shortest share interval at which 1,000 nodes stay within 4.23% stale, and
the stale and orphan rates there.

| Scenario | Shortest share interval (s) | Shares per second against 10-second shares | Stale there | Orphaned there |
|---|---:|---:|---:|---:|
| today | 8.91 | 1.12 | 4.21% | 0.00% |
| 16 weak shares a share, gossip only | 8.91 | 1.12 | 4.21% | 0.00% |
| 16 weak shares a share, carried whole | 11.14 | 0.90 | 3.86% | 0.00% |
| 32 weak shares a share, gossip only | 8.91 | 1.12 | 4.21% | 0.00% |
| 32 weak shares a share, carried whole | 11.89 | 0.84 | 4.19% | 0.00% |
| 64 weak shares a share, gossip only | 8.91 | 1.12 | 4.21% | 0.00% |
| 64 weak shares a share, carried whole | 15.76 | 0.63 | 3.87% | 0.00% |
| 3 KB coinbase, whole shares | 5.95 | 1.68 | 4.18% | 0.00% |
| 200 KiB coinbase, whole shares | 26.50 | 0.38 | 4.07% | 0.00% |
| 34 KB coinbase, without payouts | 5.62 | 1.78 | 4.04% | 0.00% |
| 200 KiB coinbase, without payouts | 5.95 | 1.68 | 4.20% | 0.00% |
| forwarded after the header check | 8.41 | 1.19 | 4.17% | 0.00% |
| 16 peers | 11.14 | 0.90 | 3.63% | 0.00% |
| 32 peers | 12.97 | 0.77 | 4.20% | 0.00% |
| 64 peers | 18.74 | 0.53 | 3.93% | 0.01% |
| uncle cap 6 | 8.91 | 1.12 | 4.21% | 0.00% |
| uncle cap 12 | 8.91 | 1.12 | 4.21% | 0.00% |
| without payouts, header check, 8 peers | 5.01 | 2.00 | 4.20% | 0.00% |
| without payouts, header check, 16 peers | 4.21 | 2.37 | 4.01% | 0.00% |
| without payouts, header check, 32 peers | 3.75 | 2.66 | 3.94% | 0.00% |
| without payouts, header check, 64 peers | 3.54 | 2.82 | 4.10% | 0.00% |

Today's own search ends at 8.91 seconds, not 10: between those intervals its
stale rate stays within 0.02 points of the ceiling. The search is as precise
as the stale rate is, and the stale rate moves by about half a point from
seed to seed ("Noise" above). Under seeds 2 and 3 (`--seed`, each with its
own ceiling from today's design under that seed: 4.18% and 4.41%) the
combination's limits move by up to 12%:

| Scenario | Seed 1 (s) | Seed 2 (s) | Seed 3 (s) |
|---|---:|---:|---:|
| without payouts, header check, 8 peers | 5.01 | 5.62 | 5.30 |
| without payouts, header check, 16 peers | 4.21 | 4.46 | 4.46 |
| without payouts, header check, 32 peers | 3.75 | 3.98 | 3.98 |
| without payouts, header check, 64 peers | 3.54 | 3.98 | 3.54 |

So a limit within about 12% of today's 8.9 to 10 seconds is not a measured
change. Read that way:

- **Weak shares, gossip only, do not move the limit** at 16, 32 or 64 per
  share. **Carried whole in every share, they move it to longer shares:**
  11.1, 11.9 and 15.8 seconds, because each share grows by their bytes.
- **The compact coinbase moves it from about 9 to 10 seconds to 5.6
  seconds**, and makes the coinbase size irrelevant: with a 200 KiB coinbase
  the limit moves from 26.5 seconds (whole shares) to 6.0 seconds, the same
  as whole shares with a 3 KB coinbase.
- **Relay before full validation alone barely moves it** (8.4 seconds):
  validation is 5.7 ms of a hop that a 36 KB push and 50 ms of latency
  dominate.
- **More peers alone move it the wrong way** (11.1, 13.0 and 18.7 seconds
  at 16, 32 and 64 peers): with whole shares a push to more peers takes
  longer than the hops it saves.
- **A larger uncle cap does not move it**: in these runs no share ever had a
  fourth uncle to name, so caps of 6 and 12 give the same runs as 3.
- **The combination moves it to 5.0, 4.2, 3.8 and 3.5 seconds** at 8, 16,
  32 and 64 peers: 2.0 to 2.8 times the share rate of 10-second shares, 1.8
  to 2.5 times what today's own search reaches. Once shares are small, more
  peers help, with little gain past 32.

### Today

The before figures, from `cargo netsim sweep scaling-today`. Columns: the
share's bytes on the wire (with weak shares carried whole, those included);
stale and orphaned shares; mean seconds until 90% and all of the nodes had
each share; P2Pool's upload per node (shares, weak shares and acks), mean
and busiest node; uncles the final chain names per chain share; and the
header bytes a node syncing the chain fetches per chain share.

| Scenario | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | Uncles per chain share | Header-sync bytes per chain share |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| today, 100 nodes, 10 s shares | 36,423 | 3.09% | 0.01% | 0.33 | 0.38 | 0.202 / 0.320 | 0.032 | 316 |
| today, 100 nodes, 5 s shares | 36,423 | 5.88% | 0.00% | 0.33 | 0.38 | 0.408 / 0.645 | 0.062 | 326 |
| today, 100 nodes, 2 s shares | 36,423 | 13.11% | 0.08% | 0.33 | 0.38 | 1.02 / 1.61 | 0.150 | 356 |
| today, 100 nodes, 1 s shares | 36,423 | 23.00% | 0.33% | 0.33 | 0.38 | 2.02 / 3.20 | 0.294 | 407 |
| today, 1,000 nodes, 10 s shares | 36,423 | 4.23% | 0.00% | 0.47 | 0.56 | 0.202 / 0.378 | 0.044 | 320 |
| today, 1,000 nodes, 5 s shares | 36,423 | 7.84% | 0.01% | 0.47 | 0.56 | 0.402 / 0.752 | 0.085 | 334 |
| today, 1,000 nodes, 2 s shares | 36,423 | 17.77% | 0.14% | 0.47 | 0.56 | 1.02 / 1.91 | 0.214 | 379 |
| today, 1,000 nodes, 1 s shares | 36,423 | 30.66% | 0.79% | 0.48 | 0.57 | 2.02 / 3.77 | 0.431 | 456 |

### 1. Weak shares

`cargo netsim sweep scaling-weak-shares`:

| Scenario | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | Uncles per chain share | Header-sync bytes per chain share |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 16 weak shares a share, gossip only, 100 nodes, 10 s shares | 36,423 | 3.09% | 0.01% | 0.33 | 0.38 | 0.291 / 0.458 | 0.032 | 316 |
| 16 weak shares a share, gossip only, 100 nodes, 5 s shares | 36,423 | 5.88% | 0.00% | 0.33 | 0.38 | 0.585 / 0.922 | 0.062 | 326 |
| 16 weak shares a share, gossip only, 100 nodes, 2 s shares | 36,423 | 13.11% | 0.08% | 0.33 | 0.38 | 1.46 / 2.30 | 0.150 | 356 |
| 16 weak shares a share, gossip only, 100 nodes, 1 s shares | 36,423 | 23.02% | 0.33% | 0.33 | 0.38 | 2.91 / 4.58 | 0.295 | 407 |
| 16 weak shares a share, gossip only, 1,000 nodes, 10 s shares | 36,423 | 4.23% | 0.00% | 0.47 | 0.56 | 0.291 / 0.542 | 0.044 | 320 |
| 16 weak shares a share, gossip only, 1,000 nodes, 5 s shares | 36,423 | 7.84% | 0.01% | 0.47 | 0.56 | 0.579 / 1.08 | 0.085 | 334 |
| 16 weak shares a share, gossip only, 1,000 nodes, 2 s shares | 36,423 | 17.77% | 0.14% | 0.47 | 0.56 | 1.46 / 2.73 | 0.214 | 379 |
| 16 weak shares a share, gossip only, 1,000 nodes, 1 s shares | 36,423 | 30.66% | 0.79% | 0.48 | 0.57 | 2.90 / 5.40 | 0.431 | 456 |
| 16 weak shares a share, carried whole, 100 nodes, 10 s shares | 49,583 | 3.43% | 0.00% | 0.38 | 0.44 | 0.364 / 0.574 | 0.036 | 317 |
| 16 weak shares a share, carried whole, 100 nodes, 5 s shares | 49,583 | 6.55% | 0.00% | 0.38 | 0.44 | 0.733 / 1.15 | 0.070 | 329 |
| 16 weak shares a share, carried whole, 100 nodes, 2 s shares | 49,583 | 14.62% | 0.09% | 0.38 | 0.44 | 1.83 / 2.88 | 0.170 | 363 |
| 16 weak shares a share, carried whole, 100 nodes, 1 s shares | 49,583 | 25.34% | 0.49% | 0.39 | 0.45 | 3.64 / 5.74 | 0.333 | 421 |
| 16 weak shares a share, carried whole, 1,000 nodes, 10 s shares | 49,583 | 4.82% | 0.00% | 0.54 | 0.65 | 0.364 / 0.678 | 0.051 | 322 |
| 16 weak shares a share, carried whole, 1,000 nodes, 5 s shares | 49,583 | 8.84% | 0.00% | 0.55 | 0.65 | 0.725 / 1.35 | 0.097 | 338 |
| 16 weak shares a share, carried whole, 1,000 nodes, 2 s shares | 49,583 | 19.95% | 0.26% | 0.55 | 0.65 | 1.84 / 3.42 | 0.246 | 390 |
| 16 weak shares a share, carried whole, 1,000 nodes, 1 s shares | 49,583 | 33.49% | 1.11% | 0.79 | 0.90 | 3.63 / 6.77 | 0.487 | 477 |
| 32 weak shares a share, gossip only, 100 nodes, 10 s shares | 36,423 | 3.09% | 0.01% | 0.33 | 0.38 | 0.379 / 0.596 | 0.032 | 316 |
| 32 weak shares a share, gossip only, 100 nodes, 5 s shares | 36,423 | 5.88% | 0.00% | 0.33 | 0.38 | 0.761 / 1.20 | 0.062 | 326 |
| 32 weak shares a share, gossip only, 100 nodes, 2 s shares | 36,423 | 13.11% | 0.08% | 0.33 | 0.38 | 1.90 / 2.99 | 0.150 | 356 |
| 32 weak shares a share, gossip only, 100 nodes, 1 s shares | 36,423 | 23.05% | 0.33% | 0.33 | 0.38 | 3.79 / 5.96 | 0.295 | 407 |
| 32 weak shares a share, gossip only, 1,000 nodes, 10 s shares | 36,423 | 4.23% | 0.00% | 0.47 | 0.56 | 0.379 / 0.705 | 0.044 | 320 |
| 32 weak shares a share, gossip only, 1,000 nodes, 5 s shares | 36,423 | 7.84% | 0.01% | 0.47 | 0.56 | 0.755 / 1.40 | 0.085 | 334 |
| 32 weak shares a share, gossip only, 1,000 nodes, 2 s shares | 36,423 | 17.77% | 0.14% | 0.47 | 0.56 | 1.91 / 3.55 | 0.214 | 379 |
| 32 weak shares a share, gossip only, 1,000 nodes, 1 s shares | 36,423 | 30.69% | 0.87% | 0.79 | 0.88 | 3.78 / 7.04 | 0.430 | 456 |
| 32 weak shares a share, carried whole, 100 nodes, 10 s shares | 62,703 | 3.68% | 0.00% | 0.43 | 0.49 | 0.526 / 0.825 | 0.038 | 318 |
| 32 weak shares a share, carried whole, 100 nodes, 5 s shares | 62,703 | 7.19% | 0.01% | 0.43 | 0.49 | 1.06 / 1.66 | 0.077 | 331 |
| 32 weak shares a share, carried whole, 100 nodes, 2 s shares | 62,703 | 16.05% | 0.12% | 0.44 | 0.50 | 2.63 / 4.14 | 0.190 | 370 |
| 32 weak shares a share, carried whole, 100 nodes, 1 s shares | 62,703 | 27.57% | 0.73% | 0.45 | 0.51 | 5.26 / 8.26 | 0.371 | 435 |
| 32 weak shares a share, carried whole, 1,000 nodes, 10 s shares | 62,703 | 5.35% | 0.00% | 0.62 | 0.74 | 0.525 / 0.977 | 0.057 | 324 |
| 32 weak shares a share, carried whole, 1,000 nodes, 5 s shares | 62,703 | 9.92% | 0.01% | 0.62 | 0.74 | 1.05 / 1.95 | 0.110 | 343 |
| 32 weak shares a share, carried whole, 1,000 nodes, 2 s shares | 62,703 | 21.88% | 0.33% | 0.62 | 0.75 | 2.65 / 4.92 | 0.276 | 401 |
| 32 weak shares a share, carried whole, 1,000 nodes, 1 s shares | 62,703 | 36.31% | 1.55% | 3.24 | 3.37 | 5.25 / 9.76 | 0.545 | 499 |
| 64 weak shares a share, gossip only, 100 nodes, 10 s shares | 36,423 | 3.09% | 0.01% | 0.33 | 0.38 | 0.556 / 0.873 | 0.032 | 316 |
| 64 weak shares a share, gossip only, 100 nodes, 5 s shares | 36,423 | 5.88% | 0.00% | 0.33 | 0.38 | 1.11 / 1.75 | 0.062 | 326 |
| 64 weak shares a share, gossip only, 100 nodes, 2 s shares | 36,423 | 13.11% | 0.08% | 0.33 | 0.38 | 2.78 / 4.37 | 0.150 | 356 |
| 64 weak shares a share, gossip only, 100 nodes, 1 s shares | 36,423 | 23.11% | 0.35% | 0.34 | 0.38 | 5.56 / 8.73 | 0.296 | 408 |
| 64 weak shares a share, gossip only, 1,000 nodes, 10 s shares | 36,423 | 4.23% | 0.00% | 0.47 | 0.56 | 0.555 / 1.03 | 0.044 | 320 |
| 64 weak shares a share, gossip only, 1,000 nodes, 5 s shares | 36,423 | 7.85% | 0.01% | 0.47 | 0.56 | 1.11 / 2.06 | 0.085 | 334 |
| 64 weak shares a share, gossip only, 1,000 nodes, 2 s shares | 36,423 | 17.78% | 0.15% | 0.47 | 0.56 | 2.79 / 5.18 | 0.214 | 379 |
| 64 weak shares a share, gossip only, 1,000 nodes, 1 s shares | 36,423 | 30.81% | 0.95% | 3.39 | 3.48 | 5.55 / 10.3 | 0.431 | 456 |
| 64 weak shares a share, carried whole, 100 nodes, 10 s shares | 89,023 | 4.48% | 0.00% | 0.53 | 0.60 | 0.851 / 1.34 | 0.047 | 321 |
| 64 weak shares a share, carried whole, 100 nodes, 5 s shares | 89,023 | 8.61% | 0.01% | 0.53 | 0.60 | 1.71 / 2.68 | 0.094 | 337 |
| 64 weak shares a share, carried whole, 100 nodes, 2 s shares | 89,023 | 18.77% | 0.20% | 0.54 | 0.62 | 4.26 / 6.70 | 0.229 | 384 |
| 64 weak shares a share, carried whole, 100 nodes, 1 s shares | 89,023 | 32.20% | 2.34% | 57.45 | 57.53 | 8.51 / 13.4 | 0.440 | 460 |
| 64 weak shares a share, carried whole, 1,000 nodes, 10 s shares | 89,023 | 6.46% | 0.00% | 0.76 | 0.91 | 0.849 / 1.58 | 0.069 | 328 |
| 64 weak shares a share, carried whole, 1,000 nodes, 5 s shares | 89,023 | 11.98% | 0.03% | 0.76 | 0.91 | 1.69 / 3.15 | 0.136 | 351 |
| 64 weak shares a share, carried whole, 1,000 nodes, 2 s shares | 89,023 | 25.55% | 0.59% | 2.19 | 2.34 | 4.28 / 7.95 | 0.335 | 422 |
| 64 weak shares a share, carried whole, 1,000 nodes, 1 s shares | 89,023 | 40.76% | 3.99% | 19.82 | 19.98 | 8.49 / 15.8 | 0.620 | 528 |

Gossip only, weak shares cost upload, not stale shares: at 64 per share they
raise the mean upload per node from 0.20 to 0.56 Mbit/s at 10-second shares
and to 2.8 Mbit/s at 2-second shares, and every stale rate stays within 0.01
points of today's down to 2-second shares. At 1-second shares (64 weak
shares a second) the slowest links fall behind at times: on 1,000 nodes the
mean time to 90% rises from 0.48 to 3.39 seconds while the stale rate moves
from 30.66% to 30.81%, so the delays fall on few shares. Carried whole,
their bytes count like a larger coinbase: 64 per share make an 89 KB share,
and 1,000 nodes go 6.46% stale at 10-second shares.

### 2. Compact coinbase

`cargo netsim sweep scaling-compact-coinbase` (today's 34 KB coinbase with
whole shares is the "Today" table):

| Scenario | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | Uncles per chain share | Header-sync bytes per chain share |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 3 KB coinbase, whole shares, 100 nodes, 10 s shares | 4,237 | 2.07% | 0.00% | 0.19 | 0.22 | 0.024 / 0.038 | 0.021 | 312 |
| 3 KB coinbase, whole shares, 100 nodes, 5 s shares | 4,237 | 3.84% | 0.00% | 0.19 | 0.22 | 0.048 / 0.076 | 0.040 | 319 |
| 3 KB coinbase, whole shares, 100 nodes, 2 s shares | 4,237 | 8.68% | 0.01% | 0.19 | 0.22 | 0.120 / 0.190 | 0.095 | 337 |
| 3 KB coinbase, whole shares, 100 nodes, 1 s shares | 4,237 | 15.89% | 0.10% | 0.19 | 0.22 | 0.239 / 0.379 | 0.188 | 369 |
| 3 KB coinbase, whole shares, 1,000 nodes, 10 s shares | 4,237 | 2.64% | 0.00% | 0.26 | 0.31 | 0.024 / 0.045 | 0.027 | 314 |
| 3 KB coinbase, whole shares, 1,000 nodes, 5 s shares | 4,237 | 4.92% | 0.00% | 0.26 | 0.31 | 0.047 / 0.089 | 0.052 | 323 |
| 3 KB coinbase, whole shares, 1,000 nodes, 2 s shares | 4,237 | 11.79% | 0.02% | 0.26 | 0.31 | 0.121 / 0.226 | 0.133 | 351 |
| 3 KB coinbase, whole shares, 1,000 nodes, 1 s shares | 4,237 | 20.98% | 0.15% | 0.26 | 0.31 | 0.238 / 0.446 | 0.263 | 396 |
| 200 KiB coinbase, whole shares, 100 nodes, 10 s shares | 211,369 | 7.81% | 0.01% | 0.98 | 1.12 | 1.18 / 1.86 | 0.084 | 334 |
| 200 KiB coinbase, whole shares, 100 nodes, 5 s shares | 211,369 | 14.09% | 0.13% | 1.00 | 1.14 | 2.38 / 3.75 | 0.162 | 361 |
| 200 KiB coinbase, whole shares, 100 nodes, 2 s shares | 211,369 | 29.50% | 1.11% | 1.10 | 1.25 | 5.93 / 9.35 | 0.403 | 446 |
| 200 KiB coinbase, whole shares, 100 nodes, 1 s shares | 211,369 | 45.81% | 9.79% | 676.33 | 676.48 | 11.8 / 18.6 | 0.665 | 545 |
| 200 KiB coinbase, whole shares, 1,000 nodes, 10 s shares | 211,369 | 11.02% | 0.06% | 1.42 | 1.71 | 1.18 / 2.20 | 0.123 | 347 |
| 200 KiB coinbase, whole shares, 1,000 nodes, 5 s shares | 211,369 | 19.56% | 0.10% | 1.44 | 1.73 | 2.34 / 4.37 | 0.242 | 389 |
| 200 KiB coinbase, whole shares, 1,000 nodes, 2 s shares | 211,369 | 38.31% | 2.34% | 9.91 | 10.22 | 5.96 / 11.1 | 0.583 | 513 |
| 200 KiB coinbase, whole shares, 1,000 nodes, 1 s shares | 211,369 | 55.61% | 11.63% | 80.31 | 80.68 | 11.8 / 21.9 | 0.990 | 674 |
| 34 KB coinbase, without payouts, 100 nodes, 10 s shares | 1,429 | 1.98% | 0.00% | 0.17 | 0.20 | 8.4e-3 / 0.013 | 0.020 | 312 |
| 34 KB coinbase, without payouts, 100 nodes, 5 s shares | 1,429 | 3.66% | 0.00% | 0.17 | 0.20 | 0.017 / 0.027 | 0.038 | 318 |
| 34 KB coinbase, without payouts, 100 nodes, 2 s shares | 1,429 | 8.26% | 0.00% | 0.17 | 0.20 | 0.042 / 0.066 | 0.090 | 336 |
| 34 KB coinbase, without payouts, 100 nodes, 1 s shares | 1,429 | 15.21% | 0.09% | 0.17 | 0.20 | 0.084 / 0.133 | 0.178 | 366 |
| 34 KB coinbase, without payouts, 1,000 nodes, 10 s shares | 1,429 | 2.52% | 0.00% | 0.24 | 0.29 | 8.4e-3 / 0.016 | 0.026 | 314 |
| 34 KB coinbase, without payouts, 1,000 nodes, 5 s shares | 1,429 | 4.66% | 0.00% | 0.24 | 0.29 | 0.017 / 0.031 | 0.049 | 322 |
| 34 KB coinbase, without payouts, 1,000 nodes, 2 s shares | 1,429 | 11.15% | 0.01% | 0.24 | 0.29 | 0.043 / 0.079 | 0.125 | 348 |
| 34 KB coinbase, without payouts, 1,000 nodes, 1 s shares | 1,429 | 20.06% | 0.13% | 0.24 | 0.29 | 0.084 / 0.156 | 0.249 | 391 |
| 200 KiB coinbase, without payouts, 100 nodes, 10 s shares | 1,429 | 2.10% | 0.00% | 0.19 | 0.22 | 8.3e-3 / 0.013 | 0.021 | 312 |
| 200 KiB coinbase, without payouts, 100 nodes, 5 s shares | 1,429 | 3.89% | 0.00% | 0.19 | 0.22 | 0.017 / 0.027 | 0.040 | 319 |
| 200 KiB coinbase, without payouts, 100 nodes, 2 s shares | 1,429 | 8.78% | 0.01% | 0.19 | 0.22 | 0.042 / 0.066 | 0.096 | 338 |
| 200 KiB coinbase, without payouts, 100 nodes, 1 s shares | 1,429 | 16.11% | 0.10% | 0.19 | 0.22 | 0.083 / 0.132 | 0.191 | 371 |
| 200 KiB coinbase, without payouts, 1,000 nodes, 10 s shares | 1,429 | 2.66% | 0.00% | 0.26 | 0.31 | 8.3e-3 / 0.015 | 0.027 | 314 |
| 200 KiB coinbase, without payouts, 1,000 nodes, 5 s shares | 1,429 | 5.00% | 0.00% | 0.26 | 0.31 | 0.016 / 0.031 | 0.053 | 323 |
| 200 KiB coinbase, without payouts, 1,000 nodes, 2 s shares | 1,429 | 11.99% | 0.03% | 0.26 | 0.31 | 0.042 / 0.079 | 0.136 | 351 |
| 200 KiB coinbase, without payouts, 1,000 nodes, 1 s shares | 1,429 | 21.16% | 0.16% | 0.26 | 0.31 | 0.083 / 0.155 | 0.266 | 397 |

Without payouts a share is 1,429 bytes whatever the payees, and the stale
rate no longer depends on the coinbase: on 1,000 nodes at 2-second shares,
11.15% (34 KB coinbase) and 11.99% (200 KiB) against 17.77% and 38.31% sent
whole. What is left is about what a 3 KB coinbase sent whole costs.

### 3. Relay before full validation

`cargo netsim sweep scaling-header-check`:

| Scenario | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | Uncles per chain share | Header-sync bytes per chain share |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| forwarded after the header check, 100 nodes, 10 s shares | 36,423 | 2.98% | 0.01% | 0.31 | 0.36 | 0.205 / 0.322 | 0.031 | 315 |
| forwarded after the header check, 100 nodes, 5 s shares | 36,423 | 5.72% | 0.00% | 0.31 | 0.36 | 0.413 / 0.649 | 0.061 | 326 |
| forwarded after the header check, 100 nodes, 2 s shares | 36,423 | 12.68% | 0.07% | 0.31 | 0.36 | 1.03 / 1.62 | 0.144 | 354 |
| forwarded after the header check, 100 nodes, 1 s shares | 36,423 | 22.31% | 0.30% | 0.32 | 0.36 | 2.05 / 3.22 | 0.283 | 403 |
| forwarded after the header check, 1,000 nodes, 10 s shares | 36,423 | 3.99% | 0.00% | 0.45 | 0.53 | 0.205 / 0.380 | 0.042 | 319 |
| forwarded after the header check, 1,000 nodes, 5 s shares | 36,423 | 7.55% | 0.01% | 0.45 | 0.53 | 0.407 / 0.756 | 0.082 | 333 |
| forwarded after the header check, 1,000 nodes, 2 s shares | 36,423 | 17.13% | 0.11% | 0.45 | 0.54 | 1.03 / 1.92 | 0.205 | 376 |
| forwarded after the header check, 1,000 nodes, 1 s shares | 36,423 | 29.79% | 0.72% | 0.45 | 0.54 | 2.04 / 3.79 | 0.414 | 450 |

On its own, a small gain: 3.99% against 4.23% on 1,000 nodes at 10-second
shares, and 0.45 against 0.47 seconds to 90% of the nodes.

### 4. Fewer hops

`cargo netsim sweep scaling-peers`:

| Scenario | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | Uncles per chain share | Header-sync bytes per chain share |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 16 peers, 100 nodes, 10 s shares | 36,423 | 3.41% | 0.00% | 0.37 | 0.42 | 0.437 / 0.585 | 0.035 | 317 |
| 16 peers, 100 nodes, 5 s shares | 36,423 | 6.60% | 0.00% | 0.37 | 0.42 | 0.881 / 1.18 | 0.071 | 329 |
| 16 peers, 100 nodes, 2 s shares | 36,423 | 14.72% | 0.07% | 0.38 | 0.42 | 2.20 / 2.94 | 0.172 | 364 |
| 16 peers, 100 nodes, 1 s shares | 36,423 | 25.44% | 0.54% | 0.39 | 0.44 | 4.37 / 5.85 | 0.334 | 421 |
| 16 peers, 1,000 nodes, 10 s shares | 36,423 | 4.54% | 0.00% | 0.49 | 0.57 | 0.436 / 0.788 | 0.048 | 321 |
| 16 peers, 1,000 nodes, 5 s shares | 36,423 | 7.98% | 0.02% | 0.49 | 0.57 | 0.867 / 1.57 | 0.087 | 334 |
| 16 peers, 1,000 nodes, 2 s shares | 36,423 | 18.55% | 0.14% | 0.50 | 0.58 | 2.21 / 3.98 | 0.226 | 383 |
| 16 peers, 1,000 nodes, 1 s shares | 36,423 | 32.01% | 1.30% | 0.81 | 0.89 | 4.35 / 7.85 | 0.451 | 464 |
| 32 peers, 100 nodes, 10 s shares | 36,423 | 4.79% | 0.00% | 0.51 | 0.55 | 0.905 / 1.17 | 0.050 | 322 |
| 32 peers, 100 nodes, 5 s shares | 36,423 | 9.09% | 0.03% | 0.52 | 0.57 | 1.82 / 2.36 | 0.100 | 339 |
| 32 peers, 100 nodes, 2 s shares | 36,423 | 19.86% | 0.47% | 0.59 | 0.64 | 4.55 / 5.88 | 0.242 | 389 |
| 32 peers, 100 nodes, 1 s shares | 36,423 | 33.82% | 8.19% | 513.09 | 513.14 | 9.05 / 11.7 | 0.387 | 441 |
| 32 peers, 1,000 nodes, 10 s shares | 36,423 | 6.02% | 0.00% | 0.65 | 0.74 | 0.905 / 1.29 | 0.064 | 327 |
| 32 peers, 1,000 nodes, 5 s shares | 36,423 | 10.85% | 0.05% | 0.68 | 0.76 | 1.80 / 2.56 | 0.121 | 346 |
| 32 peers, 1,000 nodes, 2 s shares | 36,423 | 24.01% | 0.83% | 14.30 | 14.39 | 4.58 / 6.50 | 0.305 | 411 |
| 32 peers, 1,000 nodes, 1 s shares | 36,423 | 39.28% | 5.08% | 41.31 | 41.40 | 9.03 / 12.8 | 0.563 | 506 |
| 64 peers, 100 nodes, 10 s shares | 36,423 | 6.51% | 0.01% | 0.77 | 0.83 | 1.84 / 2.17 | 0.070 | 329 |
| 64 peers, 100 nodes, 5 s shares | 36,423 | 12.54% | 0.06% | 0.80 | 0.86 | 3.72 / 4.37 | 0.143 | 354 |
| 64 peers, 100 nodes, 2 s shares | 36,423 | 28.23% | 2.74% | 4.43 | 4.50 | 9.26 / 10.9 | 0.355 | 429 |
| 64 peers, 100 nodes, 1 s shares | 36,423 | 45.30% | 14.86% | 2095.45 | 2095.52 | 18.4 / 21.7 | 0.556 | 504 |
| 64 peers, 1,000 nodes, 10 s shares | 36,423 | 7.97% | 0.01% | 0.93 | 1.03 | 1.84 / 2.31 | 0.086 | 334 |
| 64 peers, 1,000 nodes, 5 s shares | 36,423 | 14.85% | 0.36% | 4.95 | 5.04 | 3.66 / 4.60 | 0.170 | 363 |
| 64 peers, 1,000 nodes, 2 s shares | 36,423 | 31.32% | 3.99% | 100.46 | 100.56 | 9.32 / 11.7 | 0.398 | 445 |
| 64 peers, 1,000 nodes, 1 s shares | 36,423 | 48.79% | 14.80% | 269.49 | 269.59 | 18.4 / 23.1 | 0.663 | 544 |

With whole shares more peers are worse at every size and interval. A node
pushes a whole share to every peer not known to have it, so each node uploads
about as many copies as it has peers: 1.84 Mbit/s mean at 64 peers against
0.20 at 8 (10-second shares). At 2-second shares 64 peers saturate the slower
links, and the mean time to 90% of 1,000 nodes reaches 100 seconds.

### 5. Uncle cap

`cargo netsim sweep scaling-uncle-cap`:

| Scenario | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | Uncles per chain share | Header-sync bytes per chain share |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| uncle cap 6, 100 nodes, 10 s shares | 36,423 | 3.09% | 0.01% | 0.33 | 0.38 | 0.202 / 0.320 | 0.032 | 316 |
| uncle cap 6, 100 nodes, 5 s shares | 36,423 | 5.88% | 0.00% | 0.33 | 0.38 | 0.408 / 0.645 | 0.062 | 326 |
| uncle cap 6, 100 nodes, 2 s shares | 36,423 | 13.11% | 0.08% | 0.33 | 0.38 | 1.02 / 1.61 | 0.150 | 356 |
| uncle cap 6, 100 nodes, 1 s shares | 36,423 | 23.00% | 0.33% | 0.33 | 0.38 | 2.02 / 3.20 | 0.294 | 407 |
| uncle cap 6, 1,000 nodes, 10 s shares | 36,423 | 4.23% | 0.00% | 0.47 | 0.56 | 0.202 / 0.378 | 0.044 | 320 |
| uncle cap 6, 1,000 nodes, 5 s shares | 36,423 | 7.84% | 0.01% | 0.47 | 0.56 | 0.402 / 0.752 | 0.085 | 334 |
| uncle cap 6, 1,000 nodes, 2 s shares | 36,423 | 17.77% | 0.14% | 0.47 | 0.56 | 1.02 / 1.91 | 0.214 | 379 |
| uncle cap 6, 1,000 nodes, 1 s shares | 36,423 | 30.66% | 0.79% | 0.48 | 0.57 | 2.02 / 3.77 | 0.431 | 456 |
| uncle cap 12, 100 nodes, 10 s shares | 36,423 | 3.09% | 0.01% | 0.33 | 0.38 | 0.202 / 0.320 | 0.032 | 316 |
| uncle cap 12, 100 nodes, 5 s shares | 36,423 | 5.88% | 0.00% | 0.33 | 0.38 | 0.408 / 0.645 | 0.062 | 326 |
| uncle cap 12, 100 nodes, 2 s shares | 36,423 | 13.11% | 0.08% | 0.33 | 0.38 | 1.02 / 1.61 | 0.150 | 356 |
| uncle cap 12, 100 nodes, 1 s shares | 36,423 | 23.00% | 0.33% | 0.33 | 0.38 | 2.02 / 3.20 | 0.294 | 407 |
| uncle cap 12, 1,000 nodes, 10 s shares | 36,423 | 4.23% | 0.00% | 0.47 | 0.56 | 0.202 / 0.378 | 0.044 | 320 |
| uncle cap 12, 1,000 nodes, 5 s shares | 36,423 | 7.84% | 0.01% | 0.47 | 0.56 | 0.402 / 0.752 | 0.085 | 334 |
| uncle cap 12, 1,000 nodes, 2 s shares | 36,423 | 17.77% | 0.14% | 0.47 | 0.56 | 1.02 / 1.91 | 0.214 | 379 |
| uncle cap 12, 1,000 nodes, 1 s shares | 36,423 | 30.66% | 0.79% | 0.48 | 0.57 | 2.02 / 3.77 | 0.431 | 456 |

Every row is the same as today's: no share in these runs had a fourth uncle
to name, so a larger cap changes nothing and costs no extra header sync.
Smaller caps do change the runs: 1,000 nodes at 1-second shares are 0.79%
orphaned with a cap of 3, 0.81% with 2 and 2.05% with 1 (`--max-uncles`,
one run each; the stale rate is 30.66% in all three, since uncles do not
change which chain wins in this model). The orphans left at a cap of 3 are
shares no share on the final chain could name within 3 shares of depth. The
header-sync work grows with the share rate whatever the cap: on 1,000 nodes
320 bytes per chain share at 10-second shares, 379 at 2 seconds and 456 at
1 second (0.044, 0.214 and 0.431 uncles per chain share).

### 6. The combination

`cargo netsim sweep scaling-combined`:

| Scenario | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | Uncles per chain share | Header-sync bytes per chain share |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| without payouts, header check, 8 peers, 100 nodes, 10 s shares | 1,429 | 1.84% | 0.00% | 0.16 | 0.18 | 8.6e-3 / 0.013 | 0.019 | 311 |
| without payouts, header check, 8 peers, 100 nodes, 5 s shares | 1,429 | 3.44% | 0.00% | 0.16 | 0.19 | 0.017 / 0.027 | 0.036 | 317 |
| without payouts, header check, 8 peers, 100 nodes, 2 s shares | 1,429 | 7.79% | 0.00% | 0.16 | 0.19 | 0.043 / 0.067 | 0.084 | 334 |
| without payouts, header check, 8 peers, 100 nodes, 1 s shares | 1,429 | 14.33% | 0.08% | 0.16 | 0.19 | 0.086 / 0.134 | 0.166 | 362 |
| without payouts, header check, 8 peers, 1,000 nodes, 10 s shares | 1,429 | 2.32% | 0.00% | 0.22 | 0.26 | 8.6e-3 / 0.016 | 0.024 | 313 |
| without payouts, header check, 8 peers, 1,000 nodes, 5 s shares | 1,429 | 4.37% | 0.00% | 0.22 | 0.26 | 0.017 / 0.032 | 0.046 | 320 |
| without payouts, header check, 8 peers, 1,000 nodes, 2 s shares | 1,429 | 10.30% | 0.00% | 0.22 | 0.26 | 0.043 / 0.080 | 0.115 | 344 |
| without payouts, header check, 8 peers, 1,000 nodes, 1 s shares | 1,429 | 18.93% | 0.06% | 0.22 | 0.26 | 0.086 / 0.159 | 0.232 | 385 |
| without payouts, header check, 16 peers, 100 nodes, 10 s shares | 1,429 | 1.63% | 0.00% | 0.12 | 0.13 | 0.018 / 0.025 | 0.017 | 311 |
| without payouts, header check, 16 peers, 100 nodes, 5 s shares | 1,429 | 2.87% | 0.00% | 0.12 | 0.13 | 0.037 / 0.050 | 0.030 | 315 |
| without payouts, header check, 16 peers, 100 nodes, 2 s shares | 1,429 | 6.55% | 0.00% | 0.12 | 0.13 | 0.093 / 0.124 | 0.070 | 329 |
| without payouts, header check, 16 peers, 100 nodes, 1 s shares | 1,429 | 12.34% | 0.03% | 0.12 | 0.13 | 0.184 / 0.247 | 0.140 | 353 |
| without payouts, header check, 16 peers, 1,000 nodes, 10 s shares | 1,429 | 1.99% | 0.00% | 0.16 | 0.19 | 0.018 / 0.033 | 0.020 | 312 |
| without payouts, header check, 16 peers, 1,000 nodes, 5 s shares | 1,429 | 3.51% | 0.00% | 0.16 | 0.19 | 0.037 / 0.066 | 0.036 | 317 |
| without payouts, header check, 16 peers, 1,000 nodes, 2 s shares | 1,429 | 8.39% | 0.00% | 0.16 | 0.19 | 0.093 / 0.167 | 0.092 | 336 |
| without payouts, header check, 16 peers, 1,000 nodes, 1 s shares | 1,429 | 15.75% | 0.09% | 0.16 | 0.19 | 0.184 / 0.330 | 0.186 | 369 |
| without payouts, header check, 32 peers, 100 nodes, 10 s shares | 1,429 | 1.50% | 0.00% | 0.10 | 0.12 | 0.038 / 0.049 | 0.015 | 310 |
| without payouts, header check, 32 peers, 100 nodes, 5 s shares | 1,429 | 2.64% | 0.00% | 0.10 | 0.12 | 0.077 / 0.099 | 0.027 | 314 |
| without payouts, header check, 32 peers, 100 nodes, 2 s shares | 1,429 | 6.13% | 0.00% | 0.10 | 0.12 | 0.191 / 0.246 | 0.065 | 327 |
| without payouts, header check, 32 peers, 100 nodes, 1 s shares | 1,429 | 11.54% | 0.04% | 0.10 | 0.12 | 0.381 / 0.490 | 0.130 | 349 |
| without payouts, header check, 32 peers, 1,000 nodes, 10 s shares | 1,429 | 1.83% | 0.00% | 0.14 | 0.16 | 0.038 / 0.054 | 0.019 | 311 |
| without payouts, header check, 32 peers, 1,000 nodes, 5 s shares | 1,429 | 3.26% | 0.00% | 0.14 | 0.16 | 0.076 / 0.108 | 0.034 | 316 |
| without payouts, header check, 32 peers, 1,000 nodes, 2 s shares | 1,429 | 7.73% | 0.00% | 0.14 | 0.16 | 0.193 / 0.274 | 0.084 | 333 |
| without payouts, header check, 32 peers, 1,000 nodes, 1 s shares | 1,429 | 14.68% | 0.10% | 0.14 | 0.16 | 0.381 / 0.541 | 0.171 | 363 |
| without payouts, header check, 64 peers, 100 nodes, 10 s shares | 1,429 | 1.53% | 0.00% | 0.10 | 0.11 | 0.077 / 0.091 | 0.015 | 310 |
| without payouts, header check, 64 peers, 100 nodes, 5 s shares | 1,429 | 2.66% | 0.00% | 0.10 | 0.11 | 0.156 / 0.183 | 0.027 | 314 |
| without payouts, header check, 64 peers, 100 nodes, 2 s shares | 1,429 | 6.04% | 0.01% | 0.10 | 0.11 | 0.389 / 0.458 | 0.064 | 327 |
| without payouts, header check, 64 peers, 100 nodes, 1 s shares | 1,429 | 11.41% | 0.05% | 0.10 | 0.11 | 0.776 / 0.912 | 0.128 | 349 |
| without payouts, header check, 64 peers, 1,000 nodes, 10 s shares | 1,429 | 1.76% | 0.00% | 0.13 | 0.15 | 0.077 / 0.097 | 0.018 | 311 |
| without payouts, header check, 64 peers, 1,000 nodes, 5 s shares | 1,429 | 3.21% | 0.00% | 0.13 | 0.15 | 0.154 / 0.193 | 0.033 | 316 |
| without payouts, header check, 64 peers, 1,000 nodes, 2 s shares | 1,429 | 7.70% | 0.00% | 0.13 | 0.15 | 0.392 / 0.492 | 0.083 | 333 |
| without payouts, header check, 64 peers, 1,000 nodes, 1 s shares | 1,429 | 14.32% | 0.05% | 0.13 | 0.15 | 0.774 / 0.971 | 0.166 | 362 |

On 1,000 nodes at 2-second shares the combination is 10.30% stale at 8 peers
and 7.70% at 64, against today's 17.77%; at 1-second shares 18.93% to 14.32%
against 30.66%, with almost no orphans. The busiest node uploads under 1
Mbit/s even at 64 peers and 1-second shares.

### What was approximated

Beyond the model's own assumptions (fluid links, no loss, the assumed link
mix, estimated validation and job-switch times):

- **Weak shares:** the 800-byte payload is an estimate inside the design's
  0.5 to 1.2 KB. A weak share is validated in a share's validation time
  (5.7 ms), and validation is not a queue: like share validation in this
  model it never waits for the CPU. Weak shares and shares share one FIFO
  upload queue with no priority. A share carried whole holds the average
  count, not the weak shares actually pending. Weak shares are forwarded
  after validation whatever the share forwarding is, are never dropped (no
  `WEAK_DEPTH` or cap), and do not count in fork choice.
- **Compact coinbase:** the size is an estimate, as the format is not
  designed; the receiver is charged the whole share's validation time.
- **Relay before full validation:** the 1 ms header check is an estimate.
  Every share is valid in this model, so the cost of forwarding a share whose
  body later fails is not measured.
- **Peers:** the same small-world graph; a push to `k` peers splits the
  upload equally, with no cost per connection beyond each message's framing.
- **Uncle cap:** uncles stay within 3 shares of depth. Fork choice is by
  height and hash, as in the rest of this model, not by work with uncles
  counted, which this fork's nodes have done since 2026-10-30; so uncles
  never change which chain wins here. The header-sync bytes count the final
  chain and the uncles it names, not uncles named only by other uncles.
- **Run length and noise:** 1,000-node runs mine 8,640 share intervals,
  100-node runs a day. The limits come from one seed; only the combination
  was searched under two more.
- **The limit:** upstream gives "about 1,000 miners at 10-second shares",
  not a stale rate; reading it as 1,000 nodes at today's 4.23% is this
  study's choice.

Not measured here: weak shares carried as 32-byte hashes instead of whole,
counting weak work in fork choice and the validation it would add, the
dynamic share period, and uncles deeper than 3 shares.

## Limits study: upstream's practical objections

Upstream's reply to the scaling study (above): a pool always has an upper
bound, and five of the ideas have practical limits.

1. A share needs enough data to check how it paid out.
2. Relaying early probably won't move the needle much.
3. More connections are limited by link bandwidth: a 100 Mbit/s peer can't
   send a 250 kB share to more than 10 peers in under 250 ms.
4. A dynamic uncle cap could raise latency.
5. A faster share rate builds a fatter DAG and loads every node.

The scaling study guessed validation at 5 ms plus 20 ms per MB of share,
and let a node validate any number of shares at once. That hid objection 5.
This study measures validation in the node's own code, under the adopted
rules, and queues it. It then grows the network to 10,000 nodes.

### What a node pays to check a share

`validation_cost_at_bitcoin_cash_window_sizes` in `pplns_window.rs` (an
ignored test, run by hand in release mode) builds a 120,960 share chain
and times what a node does for each share it receives:
- walk the PPLNS window from the share's parent;
- build the expected coinbase outputs (the level draw, the hybrid split and
  the payout tree), then compare them.

A weak share pays only the build: its walk is cached by parent and paid block.
The build is the median of seven.

| Miners in the window | Payout level | Share | Paid by | Walk | Build | A share | A weak share |
|---:|---:|---|---|---:|---:|---:|---:|
| 1,000 | 5 | no tokens | 1,000 coinbase outputs | 6.1 ms | 0.3 ms | 6.4 ms | 0.3 ms |
| 10,000 | 5 | no tokens | 10,000 coinbase outputs | 34 ms | 3.8 ms | 38 ms | 3.8 ms |
| 100,000 | 5 | no tokens | 17,883 coinbase outputs (the draw) | 317 ms | 45 ms | 362 ms | 45 ms |
| 100,000 | 0 | no tokens | 29,400 coinbase outputs, 70,600 in the tree | 305 ms | 133 ms | 438 ms | 133 ms |
| 1,000 | 5 | tokens | 79 coinbase outputs, 921 in the tree | 6.1 ms | 1.2 ms | 7.4 ms | 1.2 ms |
| 10,000 | 5 | tokens | 79 coinbase outputs, 9,921 in the tree | 34 ms | 10.5 ms | 45 ms | 10.5 ms |
| 100,000 | 5 | tokens | 79 coinbase outputs, 17,804 in the tree | 317 ms | 59 ms | 376 ms | 59 ms |

These were measured on one core of a 2026 desktop. The scaling study's guess
matched at 1,000 miners, was 3 times too low at 10,000 and 15 times too low
at 100,000. The default payout level matters: at level 5 a 3.125 BCH block
pays at most 17,886 outputs, so 100,000 miners cost a third of what they
cost at level 0. The walk in the table above has no weak-share credits in
it. A window whose shares carry 32 weak shares each was measured
(`validation_cost_with_weak_share_credits`): the walk takes 14 ms for 1,800
shares and 5,100 payees, 174 ms with 50,100 payees, 211 ms for 18,000
shares and 51,000 payees, and 160 ms at the cap of 120,960 shares with
6,000 payees. Its time follows the number of payees, about 3.5 microseconds
each, not the number of credits.

The node validates up to one share per CPU at once (`validation_worker.rs`).
But the walk needs write access to the one shared window
(`Arc<RwLock<PplnsWindow>>`), so walks run one at a time.
`ValidationQueue::Serial` models this: a node validates one share or weak
share at a time, and later ones wait. A weak share costs the coinbase build
the first time a node sees its parent and template, and after that only the
coinbase rebuild and merkle check (the node's `WeakPayoutCache`): 0.08 ms
with 1,000 miners, 1.0 ms with 10,000 and 1.4 ms with 100,000 at the default
level (`validation_cost_of_a_cached_weak_share`, with a stand-in for the
private rebuild). The period sweep runs three cases: `each` weak share on a
template of its own, the worst case; `per node`, where the weak shares one
node's miners find share that node's template; and `one` template for the
whole pool.

### How it was run

`netsim sweep limits` (or one of `limits-coinbase`, `limits-relay-early`,
`limits-fanout`, `limits-uncles`, `limits-period`, `limits-network`) runs
each objection on 1,000 nodes, upstream's stated limit of about 1,000
miners, at the measured costs for payout level 5, one validation at a time,
on today's links: 50 Mbit/s median upload and 50 ms median latency. A run
mines for 8,640 share intervals; the share-period and network sweeps mine
for 1,080, since a thousand nodes tracking every weak share of a day at
1-second shares need more memory than a desktop has (the sweeps took 21 GB
before they were bounded; run those two with `--threads 4`). "Validation
busy" is the part of the run a node's validation worker had work. Above
100%, shares arrive faster than a node can check them, and the queue only
grows.

### 1. Enough data to check the payout

On Bitcoin Cash every node already rebuilds a share's payouts to check it:
they are a rule of the window at the share's parent, and the check compares
them output by output. A share sent without them leaves out nothing a peer
can't rebuild, and its check costs the same. Weak shares already travel this
way (`shares::weak`): the end-to-end runs of #91 and #98 checked thousands of
them. Only the node that finds a block needs the whole coinbase, and it
built it.

A token share is already small: its coinbase stops at 3,072 bytes and pays
everyone else through the payout tree, which a peer rebuilds from the same
window. It is 4,237 bytes on the wire whatever it pays.

| Scenario | Share bytes | Validation (ms) | Stale | To all (s) | P2Pool up, mean / max (Mbit/s) | Uncles per chain share | Validation busy, mean / max | Validation wait (ms) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| whole, 1,000 miners, 10 s | 36,389 | 6.4 | 4.26% | 0.56 | 0.202 / 0.378 | 0.045 | 0.06% / 0.06% | 0.0 |
| without payouts, 1,000 miners, 10 s | 1,429 | 6.4 | 2.58% | 0.29 | 8.4e-3 / 0.016 | 0.026 | 0.06% / 0.06% | 0.0 |
| token share, 1,000 miners, 10 s | 4,237 | 7.3 | 2.76% | 0.33 | 0.024 / 0.044 | 0.028 | 0.07% / 0.07% | 0.0 |
| whole, 10,000 miners, 10 s | 350,749 | 37.9 | 16.62% | 2.79 | 1.93 / 3.62 | 0.198 | 0.38% / 0.38% | 0.1 |
| without payouts, 10,000 miners, 10 s | 1,429 | 37.9 | 3.94% | 0.50 | 7.5e-3 / 0.015 | 0.041 | 0.38% / 0.38% | 0.1 |
| token share, 10,000 miners, 10 s | 4,237 | 44.6 | 4.41% | 0.57 | 0.021 / 0.041 | 0.046 | 0.45% / 0.45% | 0.1 |
| whole, 100,000 miners, 10 s | 626,131 | 361.5 | 32.62% | 6.77 | 3.21 / 6.17 | 0.469 | 3.62% / 3.62% | 5.3 |
| without payouts, 100,000 miners, 10 s | 1,429 | 361.5 | 16.34% | 2.51 | 6.2e-3 / 0.013 | 0.194 | 3.62% / 3.62% | 5.7 |
| token share, 100,000 miners, 10 s | 4,237 | 376.0 | 16.88% | 2.63 | 0.018 / 0.037 | 0.202 | 3.77% / 3.77% | 6.2 |
| whole, 1,000 miners, 2 s | 36,389 | 6.4 | 17.84% | 0.57 | 1.02 / 1.91 | 0.215 | 0.32% / 0.32% | 0.0 |
| without payouts, 1,000 miners, 2 s | 1,429 | 6.4 | 11.33% | 0.30 | 0.042 / 0.079 | 0.128 | 0.32% / 0.32% | 0.0 |
| token share, 1,000 miners, 2 s | 4,237 | 7.3 | 12.27% | 0.33 | 0.120 / 0.225 | 0.139 | 0.37% / 0.37% | 0.0 |
| whole, 10,000 miners, 2 s | 350,749 | 37.9 | 50.23% | 91.81 | 9.79 / 18.3 | 0.850 | 1.92% / 1.92% | 0.3 |
| without payouts, 10,000 miners, 2 s | 1,429 | 37.9 | 16.99% | 0.50 | 0.038 / 0.074 | 0.203 | 1.92% / 1.92% | 0.3 |
| token share, 10,000 miners, 2 s | 4,237 | 44.6 | 18.63% | 0.57 | 0.106 / 0.209 | 0.227 | 2.26% / 2.26% | 0.4 |
| whole, 100,000 miners, 2 s | 626,131 | 361.5 | 71.33% | 434.51 | 16.3 / 31.1 | 1.591 | 18.32% / 18.32% | 28.0 |
| without payouts, 100,000 miners, 2 s | 1,429 | 361.5 | 49.47% | 2.74 | 0.031 / 0.064 | 0.878 | 18.32% / 18.32% | 31.2 |
| token share, 100,000 miners, 2 s | 4,237 | 376.0 | 50.46% | 2.87 | 0.086 / 0.181 | 0.913 | 19.05% / 19.05% | 34.1 |

At 1,000 and 10,000 miners, shares without payouts keep the stale rate at
2.6% and 3.9% at 10-second shares, against 4.3% and 17% whole; token shares
come within half a point of that (2.8% and 4.4%). At 100,000 miners
validation itself (362 ms at each hop) keeps every form at 16% or more.

### 2. Relaying before full validation

| Scenario | Share bytes | Validation (ms) | Stale | To all (s) | P2Pool up, mean / max (Mbit/s) | Uncles per chain share | Validation busy, mean / max | Validation wait (ms) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| after validation, 1,000 miners, 10 s | 36,389 | 6.4 | 4.26% | 0.56 | 0.202 / 0.378 | 0.045 | 0.06% / 0.06% | 0.0 |
| after the header check, 1,000 miners, 10 s | 36,389 | 6.4 | 3.99% | 0.53 | 0.204 / 0.380 | 0.042 | 0.06% / 0.06% | 0.0 |
| after validation, 10,000 miners, 10 s | 350,749 | 37.9 | 16.62% | 2.79 | 1.93 / 3.62 | 0.198 | 0.38% / 0.38% | 0.1 |
| after the header check, 10,000 miners, 10 s | 350,749 | 37.9 | 15.54% | 2.58 | 1.97 / 3.66 | 0.182 | 0.38% / 0.38% | 0.1 |
| after validation, 100,000 miners, 10 s | 626,131 | 361.5 | 32.62% | 6.77 | 3.21 / 6.17 | 0.469 | 3.62% / 3.62% | 5.3 |
| after the header check, 100,000 miners, 10 s | 626,131 | 361.5 | 24.66% | 6.21 | 3.51 / 6.53 | 0.321 | 3.62% / 3.62% | 5.1 |
| after validation, 1,000 miners, 2 s | 36,389 | 6.4 | 17.84% | 0.57 | 1.02 / 1.91 | 0.215 | 0.32% / 0.32% | 0.0 |
| after the header check, 1,000 miners, 2 s | 36,389 | 6.4 | 17.17% | 0.54 | 1.03 / 1.92 | 0.206 | 0.32% / 0.32% | 0.0 |
| after validation, 10,000 miners, 2 s | 350,749 | 37.9 | 50.23% | 91.81 | 9.79 / 18.3 | 0.850 | 1.92% / 1.92% | 0.3 |
| after the header check, 10,000 miners, 2 s | 350,749 | 37.9 | 48.36% | 96.97 | 9.95 / 18.5 | 0.795 | 1.92% / 1.92% | 0.3 |
| after validation, 100,000 miners, 2 s | 626,131 | 361.5 | 71.33% | 434.51 | 16.3 / 31.1 | 1.591 | 18.32% / 18.32% | 28.0 |
| after the header check, 100,000 miners, 2 s | 626,131 | 361.5 | 63.35% | 527.18 | 17.8 / 33.0 | 1.213 | 18.32% / 18.32% | 26.9 |

Upstream is right: forwarding after the header check takes 0.3 to 1.9
points off the stale rate at 1,000 and 10,000 miners. It only matters when
validation is slow: 33% to 25% at 100,000 miners.

### 3. More connections against upload bandwidth

| Scenario | Share bytes | Validation (ms) | Stale | To all (s) | P2Pool up, mean / max (Mbit/s) | Uncles per chain share | Validation busy, mean / max | Validation wait (ms) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| whole, 10,000 miners, 20 Mbit/s up, 8 peers | 350,749 | 37.9 | 30.96% | 6.32 | 1.95 / 3.64 | 0.435 | 0.38% / 0.38% | 0.1 |
| whole, 10,000 miners, 20 Mbit/s up, 16 peers | 350,749 | 37.9 | 40.78% | 9.81 | 4.21 / 7.58 | 0.639 | 0.38% / 0.38% | 0.1 |
| whole, 10,000 miners, 20 Mbit/s up, 32 peers | 350,749 | 37.9 | 56.43% | 17.98 | 8.71 / 12.4 | 1.078 | 0.38% / 0.38% | 0.1 |
| whole, 10,000 miners, 20 Mbit/s up, 64 peers | 350,749 | 37.9 | 81.70% | 367.94 | 17.7 / 22.2 | 1.950 | 0.38% / 0.38% | 0.1 |
| without payouts, 10,000 miners, 20 Mbit/s up, 8 peers | 1,429 | 37.9 | 4.04% | 0.51 | 7.5e-3 / 0.015 | 0.042 | 0.38% / 0.38% | 0.1 |
| without payouts, 10,000 miners, 20 Mbit/s up, 16 peers | 1,429 | 37.9 | 3.43% | 0.40 | 0.017 / 0.031 | 0.036 | 0.38% / 0.38% | 0.1 |
| without payouts, 10,000 miners, 20 Mbit/s up, 32 peers | 1,429 | 37.9 | 3.17% | 0.35 | 0.035 / 0.051 | 0.033 | 0.38% / 0.38% | 0.1 |
| without payouts, 10,000 miners, 20 Mbit/s up, 64 peers | 1,429 | 37.9 | 3.12% | 0.36 | 0.075 / 0.095 | 0.032 | 0.38% / 0.38% | 0.1 |
| whole, 10,000 miners, 100 Mbit/s up, 8 peers | 350,749 | 37.9 | 10.94% | 1.59 | 1.90 / 3.59 | 0.122 | 0.38% / 0.38% | 0.1 |
| whole, 10,000 miners, 100 Mbit/s up, 16 peers | 350,749 | 37.9 | 13.34% | 2.09 | 4.15 / 7.51 | 0.153 | 0.38% / 0.38% | 0.1 |
| whole, 10,000 miners, 100 Mbit/s up, 32 peers | 350,749 | 37.9 | 19.25% | 3.05 | 8.64 / 12.3 | 0.235 | 0.38% / 0.38% | 0.1 |
| whole, 10,000 miners, 100 Mbit/s up, 64 peers | 350,749 | 37.9 | 28.50% | 5.64 | 17.7 / 22.1 | 0.388 | 0.38% / 0.38% | 0.1 |
| without payouts, 10,000 miners, 100 Mbit/s up, 8 peers | 1,429 | 37.9 | 3.86% | 0.49 | 7.5e-3 / 0.015 | 0.040 | 0.38% / 0.38% | 0.1 |
| without payouts, 10,000 miners, 100 Mbit/s up, 16 peers | 1,429 | 37.9 | 3.23% | 0.37 | 0.016 / 0.031 | 0.033 | 0.38% / 0.38% | 0.1 |
| without payouts, 10,000 miners, 100 Mbit/s up, 32 peers | 1,429 | 37.9 | 2.88% | 0.30 | 0.034 / 0.050 | 0.029 | 0.38% / 0.38% | 0.1 |
| without payouts, 10,000 miners, 100 Mbit/s up, 64 peers | 1,429 | 37.9 | 2.59% | 0.27 | 0.072 / 0.093 | 0.027 | 0.38% / 0.38% | 0.1 |

Upstream is right for whole shares: at 10,000 miners (a 350 kB share) more
peers raise the stale rate, from 31% at 8 peers to 82% at 64 on a 20 Mbit/s
upload, and from 11% to 29% on 100 Mbit/s. Only with shares sent without
payouts (1.4 kB) do more peers help, from 4.0% to 3.1% on 20 Mbit/s and 3.9%
to 2.6% on 100.

### 4. The uncle cap

| Scenario | Share bytes | Validation (ms) | Stale | To all (s) | P2Pool up, mean / max (Mbit/s) | Uncles per chain share | Validation busy, mean / max | Validation wait (ms) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| uncle cap 3, 10,000 miners, 10 s | 350,749 | 37.9 | 16.62% | 2.79 | 1.93 / 3.62 | 0.198 | 0.38% / 0.38% | 0.1 |
| uncle cap 6, 10,000 miners, 10 s | 350,749 | 37.9 | 16.62% | 2.79 | 1.93 / 3.62 | 0.198 | 0.38% / 0.38% | 0.1 |
| uncle cap 12, 10,000 miners, 10 s | 350,749 | 37.9 | 16.62% | 2.79 | 1.93 / 3.62 | 0.198 | 0.38% / 0.38% | 0.1 |
| uncle cap 3, 10,000 miners, 2 s | 350,749 | 37.9 | 50.23% | 91.81 | 9.79 / 18.3 | 0.850 | 1.92% / 1.92% | 0.3 |
| uncle cap 6, 10,000 miners, 2 s | 350,749 | 37.9 | 50.27% | 91.82 | 9.79 / 18.3 | 0.853 | 1.92% / 1.92% | 0.3 |
| uncle cap 12, 10,000 miners, 2 s | 350,749 | 37.9 | 50.27% | 91.82 | 9.79 / 18.3 | 0.853 | 1.92% / 1.92% | 0.3 |
| uncle cap 3, 10,000 miners, 1 s | 350,749 | 37.9 | 67.34% | 313.73 | 19.3 / 36.1 | 1.350 | 3.79% / 3.79% | 0.5 |
| uncle cap 6, 10,000 miners, 1 s | 350,749 | 37.9 | 67.39% | 313.73 | 19.3 / 36.1 | 1.400 | 3.79% / 3.79% | 0.5 |
| uncle cap 12, 10,000 miners, 1 s | 350,749 | 37.9 | 67.34% | 313.73 | 19.3 / 36.1 | 1.400 | 3.79% / 3.79% | 0.5 |

Caps of 6 and 12 change neither the stale rate nor propagation: at
10-second shares a chain share names 0.2 uncles, so a cap of 3 rarely binds,
and at 1-second shares 1.35 to 1.40. In this model the cap only decides
which stale shares are paid. It adds no latency, because every uncle was
validated when it arrived and naming one costs 32 bytes; deeper header sync
and the payout of deep forks are not modelled. Not worth changing on this
evidence.

### 5. Share period and each node's load

| Scenario | Share bytes | Validation (ms) | Stale | To all (s) | P2Pool up, mean / max (Mbit/s) | Uncles per chain share | Validation busy, mean / max | Validation wait (ms) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 1,000 miners, no weak shares, 10 s | 36,389 | 6.4 | 4.20% | 0.56 | 0.211 / 0.395 | 0.045 | 0.07% / 0.07% | 0.0 |
| 1,000 miners, no weak shares, 5 s | 36,389 | 6.4 | 7.82% | 0.56 | 0.424 / 0.793 | 0.084 | 0.13% / 0.13% | 0.0 |
| 1,000 miners, no weak shares, 2 s | 36,389 | 6.4 | 18.36% | 0.57 | 1.05 / 1.96 | 0.222 | 0.33% / 0.33% | 0.0 |
| 1,000 miners, no weak shares, 1 s | 36,389 | 6.4 | 31.18% | 0.57 | 2.10 / 3.92 | 0.442 | 0.67% / 0.67% | 0.0 |
| 1,000 miners, 32 weak shares a share, each, 10 s | 36,389 | 6.4 | 4.20% | 0.56 | 0.392 / 0.727 | 0.045 | 0.15% / 0.15% | 0.0 |
| 1,000 miners, 32 weak shares a share, per node, 10 s | 36,389 | 6.4 | 4.20% | 0.56 | 0.392 / 0.727 | 0.045 | 0.15% / 0.15% | 0.0 |
| 1,000 miners, 32 weak shares a share, one, 10 s | 36,389 | 6.4 | 4.20% | 0.56 | 0.392 / 0.727 | 0.045 | 0.09% / 0.09% | 0.0 |
| 1,000 miners, 32 weak shares a share, each, 5 s | 36,389 | 6.4 | 7.82% | 0.56 | 0.786 / 1.46 | 0.084 | 0.30% / 0.30% | 0.0 |
| 1,000 miners, 32 weak shares a share, per node, 5 s | 36,389 | 6.4 | 7.82% | 0.56 | 0.786 / 1.46 | 0.084 | 0.30% / 0.30% | 0.0 |
| 1,000 miners, 32 weak shares a share, one, 5 s | 36,389 | 6.4 | 7.82% | 0.56 | 0.786 / 1.46 | 0.084 | 0.19% / 0.19% | 0.0 |
| 1,000 miners, 32 weak shares a share, each, 2 s | 36,389 | 6.4 | 18.36% | 0.57 | 1.95 / 3.62 | 0.222 | 0.75% / 0.75% | 0.0 |
| 1,000 miners, 32 weak shares a share, per node, 2 s | 36,389 | 6.4 | 18.36% | 0.57 | 1.95 / 3.62 | 0.222 | 0.74% / 0.74% | 0.0 |
| 1,000 miners, 32 weak shares a share, one, 2 s | 36,389 | 6.4 | 18.36% | 0.57 | 1.95 / 3.62 | 0.222 | 0.46% / 0.46% | 0.0 |
| 1,000 miners, 32 weak shares a share, each, 1 s | 36,389 | 6.4 | 31.18% | 0.57 | 3.90 / 7.24 | 0.443 | 1.50% / 1.50% | 0.0 |
| 1,000 miners, 32 weak shares a share, per node, 1 s | 36,389 | 6.4 | 31.18% | 0.57 | 3.90 / 7.24 | 0.443 | 1.48% / 1.48% | 0.0 |
| 1,000 miners, 32 weak shares a share, one, 1 s | 36,389 | 6.4 | 31.18% | 0.57 | 3.91 / 7.24 | 0.443 | 0.93% / 0.93% | 0.0 |
| 10,000 miners, no weak shares, 10 s | 350,749 | 37.9 | 16.88% | 2.80 | 2.02 / 3.78 | 0.203 | 0.40% / 0.40% | 0.1 |
| 10,000 miners, no weak shares, 5 s | 350,749 | 37.9 | 28.09% | 2.88 | 4.06 / 7.61 | 0.381 | 0.80% / 0.80% | 0.1 |
| 10,000 miners, no weak shares, 2 s | 350,749 | 37.9 | 50.41% | 12.21 | 10.1 / 18.8 | 0.883 | 1.97% / 1.97% | 0.3 |
| 10,000 miners, no weak shares, 1 s | 350,749 | 37.9 | 67.83% | 45.26 | 20.1 / 37.6 | 1.334 | 3.94% / 3.94% | 0.5 |
| 10,000 miners, 32 weak shares a share, each, 10 s | 350,749 | 37.9 | 16.88% | 2.81 | 2.20 / 4.11 | 0.203 | 1.60% / 1.60% | 0.1 |
| 10,000 miners, 32 weak shares a share, per node, 10 s | 350,749 | 37.9 | 16.88% | 2.81 | 2.20 / 4.11 | 0.203 | 1.58% / 1.58% | 0.1 |
| 10,000 miners, 32 weak shares a share, one, 10 s | 350,749 | 37.9 | 16.88% | 2.81 | 2.20 / 4.11 | 0.203 | 0.76% / 0.76% | 0.1 |
| 10,000 miners, 32 weak shares a share, each, 5 s | 350,749 | 37.9 | 28.09% | 2.89 | 4.42 / 8.26 | 0.383 | 3.20% / 3.20% | 0.3 |
| 10,000 miners, 32 weak shares a share, per node, 5 s | 350,749 | 37.9 | 28.09% | 2.89 | 4.42 / 8.26 | 0.383 | 3.16% / 3.16% | 0.3 |
| 10,000 miners, 32 weak shares a share, one, 5 s | 350,749 | 37.9 | 28.09% | 2.89 | 4.42 / 8.27 | 0.383 | 1.52% / 1.52% | 0.2 |
| 10,000 miners, 32 weak shares a share, each, 2 s | 350,749 | 37.9 | 50.41% | 15.60 | 10.9 / 20.5 | 0.856 | 7.98% / 7.98% | 0.9 |
| 10,000 miners, 32 weak shares a share, per node, 2 s | 350,749 | 37.9 | 50.50% | 15.61 | 10.9 / 20.4 | 0.861 | 7.85% / 7.85% | 0.9 |
| 10,000 miners, 32 weak shares a share, one, 2 s | 350,749 | 37.9 | 50.41% | 15.62 | 11.0 / 20.5 | 0.863 | 3.81% / 3.81% | 0.7 |
| 10,000 miners, 32 weak shares a share, each, 1 s | 350,749 | 37.9 | 68.73% | 53.75 | 21.9 / 40.9 | 1.327 | 15.96% / 15.96% | 3.0 |
| 10,000 miners, 32 weak shares a share, per node, 1 s | 350,749 | 37.9 | 68.64% | 53.65 | 21.9 / 40.9 | 1.329 | 15.60% / 15.61% | 3.0 |
| 10,000 miners, 32 weak shares a share, one, 1 s | 350,749 | 37.9 | 68.64% | 53.74 | 21.9 / 40.9 | 1.334 | 7.88% / 7.88% | 2.0 |
| 100,000 miners, no weak shares, 10 s | 626,131 | 361.5 | 33.21% | 6.81 | 3.35 / 6.45 | 0.486 | 3.79% / 3.79% | 5.2 |
| 100,000 miners, no weak shares, 5 s | 626,131 | 361.5 | 48.89% | 10.25 | 6.74 / 13.0 | 0.832 | 7.61% / 7.61% | 10.6 |
| 100,000 miners, no weak shares, 2 s | 626,131 | 361.5 | 71.83% | 58.18 | 16.7 / 32.0 | 1.631 | 18.81% / 18.81% | 27.0 |
| 100,000 miners, no weak shares, 1 s | 626,131 | 361.5 | 84.10% | 230.18 | 33.9 / 64.4 | 2.123 | 37.59% / 37.59% | 53.6 |
| 100,000 miners, 32 weak shares a share, each, 10 s | 626,131 | 361.5 | 33.57% | 6.83 | 3.51 / 6.74 | 0.493 | 18.16% / 18.16% | 11.3 |
| 100,000 miners, 32 weak shares a share, per node, 10 s | 626,131 | 361.5 | 33.57% | 6.83 | 3.51 / 6.74 | 0.493 | 17.78% / 17.78% | 11.2 |
| 100,000 miners, 32 weak shares a share, one, 10 s | 626,131 | 361.5 | 33.21% | 6.81 | 3.53 / 6.78 | 0.485 | 4.62% / 4.62% | 7.0 |
| 100,000 miners, 32 weak shares a share, each, 5 s | 626,131 | 361.5 | 49.24% | 11.28 | 7.04 / 13.5 | 0.844 | 36.35% / 36.35% | 29.7 |
| 100,000 miners, 32 weak shares a share, per node, 5 s | 626,131 | 361.5 | 49.07% | 11.26 | 7.04 / 13.5 | 0.833 | 35.58% / 35.60% | 29.2 |
| 100,000 miners, 32 weak shares a share, one, 5 s | 626,131 | 361.5 | 48.89% | 11.37 | 7.09 / 13.6 | 0.826 | 9.26% / 9.26% | 15.4 |
| 100,000 miners, 32 weak shares a share, each, 2 s | 626,131 | 361.5 | 74.44% | 62.23 | 17.2 / 33.2 | 1.778 | 90.66% / 90.66% | 243.8 |
| 100,000 miners, 32 weak shares a share, per node, 2 s | 626,131 | 361.5 | 73.54% | 62.49 | 17.2 / 33.2 | 1.705 | 87.79% / 87.93% | 218.7 |
| 100,000 miners, 32 weak shares a share, one, 2 s | 626,131 | 361.5 | 71.92% | 64.69 | 17.6 / 33.6 | 1.616 | 24.80% / 24.81% | 47.8 |
| 100,000 miners, 32 weak shares a share, each, 1 s | 626,131 | 361.5 | 98.74% | 590.34 | 23.4 / 44.4 | 2.214 | 181.28% / 181.28% | 104067.8 |
| 100,000 miners, 32 weak shares a share, per node, 1 s | 626,131 | 361.5 | 98.65% | 481.67 | 23.8 / 44.9 | 2.133 | 164.33% / 164.90% | 80995.3 |
| 100,000 miners, 32 weak shares a share, one, 1 s | 626,131 | 361.5 | 83.92% | 261.59 | 35.6 / 67.4 | 1.983 | 53.46% / 53.46% | 122.5 |
| 10,000 miners, nodes 5x slower, 32 weak shares a share, each, 10 s | 350,749 | 189.5 | 22.05% | 3.81 | 2.06 / 3.95 | 0.280 | 7.99% / 7.99% | 2.4 |
| 10,000 miners, nodes 5x slower, 32 weak shares a share, per node, 10 s | 350,749 | 189.5 | 22.05% | 3.81 | 2.06 / 3.95 | 0.280 | 7.88% / 7.88% | 2.4 |
| 10,000 miners, nodes 5x slower, 32 weak shares a share, one, 10 s | 350,749 | 189.5 | 22.05% | 3.81 | 2.07 / 3.96 | 0.280 | 3.78% / 3.78% | 1.8 |
| 10,000 miners, nodes 5x slower, 32 weak shares a share, each, 5 s | 350,749 | 189.5 | 35.29% | 3.89 | 4.14 / 7.95 | 0.520 | 16.00% / 16.00% | 5.5 |
| 10,000 miners, nodes 5x slower, 32 weak shares a share, per node, 5 s | 350,749 | 189.5 | 35.29% | 3.89 | 4.14 / 7.95 | 0.520 | 15.77% / 15.77% | 5.5 |
| 10,000 miners, nodes 5x slower, 32 weak shares a share, one, 5 s | 350,749 | 189.5 | 35.29% | 3.88 | 4.16 / 7.97 | 0.520 | 7.57% / 7.57% | 4.1 |
| 10,000 miners, nodes 5x slower, 32 weak shares a share, each, 2 s | 350,749 | 189.5 | 58.87% | 13.85 | 10.2 / 19.6 | 1.187 | 39.90% / 39.90% | 20.4 |
| 10,000 miners, nodes 5x slower, 32 weak shares a share, per node, 2 s | 350,749 | 189.5 | 58.96% | 13.88 | 10.2 / 19.6 | 1.198 | 39.21% / 39.22% | 20.3 |
| 10,000 miners, nodes 5x slower, 32 weak shares a share, one, 2 s | 350,749 | 189.5 | 58.51% | 14.05 | 10.3 / 19.7 | 1.159 | 18.98% / 18.98% | 12.6 |
| 10,000 miners, nodes 5x slower, 32 weak shares a share, each, 1 s | 350,749 | 189.5 | 75.92% | 46.71 | 20.4 / 39.1 | 1.827 | 79.78% / 79.78% | 87.4 |
| 10,000 miners, nodes 5x slower, 32 weak shares a share, per node, 1 s | 350,749 | 189.5 | 76.01% | 46.83 | 20.4 / 39.0 | 1.811 | 77.91% / 77.93% | 83.3 |
| 10,000 miners, nodes 5x slower, 32 weak shares a share, one, 1 s | 350,749 | 189.5 | 74.75% | 47.84 | 20.7 / 39.4 | 1.764 | 39.03% / 39.04% | 33.7 |
| 100,000 miners, token shares, 32 weak shares a share, each, 10 s | 4,237 | 376.0 | 16.52% | 2.67 | 0.164 / 0.324 | 0.198 | 22.94% / 22.94% | 13.1 |
| 100,000 miners, token shares, 32 weak shares a share, per node, 10 s | 4,237 | 376.0 | 16.52% | 2.66 | 0.165 / 0.325 | 0.198 | 22.43% / 22.43% | 13.0 |
| 100,000 miners, token shares, 32 weak shares a share, one, 10 s | 4,237 | 376.0 | 16.16% | 2.62 | 0.195 / 0.364 | 0.193 | 4.52% / 4.52% | 7.4 |
| 100,000 miners, token shares, 32 weak shares a share, each, 5 s | 4,237 | 376.0 | 29.78% | 2.80 | 0.318 / 0.631 | 0.415 | 45.91% / 45.91% | 33.1 |
| 100,000 miners, token shares, 32 weak shares a share, per node, 5 s | 4,237 | 376.0 | 29.78% | 2.80 | 0.319 / 0.633 | 0.415 | 44.91% / 44.91% | 32.6 |
| 100,000 miners, token shares, 32 weak shares a share, one, 5 s | 4,237 | 376.0 | 28.62% | 2.69 | 0.384 / 0.718 | 0.394 | 9.02% / 9.02% | 15.3 |
| 100,000 miners, token shares, 32 weak shares a share, each, 2 s | 4,237 | 376.0 | 96.22% | 203.04 | 0.594 / 1.04 | 2.465 | 114.56% / 114.56% | 33945.4 |
| 100,000 miners, token shares, 32 weak shares a share, per node, 2 s | 4,237 | 376.0 | 94.60% | 112.43 | 0.596 / 1.03 | 2.066 | 107.48% / 107.73% | 18259.5 |
| 100,000 miners, token shares, 32 weak shares a share, one, 2 s | 4,237 | 376.0 | 50.86% | 2.88 | 0.911 / 1.71 | 0.947 | 21.98% / 21.98% | 40.3 |
| 100,000 miners, token shares, 32 weak shares a share, each, 1 s | 4,237 | 376.0 | 99.01% | 957.45 | 1.18 / 2.06 | 2.333 | 229.08% / 229.08% | 187151.3 |
| 100,000 miners, token shares, 32 weak shares a share, per node, 1 s | 4,237 | 376.0 | 98.83% | 801.10 | 1.18 / 2.10 | 2.077 | 205.80% / 206.70% | 153001.7 |
| 100,000 miners, token shares, 32 weak shares a share, one, 1 s | 4,237 | 376.0 | 68.91% | 3.31 | 1.68 / 3.20 | 1.480 | 43.50% / 43.50% | 92.8 |
| 100,000 miners, all at level 0, 32 weak shares a share, each, 10 s | 1,028,473 | 437.6 | 42.50% | 13.88 | 5.74 / 11.0 | 0.686 | 47.13% / 47.13% | 57.4 |
| 100,000 miners, all at level 0, 32 weak shares a share, per node, 10 s | 1,028,473 | 437.6 | 42.32% | 13.81 | 5.74 / 10.9 | 0.686 | 45.89% / 45.95% | 55.7 |
| 100,000 miners, all at level 0, 32 weak shares a share, one, 10 s | 1,028,473 | 437.6 | 41.79% | 13.74 | 5.80 / 11.0 | 0.666 | 6.49% / 6.49% | 13.5 |
| 100,000 miners, all at level 0, 32 weak shares a share, each, 5 s | 1,028,473 | 437.6 | 63.91% | 50.88 | 11.3 / 21.6 | 1.331 | 94.31% / 94.31% | 436.7 |
| 100,000 miners, all at level 0, 32 weak shares a share, per node, 5 s | 1,028,473 | 437.6 | 63.38% | 51.27 | 11.4 / 21.7 | 1.348 | 91.53% / 91.66% | 352.0 |
| 100,000 miners, all at level 0, 32 weak shares a share, one, 5 s | 1,028,473 | 437.6 | 59.56% | 52.95 | 11.7 / 22.1 | 1.176 | 14.04% / 14.04% | 31.4 |
| 100,000 miners, all at level 0, 32 weak shares a share, each, 2 s | 1,028,473 | 437.6 | 99.01% | 2013.15 | 18.0 / 30.8 | 2.182 | 235.50% / 235.50% | 393030.4 |
| 100,000 miners, all at level 0, 32 weak shares a share, per node, 2 s | 1,028,473 | 437.6 | 98.92% | 1656.77 | 18.1 / 32.9 | 2.167 | 209.18% / 210.62% | 313671.5 |
| 100,000 miners, all at level 0, 32 weak shares a share, one, 2 s | 1,028,473 | 437.6 | 79.48% | 244.34 | 29.1 / 54.9 | 1.767 | 43.12% / 43.13% | 115.0 |
| 100,000 miners, all at level 0, 32 weak shares a share, each, 1 s | 1,028,473 | 437.6 | 99.37% | 2908.26 | 35.7 / 61.9 | 2.143 | 470.97% / 470.97% | 636793.3 |
| 100,000 miners, all at level 0, 32 weak shares a share, per node, 1 s | 1,028,473 | 437.6 | 99.37% | 2449.10 | 35.9 / 65.3 | 1.714 | 406.39% / 409.17% | 523496.1 |
| 100,000 miners, all at level 0, 32 weak shares a share, one, 1 s | 1,028,473 | 437.6 | 89.67% | 754.48 | 58.8 / 110 | 2.214 | 91.37% / 91.41% | 369.2 |

Upstream is right, and the queue shows where:
- At 1,000 miners validation stays under 2% of a node's time even at
  1-second shares with 32 weak shares a share. The limit is propagation:
  4.2% stale at 10-second shares, 31% at 1-second.
- At 10,000 miners whole shares are already 17% stale at 10-second shares.
  Weak shares take 16% of a node's time at 1-second shares, and 78% on a
  node five times slower; half that when the pool mines one template.
- At 100,000 miners at the default level, with a template a node, a node is
  18% busy at 10-second shares, 36% at 5-second and 88% at 2-second; at
  1-second shares the queue grows without end. Token shares cost a little
  more (22%, 45%, then overflow at 2-second), and every miner at level 0
  much more: 46% at 10-second shares, 92% at 5-second, overflow at 2-second.
- The node's payout cache decides how much of that is real. With a template
  a node, as in this pool of 1,000 equal nodes, the load is within a few
  points of the worst case, because the 32 weak shares of a share come from
  about 32 different nodes. With one template for the whole pool a node
  with 100,000 miners is 5% busy at 10-second shares and 53% at 1-second
  (44% on token shares, 91% at level 0), and no share period overflows.

So a faster share period is bounded by propagation for small pools and by
validation, mostly of weak shares mined on different templates, for pools
paying tens of thousands of miners. At the adopted 10-second period and
default level, 100,000 miners fit with room; what does not fit is 100,000
miners at level 0 on shares faster than 5 seconds, unless the pool's nodes
build the same template.
### 6. A bigger network

| Scenario | Share bytes | Validation (ms) | Stale | To all (s) | P2Pool up, mean / max (Mbit/s) | Uncles per chain share | Validation busy, mean / max | Validation wait (ms) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 1,000 nodes, whole, 10,000 miners, no weak shares | 350,749 | 37.9 | 16.88% | 2.80 | 2.02 / 3.78 | 0.203 | 0.40% / 0.40% | 0.1 |
| 1,000 nodes, whole, 10,000 miners, 32 weak shares a share | 350,749 | 37.9 | 16.88% | 2.81 | 2.20 / 4.11 | 0.203 | 1.60% / 1.60% | 0.1 |
| 1,000 nodes, without payouts, 10,000 miners, no weak shares | 1,429 | 37.9 | 4.11% | 0.50 | 7.9e-3 / 0.015 | 0.044 | 0.40% / 0.40% | 0.1 |
| 1,000 nodes, without payouts, 10,000 miners, 32 weak shares a share | 1,429 | 37.9 | 4.11% | 0.50 | 0.186 / 0.343 | 0.044 | 1.60% / 1.60% | 0.1 |
| 3,000 nodes, whole, 10,000 miners, no weak shares | 350,749 | 37.9 | 17.23% | 3.29 | 2.02 / 4.09 | 0.202 | 0.40% / 0.40% | 0.0 |
| 3,000 nodes, whole, 10,000 miners, 32 weak shares a share | 350,749 | 37.9 | 17.23% | 3.29 | 2.20 / 4.44 | 0.202 | 1.60% / 1.60% | 0.1 |
| 3,000 nodes, without payouts, 10,000 miners, no weak shares | 1,429 | 37.9 | 4.55% | 0.56 | 7.9e-3 / 0.016 | 0.049 | 0.40% / 0.40% | 0.1 |
| 3,000 nodes, without payouts, 10,000 miners, 32 weak shares a share | 1,429 | 37.9 | 4.55% | 0.56 | 0.186 / 0.371 | 0.049 | 1.60% / 1.60% | 0.1 |
| 10,000 nodes, whole, 10,000 miners, no weak shares | 350,749 | 37.9 | 19.73% | 4.40 | 2.02 / 4.37 | 0.242 | 0.40% / 0.40% | 0.1 |
| 10,000 nodes, without payouts, 10,000 miners, no weak shares | 1,429 | 37.9 | 4.64% | 0.63 | 7.8e-3 / 0.018 | 0.050 | 0.40% / 0.40% | 0.1 |

Ten times the nodes cost little: from 1,000 to 10,000 nodes the stale rate
of shares sent without payouts goes from 4.1% to 4.6%, and of whole shares
from 16.9% to 19.7%. Hops grow with the logarithm of the node count, so the
share's size and its validation at each hop matter far more than the size
of the network. Weak shares do not change the stale rate at 1,000 or 3,000
nodes; they add 0.2 Mbit/s of upload a node.

The window walk with 32 weak shares on each share was measured
(`validation_cost_with_weak_share_credits`): 14 ms for 1,800 shares and
5,100 payees, 174 ms with 50,100 payees, 211 ms for 18,000 shares and
51,000 payees, and 160 ms at the cap of 120,960 shares with 6,000 payees.
Its time follows the number of payees, about 3.5 microseconds each, not
the number of credits.

### What this study did not do

- It did not run real nodes at these sizes: the costs are measured, the
  network is modelled. A 12-node swarm with 1,200 miners is in the pull
  request that added this study.
- It did not measure deep forks, header sync of a wide DAG, or uncles
  deeper than 3.
- Validation was measured on one desktop; a small node is taken as 5 times
  slower, not measured.

## Code

| What | Where |
|---|---|
| Constants and their sources | `p2poolv2_netsim/src/constants.rs` |
| Coinbase layouts, share sizes, limits | `p2poolv2_netsim/src/sizes.rs` |
| Window and payees | `p2poolv2_netsim/src/payees.rs` |
| Small-world graph | `p2poolv2_netsim/src/topology.rs` |
| Event simulation | `p2poolv2_netsim/src/engine.rs` |
| BCHN, template and Stratum traffic | `p2poolv2_netsim/src/traffic.rs` |
| Scenarios, sweeps, reports | `scenario.rs`, `sweep.rs`, `report.rs` |
| The scaling study's ideas, grid and limit search | `p2poolv2_netsim/src/scaling.rs` |
| The limits study: upstream's objections at measured validation costs | `p2poolv2_netsim/src/limits.rs` |
| What checking a share costs a node (an ignored timing test) | `p2poolv2_lib/src/accounting/payout/sharechain_pplns/pplns_window.rs`, `validation_cost_at_bitcoin_cash_window_sizes` |
| Command line | `p2poolv2_netsim/src/bin/netsim.rs` |

The crate is new code for the Bitcoin Cash fork, under AGPL-3.0-or-later
(decision 13). Upstream's ns-3 repository has no licence file, so its idea is
credited here and in the crate's README, and none of its code is used.
