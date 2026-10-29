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
  10,000 and 25% at 29,402 (a 1 MB coinbase; 1% orphaned). With 1 MB shares
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
cargo netsim sweep all                   # every sweep below
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

### Payees and the coinbase layout

| Scenario | Payees | Coinbase bytes | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | BCHN relay (Mbit/s) | Templates (Mbit/s) | Stratum per miner (kbit/s) | Limits broken |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| no-tokens, 10 payees | 10 | 661 | 1,807 | 1.98% | 0.00% | 0.17 | 0.20 | 0.011 / 0.017 | 0.766 | 76.0 | 4.49 | none |
| no-tokens, 100 payees | 100 | 3,721 | 4,947 | 2.08% | 0.00% | 0.19 | 0.22 | 0.028 / 0.044 | 0.766 | 76.0 | 14.7 | none |
| no-tokens, 1,000 payees | 1,000 | 34,323 | 36,389 | 3.08% | 0.01% | 0.33 | 0.38 | 0.202 / 0.320 | 0.766 | 76.0 | 116 | none |
| no-tokens, 6,010 payees | 6,010 | 204,663 | 211,369 | 7.81% | 0.02% | 0.98 | 1.12 | 1.18 / 1.86 | 0.766 | 76.0 | 681 | SV2 extended 64 KiB field |
| no-tokens, 10,000 payees | 10,000 | 340,323 | 350,749 | 10.92% | 0.06% | 1.51 | 1.72 | 1.96 / 3.08 | 0.766 | 76.0 | 1,131 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, 29,402 payees | 29,402 | 999,991 | 1,028,497 | 24.51% | 1.10% | 4.47 | 5.03 | 5.76 / 9.02 | 0.766 | 76.0 | 3,317 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, 100,000 payees | 100,000 | 999,967 | 1,028,473 | 24.51% | 1.11% | 4.47 | 5.03 | 5.76 / 9.02 | 0.766 | 76.0 | 3,317 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, 1,000,000 payees | 1,000,000 | 999,967 | 1,028,473 | 24.51% | 1.11% | 4.47 | 5.03 | 5.76 / 9.02 | 0.766 | 76.0 | 3,317 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| tokens-3072, 100 payees | 100 | 3,051 | 4,237 | 2.07% | 0.00% | 0.19 | 0.22 | 0.024 / 0.038 | 0.766 | 76.0 | 12.5 | none |
| tokens-3072, 10,000 payees | 10,000 | 3,051 | 4,237 | 2.07% | 0.00% | 0.19 | 0.22 | 0.024 / 0.038 | 0.766 | 76.0 | 12.5 | none |
| tokens-3072, 1,000,000 payees | 1,000,000 | 3,051 | 4,237 | 2.07% | 0.00% | 0.19 | 0.22 | 0.024 / 0.038 | 0.766 | 76.0 | 12.5 | none |
| cap-11, 1,000,000 payees | 1,000,000 | 782 | 1,928 | 2.00% | 0.00% | 0.17 | 0.20 | 0.011 / 0.018 | 0.766 | 76.0 | 4.95 | none |
| uncapped, 100,000 payees | 100,000 | 3,400,325 | 3,494,591 | 51.73% | 16.87% | never | never | 19.5 / 30.7 | 0.766 | 76.0 | 11,275 | BCH 1 MB transaction, hybrid 1,204,800-byte share body, 1.25 MiB P2P message, pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |

Each payee paid directly adds 34 bytes to every share, and pushing whole
shares to every peer multiplies that: at 6,010 payees the busiest node
uploads 1.9 Mbit/s and 7.8% of shares go stale; at 29,402 (a 1 MB coinbase)
a quarter do and 1% are orphaned. Past 29,402 the hybrid rule's tree keeps the
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
| no-tokens, 6,010 payees, full relay | 6,010 | 204,663 | 211,369 | 7.81% | 0.02% | 0.98 | 1.12 | 1.18 / 1.86 | 0.766 | 76.0 | 681 | SV2 extended 64 KiB field |
| no-tokens, 29,402 payees, full relay | 29,402 | 999,991 | 1,028,497 | 24.51% | 1.10% | 4.47 | 5.03 | 5.76 / 9.02 | 0.766 | 76.0 | 3,317 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, 1,000 payees, without-payouts relay | 1,000 | 34,323 | 1,429 | 1.98% | 0.00% | 0.17 | 0.20 | 8.4e-3 / 0.013 | 0.766 | 76.0 | 116 | none |
| no-tokens, 6,010 payees, without-payouts relay | 6,010 | 204,663 | 1,429 | 2.10% | 0.00% | 0.19 | 0.22 | 8.3e-3 / 0.013 | 0.766 | 76.0 | 681 | SV2 extended 64 KiB field |
| no-tokens, 29,402 payees, without-payouts relay | 29,402 | 999,991 | 1,429 | 2.65% | 0.01% | 0.26 | 0.30 | 7.8e-3 / 0.013 | 0.766 | 76.0 | 3,317 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |

Without the payouts a share is about 1.4 KB at any payee count, and stale
shares stay between 2.0% and 2.7%: the share chain no longer cares how many
miners are paid directly. The remaining growth is validation: rebuilding a
1 MB coinbase is taken to cost 20 ms. The coinbase itself still reaches the
miners in their Stratum jobs (below) and BCH in the found block.

### Pool size: the window bounds the payees

A million equally sized miners, for pools from 0.1% to 30% of BCH:

| Scenario | Payees | Coinbase bytes | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | BCHN relay (Mbit/s) | Templates (Mbit/s) | Stratum per miner (kbit/s) | Limits broken |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| no-tokens, pool 0.1% of BCH | 113,932 | 999,967 | 1,028,473 | 24.51% | 1.11% | 4.47 | 5.03 | 5.76 / 9.02 | 0.766 | 76.0 | 3,317 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, pool 0.3% of BCH | 58,236 | 999,967 | 1,028,473 | 24.51% | 1.11% | 4.47 | 5.03 | 5.76 / 9.02 | 0.766 | 76.0 | 3,317 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, pool 1% of BCH | 17,840 | 606,883 | 624,629 | 16.89% | 0.27% | 2.58 | 2.93 | 3.49 / 5.50 | 0.766 | 76.0 | 2,014 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, pool 3% of BCH | 5,983 | 203,745 | 210,451 | 7.77% | 0.03% | 0.98 | 1.12 | 1.18 / 1.85 | 0.766 | 76.0 | 678 | SV2 extended 64 KiB field |
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
| 1 GB blocks, mixed links | 1,001 | 34,357 | 36,583 | 5.77% | 0.96% | 1.62 | 10.86 | 0.204 / 0.321 | 24.0 | 2,375 | 117 | BCHN relay over the link on 14 of 100 nodes |
| 2 GB blocks, mixed links | 1,001 | 34,357 | 36,615 | 8.09% | 1.79% | 3.53 | 173.99 | 0.203 / 0.324 | 47.9 | 4,750 | 117 | BCHN relay over the link on 52 of 100 nodes |
| 2 MB blocks, data-centre links | 1,001 | 34,357 | 36,295 | 1.62% | 0.01% | 0.12 | 0.14 | 0.197 / 0.317 | 0.048 | 4.75 | 116 | none |
| 32 MB blocks, data-centre links | 1,001 | 34,357 | 36,423 | 1.62% | 0.01% | 0.12 | 0.14 | 0.198 / 0.319 | 0.766 | 76.0 | 116 | none |
| 256 MB blocks, data-centre links | 1,001 | 34,357 | 36,519 | 1.62% | 0.01% | 0.12 | 0.14 | 0.198 / 0.319 | 6.13 | 608 | 117 | none |
| 1 GB blocks, data-centre links | 1,001 | 34,357 | 36,583 | 1.62% | 0.01% | 0.12 | 0.14 | 0.199 / 0.320 | 24.0 | 2,375 | 117 | none |
| 2 GB blocks, data-centre links | 1,001 | 34,357 | 36,615 | 1.62% | 0.01% | 0.12 | 0.14 | 0.199 / 0.320 | 47.9 | 4,750 | 117 | none |

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
| tokens-3072, 1,000 payees, upload 5 Mbit/s | 1,000 | 3,051 | 4,237 | 3.97% | 0.03% | 0.44 | 0.50 | 0.024 / 0.038 | 0.766 | 76.0 | 12.5 | none |
| tokens-3072, 1,000 payees, upload 20 Mbit/s | 1,000 | 3,051 | 4,237 | 2.34% | 0.00% | 0.22 | 0.26 | 0.024 / 0.038 | 0.766 | 76.0 | 12.5 | none |
| tokens-3072, 1,000 payees, upload 50 Mbit/s | 1,000 | 3,051 | 4,237 | 2.07% | 0.00% | 0.19 | 0.22 | 0.024 / 0.038 | 0.766 | 76.0 | 12.5 | none |
| tokens-3072, 1,000 payees, upload 200 Mbit/s | 1,000 | 3,051 | 4,237 | 1.93% | 0.00% | 0.17 | 0.20 | 0.024 / 0.038 | 0.766 | 76.0 | 12.5 | none |
| tokens-3072, 1,000 payees, upload 1000 Mbit/s | 1,000 | 3,051 | 4,237 | 1.87% | 0.00% | 0.16 | 0.19 | 0.024 / 0.038 | 0.766 | 76.0 | 12.5 | none |
| no-tokens, 6,010 payees, upload 5 Mbit/s | 6,010 | 204,663 | 211,369 | 43.51% | 12.87% | never | never | 1.16 / 1.83 | 0.766 | 76.0 | 681 | SV2 extended 64 KiB field |
| no-tokens, 6,010 payees, upload 20 Mbit/s | 6,010 | 204,663 | 211,369 | 15.24% | 0.20% | 2.25 | 2.56 | 1.18 / 1.87 | 0.766 | 76.0 | 681 | SV2 extended 64 KiB field |
| no-tokens, 6,010 payees, upload 50 Mbit/s | 6,010 | 204,663 | 211,369 | 7.81% | 0.02% | 0.98 | 1.12 | 1.18 / 1.86 | 0.766 | 76.0 | 681 | SV2 extended 64 KiB field |
| no-tokens, 6,010 payees, upload 200 Mbit/s | 6,010 | 204,663 | 211,369 | 3.54% | 0.03% | 0.40 | 0.46 | 1.17 / 1.84 | 0.766 | 76.0 | 681 | SV2 extended 64 KiB field |
| no-tokens, 6,010 payees, upload 1000 Mbit/s | 6,010 | 204,663 | 211,369 | 2.45% | 0.01% | 0.23 | 0.27 | 1.15 / 1.84 | 0.766 | 76.0 | 681 | SV2 extended 64 KiB field |
| no-tokens, 29,402 payees, upload 5 Mbit/s | 29,402 | 999,991 | 1,028,497 | 78.32% | 58.20% | never | never | 5.21 / 8.23 | 0.766 | 76.0 | 3,317 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, 29,402 payees, upload 20 Mbit/s | 29,402 | 999,991 | 1,028,497 | 44.59% | 12.34% | never | never | 5.77 / 9.06 | 0.766 | 76.0 | 3,317 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, 29,402 payees, upload 50 Mbit/s | 29,402 | 999,991 | 1,028,497 | 24.51% | 1.10% | 4.47 | 5.03 | 5.76 / 9.02 | 0.766 | 76.0 | 3,317 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, 29,402 payees, upload 200 Mbit/s | 29,402 | 999,991 | 1,028,497 | 9.40% | 0.06% | 1.21 | 1.38 | 5.68 / 8.98 | 0.766 | 76.0 | 3,317 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, 29,402 payees, upload 1000 Mbit/s | 29,402 | 999,991 | 1,028,497 | 4.00% | 0.02% | 0.47 | 0.53 | 5.52 / 8.84 | 0.766 | 76.0 | 3,317 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |

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
| latency 150 ms | 1,001 | 34,357 | 36,423 | 5.08% | 0.02% | 0.63 | 0.73 | 0.204 / 0.322 | 0.766 | 76.0 | 116 | none |
| latency 400 ms | 1,001 | 34,357 | 36,423 | 9.85% | 0.08% | 1.34 | 1.57 | 0.205 / 0.322 | 0.766 | 76.0 | 116 | none |

Share interval, for a 3 KB and a 200 KiB coinbase:

| Scenario | Payees | Coinbase bytes | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | BCHN relay (Mbit/s) | Templates (Mbit/s) | Stratum per miner (kbit/s) | Limits broken |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| tokens-3072, 1,000 payees, 1 s shares | 1,000 | 3,051 | 4,237 | 15.89% | 0.50% | 0.19 | 0.22 | 0.239 / 0.379 | 0.766 | 76.0 | 68.1 | none |
| tokens-3072, 1,000 payees, 2 s shares | 1,000 | 3,051 | 4,237 | 8.68% | 0.12% | 0.19 | 0.22 | 0.120 / 0.190 | 0.766 | 76.0 | 37.2 | none |
| tokens-3072, 1,000 payees, 5 s shares | 1,000 | 3,051 | 4,237 | 3.84% | 0.01% | 0.19 | 0.22 | 0.048 / 0.076 | 0.766 | 76.0 | 18.6 | none |
| tokens-3072, 1,000 payees, 10 s shares | 1,000 | 3,051 | 4,237 | 2.07% | 0.00% | 0.19 | 0.22 | 0.024 / 0.038 | 0.766 | 76.0 | 12.5 | none |
| tokens-3072, 1,000 payees, 30 s shares | 1,000 | 3,051 | 4,237 | 0.52% | 0.00% | 0.19 | 0.22 | 8.0e-3 / 0.013 | 0.766 | 76.0 | 8.34 | none |
| no-tokens, 6,010 payees, 1 s shares | 6,010 | 204,663 | 211,369 | 45.81% | 11.07% | 676.34 | 676.49 | 11.8 / 18.6 | 0.766 | 76.0 | 3,719 | SV2 extended 64 KiB field |
| no-tokens, 6,010 payees, 2 s shares | 6,010 | 204,663 | 211,369 | 29.50% | 1.79% | 1.10 | 1.25 | 5.93 / 9.35 | 0.766 | 76.0 | 2,031 | SV2 extended 64 KiB field |
| no-tokens, 6,010 payees, 5 s shares | 6,010 | 204,663 | 211,369 | 14.09% | 0.24% | 1.00 | 1.14 | 2.38 / 3.75 | 0.766 | 76.0 | 1,018 | SV2 extended 64 KiB field |
| no-tokens, 6,010 payees, 10 s shares | 6,010 | 204,663 | 211,369 | 7.81% | 0.02% | 0.98 | 1.12 | 1.18 / 1.86 | 0.766 | 76.0 | 681 | SV2 extended 64 KiB field |
| no-tokens, 6,010 payees, 30 s shares | 6,010 | 204,663 | 211,369 | 2.74% | 0.00% | 0.98 | 1.11 | 0.394 / 0.621 | 0.766 | 76.0 | 456 | SV2 extended 64 KiB field |

Number of nodes:

| Scenario | Payees | Coinbase bytes | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | BCHN relay (Mbit/s) | Templates (Mbit/s) | Stratum per miner (kbit/s) | Limits broken |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| 10 nodes | 1,001 | 34,357 | 36,423 | 2.09% | 0.01% | 0.18 | 0.19 | 0.207 / 0.237 | 0.766 | 76.0 | 116 | none |
| 50 nodes | 1,001 | 34,357 | 36,423 | 2.76% | 0.00% | 0.29 | 0.34 | 0.203 / 0.293 | 0.766 | 76.0 | 116 | none |
| 200 nodes | 1,001 | 34,357 | 36,423 | 3.47% | 0.00% | 0.38 | 0.44 | 0.202 / 0.375 | 0.766 | 76.0 | 116 | none |
| 1000 nodes | 1,001 | 34,357 | 36,423 | 4.23% | 0.02% | 0.47 | 0.56 | 0.202 / 0.378 | 0.766 | 76.0 | 116 | none |

Stale shares grow about linearly with how long a share takes to reach the
other miners compared with the share interval: latency, hops (more nodes)
and the share's own transfer time all add up. Ten-second shares keep that
small; 1-second shares would make 16% of small shares stale, and 46% with a
200 KiB coinbase.

Other delays matter as much as the network. Single runs from the default:

| Change from the default | Stale | Orphaned |
|---|---:|---:|
| none | 3.09% | 0.01% |
| miners take 0.5 s to get new work (`--work-switch-seconds 0.5`) | 7.19% | 0.17% |
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
| seed 2 | 1,001 | 34,357 | 36,423 | 2.97% | 0.01% | 0.35 | 0.40 | 0.202 / 0.465 | 0.766 | 76.0 | 116 | none |
| seed 3 | 1,001 | 34,357 | 36,423 | 3.19% | 0.01% | 0.33 | 0.39 | 0.206 / 0.355 | 0.766 | 76.0 | 116 | none |
| seed 4 | 1,001 | 34,357 | 36,423 | 2.87% | 0.00% | 0.33 | 0.38 | 0.200 / 0.346 | 0.766 | 76.0 | 116 | none |
| seed 5 | 1,001 | 34,357 | 36,423 | 3.47% | 0.02% | 0.37 | 0.43 | 0.203 / 0.378 | 0.766 | 76.0 | 116 | none |
| seed 6 | 1,001 | 34,357 | 36,423 | 2.90% | 0.00% | 0.31 | 0.36 | 0.202 / 0.349 | 0.766 | 76.0 | 116 | none |
| seed 7 | 1,001 | 34,357 | 36,423 | 3.26% | 0.00% | 0.34 | 0.40 | 0.202 / 0.349 | 0.766 | 76.0 | 116 | none |
| seed 8 | 1,001 | 34,357 | 36,423 | 2.89% | 0.01% | 0.33 | 0.38 | 0.202 / 0.320 | 0.766 | 76.0 | 116 | none |

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
| Command line | `p2poolv2_netsim/src/bin/netsim.rs` |

The crate is new code for the Bitcoin Cash fork, under AGPL-3.0-or-later
(decision 13). Upstream's ns-3 repository has no licence file, so its idea is
credited here and in the crate's README, and none of its code is used.
