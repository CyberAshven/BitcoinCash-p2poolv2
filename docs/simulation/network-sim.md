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

The model runs on one-minute Bitcoin Cash blocks (CHIP-2025-03, on chipnet
since 2026-11-15 and on mainnet from 2027-05-15): a block every minute, a
tenth of the ten-minute reward and of the ABLA floors (3.2 MB on mainnet,
200 kB on chipnet, 25.6 MB to start on scalenet). Every number below comes
from that model. The ten-minute tables are in this file at commit 05de7fa5;
where a conclusion changed, the text says so.

## The short answer

- **The coinbase limits P2Pool first, as a bandwidth cost.** Every payee
  paid directly puts 34 bytes into every share and into every Stratum job.
  Under the hybrid payout rule a share without tokens pays payees
  directly up to BCH's 1,000,000-byte transaction limit (29,402 P2PKH
  payees) and the rest through a payout tree, so BCH never rejects a block
  for its coinbase. But with whole shares on the wire, as nodes send them,
  stale shares climb with the direct payees on the assumed mix of
  home and data-centre links: 3% at 1,000 payees, 8% at 6,010, 11% at
  10,000 and 25% at 29,402 (a 1 MB coinbase; 0.6% orphaned). With 1 MB shares
  only data-centre links keep it near 4%.
- **Sending shares without their payouts removes that cost.** Every node
  rebuilds the payouts to validate a share anyway, as the design points
  out. Without them a share is about 1.3 KB whatever the payee count, and
  stale shares at 29,402 payees fall from 25% to 2.6%.
- **For miners the coinbase stays a per-job cost.** A Stratum V1 job carries
  the coinbase in hex about every 4.6 seconds: 124 kbit/s per miner at 1,000
  payees and 731 kbit/s at 6,010. A Stratum V2 extended job cannot carry a
  coinbase of more than 1,922 payees at all (65,535-byte fields), and the
  device evidence from other pools puts stock firmware between 12 and 461
  payees. Only Stratum V2 standard channels carry no coinbase.
- **The PPLNS window bounds the payees, ten times more tightly than with
  ten-minute blocks.** On BCH it holds three BCH blocks of the pool's work,
  about `18 / f` shares for a pool with fraction `f` of BCH's hashrate: a
  tenth of the ten-minute window. A pool above 0.3% of BCH has fewer than
  about 6,000 payees whatever its miner count; only below about 0.06% can a
  crowd of small miners reach the 1 MB limit and the tree. The default pool
  (1% of BCH) has a 1,800-share window, so of a thousand equal miners 835
  hold a share in it.
- **With merge-mined tokens the 3,072-byte threshold keeps the coinbase at
  3,051 bytes** for any number of payees (79 direct, the rest in the tree),
  and stale shares at about 2%, like the old 11-payout cap.
- **The share chain itself is not the limit at 10-second shares.** With
  small shares 2 to 3% of shares go stale, nearly all named as uncles (paid
  9/10), and a share reaches 90% of 100 nodes in 0.2 to 0.3 s. It grows
  with latency (10% at 400 ms one way), node count (3.8% at 1,000 nodes),
  slow job switching (7% if miners take 0.5 s to get new work) and faster
  shares (16% at 1-second shares).
- **Raw bandwidth binds through BCHN and templates at large blocks, not
  through the share chain, and a block size now means ten times the
  traffic.** A block every minute carries ten times the transactions a
  second that the same size every ten minutes did: a node's BCHN needs about
  6.1 Mbit/s each way at full 25.6 MB blocks, 240 Mbit/s at 1 GB and 479
  Mbit/s at BCHN's 2 GB guard just to relay transactions. On the assumed link
  mix 99 of 100 nodes cannot carry that at 1 GB and none at 2 GB. On
  data-centre links block size leaves the stale rate at 1.6% from 200 kB to
  2 GB. Fetching a full `getblocktemplate` every 10 seconds costs 8.7 Mbit/s
  between node and BCHN at full 3.2 MB blocks and 5.45 Gbit/s at 2 GB;
  BCHN's `getblocktemplatelight` avoids it.
- **A block's coinbase now costs ten times the BCH orphan risk per second
  of delay.** With a block a minute, a 1 MB coinbase's 0.64 s over four hops
  at 50 Mbit/s risks 1.06% of the pool's blocks, against 0.11% with
  ten-minute blocks.
- **Upstream's limit of about 1,000 miners at 10-second shares (3.83% stale
  on 1,000 nodes) moves only with smaller shares.** Sending shares without
  their payouts, forwarding them after the header check and giving each
  node 8 to 64 peers keeps 1,000 nodes within 3.83% stale down to 5.3 to 4.0
  second shares (5.0 to 3.5 with ten-minute blocks). Each alone does less:
  shares without payouts reach 6.3 seconds, header-check forwarding and
  larger uncle caps change nothing measurable, and more peers with whole
  shares make it worse. With one-minute blocks today's design gains nothing
  from shorter shares at all: the window then holds more shares and the
  coinbase more payees. Weak shares gossiped beside the shares cost upload
  but no stale shares; carried whole in every share they push the limit out
  to 11 to 16 seconds (section "Scaling propagation instead of capping
  it").

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
  from each link first, and every BCH block (one every minute on average)
  takes the link for its compact block, once down and once up, so
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
  window of `min(120,960, 3 * 6 / f)` shares (`BCH_PPLNS_WINDOW_BLOCKS` = 3
  one-minute blocks of six 10-second shares each), plus the donation key
  that every share's default
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
| Pool's fraction of BCH's hashrate | `--pool-fraction` | 0.01 (window of 1,800 shares) |
| Share interval | `--share-interval` | 10 s |
| BCH block size, average transaction | `--block-bytes`, `--transaction-bytes` | 3.2 MB (mainnet's ABLA floor for a one-minute block), 226 bytes |
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
(for a one-minute block a 3.2 MB floor on mainnet, 200 kB on chipnet and
25.6 MB to start on scalenet, a tenth of the ten-minute values; the model
keeps BCHN's 2 GB guard per block, unscaled) and never raises the
transaction limit.
None of these coinbase limits depends on the block interval.

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

The tables were regenerated on 2026-11-15 for one-minute blocks. Against
the ten-minute tables (2026-11-12, commit 5b67ca11): the default share pays
836 payees, not 1,001, since the 1% pool's window is 1,800 shares; the
default 3.2 MB block puts a 14-hash merkle branch in every share instead of
18 (128 bytes less); BCH blocks interrupt the links every minute with a
smaller compact block; and a miner gets a job about every 4.6 seconds
instead of 5. Stale rates at the same coinbase moved by up to 0.4 points,
about the seed-to-seed noise below. The earlier tables, before commit
7e1c2707 ("Pick uncles from the mining branch only", 2026-10-30), had
higher orphan rates: that commit made the model pick uncles as upstream's
`find_uncles` does.

### Payees and the coinbase layout

| Scenario | Payees | Coinbase bytes | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | BCHN relay (Mbit/s) | Templates (Mbit/s) | Stratum per miner (kbit/s) | Limits broken |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| no-tokens, 10 payees | 10 | 661 | 1,679 | 1.95% | 0.00% | 0.17 | 0.20 | 9.9e-3 / 0.016 | 0.766 | 8.72 | 4.36 | none |
| no-tokens, 100 payees | 100 | 3,721 | 4,819 | 2.08% | 0.00% | 0.19 | 0.22 | 0.027 / 0.043 | 0.766 | 8.72 | 15.2 | none |
| no-tokens, 1,000 payees | 1,000 | 34,323 | 36,261 | 3.01% | 0.00% | 0.33 | 0.38 | 0.203 / 0.321 | 0.766 | 8.72 | 124 | none |
| no-tokens, 6,010 payees | 6,010 | 204,663 | 211,241 | 7.64% | 0.01% | 0.99 | 1.13 | 1.19 / 1.87 | 0.766 | 8.72 | 731 | SV2 extended 64 KiB field |
| no-tokens, 10,000 payees | 10,000 | 340,323 | 350,621 | 10.94% | 0.00% | 1.51 | 1.73 | 1.97 / 3.10 | 0.766 | 8.72 | 1,214 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, 29,402 payees | 29,402 | 999,991 | 1,028,369 | 24.70% | 0.62% | 4.48 | 5.05 | 5.79 / 9.08 | 0.766 | 8.72 | 3,564 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, 100,000 payees | 100,000 | 999,967 | 1,028,345 | 24.70% | 0.62% | 4.48 | 5.05 | 5.79 / 9.08 | 0.766 | 8.72 | 3,564 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, 1,000,000 payees | 1,000,000 | 999,967 | 1,028,345 | 24.70% | 0.62% | 4.48 | 5.05 | 5.79 / 9.08 | 0.766 | 8.72 | 3,564 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| tokens-3072, 100 payees | 100 | 3,051 | 4,109 | 2.05% | 0.00% | 0.19 | 0.22 | 0.023 / 0.037 | 0.766 | 8.72 | 12.9 | none |
| tokens-3072, 10,000 payees | 10,000 | 3,051 | 4,109 | 2.05% | 0.00% | 0.19 | 0.22 | 0.023 / 0.037 | 0.766 | 8.72 | 12.9 | none |
| tokens-3072, 1,000,000 payees | 1,000,000 | 3,051 | 4,109 | 2.05% | 0.00% | 0.19 | 0.22 | 0.023 / 0.037 | 0.766 | 8.72 | 12.9 | none |
| cap-11, 1,000,000 payees | 1,000,000 | 782 | 1,800 | 1.97% | 0.00% | 0.17 | 0.20 | 0.011 / 0.017 | 0.766 | 8.72 | 4.78 | none |
| uncapped, 100,000 payees | 100,000 | 3,400,325 | 3,494,463 | 51.70% | 15.77% | never | never | 19.6 / 30.8 | 0.766 | 8.72 | 12,113 | BCH 1 MB transaction, hybrid 1,204,800-byte share body, 1.25 MiB P2P message, pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |

Each payee paid directly adds 34 bytes to every share, and pushing whole
shares to every peer multiplies that: at 6,010 payees the busiest node
uploads 1.9 Mbit/s and 7.6% of shares go stale; at 29,402 (a 1 MB coinbase)
a quarter do and 0.6% are orphaned. Past 29,402 the hybrid rule's tree keeps the
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
| no-tokens, 1,000 payees, full relay | 1,000 | 34,323 | 36,261 | 3.01% | 0.00% | 0.33 | 0.38 | 0.203 / 0.321 | 0.766 | 8.72 | 124 | none |
| no-tokens, 6,010 payees, full relay | 6,010 | 204,663 | 211,241 | 7.64% | 0.01% | 0.99 | 1.13 | 1.19 / 1.87 | 0.766 | 8.72 | 731 | SV2 extended 64 KiB field |
| no-tokens, 29,402 payees, full relay | 29,402 | 999,991 | 1,028,369 | 24.70% | 0.62% | 4.48 | 5.05 | 5.79 / 9.08 | 0.766 | 8.72 | 3,564 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, 1,000 payees, without-payouts relay | 1,000 | 34,323 | 1,301 | 1.95% | 0.00% | 0.17 | 0.20 | 7.8e-3 / 0.012 | 0.766 | 8.72 | 124 | none |
| no-tokens, 6,010 payees, without-payouts relay | 6,010 | 204,663 | 1,301 | 2.09% | 0.00% | 0.19 | 0.22 | 7.6e-3 / 0.012 | 0.766 | 8.72 | 731 | SV2 extended 64 KiB field |
| no-tokens, 29,402 payees, without-payouts relay | 29,402 | 999,991 | 1,301 | 2.55% | 0.00% | 0.26 | 0.30 | 7.2e-3 / 0.012 | 0.766 | 8.72 | 3,564 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |

Without the payouts a share is about 1.3 KB at any payee count, and stale
shares stay between 2.0% and 2.6%: the share chain no longer cares how many
miners are paid directly. The remaining growth is validation: rebuilding a
1 MB coinbase is taken to cost 20 ms. The coinbase itself still reaches the
miners in their Stratum jobs (below) and BCH in the found block.

### Pool size: the window bounds the payees

A million equally sized miners, for pools from 0.1% to 30% of BCH:

| Scenario | Payees | Coinbase bytes | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | BCHN relay (Mbit/s) | Templates (Mbit/s) | Stratum per miner (kbit/s) | Limits broken |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| no-tokens, pool 0.1% of BCH | 17,840 | 606,883 | 624,501 | 17.09% | 0.17% | 2.59 | 2.94 | 3.52 / 5.53 | 0.766 | 8.72 | 2,164 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, pool 0.3% of BCH | 5,983 | 203,745 | 210,323 | 7.61% | 0.01% | 0.99 | 1.13 | 1.18 / 1.86 | 0.766 | 8.72 | 728 | SV2 extended 64 KiB field |
| no-tokens, pool 1% of BCH | 1,799 | 61,489 | 64,147 | 3.77% | 0.00% | 0.44 | 0.50 | 0.359 / 0.564 | 0.766 | 8.72 | 221 | none |
| no-tokens, pool 3% of BCH | 601 | 20,757 | 22,295 | 2.57% | 0.00% | 0.27 | 0.31 | 0.125 / 0.197 | 0.766 | 8.72 | 76.0 | none |
| no-tokens, pool 10% of BCH | 181 | 6,475 | 7,653 | 2.18% | 0.00% | 0.20 | 0.24 | 0.043 / 0.068 | 0.766 | 8.72 | 25.1 | none |
| no-tokens, pool 30% of BCH | 61 | 2,395 | 3,453 | 2.02% | 0.00% | 0.18 | 0.21 | 0.020 / 0.031 | 0.766 | 8.72 | 10.6 | none |
| tokens-3072, pool 0.1% of BCH | 17,840 | 3,051 | 4,109 | 2.05% | 0.00% | 0.19 | 0.22 | 0.023 / 0.037 | 0.766 | 8.72 | 12.9 | none |
| tokens-3072, pool 0.3% of BCH | 5,983 | 3,051 | 4,109 | 2.05% | 0.00% | 0.19 | 0.22 | 0.023 / 0.037 | 0.766 | 8.72 | 12.9 | none |
| tokens-3072, pool 1% of BCH | 1,799 | 3,051 | 4,109 | 2.05% | 0.00% | 0.19 | 0.22 | 0.023 / 0.037 | 0.766 | 8.72 | 12.9 | none |
| tokens-3072, pool 3% of BCH | 601 | 3,051 | 4,109 | 2.05% | 0.00% | 0.19 | 0.22 | 0.023 / 0.037 | 0.766 | 8.72 | 12.9 | none |
| tokens-3072, pool 10% of BCH | 181 | 3,051 | 4,109 | 2.05% | 0.00% | 0.19 | 0.22 | 0.023 / 0.037 | 0.766 | 8.72 | 12.9 | none |
| tokens-3072, pool 30% of BCH | 61 | 2,395 | 3,453 | 2.02% | 0.00% | 0.18 | 0.21 | 0.020 / 0.031 | 0.766 | 8.72 | 10.6 | none |

The window is three BCH blocks of the pool's work, so a growing pool pays
fewer, larger miners per block. With one-minute blocks that is a tenth of
the ten-minute window at every pool size: at 10% of BCH the window is 180
shares and a million miners fill at most 180 payee slots, 181 payees with
the donation key (a 6.5 KB coinbase). At 1% the coinbase has 1,799 payees
(61 KB), at 0.3% 5,983 (204 KB) and at 0.1% 17,840 (607 KB). No pool in
the sweep reaches the 1 MB limit any more; that takes a pool below about
0.06% of BCH (with ten-minute blocks, 0.6%). Uneven hashrate helps: with a
Pareto spread (shape 1.16, 80% of the hashrate on 20% of the miners;
`--miner-hashrate pareto:1.16`), a million miners make 1,499 payees at 1%
of BCH (51 KB) and 12,624 at 0.1% (430 KB).

Whether a million miners is realistic depends on BCH's hashrate. As an
illustration only: if BCH had 3 EH/s, a million 1 TH/s miners would be a
third of it, where the window is 54 shares.

### Block size

The default share (a thousand miners, 835 of them in the window, and the
donation key: 836 payees, no tokens) on mixed and on data-centre (1 Gbit/s)
links, one-minute blocks from chipnet's 200 kB floor to BCHN's 2 GB guard:

| Scenario | Payees | Coinbase bytes | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | BCHN relay (Mbit/s) | Templates (Mbit/s) | Stratum per miner (kbit/s) | Limits broken |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| 200 kB blocks, mixed links | 836 | 28,747 | 30,397 | 2.84% | 0.00% | 0.30 | 0.35 | 0.170 / 0.270 | 0.048 | 0.546 | 104 | none |
| 3.2 MB blocks, mixed links | 836 | 28,747 | 30,525 | 2.85% | 0.00% | 0.30 | 0.35 | 0.171 / 0.271 | 0.766 | 8.72 | 104 | none |
| 25.6 MB blocks, mixed links | 836 | 28,747 | 30,621 | 3.20% | 0.00% | 0.34 | 0.39 | 0.171 / 0.271 | 6.13 | 69.8 | 105 | none |
| 1 GB blocks, mixed links | 836 | 28,747 | 30,813 | 0.74% | 0.00% | 7184.00 | never | 0.171 / 0.274 | 240 | 2,726 | 105 | BCHN relay over the link on 99 of 100 nodes |
| 2 GB blocks, mixed links | 836 | 28,747 | 30,845 | 97.92% | 97.45% | never | never | 0.104 / 0.188 | 479 | 5,451 | 106 | BCHN relay over the link on 100 of 100 nodes |
| 200 kB blocks, data-centre links | 836 | 28,747 | 30,397 | 1.55% | 0.00% | 0.12 | 0.13 | 0.166 / 0.267 | 0.048 | 0.546 | 104 | none |
| 3.2 MB blocks, data-centre links | 836 | 28,747 | 30,525 | 1.56% | 0.00% | 0.12 | 0.13 | 0.167 / 0.269 | 0.766 | 8.72 | 104 | none |
| 25.6 MB blocks, data-centre links | 836 | 28,747 | 30,621 | 1.56% | 0.00% | 0.12 | 0.13 | 0.167 / 0.269 | 6.13 | 69.8 | 105 | none |
| 1 GB blocks, data-centre links | 836 | 28,747 | 30,813 | 1.57% | 0.00% | 0.12 | 0.14 | 0.169 / 0.271 | 240 | 2,726 | 105 | none |
| 2 GB blocks, data-centre links | 836 | 28,747 | 30,845 | 1.61% | 0.00% | 0.13 | 0.15 | 0.169 / 0.271 | 479 | 5,451 | 106 | none |

The share chain does not care about block size: its merkle branch grows by
one 32-byte hash for each doubling, and on data-centre links the stale rate
is 1.6% at every size. What grows is BCHN, and a block every minute carries
ten times the transactions a second that the same size every ten minutes
did: to relay full 25.6 MB blocks a node needs about 6.1 Mbit/s each way,
240 Mbit/s at 1 GB and 479 Mbit/s at 2 GB, and a 2 GB compact block is 53
MB every minute. The 24 and 48 Mbit/s that 1 GB and 2 GB ten-minute blocks
needed now come with 100 MB and 200 MB one-minute blocks. On the mixed links
99 of 100 nodes cannot carry the 1 GB relay and none the 2 GB. The model
moves the miners of a node short of BCHN's relay to the other nodes, so at
1 GB the one node left finds every share (0.74% stale, and shares take
hours to reach the nodes behind their BCHN load), and at 2 GB, with no node
left, every node keeps mining and 98% of shares go stale. Those rows show
nodes that cannot follow BCH at all, P2Pool or not: a cost of running a BCH
full node at that size. P2Pool adds the block templates it fetches from
BCHN: a full `getblocktemplate` every 10 seconds and on each block is about
three times the block in JSON, 8.7 Mbit/s at full 3.2 MB blocks, 70 Mbit/s
at 25.6 MB and 5.45 Gbit/s at 2 GB. It stays inside one machine when BCHN
runs on the same host, but not when BCHN is reached over a network, and
BCHN has to build that JSON
each time. BCHN's `getblocktemplatelight` and `submitblocklight` send only
the merkle branch.

### Bandwidth

Upload medians from 5 Mbit/s to 1 Gbit/s (download four times the upload),
for a 3 KB coinbase (tokens), a 200 KiB one (6,010 payees) and a 1 MB one
(29,402):

| Scenario | Payees | Coinbase bytes | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | BCHN relay (Mbit/s) | Templates (Mbit/s) | Stratum per miner (kbit/s) | Limits broken |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| tokens-3072, 1,000 payees, upload 5 Mbit/s | 1,000 | 3,051 | 4,109 | 3.69% | 0.00% | 0.42 | 0.47 | 0.024 / 0.037 | 0.766 | 8.72 | 12.9 | none |
| tokens-3072, 1,000 payees, upload 20 Mbit/s | 1,000 | 3,051 | 4,109 | 2.23% | 0.00% | 0.22 | 0.25 | 0.023 / 0.037 | 0.766 | 8.72 | 12.9 | none |
| tokens-3072, 1,000 payees, upload 50 Mbit/s | 1,000 | 3,051 | 4,109 | 2.05% | 0.00% | 0.19 | 0.22 | 0.023 / 0.037 | 0.766 | 8.72 | 12.9 | none |
| tokens-3072, 1,000 payees, upload 200 Mbit/s | 1,000 | 3,051 | 4,109 | 1.92% | 0.00% | 0.17 | 0.20 | 0.023 / 0.037 | 0.766 | 8.72 | 12.9 | none |
| tokens-3072, 1,000 payees, upload 1000 Mbit/s | 1,000 | 3,051 | 4,109 | 1.88% | 0.00% | 0.16 | 0.19 | 0.023 / 0.037 | 0.766 | 8.72 | 12.9 | none |
| no-tokens, 6,010 payees, upload 5 Mbit/s | 6,010 | 204,663 | 211,241 | 43.02% | 13.14% | never | never | 1.16 / 1.83 | 0.766 | 8.72 | 731 | SV2 extended 64 KiB field |
| no-tokens, 6,010 payees, upload 20 Mbit/s | 6,010 | 204,663 | 211,241 | 15.17% | 0.13% | 2.27 | 2.57 | 1.19 / 1.88 | 0.766 | 8.72 | 731 | SV2 extended 64 KiB field |
| no-tokens, 6,010 payees, upload 50 Mbit/s | 6,010 | 204,663 | 211,241 | 7.64% | 0.01% | 0.99 | 1.13 | 1.19 / 1.87 | 0.766 | 8.72 | 731 | SV2 extended 64 KiB field |
| no-tokens, 6,010 payees, upload 200 Mbit/s | 6,010 | 204,663 | 211,241 | 3.53% | 0.00% | 0.40 | 0.46 | 1.17 / 1.85 | 0.766 | 8.72 | 731 | SV2 extended 64 KiB field |
| no-tokens, 6,010 payees, upload 1000 Mbit/s | 6,010 | 204,663 | 211,241 | 2.38% | 0.00% | 0.23 | 0.27 | 1.16 / 1.85 | 0.766 | 8.72 | 731 | SV2 extended 64 KiB field |
| no-tokens, 29,402 payees, upload 5 Mbit/s | 29,402 | 999,991 | 1,028,369 | 78.66% | 57.14% | never | never | 5.23 / 8.24 | 0.766 | 8.72 | 3,564 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, 29,402 payees, upload 20 Mbit/s | 29,402 | 999,991 | 1,028,369 | 44.78% | 12.47% | never | never | 5.80 / 9.11 | 0.766 | 8.72 | 3,564 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, 29,402 payees, upload 50 Mbit/s | 29,402 | 999,991 | 1,028,369 | 24.70% | 0.62% | 4.48 | 5.05 | 5.79 / 9.08 | 0.766 | 8.72 | 3,564 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, 29,402 payees, upload 200 Mbit/s | 29,402 | 999,991 | 1,028,369 | 8.99% | 0.00% | 1.21 | 1.38 | 5.72 / 9.03 | 0.766 | 8.72 | 3,564 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |
| no-tokens, 29,402 payees, upload 1000 Mbit/s | 29,402 | 999,991 | 1,028,369 | 3.92% | 0.00% | 0.47 | 0.53 | 5.55 / 8.88 | 0.766 | 8.72 | 3,564 | pre-hybrid 200 KiB share body, SV2 extended 64 KiB field |

Small shares go stale 3.7% of the time even when the median node has 5
Mbit/s up. A 200 KiB coinbase needs about 50 Mbit/s for stale shares to stay
under 8%, and a 1 MB coinbase needs data-centre links (9.0% at 200 Mbit/s,
3.9% at 1 Gbit/s) when shares travel whole.

### Latency, share interval and node count

One-way latency, median of each link:

| Scenario | Payees | Coinbase bytes | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | BCHN relay (Mbit/s) | Templates (Mbit/s) | Stratum per miner (kbit/s) | Limits broken |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| latency 10 ms | 836 | 28,747 | 30,525 | 2.05% | 0.00% | 0.17 | 0.20 | 0.169 / 0.266 | 0.766 | 8.72 | 104 | none |
| latency 50 ms | 836 | 28,747 | 30,525 | 2.85% | 0.00% | 0.30 | 0.35 | 0.171 / 0.271 | 0.766 | 8.72 | 104 | none |
| latency 150 ms | 836 | 28,747 | 30,525 | 4.80% | 0.00% | 0.60 | 0.70 | 0.172 / 0.272 | 0.766 | 8.72 | 104 | none |
| latency 400 ms | 836 | 28,747 | 30,525 | 9.56% | 0.03% | 1.31 | 1.54 | 0.173 / 0.272 | 0.766 | 8.72 | 104 | none |

Share interval, for a 3 KB and a 200 KiB coinbase:

| Scenario | Payees | Coinbase bytes | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | BCHN relay (Mbit/s) | Templates (Mbit/s) | Stratum per miner (kbit/s) | Limits broken |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| tokens-3072, 1,000 payees, 1 s shares | 1,000 | 3,051 | 4,109 | 16.02% | 0.07% | 0.19 | 0.22 | 0.233 / 0.369 | 0.766 | 8.72 | 66.2 | none |
| tokens-3072, 1,000 payees, 2 s shares | 1,000 | 3,051 | 4,109 | 8.86% | 0.01% | 0.19 | 0.22 | 0.116 / 0.184 | 0.766 | 8.72 | 36.6 | none |
| tokens-3072, 1,000 payees, 5 s shares | 1,000 | 3,051 | 4,109 | 3.52% | 0.00% | 0.19 | 0.22 | 0.046 / 0.073 | 0.766 | 8.72 | 18.8 | none |
| tokens-3072, 1,000 payees, 10 s shares | 1,000 | 3,051 | 4,109 | 2.05% | 0.00% | 0.19 | 0.22 | 0.023 / 0.037 | 0.766 | 8.72 | 12.9 | none |
| tokens-3072, 1,000 payees, 30 s shares | 1,000 | 3,051 | 4,109 | 0.91% | 0.00% | 0.19 | 0.22 | 7.9e-3 / 0.013 | 0.766 | 8.72 | 8.90 | none |
| no-tokens, 6,010 payees, 1 s shares | 6,010 | 204,663 | 211,241 | 45.81% | 9.70% | 689.11 | 689.26 | 11.8 / 18.7 | 0.766 | 8.72 | 3,767 | SV2 extended 64 KiB field |
| no-tokens, 6,010 payees, 2 s shares | 6,010 | 204,663 | 211,241 | 29.82% | 1.11% | 1.11 | 1.25 | 5.91 / 9.31 | 0.766 | 8.72 | 2,080 | SV2 extended 64 KiB field |
| no-tokens, 6,010 payees, 5 s shares | 6,010 | 204,663 | 211,241 | 14.00% | 0.08% | 1.00 | 1.14 | 2.36 / 3.72 | 0.766 | 8.72 | 1,068 | SV2 extended 64 KiB field |
| no-tokens, 6,010 payees, 10 s shares | 6,010 | 204,663 | 211,241 | 7.64% | 0.01% | 0.99 | 1.13 | 1.19 / 1.87 | 0.766 | 8.72 | 731 | SV2 extended 64 KiB field |
| no-tokens, 6,010 payees, 30 s shares | 6,010 | 204,663 | 211,241 | 2.87% | 0.00% | 0.98 | 1.12 | 0.405 / 0.638 | 0.766 | 8.72 | 506 | SV2 extended 64 KiB field |

Number of nodes:

| Scenario | Payees | Coinbase bytes | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | BCHN relay (Mbit/s) | Templates (Mbit/s) | Stratum per miner (kbit/s) | Limits broken |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| 10 nodes | 836 | 28,747 | 30,525 | 1.84% | 0.00% | 0.16 | 0.18 | 0.175 / 0.200 | 0.766 | 8.72 | 104 | none |
| 50 nodes | 836 | 28,747 | 30,525 | 2.48% | 0.00% | 0.27 | 0.31 | 0.171 / 0.246 | 0.766 | 8.72 | 104 | none |
| 200 nodes | 836 | 28,747 | 30,525 | 3.08% | 0.00% | 0.35 | 0.41 | 0.171 / 0.318 | 0.766 | 8.72 | 104 | none |
| 1000 nodes | 836 | 28,747 | 30,525 | 3.83% | 0.00% | 0.43 | 0.52 | 0.171 / 0.319 | 0.766 | 8.72 | 104 | none |

Stale shares grow about linearly with how long a share takes to reach the
other miners compared with the share interval: latency, hops (more nodes)
and the share's own transfer time all add up. Ten-second shares keep that
small; 1-second shares would make 16% of small shares stale, and 46% with a
200 KiB coinbase.

Other delays matter as much as the network. Single runs from the default:

| Change from the default | Stale | Orphaned |
|---|---:|---:|
| none | 2.85% | 0.00% |
| miners take 0.5 s to get new work (`--work-switch-seconds 0.5`) | 6.82% | 0.01% |
| validation takes 100 ms (`--validation-seconds 0.1`) | 5.73% | 0.01% |
| hashrate on nodes Pareto 1.16 (`--node-hashrate pareto:1.16`) | 2.48% | 0.00% |

### Stratum jobs

Every miner gets a new job for each new share-chain tip (the payouts
change), each template poll and each BCH block: with a block every minute,
about one job every 4.6 seconds (every 5 with ten-minute blocks). A Stratum
V1 job carries the coinbase twice over (hex), a V2 extended job once
(binary), a V2 standard job only a merkle root:

| Coinbase | V1 job | V1 per miner | V2 extended per miner | V2 standard per miner |
|---|---:|---:|---:|---:|
| 3,051 bytes (tokens) | 7,416 bytes | 12.9 kbit/s | 6.4 kbit/s | 0.19 kbit/s |
| 28,747 bytes (836 payees) | 60,248 bytes | 104 kbit/s | 52 kbit/s | 0.19 kbit/s |
| 204,663 bytes (6,010 payees) | 421,720 bytes | 731 kbit/s | 365 kbit/s (does not fit its fields) | 0.19 kbit/s |

A node serving 10,000 V1 miners needs 129 Mbit/s for their jobs with the
3 KB coinbase, and 7.3 Gbit/s at 6,010 payees. For a pool of many small
miners this, not the share chain, is the largest load a node carries, and
no change to how shares travel reduces it.

### Noise

The default under eight seeds (each seed also draws a different graph and
different links):

| Scenario | Payees | Coinbase bytes | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | BCHN relay (Mbit/s) | Templates (Mbit/s) | Stratum per miner (kbit/s) | Limits broken |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| seed 1 | 836 | 28,747 | 30,525 | 2.85% | 0.00% | 0.30 | 0.35 | 0.171 / 0.271 | 0.766 | 8.72 | 104 | none |
| seed 2 | 836 | 28,747 | 30,525 | 2.88% | 0.00% | 0.32 | 0.37 | 0.170 / 0.392 | 0.766 | 8.72 | 104 | none |
| seed 3 | 836 | 28,747 | 30,525 | 2.91% | 0.00% | 0.31 | 0.36 | 0.172 / 0.297 | 0.766 | 8.72 | 104 | none |
| seed 4 | 836 | 28,747 | 30,525 | 2.82% | 0.00% | 0.31 | 0.35 | 0.168 / 0.292 | 0.766 | 8.72 | 104 | none |
| seed 5 | 836 | 28,747 | 30,525 | 3.35% | 0.00% | 0.34 | 0.39 | 0.171 / 0.317 | 0.766 | 8.72 | 104 | none |
| seed 6 | 836 | 28,747 | 30,525 | 2.68% | 0.00% | 0.29 | 0.33 | 0.168 / 0.290 | 0.766 | 8.72 | 104 | none |
| seed 7 | 836 | 28,747 | 30,525 | 2.69% | 0.00% | 0.31 | 0.36 | 0.168 / 0.291 | 0.766 | 8.72 | 104 | none |
| seed 8 | 836 | 28,747 | 30,525 | 2.88% | 0.00% | 0.30 | 0.35 | 0.171 / 0.271 | 0.766 | 8.72 | 104 | none |

The stale rate moves between 2.7% and 3.4% from seed to seed: differences
smaller than about half a point between two rows of a table are noise.

### A found block's coinbase

The coinbase is the one transaction in a found block no other BCH node has,
so a compact block carries it whole. Over four BCH hops at the median 50
Mbit/s, a 1 MB coinbase adds about 0.64 s. With a block every minute that
is 1.06% extra risk of the block being orphaned on BCH, ten times the 0.11%
of ten-minute blocks: no longer small next to the costs above. The default
29 KB coinbase adds 0.018 s and 0.03%. A pool with a large coinbase loses
about 1.7% of its blocks for every second the coinbase adds; sending found
blocks to BCH nodes over faster links than the median, or keeping the
coinbase small, matters ten times more than before.

## What this means

1. **Under the hybrid rule the coinbase never makes a block invalid; its
   cost is bandwidth.** A share without tokens pays up to 29,402 payees
   directly. The rule raised the 200 KiB share limit (it binds from 6,010
   payees) to 1,204,800 bytes and the P2P message limit to 1.25 MiB to
   carry such shares. Sent whole, they make 8% of shares stale at 6,000
   payees and 25% at 29,402 on the assumed links.
2. **Sending shares without their payouts is what makes direct payouts
   cheap for the share chain**: 2.0 to 2.6% stale from 1,000 to 29,402
   payees. It is a change to how shares travel, with its own protocol tag
   as the design says, not a payout rule change, and nothing here builds
   it.
3. **For miners, a large coinbase stays expensive.** Stratum V1 sends it to
   every miner about every 4.6 seconds; Stratum V2 extended and declared
   jobs cannot carry more than 1,922 payees; and stock firmware is reported
   to take between 12 and 461. The chipnet test with real devices (design,
   section 8.2; audit M2) has to settle what pool-built jobs can carry, and
   Stratum V2 standard channels avoid the question.
4. **The window keeps most pools far from the limits, more so with
   one-minute blocks.** Above 0.3% of BCH a coinbase has fewer than about
   6,000 payees; only a pool under about 0.06% of BCH with very many small
   miners reaches the 1 MB limit and the tree (0.6% with ten-minute blocks).
5. **The share chain at 10-second shares is not the bottleneck.** Stale
   shares stay at 2 to 4% for small shares, almost all paid as uncles; fast
   job switching and quick validation keep it there.
6. **At large blocks, plan for the BCH node, ten times sooner than with
   ten-minute blocks.** A one-minute block of 100 MB needs the 24 Mbit/s of
   relay a 1 GB ten-minute block did, and 1 GB one-minute blocks need 240
   Mbit/s. Run BCHN on links that carry its relay with room to spare, and
   fetch templates with `getblocktemplatelight` rather than full
   `getblocktemplate`.
7. **A large coinbase costs BCH blocks.** At 1 MB it adds 1.06% orphan risk
   to every block the pool finds, against 0.11% with ten-minute blocks.

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
today's design: 3.83% stale and 0.00% orphaned (the "Number of nodes" table
above; 4.23% with ten-minute blocks). Each idea was run on 100 and 1,000
nodes at 10, 5, 2 and 1-second shares, with everything else at the defaults
(1,000 miners, the mixed links). The window holds three one-minute blocks
of the 1% pool's shares, 1,800 at 10-second shares and more at shorter
ones, so the default coinbase grows as the interval shrinks: 836 payees (a
29 KB coinbase) at 10 seconds, 974 (33 KB) at 5 and 1,001 (34 KB) at 2 and
1. With ten-minute blocks it was 1,001 payees at every interval. Then, for
each idea, a search found the shortest share interval at which 1,000 nodes
stay at or below today's stale rate at 10-second shares: how many shares a
second the idea lets 1,000 nodes carry for the stale rate they have today.

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
  has it (each share carries the average count; 64 add 51,200 bytes). Since
  2026-11-15 the node relays a new share with its weak shares listed by
  hash (`ShareWithWeakHashes`): `gossip-only` plus 32 bytes a weak share in
  the share message, the weak shares its pool took in the last 400 ms whole
  beside them, and a fetch for any other weak share a peer lacks; `whole` is
  the node before it.
- **2. Compact coinbase:** shares sent without the payout outputs, as in
  "Sending shares without their payouts" above: 1,301 bytes on the wire at
  3.2 MB blocks, the design's "about 1 KB" plus a 14-hash merkle branch.
  Validation still costs the whole share, since the receiver rebuilds the
  payouts. Compared with whole shares carrying a 3 KB coinbase (tokens,
  1,000 payees), today's 29 KB one and a 200 KiB one (6,010 payees).
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
memory at once; it does not change any number. On a 32-thread machine, with
the limits study running beside them, the sweeps took 57 minutes with 12
threads and the search 145 minutes with 14.

`interval-limit` first runs today's design on the given node count at
10-second shares: its stale rate is the ceiling. For each scenario it then
runs 10 seconds, then 0.25 or 40 seconds depending on the side the ceiling
is on, then halves that bracket six times on a log scale, and reports the
shortest interval tried that stayed within the ceiling (to about 6% below 10
seconds and 2% above).

### Where each idea reaches the limit

From `cargo netsim interval-limit scaling --nodes 1000` (seed 1): the
shortest share interval at which 1,000 nodes stay within 3.83% stale, and
the stale and orphan rates there.

| Scenario | Shortest share interval (s) | Shares per second against 10-second shares | Stale there | Orphaned there |
|---|---:|---:|---:|---:|
| today | 10.00 | 1.00 | 3.83% | 0.00% |
| 16 weak shares a share, gossip only | 10.00 | 1.00 | 3.83% | 0.00% |
| 16 weak shares a share, carried whole | 11.14 | 0.90 | 3.58% | 0.00% |
| 32 weak shares a share, gossip only | 10.00 | 1.00 | 3.83% | 0.00% |
| 32 weak shares a share, carried whole | 12.69 | 0.79 | 3.71% | 0.00% |
| 64 weak shares a share, gossip only | 10.00 | 1.00 | 3.83% | 0.00% |
| 64 weak shares a share, carried whole | 15.76 | 0.63 | 3.56% | 0.00% |
| 3 KB coinbase, whole shares | 6.68 | 1.50 | 3.66% | 0.00% |
| 200 KiB coinbase, whole shares | 29.54 | 0.34 | 3.67% | 0.00% |
| 29 KB coinbase, without payouts | 6.31 | 1.59 | 3.82% | 0.00% |
| 200 KiB coinbase, without payouts | 6.68 | 1.50 | 3.67% | 0.00% |
| forwarded after the header check | 9.44 | 1.06 | 3.77% | 0.00% |
| 16 peers | 10.44 | 0.96 | 3.82% | 0.00% |
| 32 peers | 12.97 | 0.77 | 3.65% | 0.00% |
| 64 peers | 15.76 | 0.63 | 3.67% | 0.00% |
| uncle cap 6 | 10.00 | 1.00 | 3.83% | 0.00% |
| uncle cap 12 | 10.00 | 1.00 | 3.83% | 0.00% |
| without payouts, header check, 8 peers | 5.30 | 1.89 | 3.82% | 0.00% |
| without payouts, header check, 16 peers | 4.46 | 2.24 | 3.36% | 0.00% |
| without payouts, header check, 32 peers | 4.21 | 2.37 | 3.43% | 0.00% |
| without payouts, header check, 64 peers | 3.98 | 2.51 | 3.64% | 0.00% |

Today's own search ends at 10 seconds: with one-minute blocks today's
whole share grows at shorter intervals (the window holds more shares, so the
coinbase pays more of the thousand miners), and no shorter interval stays
within the ceiling. With ten-minute blocks the coinbase was the same at every
interval and the search ended at 8.91 seconds. The search is as precise as
the stale rate is, and the stale rate moves by about half a point from seed
to seed ("Noise" above). Under seeds 2 and 3 (`--seed`, each with its own
ceiling from today's design under that seed: 3.93% and 3.84%) the
combination's limits move by up to 19%:

| Scenario | Seed 1 (s) | Seed 2 (s) | Seed 3 (s) |
|---|---:|---:|---:|
| without payouts, header check, 8 peers | 5.30 | 5.62 | 6.31 |
| without payouts, header check, 16 peers | 4.46 | 4.46 | 5.01 |
| without payouts, header check, 32 peers | 4.21 | 4.21 | 3.98 |
| without payouts, header check, 64 peers | 3.98 | 3.98 | 3.98 |

So a limit within about 20% of today's 10 seconds is not a measured change.
Read that way:

- **Weak shares, gossip only, do not move the limit** at 16, 32 or 64 per
  share. **Carried whole in every share, they move it to longer shares:**
  11.1, 12.7 and 15.8 seconds, because each share grows by their bytes.
- **The compact coinbase moves it from 10 seconds to 6.3 seconds**, and
  makes the coinbase size irrelevant: with a 200 KiB coinbase the limit
  moves from 29.5 seconds (whole shares) to 6.7 seconds, the same as whole
  shares with a 3 KB coinbase. (5.6 seconds with ten-minute blocks.)
- **Relay before full validation alone barely moves it** (9.4 seconds):
  validation is 5.6 ms of a hop that a 31 KB push and 50 ms of latency
  dominate.
- **More peers alone move it the wrong way** (10.4, 13.0 and 15.8 seconds
  at 16, 32 and 64 peers): with whole shares a push to more peers takes
  longer than the hops it saves.
- **A larger uncle cap does not move it**: in these runs no share ever had a
  fourth uncle to name, so caps of 6 and 12 give the same runs as 3.
- **The combination moves it to 5.3, 4.5, 4.2 and 4.0 seconds** at 8, 16,
  32 and 64 peers: 1.9 to 2.5 times the share rate of 10-second shares
  (5.0 to 3.5 seconds, 2.0 to 2.8 times, with ten-minute blocks). Once
  shares are small, more peers help, with little gain past 32.

### Today

The before figures, from `cargo netsim sweep scaling-today`. Columns: the
share's bytes on the wire (with weak shares carried whole, those included);
stale and orphaned shares; mean seconds until 90% and all of the nodes had
each share; P2Pool's upload per node (shares, weak shares and acks), mean
and busiest node; uncles the final chain names per chain share; and the
header bytes a node syncing the chain fetches per chain share.

| Scenario | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | Uncles per chain share | Header-sync bytes per chain share |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| today, 100 nodes, 10 s shares | 30,525 | 2.85% | 0.00% | 0.30 | 0.35 | 0.171 / 0.271 | 0.029 | 315 |
| today, 100 nodes, 5 s shares | 35,337 | 5.43% | 0.00% | 0.32 | 0.37 | 0.393 / 0.622 | 0.057 | 324 |
| today, 100 nodes, 2 s shares | 36,295 | 13.34% | 0.07% | 0.33 | 0.38 | 1.01 / 1.60 | 0.153 | 357 |
| today, 100 nodes, 1 s shares | 36,295 | 23.14% | 0.34% | 0.33 | 0.38 | 2.03 / 3.20 | 0.297 | 408 |
| today, 1,000 nodes, 10 s shares | 30,525 | 3.83% | 0.00% | 0.43 | 0.52 | 0.171 / 0.319 | 0.040 | 318 |
| today, 1,000 nodes, 5 s shares | 35,337 | 8.01% | 0.01% | 0.46 | 0.55 | 0.392 / 0.733 | 0.087 | 335 |
| today, 1,000 nodes, 2 s shares | 36,295 | 17.88% | 0.09% | 0.47 | 0.56 | 1.00 / 1.88 | 0.216 | 379 |
| today, 1,000 nodes, 1 s shares | 36,295 | 30.42% | 0.62% | 0.47 | 0.56 | 2.02 / 3.77 | 0.428 | 455 |

### 1. Weak shares

`cargo netsim sweep scaling-weak-shares`:

| Scenario | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | Uncles per chain share | Header-sync bytes per chain share |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 16 weak shares a share, gossip only, 100 nodes, 10 s shares | 30,525 | 2.85% | 0.00% | 0.30 | 0.35 | 0.259 / 0.409 | 0.029 | 315 |
| 16 weak shares a share, gossip only, 100 nodes, 5 s shares | 35,337 | 5.43% | 0.00% | 0.32 | 0.37 | 0.569 / 0.898 | 0.057 | 324 |
| 16 weak shares a share, gossip only, 100 nodes, 2 s shares | 36,295 | 13.35% | 0.07% | 0.33 | 0.38 | 1.45 / 2.29 | 0.153 | 357 |
| 16 weak shares a share, gossip only, 100 nodes, 1 s shares | 36,295 | 23.16% | 0.34% | 0.33 | 0.38 | 2.91 / 4.58 | 0.297 | 408 |
| 16 weak shares a share, gossip only, 1,000 nodes, 10 s shares | 30,525 | 3.83% | 0.00% | 0.43 | 0.52 | 0.259 / 0.482 | 0.040 | 318 |
| 16 weak shares a share, gossip only, 1,000 nodes, 5 s shares | 35,337 | 8.01% | 0.01% | 0.46 | 0.55 | 0.568 / 1.06 | 0.087 | 335 |
| 16 weak shares a share, gossip only, 1,000 nodes, 2 s shares | 36,295 | 17.89% | 0.09% | 0.47 | 0.56 | 1.44 / 2.69 | 0.217 | 379 |
| 16 weak shares a share, gossip only, 1,000 nodes, 1 s shares | 36,295 | 30.42% | 0.64% | 0.47 | 0.56 | 2.90 / 5.41 | 0.428 | 455 |
| 16 weak shares a share, carried whole, 100 nodes, 10 s shares | 43,685 | 3.18% | 0.00% | 0.36 | 0.41 | 0.333 / 0.525 | 0.033 | 316 |
| 16 weak shares a share, carried whole, 100 nodes, 5 s shares | 48,497 | 6.06% | 0.00% | 0.38 | 0.43 | 0.716 / 1.13 | 0.064 | 327 |
| 16 weak shares a share, carried whole, 100 nodes, 2 s shares | 49,415 | 14.94% | 0.07% | 0.38 | 0.44 | 1.82 / 2.87 | 0.175 | 365 |
| 16 weak shares a share, carried whole, 100 nodes, 1 s shares | 49,415 | 25.50% | 0.50% | 0.39 | 0.45 | 3.64 / 5.74 | 0.336 | 422 |
| 16 weak shares a share, carried whole, 1,000 nodes, 10 s shares | 43,685 | 4.41% | 0.00% | 0.51 | 0.61 | 0.333 / 0.620 | 0.046 | 321 |
| 16 weak shares a share, carried whole, 1,000 nodes, 5 s shares | 48,497 | 9.06% | 0.01% | 0.54 | 0.64 | 0.715 / 1.33 | 0.100 | 339 |
| 16 weak shares a share, carried whole, 1,000 nodes, 2 s shares | 49,415 | 20.00% | 0.19% | 0.55 | 0.65 | 1.81 / 3.37 | 0.248 | 390 |
| 16 weak shares a share, carried whole, 1,000 nodes, 1 s shares | 49,415 | 33.31% | 1.10% | 0.71 | 0.81 | 3.63 / 6.77 | 0.483 | 475 |
| 32 weak shares a share, gossip only, 100 nodes, 10 s shares | 30,525 | 2.85% | 0.00% | 0.30 | 0.35 | 0.348 / 0.547 | 0.029 | 315 |
| 32 weak shares a share, gossip only, 100 nodes, 5 s shares | 35,337 | 5.43% | 0.00% | 0.32 | 0.37 | 0.746 / 1.17 | 0.057 | 324 |
| 32 weak shares a share, gossip only, 100 nodes, 2 s shares | 36,295 | 13.36% | 0.07% | 0.33 | 0.38 | 1.89 / 2.98 | 0.153 | 358 |
| 32 weak shares a share, gossip only, 100 nodes, 1 s shares | 36,295 | 23.18% | 0.35% | 0.33 | 0.38 | 3.79 / 5.97 | 0.297 | 408 |
| 32 weak shares a share, gossip only, 1,000 nodes, 10 s shares | 30,525 | 3.83% | 0.00% | 0.43 | 0.52 | 0.347 / 0.645 | 0.040 | 318 |
| 32 weak shares a share, gossip only, 1,000 nodes, 5 s shares | 35,337 | 8.01% | 0.01% | 0.46 | 0.55 | 0.745 / 1.39 | 0.087 | 335 |
| 32 weak shares a share, gossip only, 1,000 nodes, 2 s shares | 36,295 | 17.89% | 0.09% | 0.47 | 0.56 | 1.88 / 3.51 | 0.217 | 379 |
| 32 weak shares a share, gossip only, 1,000 nodes, 1 s shares | 36,295 | 30.43% | 0.69% | 0.66 | 0.75 | 3.78 / 7.04 | 0.427 | 455 |
| 32 weak shares a share, carried whole, 100 nodes, 10 s shares | 56,805 | 3.57% | 0.00% | 0.41 | 0.47 | 0.495 / 0.777 | 0.037 | 318 |
| 32 weak shares a share, carried whole, 100 nodes, 5 s shares | 61,657 | 6.70% | 0.00% | 0.43 | 0.49 | 1.04 / 1.63 | 0.072 | 329 |
| 32 weak shares a share, carried whole, 100 nodes, 2 s shares | 62,575 | 16.33% | 0.12% | 0.44 | 0.50 | 2.62 / 4.12 | 0.194 | 372 |
| 32 weak shares a share, carried whole, 100 nodes, 1 s shares | 62,575 | 27.77% | 0.75% | 0.45 | 0.51 | 5.26 / 8.28 | 0.374 | 436 |
| 32 weak shares a share, carried whole, 1,000 nodes, 10 s shares | 56,805 | 4.80% | 0.00% | 0.58 | 0.70 | 0.495 / 0.920 | 0.051 | 322 |
| 32 weak shares a share, carried whole, 1,000 nodes, 5 s shares | 61,657 | 10.10% | 0.06% | 0.61 | 0.73 | 1.04 / 1.93 | 0.112 | 343 |
| 32 weak shares a share, carried whole, 1,000 nodes, 2 s shares | 62,575 | 21.78% | 0.37% | 0.62 | 0.74 | 2.61 / 4.86 | 0.274 | 400 |
| 32 weak shares a share, carried whole, 1,000 nodes, 1 s shares | 62,575 | 36.07% | 1.45% | 2.10 | 2.23 | 5.25 / 9.77 | 0.541 | 497 |
| 64 weak shares a share, gossip only, 100 nodes, 10 s shares | 30,525 | 2.85% | 0.00% | 0.30 | 0.35 | 0.524 / 0.824 | 0.029 | 315 |
| 64 weak shares a share, gossip only, 100 nodes, 5 s shares | 35,337 | 5.43% | 0.00% | 0.32 | 0.37 | 1.10 / 1.72 | 0.057 | 324 |
| 64 weak shares a share, gossip only, 100 nodes, 2 s shares | 36,295 | 13.36% | 0.07% | 0.33 | 0.38 | 2.77 / 4.36 | 0.153 | 357 |
| 64 weak shares a share, gossip only, 100 nodes, 1 s shares | 36,295 | 23.25% | 0.35% | 0.33 | 0.38 | 5.56 / 8.74 | 0.298 | 408 |
| 64 weak shares a share, gossip only, 1,000 nodes, 10 s shares | 30,525 | 3.83% | 0.00% | 0.43 | 0.52 | 0.524 / 0.972 | 0.040 | 318 |
| 64 weak shares a share, gossip only, 1,000 nodes, 5 s shares | 35,337 | 8.01% | 0.01% | 0.46 | 0.55 | 1.10 / 2.04 | 0.087 | 335 |
| 64 weak shares a share, gossip only, 1,000 nodes, 2 s shares | 36,295 | 17.89% | 0.09% | 0.47 | 0.56 | 2.77 / 5.14 | 0.217 | 379 |
| 64 weak shares a share, gossip only, 1,000 nodes, 1 s shares | 36,295 | 30.52% | 0.81% | 2.10 | 2.19 | 5.55 / 10.3 | 0.427 | 455 |
| 64 weak shares a share, carried whole, 100 nodes, 10 s shares | 83,125 | 4.29% | 0.00% | 0.51 | 0.58 | 0.820 / 1.29 | 0.045 | 320 |
| 64 weak shares a share, carried whole, 100 nodes, 5 s shares | 87,937 | 8.03% | 0.01% | 0.53 | 0.60 | 1.69 / 2.65 | 0.087 | 335 |
| 64 weak shares a share, carried whole, 100 nodes, 2 s shares | 88,895 | 19.01% | 0.19% | 0.54 | 0.62 | 4.25 / 6.68 | 0.232 | 385 |
| 64 weak shares a share, carried whole, 100 nodes, 1 s shares | 88,895 | 32.27% | 2.29% | 61.66 | 61.74 | 8.52 / 13.4 | 0.443 | 461 |
| 64 weak shares a share, carried whole, 1,000 nodes, 10 s shares | 83,125 | 5.94% | 0.00% | 0.73 | 0.87 | 0.819 / 1.52 | 0.063 | 326 |
| 64 weak shares a share, carried whole, 1,000 nodes, 5 s shares | 87,937 | 11.97% | 0.02% | 0.75 | 0.90 | 1.68 / 3.13 | 0.136 | 351 |
| 64 weak shares a share, carried whole, 1,000 nodes, 2 s shares | 88,895 | 25.40% | 0.62% | 1.69 | 1.84 | 4.23 / 7.86 | 0.332 | 420 |
| 64 weak shares a share, carried whole, 1,000 nodes, 1 s shares | 88,895 | 40.75% | 3.89% | 16.59 | 16.75 | 8.50 / 15.8 | 0.622 | 528 |

Gossip only, weak shares cost upload, not stale shares: at 64 per share they
raise the mean upload per node from 0.17 to 0.52 Mbit/s at 10-second shares
and to 2.8 Mbit/s at 2-second shares, and every stale rate stays within 0.02
points of today's down to 2-second shares. At 1-second shares (64 weak
shares a second) the slowest links fall behind at times: on 1,000 nodes the
mean time to 90% rises from 0.47 to 2.10 seconds while the stale rate moves
from 30.42% to 30.52%, so the delays fall on few shares. Carried whole,
their bytes count like a larger coinbase: 64 per share make an 83 KB share,
and 1,000 nodes go 5.94% stale at 10-second shares.

### 2. Compact coinbase

`cargo netsim sweep scaling-compact-coinbase` (today's coinbase with whole
shares is the "Today" table; the "29 KB coinbase" rows name it at
10-second shares, and at shorter intervals it grows to 33 and 34 KB, which
without payouts only changes the validation time):

| Scenario | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | Uncles per chain share | Header-sync bytes per chain share |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 3 KB coinbase, whole shares, 100 nodes, 10 s shares | 4,109 | 2.05% | 0.00% | 0.19 | 0.22 | 0.023 / 0.037 | 0.021 | 312 |
| 3 KB coinbase, whole shares, 100 nodes, 5 s shares | 4,109 | 3.52% | 0.00% | 0.19 | 0.22 | 0.046 / 0.073 | 0.036 | 317 |
| 3 KB coinbase, whole shares, 100 nodes, 2 s shares | 4,109 | 8.86% | 0.01% | 0.19 | 0.22 | 0.116 / 0.184 | 0.097 | 338 |
| 3 KB coinbase, whole shares, 100 nodes, 1 s shares | 4,109 | 16.02% | 0.07% | 0.19 | 0.22 | 0.233 / 0.369 | 0.190 | 370 |
| 3 KB coinbase, whole shares, 1,000 nodes, 10 s shares | 4,109 | 2.59% | 0.00% | 0.26 | 0.31 | 0.023 / 0.044 | 0.027 | 314 |
| 3 KB coinbase, whole shares, 1,000 nodes, 5 s shares | 4,109 | 4.84% | 0.00% | 0.26 | 0.31 | 0.046 / 0.086 | 0.051 | 322 |
| 3 KB coinbase, whole shares, 1,000 nodes, 2 s shares | 4,109 | 12.04% | 0.02% | 0.26 | 0.31 | 0.115 / 0.215 | 0.137 | 352 |
| 3 KB coinbase, whole shares, 1,000 nodes, 1 s shares | 4,109 | 20.86% | 0.23% | 0.26 | 0.31 | 0.232 / 0.434 | 0.261 | 395 |
| 200 KiB coinbase, whole shares, 100 nodes, 10 s shares | 211,241 | 7.64% | 0.01% | 0.99 | 1.13 | 1.19 / 1.87 | 0.083 | 333 |
| 200 KiB coinbase, whole shares, 100 nodes, 5 s shares | 211,241 | 14.00% | 0.08% | 1.00 | 1.14 | 2.36 / 3.72 | 0.162 | 360 |
| 200 KiB coinbase, whole shares, 100 nodes, 2 s shares | 211,241 | 29.82% | 1.11% | 1.11 | 1.25 | 5.91 / 9.31 | 0.409 | 449 |
| 200 KiB coinbase, whole shares, 100 nodes, 1 s shares | 211,241 | 45.81% | 9.70% | 689.11 | 689.26 | 11.8 / 18.7 | 0.666 | 545 |
| 200 KiB coinbase, whole shares, 1,000 nodes, 10 s shares | 211,241 | 11.09% | 0.00% | 1.42 | 1.71 | 1.19 / 2.21 | 0.125 | 348 |
| 200 KiB coinbase, whole shares, 1,000 nodes, 5 s shares | 211,241 | 19.49% | 0.19% | 1.44 | 1.73 | 2.35 / 4.38 | 0.240 | 388 |
| 200 KiB coinbase, whole shares, 1,000 nodes, 2 s shares | 211,241 | 37.90% | 2.22% | 7.31 | 7.62 | 5.86 / 10.9 | 0.574 | 510 |
| 200 KiB coinbase, whole shares, 1,000 nodes, 1 s shares | 211,241 | 55.67% | 11.57% | 77.57 | 77.94 | 11.8 / 21.9 | 0.994 | 676 |
| 29 KB coinbase, without payouts, 100 nodes, 10 s shares | 1,301 | 1.95% | 0.00% | 0.17 | 0.20 | 7.8e-3 / 0.012 | 0.020 | 312 |
| 29 KB coinbase, without payouts, 100 nodes, 5 s shares | 1,301 | 3.37% | 0.00% | 0.17 | 0.20 | 0.015 / 0.024 | 0.035 | 317 |
| 29 KB coinbase, without payouts, 100 nodes, 2 s shares | 1,301 | 8.48% | 0.01% | 0.17 | 0.20 | 0.039 / 0.061 | 0.093 | 337 |
| 29 KB coinbase, without payouts, 100 nodes, 1 s shares | 1,301 | 15.34% | 0.06% | 0.17 | 0.20 | 0.078 / 0.122 | 0.181 | 367 |
| 29 KB coinbase, without payouts, 1,000 nodes, 10 s shares | 1,301 | 2.48% | 0.00% | 0.24 | 0.29 | 7.8e-3 / 0.014 | 0.025 | 314 |
| 29 KB coinbase, without payouts, 1,000 nodes, 5 s shares | 1,301 | 4.56% | 0.00% | 0.24 | 0.29 | 0.015 / 0.029 | 0.048 | 321 |
| 29 KB coinbase, without payouts, 1,000 nodes, 2 s shares | 1,301 | 11.36% | 0.01% | 0.24 | 0.29 | 0.038 / 0.071 | 0.128 | 349 |
| 29 KB coinbase, without payouts, 1,000 nodes, 1 s shares | 1,301 | 19.97% | 0.19% | 0.24 | 0.29 | 0.077 / 0.144 | 0.247 | 390 |
| 200 KiB coinbase, without payouts, 100 nodes, 10 s shares | 1,301 | 2.09% | 0.00% | 0.19 | 0.22 | 7.6e-3 / 0.012 | 0.021 | 312 |
| 200 KiB coinbase, without payouts, 100 nodes, 5 s shares | 1,301 | 3.56% | 0.00% | 0.19 | 0.22 | 0.015 / 0.024 | 0.037 | 317 |
| 200 KiB coinbase, without payouts, 100 nodes, 2 s shares | 1,301 | 8.97% | 0.01% | 0.19 | 0.22 | 0.038 / 0.060 | 0.098 | 339 |
| 200 KiB coinbase, without payouts, 100 nodes, 1 s shares | 1,301 | 16.25% | 0.07% | 0.19 | 0.22 | 0.077 / 0.121 | 0.193 | 371 |
| 200 KiB coinbase, without payouts, 1,000 nodes, 10 s shares | 1,301 | 2.61% | 0.00% | 0.26 | 0.31 | 7.6e-3 / 0.014 | 0.027 | 314 |
| 200 KiB coinbase, without payouts, 1,000 nodes, 5 s shares | 1,301 | 4.91% | 0.00% | 0.26 | 0.31 | 0.015 / 0.028 | 0.052 | 323 |
| 200 KiB coinbase, without payouts, 1,000 nodes, 2 s shares | 1,301 | 12.12% | 0.03% | 0.26 | 0.31 | 0.038 / 0.071 | 0.137 | 352 |
| 200 KiB coinbase, without payouts, 1,000 nodes, 1 s shares | 1,301 | 21.13% | 0.24% | 0.26 | 0.31 | 0.076 / 0.143 | 0.265 | 396 |

Without payouts a share is 1,301 bytes whatever the payees, and the stale
rate no longer depends on the coinbase: on 1,000 nodes at 2-second shares,
11.36% (today's coinbase) and 12.12% (200 KiB) against 17.88% and 37.90%
sent whole. What is left is about what a 3 KB coinbase sent whole costs.

### 3. Relay before full validation

`cargo netsim sweep scaling-header-check`:

| Scenario | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | Uncles per chain share | Header-sync bytes per chain share |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| forwarded after the header check, 100 nodes, 10 s shares | 30,525 | 2.74% | 0.00% | 0.29 | 0.33 | 0.173 / 0.272 | 0.028 | 314 |
| forwarded after the header check, 100 nodes, 5 s shares | 35,337 | 5.27% | 0.00% | 0.31 | 0.36 | 0.397 / 0.625 | 0.056 | 324 |
| forwarded after the header check, 100 nodes, 2 s shares | 36,295 | 12.94% | 0.06% | 0.31 | 0.36 | 1.02 / 1.61 | 0.148 | 356 |
| forwarded after the header check, 100 nodes, 1 s shares | 36,295 | 22.47% | 0.31% | 0.32 | 0.36 | 2.05 / 3.22 | 0.286 | 404 |
| forwarded after the header check, 1,000 nodes, 10 s shares | 30,525 | 3.68% | 0.00% | 0.41 | 0.49 | 0.173 / 0.321 | 0.038 | 318 |
| forwarded after the header check, 1,000 nodes, 5 s shares | 35,337 | 7.67% | 0.00% | 0.44 | 0.53 | 0.396 / 0.737 | 0.083 | 333 |
| forwarded after the header check, 1,000 nodes, 2 s shares | 36,295 | 17.36% | 0.07% | 0.45 | 0.53 | 1.01 / 1.89 | 0.209 | 377 |
| forwarded after the header check, 1,000 nodes, 1 s shares | 36,295 | 29.50% | 0.58% | 0.45 | 0.54 | 2.04 / 3.79 | 0.410 | 449 |

On its own, a small gain: 3.68% against 3.83% on 1,000 nodes at 10-second
shares, and 0.41 against 0.43 seconds to 90% of the nodes.

### 4. Fewer hops

`cargo netsim sweep scaling-peers`:

| Scenario | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | Uncles per chain share | Header-sync bytes per chain share |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 16 peers, 100 nodes, 10 s shares | 30,525 | 3.10% | 0.00% | 0.33 | 0.38 | 0.369 / 0.493 | 0.032 | 316 |
| 16 peers, 100 nodes, 5 s shares | 35,337 | 6.12% | 0.00% | 0.36 | 0.41 | 0.847 / 1.13 | 0.065 | 327 |
| 16 peers, 100 nodes, 2 s shares | 36,295 | 14.81% | 0.07% | 0.38 | 0.42 | 2.18 / 2.92 | 0.173 | 364 |
| 16 peers, 100 nodes, 1 s shares | 36,295 | 25.77% | 0.59% | 0.39 | 0.44 | 4.37 / 5.85 | 0.339 | 423 |
| 16 peers, 1,000 nodes, 10 s shares | 30,525 | 4.08% | 0.00% | 0.44 | 0.51 | 0.368 / 0.664 | 0.042 | 319 |
| 16 peers, 1,000 nodes, 5 s shares | 35,337 | 8.27% | 0.02% | 0.48 | 0.55 | 0.845 / 1.53 | 0.090 | 336 |
| 16 peers, 1,000 nodes, 2 s shares | 36,295 | 18.66% | 0.13% | 0.49 | 0.57 | 2.16 / 3.90 | 0.228 | 383 |
| 16 peers, 1,000 nodes, 1 s shares | 36,295 | 31.58% | 1.20% | 0.93 | 1.01 | 4.35 / 7.85 | 0.444 | 461 |
| 32 peers, 100 nodes, 10 s shares | 30,525 | 4.30% | 0.00% | 0.44 | 0.48 | 0.763 / 0.986 | 0.045 | 320 |
| 32 peers, 100 nodes, 5 s shares | 35,337 | 8.58% | 0.03% | 0.51 | 0.55 | 1.75 / 2.27 | 0.094 | 337 |
| 32 peers, 100 nodes, 2 s shares | 36,295 | 19.95% | 0.49% | 0.59 | 0.63 | 4.52 / 5.84 | 0.243 | 389 |
| 32 peers, 100 nodes, 1 s shares | 36,295 | 33.84% | 8.21% | 536.81 | 536.86 | 9.06 / 11.7 | 0.387 | 441 |
| 32 peers, 1,000 nodes, 10 s shares | 30,525 | 5.08% | 0.00% | 0.57 | 0.65 | 0.764 / 1.08 | 0.054 | 323 |
| 32 peers, 1,000 nodes, 5 s shares | 35,337 | 10.72% | 0.05% | 0.66 | 0.74 | 1.75 / 2.49 | 0.120 | 346 |
| 32 peers, 1,000 nodes, 2 s shares | 36,295 | 23.70% | 0.76% | 18.38 | 18.47 | 4.49 / 6.37 | 0.301 | 409 |
| 32 peers, 1,000 nodes, 1 s shares | 36,295 | 39.15% | 5.08% | 47.52 | 47.61 | 9.03 / 12.8 | 0.560 | 504 |
| 64 peers, 100 nodes, 10 s shares | 30,525 | 5.73% | 0.00% | 0.66 | 0.71 | 1.56 / 1.83 | 0.061 | 326 |
| 64 peers, 100 nodes, 5 s shares | 35,337 | 12.01% | 0.05% | 0.77 | 0.83 | 3.57 / 4.20 | 0.136 | 352 |
| 64 peers, 100 nodes, 2 s shares | 36,295 | 28.29% | 2.72% | 3.38 | 3.44 | 9.20 / 10.8 | 0.357 | 430 |
| 64 peers, 100 nodes, 1 s shares | 36,295 | 45.16% | 14.68% | 2105.04 | 2105.11 | 18.5 / 21.7 | 0.556 | 504 |
| 64 peers, 1,000 nodes, 10 s shares | 30,525 | 6.70% | 0.00% | 0.80 | 0.88 | 1.55 / 1.95 | 0.072 | 329 |
| 64 peers, 1,000 nodes, 5 s shares | 35,337 | 14.66% | 0.32% | 2.67 | 2.76 | 3.57 / 4.48 | 0.168 | 363 |
| 64 peers, 1,000 nodes, 2 s shares | 36,295 | 31.27% | 3.81% | 88.79 | 88.89 | 9.13 / 11.5 | 0.400 | 445 |
| 64 peers, 1,000 nodes, 1 s shares | 36,295 | 48.65% | 15.32% | 287.90 | 288.00 | 18.4 / 23.1 | 0.649 | 538 |

With whole shares more peers are worse at every size and interval. A node
pushes a whole share to every peer not known to have it, so each node uploads
about as many copies as it has peers: 1.55 Mbit/s mean at 64 peers against
0.17 at 8 (10-second shares). At 2-second shares 64 peers saturate the slower
links, and the mean time to 90% of 1,000 nodes reaches 89 seconds.

### 5. Uncle cap

`cargo netsim sweep scaling-uncle-cap`:

| Scenario | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | Uncles per chain share | Header-sync bytes per chain share |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| uncle cap 6, 100 nodes, 10 s shares | 30,525 | 2.85% | 0.00% | 0.30 | 0.35 | 0.171 / 0.271 | 0.029 | 315 |
| uncle cap 6, 100 nodes, 5 s shares | 35,337 | 5.43% | 0.00% | 0.32 | 0.37 | 0.393 / 0.622 | 0.057 | 324 |
| uncle cap 6, 100 nodes, 2 s shares | 36,295 | 13.34% | 0.07% | 0.33 | 0.38 | 1.01 / 1.60 | 0.153 | 357 |
| uncle cap 6, 100 nodes, 1 s shares | 36,295 | 23.14% | 0.34% | 0.33 | 0.38 | 2.03 / 3.20 | 0.297 | 408 |
| uncle cap 6, 1,000 nodes, 10 s shares | 30,525 | 3.83% | 0.00% | 0.43 | 0.52 | 0.171 / 0.319 | 0.040 | 318 |
| uncle cap 6, 1,000 nodes, 5 s shares | 35,337 | 8.01% | 0.01% | 0.46 | 0.55 | 0.392 / 0.733 | 0.087 | 335 |
| uncle cap 6, 1,000 nodes, 2 s shares | 36,295 | 17.88% | 0.09% | 0.47 | 0.56 | 1.00 / 1.88 | 0.216 | 379 |
| uncle cap 6, 1,000 nodes, 1 s shares | 36,295 | 30.42% | 0.62% | 0.47 | 0.56 | 2.02 / 3.77 | 0.428 | 455 |
| uncle cap 12, 100 nodes, 10 s shares | 30,525 | 2.85% | 0.00% | 0.30 | 0.35 | 0.171 / 0.271 | 0.029 | 315 |
| uncle cap 12, 100 nodes, 5 s shares | 35,337 | 5.43% | 0.00% | 0.32 | 0.37 | 0.393 / 0.622 | 0.057 | 324 |
| uncle cap 12, 100 nodes, 2 s shares | 36,295 | 13.34% | 0.07% | 0.33 | 0.38 | 1.01 / 1.60 | 0.153 | 357 |
| uncle cap 12, 100 nodes, 1 s shares | 36,295 | 23.14% | 0.34% | 0.33 | 0.38 | 2.03 / 3.20 | 0.297 | 408 |
| uncle cap 12, 1,000 nodes, 10 s shares | 30,525 | 3.83% | 0.00% | 0.43 | 0.52 | 0.171 / 0.319 | 0.040 | 318 |
| uncle cap 12, 1,000 nodes, 5 s shares | 35,337 | 8.01% | 0.01% | 0.46 | 0.55 | 0.392 / 0.733 | 0.087 | 335 |
| uncle cap 12, 1,000 nodes, 2 s shares | 36,295 | 17.88% | 0.09% | 0.47 | 0.56 | 1.00 / 1.88 | 0.216 | 379 |
| uncle cap 12, 1,000 nodes, 1 s shares | 36,295 | 30.42% | 0.62% | 0.47 | 0.56 | 2.02 / 3.77 | 0.428 | 455 |

Every row is the same as today's: no share in these runs had a fourth uncle
to name, so a larger cap changes nothing and costs no extra header sync.
Smaller caps do change the runs: 1,000 nodes at 1-second shares are 0.62%
orphaned with a cap of 3, 0.66% with 2 and 1.93% with 1 (`--max-uncles`,
one run each; the stale rate is 30.42% in all three, since uncles do not
change which chain wins in this model). The orphans left at a cap of 3 are
shares no share on the final chain could name within 3 shares of depth. The
header-sync work grows with the share rate whatever the cap: on 1,000 nodes
318 bytes per chain share at 10-second shares, 379 at 2 seconds and 455 at
1 second (0.040, 0.216 and 0.428 uncles per chain share).

### 6. The combination

`cargo netsim sweep scaling-combined`:

| Scenario | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | Uncles per chain share | Header-sync bytes per chain share |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| without payouts, header check, 8 peers, 100 nodes, 10 s shares | 1,301 | 1.84% | 0.00% | 0.16 | 0.18 | 7.9e-3 / 0.012 | 0.019 | 311 |
| without payouts, header check, 8 peers, 100 nodes, 5 s shares | 1,301 | 3.17% | 0.00% | 0.16 | 0.18 | 0.016 / 0.024 | 0.033 | 316 |
| without payouts, header check, 8 peers, 100 nodes, 2 s shares | 1,301 | 7.94% | 0.01% | 0.16 | 0.18 | 0.039 / 0.061 | 0.086 | 334 |
| without payouts, header check, 8 peers, 100 nodes, 1 s shares | 1,301 | 14.50% | 0.06% | 0.16 | 0.18 | 0.079 / 0.123 | 0.169 | 363 |
| without payouts, header check, 8 peers, 1,000 nodes, 10 s shares | 1,301 | 2.30% | 0.00% | 0.22 | 0.26 | 7.9e-3 / 0.015 | 0.023 | 313 |
| without payouts, header check, 8 peers, 1,000 nodes, 5 s shares | 1,301 | 4.30% | 0.00% | 0.22 | 0.26 | 0.016 / 0.029 | 0.045 | 320 |
| without payouts, header check, 8 peers, 1,000 nodes, 2 s shares | 1,301 | 10.65% | 0.02% | 0.22 | 0.26 | 0.039 / 0.072 | 0.119 | 346 |
| without payouts, header check, 8 peers, 1,000 nodes, 1 s shares | 1,301 | 18.80% | 0.14% | 0.22 | 0.26 | 0.079 / 0.146 | 0.230 | 384 |
| without payouts, header check, 16 peers, 100 nodes, 10 s shares | 1,301 | 1.57% | 0.00% | 0.12 | 0.13 | 0.017 / 0.023 | 0.016 | 310 |
| without payouts, header check, 16 peers, 100 nodes, 5 s shares | 1,301 | 2.64% | 0.00% | 0.12 | 0.13 | 0.034 / 0.045 | 0.027 | 314 |
| without payouts, header check, 16 peers, 100 nodes, 2 s shares | 1,301 | 6.65% | 0.00% | 0.12 | 0.13 | 0.085 / 0.113 | 0.071 | 329 |
| without payouts, header check, 16 peers, 100 nodes, 1 s shares | 1,301 | 12.42% | 0.02% | 0.12 | 0.13 | 0.170 / 0.227 | 0.142 | 353 |
| without payouts, header check, 16 peers, 1,000 nodes, 10 s shares | 1,301 | 2.00% | 0.00% | 0.16 | 0.19 | 0.017 / 0.030 | 0.020 | 312 |
| without payouts, header check, 16 peers, 1,000 nodes, 5 s shares | 1,301 | 3.37% | 0.00% | 0.16 | 0.19 | 0.034 / 0.060 | 0.035 | 317 |
| without payouts, header check, 16 peers, 1,000 nodes, 2 s shares | 1,301 | 8.70% | 0.00% | 0.16 | 0.19 | 0.084 / 0.150 | 0.095 | 337 |
| without payouts, header check, 16 peers, 1,000 nodes, 1 s shares | 1,301 | 15.31% | 0.07% | 0.16 | 0.19 | 0.169 / 0.303 | 0.180 | 367 |
| without payouts, header check, 32 peers, 100 nodes, 10 s shares | 1,301 | 1.48% | 0.00% | 0.10 | 0.11 | 0.035 / 0.045 | 0.015 | 310 |
| without payouts, header check, 32 peers, 100 nodes, 5 s shares | 1,301 | 2.48% | 0.00% | 0.10 | 0.11 | 0.070 / 0.090 | 0.025 | 314 |
| without payouts, header check, 32 peers, 100 nodes, 2 s shares | 1,301 | 6.32% | 0.00% | 0.10 | 0.11 | 0.175 / 0.225 | 0.067 | 328 |
| without payouts, header check, 32 peers, 100 nodes, 1 s shares | 1,301 | 11.55% | 0.02% | 0.10 | 0.11 | 0.351 / 0.451 | 0.130 | 349 |
| without payouts, header check, 32 peers, 1,000 nodes, 10 s shares | 1,301 | 1.72% | 0.00% | 0.14 | 0.16 | 0.035 / 0.050 | 0.018 | 311 |
| without payouts, header check, 32 peers, 1,000 nodes, 5 s shares | 1,301 | 3.19% | 0.00% | 0.14 | 0.16 | 0.070 / 0.099 | 0.033 | 316 |
| without payouts, header check, 32 peers, 1,000 nodes, 2 s shares | 1,301 | 7.98% | 0.00% | 0.14 | 0.16 | 0.174 / 0.247 | 0.087 | 334 |
| without payouts, header check, 32 peers, 1,000 nodes, 1 s shares | 1,301 | 14.30% | 0.08% | 0.14 | 0.16 | 0.350 / 0.497 | 0.166 | 362 |
| without payouts, header check, 64 peers, 100 nodes, 10 s shares | 1,301 | 1.51% | 0.00% | 0.10 | 0.11 | 0.071 / 0.084 | 0.015 | 310 |
| without payouts, header check, 64 peers, 100 nodes, 5 s shares | 1,301 | 2.44% | 0.00% | 0.10 | 0.11 | 0.142 / 0.167 | 0.025 | 313 |
| without payouts, header check, 64 peers, 100 nodes, 2 s shares | 1,301 | 6.18% | 0.00% | 0.10 | 0.11 | 0.356 / 0.418 | 0.066 | 327 |
| without payouts, header check, 64 peers, 100 nodes, 1 s shares | 1,301 | 11.34% | 0.03% | 0.10 | 0.11 | 0.714 / 0.840 | 0.127 | 348 |
| without payouts, header check, 64 peers, 1,000 nodes, 10 s shares | 1,301 | 1.79% | 0.00% | 0.13 | 0.15 | 0.071 / 0.090 | 0.018 | 311 |
| without payouts, header check, 64 peers, 1,000 nodes, 5 s shares | 1,301 | 3.01% | 0.00% | 0.13 | 0.15 | 0.142 / 0.178 | 0.031 | 316 |
| without payouts, header check, 64 peers, 1,000 nodes, 2 s shares | 1,301 | 7.77% | 0.00% | 0.13 | 0.15 | 0.353 / 0.443 | 0.084 | 334 |
| without payouts, header check, 64 peers, 1,000 nodes, 1 s shares | 1,301 | 13.96% | 0.06% | 0.13 | 0.15 | 0.712 / 0.893 | 0.161 | 360 |

On 1,000 nodes at 2-second shares the combination is 10.65% stale at 8 peers
and 7.77% at 64, against today's 17.88%; at 1-second shares 18.80% to 13.96%
against 30.42%, with almost no orphans. The busiest node uploads under 1
Mbit/s even at 64 peers and 1-second shares.

### What was approximated

Beyond the model's own assumptions (fluid links, no loss, the assumed link
mix, estimated validation and job-switch times):

- **Weak shares:** the 800-byte payload is an estimate inside the design's
  0.5 to 1.2 KB. A weak share is validated in a share's validation time
  (5.6 to 5.7 ms), and validation is not a queue: like share validation in this
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
  not a stale rate; reading it as 1,000 nodes at today's 3.83% (4.23% with
  ten-minute blocks) is this study's choice.

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
cost at level 0. These costs were measured before one-minute blocks, with a
ten-minute block's 3.125 BCH and a 120,960 share window, and the limits
study below still uses them. With one-minute blocks a 0.3125 BCH block pays
at most about 1,790 outputs at level 5, so the 100,000-miner rows overstate
the coinbase build, and the window of any pool above 0.015% of BCH holds
fewer shares than the measured 120,960, so the walk is shorter too; the
timing tests have to be run again for one-minute numbers. The walk in the
table above has no weak-share credits in
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
window. It is 4,109 bytes on the wire whatever it pays.

| Scenario | Share bytes | Validation (ms) | Stale | To all (s) | P2Pool up, mean / max (Mbit/s) | Uncles per chain share | Validation busy, mean / max | Validation wait (ms) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| whole, 1,000 miners, 10 s | 36,261 | 6.4 | 4.15% | 0.56 | 0.202 / 0.379 | 0.043 | 0.06% / 0.06% | 0.0 |
| without payouts, 1,000 miners, 10 s | 1,301 | 6.4 | 2.52% | 0.29 | 7.7e-3 / 0.014 | 0.026 | 0.06% / 0.06% | 0.0 |
| token share, 1,000 miners, 10 s | 4,109 | 7.3 | 2.69% | 0.33 | 0.023 / 0.043 | 0.028 | 0.07% / 0.07% | 0.0 |
| whole, 10,000 miners, 10 s | 350,621 | 37.9 | 16.46% | 2.79 | 1.94 / 3.64 | 0.196 | 0.38% / 0.38% | 0.1 |
| without payouts, 10,000 miners, 10 s | 1,301 | 37.9 | 3.87% | 0.50 | 6.9e-3 / 0.013 | 0.040 | 0.38% / 0.38% | 0.1 |
| token share, 10,000 miners, 10 s | 4,109 | 44.6 | 4.24% | 0.56 | 0.021 / 0.040 | 0.044 | 0.45% / 0.45% | 0.1 |
| whole, 100,000 miners, 10 s | 626,003 | 361.5 | 32.56% | 6.85 | 3.23 / 6.21 | 0.466 | 3.65% / 3.65% | 5.1 |
| without payouts, 100,000 miners, 10 s | 1,301 | 361.5 | 16.37% | 2.51 | 5.7e-3 / 0.012 | 0.195 | 3.65% / 3.65% | 5.5 |
| token share, 100,000 miners, 10 s | 4,109 | 376.0 | 17.03% | 2.62 | 0.017 / 0.036 | 0.204 | 3.79% / 3.79% | 5.9 |
| whole, 1,000 miners, 2 s | 36,261 | 6.4 | 18.00% | 0.56 | 1.00 / 1.87 | 0.218 | 0.32% / 0.32% | 0.0 |
| without payouts, 1,000 miners, 2 s | 1,301 | 6.4 | 11.53% | 0.29 | 0.038 / 0.071 | 0.130 | 0.32% / 0.32% | 0.0 |
| token share, 1,000 miners, 2 s | 4,109 | 7.3 | 12.42% | 0.33 | 0.114 / 0.215 | 0.141 | 0.37% / 0.37% | 0.0 |
| whole, 10,000 miners, 2 s | 350,621 | 37.9 | 50.02% | 76.89 | 9.62 / 18.0 | 0.841 | 1.89% / 1.89% | 0.3 |
| without payouts, 10,000 miners, 2 s | 1,301 | 37.9 | 17.24% | 0.50 | 0.034 / 0.066 | 0.207 | 1.89% / 1.89% | 0.3 |
| token share, 10,000 miners, 2 s | 4,109 | 44.6 | 18.62% | 0.57 | 0.102 / 0.199 | 0.227 | 2.22% / 2.22% | 0.4 |
| whole, 100,000 miners, 2 s | 626,003 | 361.5 | 70.63% | 390.87 | 16.0 / 30.7 | 1.578 | 18.01% / 18.01% | 27.8 |
| without payouts, 100,000 miners, 2 s | 1,301 | 361.5 | 49.29% | 2.73 | 0.028 / 0.057 | 0.874 | 18.01% / 18.01% | 31.7 |
| token share, 100,000 miners, 2 s | 4,109 | 376.0 | 50.41% | 2.86 | 0.082 / 0.173 | 0.914 | 18.73% / 18.73% | 34.7 |

At 1,000 and 10,000 miners, shares without payouts keep the stale rate at
2.5% and 3.9% at 10-second shares, against 4.2% and 16% whole; token shares
come within half a point of that (2.7% and 4.2%). At 100,000 miners
validation itself (362 ms at each hop) keeps every form at 16% or more.
These rows pay the measured miner counts directly, so one-minute blocks
change them only through the smaller merkle branch and the minute blocks
on the links: within 0.7 points of the ten-minute tables.

### 2. Relaying before full validation

| Scenario | Share bytes | Validation (ms) | Stale | To all (s) | P2Pool up, mean / max (Mbit/s) | Uncles per chain share | Validation busy, mean / max | Validation wait (ms) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| after validation, 1,000 miners, 10 s | 36,261 | 6.4 | 4.15% | 0.56 | 0.202 / 0.379 | 0.043 | 0.06% / 0.06% | 0.0 |
| after the header check, 1,000 miners, 10 s | 36,261 | 6.4 | 3.91% | 0.53 | 0.205 / 0.381 | 0.041 | 0.06% / 0.06% | 0.0 |
| after validation, 10,000 miners, 10 s | 350,621 | 37.9 | 16.46% | 2.79 | 1.94 / 3.64 | 0.196 | 0.38% / 0.38% | 0.1 |
| after the header check, 10,000 miners, 10 s | 350,621 | 37.9 | 15.55% | 2.58 | 1.98 / 3.68 | 0.183 | 0.38% / 0.38% | 0.1 |
| after validation, 100,000 miners, 10 s | 626,003 | 361.5 | 32.56% | 6.85 | 3.23 / 6.21 | 0.466 | 3.65% / 3.65% | 5.1 |
| after the header check, 100,000 miners, 10 s | 626,003 | 361.5 | 24.68% | 8.05 | 3.54 / 6.57 | 0.322 | 3.65% / 3.65% | 4.8 |
| after validation, 1,000 miners, 2 s | 36,261 | 6.4 | 18.00% | 0.56 | 1.00 / 1.87 | 0.218 | 0.32% / 0.32% | 0.0 |
| after the header check, 1,000 miners, 2 s | 36,261 | 6.4 | 17.39% | 0.53 | 1.01 / 1.88 | 0.209 | 0.32% / 0.32% | 0.0 |
| after validation, 10,000 miners, 2 s | 350,621 | 37.9 | 50.02% | 76.89 | 9.62 / 18.0 | 0.841 | 1.89% / 1.89% | 0.3 |
| after the header check, 10,000 miners, 2 s | 350,621 | 37.9 | 47.95% | 81.35 | 9.78 / 18.2 | 0.780 | 1.89% / 1.89% | 0.3 |
| after validation, 100,000 miners, 2 s | 626,003 | 361.5 | 70.63% | 390.87 | 16.0 / 30.7 | 1.578 | 18.01% / 18.01% | 27.8 |
| after the header check, 100,000 miners, 2 s | 626,003 | 361.5 | 63.01% | 479.47 | 17.5 / 32.4 | 1.204 | 18.01% / 18.01% | 26.0 |

Upstream is right: forwarding after the header check takes 0.2 to 2.1
points off the stale rate at 1,000 and 10,000 miners. It only matters when
validation is slow: 33% to 25% at 100,000 miners.

### 3. More connections against upload bandwidth

| Scenario | Share bytes | Validation (ms) | Stale | To all (s) | P2Pool up, mean / max (Mbit/s) | Uncles per chain share | Validation busy, mean / max | Validation wait (ms) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| whole, 10,000 miners, 20 Mbit/s up, 8 peers | 350,621 | 37.9 | 31.27% | 6.31 | 1.96 / 3.66 | 0.443 | 0.38% / 0.38% | 0.1 |
| whole, 10,000 miners, 20 Mbit/s up, 16 peers | 350,621 | 37.9 | 41.25% | 9.81 | 4.23 / 7.63 | 0.657 | 0.38% / 0.38% | 0.1 |
| whole, 10,000 miners, 20 Mbit/s up, 32 peers | 350,621 | 37.9 | 56.79% | 17.98 | 8.76 / 12.4 | 1.098 | 0.38% / 0.38% | 0.1 |
| whole, 10,000 miners, 20 Mbit/s up, 64 peers | 350,621 | 37.9 | 81.52% | 347.47 | 17.8 / 22.4 | 1.866 | 0.38% / 0.38% | 0.1 |
| without payouts, 10,000 miners, 20 Mbit/s up, 8 peers | 1,301 | 37.9 | 3.93% | 0.51 | 6.9e-3 / 0.013 | 0.041 | 0.38% / 0.38% | 0.1 |
| without payouts, 10,000 miners, 20 Mbit/s up, 16 peers | 1,301 | 37.9 | 3.31% | 0.39 | 0.015 / 0.028 | 0.034 | 0.38% / 0.38% | 0.1 |
| without payouts, 10,000 miners, 20 Mbit/s up, 32 peers | 1,301 | 37.9 | 3.10% | 0.34 | 0.032 / 0.047 | 0.032 | 0.38% / 0.38% | 0.1 |
| without payouts, 10,000 miners, 20 Mbit/s up, 64 peers | 1,301 | 37.9 | 2.87% | 0.35 | 0.069 / 0.087 | 0.030 | 0.38% / 0.38% | 0.1 |
| whole, 10,000 miners, 100 Mbit/s up, 8 peers | 350,621 | 37.9 | 10.75% | 1.59 | 1.91 / 3.61 | 0.121 | 0.38% / 0.38% | 0.1 |
| whole, 10,000 miners, 100 Mbit/s up, 16 peers | 350,621 | 37.9 | 13.32% | 2.08 | 4.17 / 7.55 | 0.153 | 0.38% / 0.38% | 0.1 |
| whole, 10,000 miners, 100 Mbit/s up, 32 peers | 350,621 | 37.9 | 19.14% | 3.04 | 8.69 / 12.4 | 0.234 | 0.38% / 0.38% | 0.1 |
| whole, 10,000 miners, 100 Mbit/s up, 64 peers | 350,621 | 37.9 | 28.23% | 5.62 | 17.8 / 22.3 | 0.382 | 0.38% / 0.38% | 0.1 |
| without payouts, 10,000 miners, 100 Mbit/s up, 8 peers | 1,301 | 37.9 | 3.82% | 0.49 | 6.9e-3 / 0.013 | 0.040 | 0.38% / 0.38% | 0.1 |
| without payouts, 10,000 miners, 100 Mbit/s up, 16 peers | 1,301 | 37.9 | 3.09% | 0.37 | 0.015 / 0.028 | 0.032 | 0.38% / 0.38% | 0.1 |
| without payouts, 10,000 miners, 100 Mbit/s up, 32 peers | 1,301 | 37.9 | 2.82% | 0.30 | 0.032 / 0.046 | 0.029 | 0.38% / 0.38% | 0.1 |
| without payouts, 10,000 miners, 100 Mbit/s up, 64 peers | 1,301 | 37.9 | 2.53% | 0.26 | 0.067 / 0.085 | 0.026 | 0.38% / 0.38% | 0.1 |

Upstream is right for whole shares: at 10,000 miners (a 350 kB share) more
peers raise the stale rate, from 31% at 8 peers to 82% at 64 on a 20 Mbit/s
upload, and from 11% to 28% on 100 Mbit/s. Only with shares sent without
payouts (1.3 kB) do more peers help, from 3.9% to 2.9% on 20 Mbit/s and 3.8%
to 2.5% on 100.

### 4. The uncle cap

| Scenario | Share bytes | Validation (ms) | Stale | To all (s) | P2Pool up, mean / max (Mbit/s) | Uncles per chain share | Validation busy, mean / max | Validation wait (ms) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| uncle cap 3, 10,000 miners, 10 s | 350,621 | 37.9 | 16.46% | 2.79 | 1.94 / 3.64 | 0.196 | 0.38% / 0.38% | 0.1 |
| uncle cap 6, 10,000 miners, 10 s | 350,621 | 37.9 | 16.46% | 2.79 | 1.94 / 3.64 | 0.196 | 0.38% / 0.38% | 0.1 |
| uncle cap 12, 10,000 miners, 10 s | 350,621 | 37.9 | 16.46% | 2.79 | 1.94 / 3.64 | 0.196 | 0.38% / 0.38% | 0.1 |
| uncle cap 3, 10,000 miners, 2 s | 350,621 | 37.9 | 50.02% | 76.89 | 9.62 / 18.0 | 0.841 | 1.89% / 1.89% | 0.3 |
| uncle cap 6, 10,000 miners, 2 s | 350,621 | 37.9 | 50.00% | 76.89 | 9.62 / 18.0 | 0.841 | 1.89% / 1.89% | 0.3 |
| uncle cap 12, 10,000 miners, 2 s | 350,621 | 37.9 | 50.00% | 76.89 | 9.62 / 18.0 | 0.841 | 1.89% / 1.89% | 0.3 |
| uncle cap 3, 10,000 miners, 1 s | 350,621 | 37.9 | 67.63% | 310.99 | 19.4 / 36.2 | 1.381 | 3.80% / 3.80% | 0.5 |
| uncle cap 6, 10,000 miners, 1 s | 350,621 | 37.9 | 67.61% | 311.00 | 19.4 / 36.2 | 1.409 | 3.80% / 3.80% | 0.5 |
| uncle cap 12, 10,000 miners, 1 s | 350,621 | 37.9 | 67.56% | 311.02 | 19.4 / 36.2 | 1.402 | 3.80% / 3.80% | 0.5 |

Caps of 6 and 12 change neither the stale rate nor propagation: at
10-second shares a chain share names 0.2 uncles, so a cap of 3 rarely binds,
and at 1-second shares 1.38 to 1.41. In this model the cap only decides
which stale shares are paid. It adds no latency, because every uncle was
validated when it arrived and naming one costs 32 bytes; deeper header sync
and the payout of deep forks are not modelled. Not worth changing on this
evidence.

### 5. Share period and each node's load

| Scenario | Share bytes | Validation (ms) | Stale | To all (s) | P2Pool up, mean / max (Mbit/s) | Uncles per chain share | Validation busy, mean / max | Validation wait (ms) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 1,000 miners, no weak shares, 10 s | 36,261 | 6.4 | 4.91% | 0.56 | 0.211 / 0.395 | 0.051 | 0.07% / 0.07% | 0.0 |
| 1,000 miners, no weak shares, 5 s | 36,261 | 6.4 | 7.54% | 0.56 | 0.419 / 0.782 | 0.082 | 0.13% / 0.13% | 0.0 |
| 1,000 miners, no weak shares, 2 s | 36,261 | 6.4 | 16.82% | 0.56 | 1.04 / 1.94 | 0.202 | 0.33% / 0.33% | 0.0 |
| 1,000 miners, no weak shares, 1 s | 36,261 | 6.4 | 31.52% | 0.57 | 2.10 / 3.94 | 0.448 | 0.67% / 0.67% | 0.0 |
| 1,000 miners, 32 weak shares a share, each, 10 s | 36,261 | 6.4 | 4.91% | 0.56 | 0.392 / 0.727 | 0.051 | 0.15% / 0.15% | 0.0 |
| 1,000 miners, 32 weak shares a share, per node, 10 s | 36,261 | 6.4 | 4.91% | 0.56 | 0.392 / 0.727 | 0.051 | 0.15% / 0.15% | 0.0 |
| 1,000 miners, 32 weak shares a share, one, 10 s | 36,261 | 6.4 | 4.91% | 0.56 | 0.392 / 0.727 | 0.051 | 0.09% / 0.09% | 0.0 |
| 1,000 miners, 32 weak shares a share, each, 5 s | 36,261 | 6.4 | 7.54% | 0.56 | 0.780 / 1.45 | 0.082 | 0.30% / 0.30% | 0.0 |
| 1,000 miners, 32 weak shares a share, per node, 5 s | 36,261 | 6.4 | 7.54% | 0.56 | 0.780 / 1.45 | 0.082 | 0.30% / 0.30% | 0.0 |
| 1,000 miners, 32 weak shares a share, one, 5 s | 36,261 | 6.4 | 7.54% | 0.56 | 0.781 / 1.45 | 0.082 | 0.19% / 0.19% | 0.0 |
| 1,000 miners, 32 weak shares a share, each, 2 s | 36,261 | 6.4 | 16.82% | 0.56 | 1.94 / 3.60 | 0.202 | 0.75% / 0.75% | 0.0 |
| 1,000 miners, 32 weak shares a share, per node, 2 s | 36,261 | 6.4 | 16.82% | 0.56 | 1.94 / 3.60 | 0.202 | 0.74% / 0.74% | 0.0 |
| 1,000 miners, 32 weak shares a share, one, 2 s | 36,261 | 6.4 | 16.82% | 0.56 | 1.94 / 3.60 | 0.202 | 0.46% / 0.46% | 0.0 |
| 1,000 miners, 32 weak shares a share, each, 1 s | 36,261 | 6.4 | 31.52% | 0.59 | 3.91 / 7.26 | 0.448 | 1.50% / 1.50% | 0.0 |
| 1,000 miners, 32 weak shares a share, per node, 1 s | 36,261 | 6.4 | 31.52% | 0.59 | 3.91 / 7.26 | 0.448 | 1.49% / 1.49% | 0.0 |
| 1,000 miners, 32 weak shares a share, one, 1 s | 36,261 | 6.4 | 31.52% | 0.59 | 3.91 / 7.26 | 0.448 | 0.93% / 0.93% | 0.0 |
| 10,000 miners, no weak shares, 10 s | 350,621 | 37.9 | 17.48% | 2.78 | 2.03 / 3.79 | 0.206 | 0.40% / 0.40% | 0.1 |
| 10,000 miners, no weak shares, 5 s | 350,621 | 37.9 | 28.46% | 2.87 | 4.02 / 7.51 | 0.389 | 0.79% / 0.79% | 0.1 |
| 10,000 miners, no weak shares, 2 s | 350,621 | 37.9 | 49.36% | 16.09 | 9.96 / 18.6 | 0.819 | 1.95% / 1.95% | 0.2 |
| 10,000 miners, no weak shares, 1 s | 350,621 | 37.9 | 68.21% | 46.73 | 20.3 / 37.9 | 1.370 | 3.97% / 3.97% | 0.5 |
| 10,000 miners, 32 weak shares a share, each, 10 s | 350,621 | 37.9 | 17.48% | 2.78 | 2.20 / 4.12 | 0.206 | 1.60% / 1.60% | 0.1 |
| 10,000 miners, 32 weak shares a share, per node, 10 s | 350,621 | 37.9 | 17.48% | 2.78 | 2.21 / 4.12 | 0.206 | 1.58% / 1.58% | 0.1 |
| 10,000 miners, 32 weak shares a share, one, 10 s | 350,621 | 37.9 | 17.48% | 2.78 | 2.21 / 4.12 | 0.206 | 0.76% / 0.76% | 0.1 |
| 10,000 miners, 32 weak shares a share, each, 5 s | 350,621 | 37.9 | 28.46% | 2.90 | 4.38 / 8.17 | 0.389 | 3.19% / 3.19% | 0.3 |
| 10,000 miners, 32 weak shares a share, per node, 5 s | 350,621 | 37.9 | 28.46% | 2.90 | 4.38 / 8.17 | 0.389 | 3.15% / 3.15% | 0.3 |
| 10,000 miners, 32 weak shares a share, one, 5 s | 350,621 | 37.9 | 28.46% | 2.90 | 4.38 / 8.18 | 0.389 | 1.51% / 1.51% | 0.2 |
| 10,000 miners, 32 weak shares a share, each, 2 s | 350,621 | 37.9 | 49.64% | 20.21 | 10.8 / 20.3 | 0.819 | 7.96% / 7.96% | 0.9 |
| 10,000 miners, 32 weak shares a share, per node, 2 s | 350,621 | 37.9 | 49.55% | 20.20 | 10.8 / 20.3 | 0.820 | 7.83% / 7.84% | 0.9 |
| 10,000 miners, 32 weak shares a share, one, 2 s | 350,621 | 37.9 | 49.55% | 20.26 | 10.9 / 20.3 | 0.818 | 3.79% / 3.79% | 0.7 |
| 10,000 miners, 32 weak shares a share, each, 1 s | 350,621 | 37.9 | 68.48% | 55.26 | 22.1 / 41.2 | 1.292 | 15.99% / 15.99% | 3.1 |
| 10,000 miners, 32 weak shares a share, per node, 1 s | 350,621 | 37.9 | 68.30% | 55.28 | 22.1 / 41.2 | 1.291 | 15.65% / 15.65% | 3.0 |
| 10,000 miners, 32 weak shares a share, one, 1 s | 350,621 | 37.9 | 68.75% | 55.34 | 22.1 / 41.2 | 1.303 | 7.91% / 7.92% | 2.0 |
| 100,000 miners, no weak shares, 10 s | 626,003 | 361.5 | 34.08% | 6.78 | 3.36 / 6.47 | 0.503 | 3.80% / 3.80% | 5.2 |
| 100,000 miners, no weak shares, 5 s | 626,003 | 361.5 | 49.28% | 10.37 | 6.67 / 12.8 | 0.870 | 7.54% / 7.54% | 11.0 |
| 100,000 miners, no weak shares, 2 s | 626,003 | 361.5 | 71.27% | 63.61 | 16.6 / 31.8 | 1.614 | 18.64% / 18.64% | 26.6 |
| 100,000 miners, no weak shares, 1 s | 626,003 | 361.5 | 83.84% | 224.43 | 34.2 / 64.9 | 2.077 | 37.89% / 37.89% | 55.0 |
| 100,000 miners, 32 weak shares a share, each, 10 s | 626,003 | 361.5 | 34.17% | 6.80 | 3.52 / 6.76 | 0.505 | 18.17% / 18.17% | 11.2 |
| 100,000 miners, 32 weak shares a share, per node, 10 s | 626,003 | 361.5 | 34.08% | 6.80 | 3.52 / 6.76 | 0.503 | 17.79% / 17.79% | 11.2 |
| 100,000 miners, 32 weak shares a share, one, 10 s | 626,003 | 361.5 | 34.08% | 6.79 | 3.54 / 6.80 | 0.503 | 4.62% / 4.62% | 7.0 |
| 100,000 miners, 32 weak shares a share, each, 5 s | 626,003 | 361.5 | 49.55% | 11.24 | 6.97 / 13.4 | 0.871 | 36.28% / 36.28% | 30.0 |
| 100,000 miners, 32 weak shares a share, per node, 5 s | 626,003 | 361.5 | 49.37% | 11.23 | 6.98 / 13.4 | 0.865 | 35.46% / 35.47% | 29.7 |
| 100,000 miners, 32 weak shares a share, one, 5 s | 626,003 | 361.5 | 49.19% | 11.35 | 7.03 / 13.5 | 0.864 | 9.18% / 9.18% | 15.5 |
| 100,000 miners, 32 weak shares a share, each, 2 s | 626,003 | 361.5 | 73.64% | 68.24 | 17.0 / 32.9 | 1.628 | 90.49% / 90.49% | 242.2 |
| 100,000 miners, 32 weak shares a share, per node, 2 s | 626,003 | 361.5 | 73.55% | 68.47 | 17.1 / 32.9 | 1.690 | 87.68% / 87.72% | 214.7 |
| 100,000 miners, 32 weak shares a share, one, 2 s | 626,003 | 361.5 | 71.27% | 70.62 | 17.4 / 33.3 | 1.580 | 24.39% / 24.40% | 47.5 |
| 100,000 miners, 32 weak shares a share, each, 1 s | 626,003 | 361.5 | 98.75% | 597.94 | 23.5 / 44.6 | 2.214 | 181.58% / 181.58% | 103784.2 |
| 100,000 miners, 32 weak shares a share, per node, 1 s | 626,003 | 361.5 | 98.57% | 487.51 | 24.0 / 45.3 | 2.062 | 164.67% / 165.23% | 80456.5 |
| 100,000 miners, 32 weak shares a share, one, 1 s | 626,003 | 361.5 | 84.29% | 263.93 | 35.9 / 67.8 | 2.028 | 53.57% / 53.60% | 126.8 |
| 10,000 miners, nodes 5x slower, 32 weak shares a share, each, 10 s | 350,621 | 189.5 | 22.66% | 3.78 | 2.07 / 3.97 | 0.283 | 8.00% / 8.00% | 2.4 |
| 10,000 miners, nodes 5x slower, 32 weak shares a share, per node, 10 s | 350,621 | 189.5 | 22.66% | 3.78 | 2.07 / 3.97 | 0.283 | 7.89% / 7.89% | 2.4 |
| 10,000 miners, nodes 5x slower, 32 weak shares a share, one, 10 s | 350,621 | 189.5 | 22.66% | 3.78 | 2.08 / 3.98 | 0.283 | 3.79% / 3.79% | 1.9 |
| 10,000 miners, nodes 5x slower, 32 weak shares a share, each, 5 s | 350,621 | 189.5 | 36.00% | 3.89 | 4.11 / 7.85 | 0.546 | 15.97% / 15.97% | 5.4 |
| 10,000 miners, nodes 5x slower, 32 weak shares a share, per node, 5 s | 350,621 | 189.5 | 36.00% | 3.89 | 4.11 / 7.85 | 0.546 | 15.73% / 15.73% | 5.4 |
| 10,000 miners, nodes 5x slower, 32 weak shares a share, one, 5 s | 350,621 | 189.5 | 36.09% | 3.89 | 4.12 / 7.87 | 0.548 | 7.52% / 7.52% | 4.0 |
| 10,000 miners, nodes 5x slower, 32 weak shares a share, each, 2 s | 350,621 | 189.5 | 57.73% | 17.90 | 10.2 / 19.5 | 1.106 | 39.81% / 39.81% | 19.9 |
| 10,000 miners, nodes 5x slower, 32 weak shares a share, per node, 2 s | 350,621 | 189.5 | 57.64% | 17.95 | 10.2 / 19.5 | 1.096 | 39.14% / 39.14% | 19.7 |
| 10,000 miners, nodes 5x slower, 32 weak shares a share, one, 2 s | 350,621 | 189.5 | 57.64% | 18.17 | 10.2 / 19.5 | 1.096 | 18.89% / 18.89% | 12.2 |
| 10,000 miners, nodes 5x slower, 32 weak shares a share, each, 1 s | 350,621 | 189.5 | 75.45% | 48.48 | 20.5 / 39.4 | 1.766 | 79.93% / 79.93% | 89.1 |
| 10,000 miners, nodes 5x slower, 32 weak shares a share, per node, 1 s | 350,621 | 189.5 | 75.80% | 48.60 | 20.6 / 39.4 | 1.770 | 78.04% / 78.06% | 84.8 |
| 10,000 miners, nodes 5x slower, 32 weak shares a share, one, 1 s | 350,621 | 189.5 | 75.00% | 49.61 | 20.8 / 39.6 | 1.777 | 39.25% / 39.25% | 34.4 |
| 100,000 miners, token shares, 32 weak shares a share, each, 10 s | 4,109 | 376.0 | 17.66% | 2.67 | 0.163 / 0.322 | 0.209 | 22.95% / 22.95% | 13.2 |
| 100,000 miners, token shares, 32 weak shares a share, per node, 10 s | 4,109 | 376.0 | 17.66% | 2.67 | 0.164 / 0.323 | 0.209 | 22.47% / 22.48% | 13.1 |
| 100,000 miners, token shares, 32 weak shares a share, one, 10 s | 4,109 | 376.0 | 17.48% | 2.63 | 0.195 / 0.363 | 0.207 | 4.53% / 4.53% | 7.3 |
| 100,000 miners, token shares, 32 weak shares a share, each, 5 s | 4,109 | 376.0 | 30.34% | 2.80 | 0.316 / 0.627 | 0.424 | 45.84% / 45.84% | 32.9 |
| 100,000 miners, token shares, 32 weak shares a share, per node, 5 s | 4,109 | 376.0 | 30.43% | 2.80 | 0.318 / 0.629 | 0.426 | 44.84% / 44.84% | 32.3 |
| 100,000 miners, token shares, 32 weak shares a share, one, 5 s | 4,109 | 376.0 | 29.71% | 2.69 | 0.383 / 0.715 | 0.414 | 8.90% / 8.91% | 15.1 |
| 100,000 miners, token shares, 32 weak shares a share, each, 2 s | 4,109 | 376.0 | 95.91% | 198.67 | 0.592 / 1.03 | 2.478 | 114.38% / 114.38% | 32308.2 |
| 100,000 miners, token shares, 32 weak shares a share, per node, 2 s | 4,109 | 376.0 | 94.00% | 106.83 | 0.593 / 1.10 | 2.299 | 107.42% / 107.65% | 16897.3 |
| 100,000 miners, token shares, 32 weak shares a share, one, 2 s | 4,109 | 376.0 | 50.09% | 2.87 | 0.908 / 1.70 | 0.888 | 21.89% / 21.89% | 39.0 |
| 100,000 miners, token shares, 32 weak shares a share, each, 1 s | 4,109 | 376.0 | 98.93% | 967.54 | 1.18 / 2.05 | 2.000 | 229.40% / 229.40% | 187052.8 |
| 100,000 miners, token shares, 32 weak shares a share, per node, 1 s | 4,109 | 376.0 | 98.93% | 810.25 | 1.18 / 2.11 | 2.167 | 206.44% / 207.26% | 153485.3 |
| 100,000 miners, token shares, 32 weak shares a share, one, 1 s | 4,109 | 376.0 | 69.11% | 3.32 | 1.67 / 3.19 | 1.562 | 43.91% / 43.93% | 94.3 |
| 100,000 miners, all at level 0, 32 weak shares a share, each, 10 s | 1,028,345 | 437.6 | 43.18% | 13.51 | 5.76 / 11.0 | 0.696 | 47.15% / 47.15% | 58.3 |
| 100,000 miners, all at level 0, 32 weak shares a share, per node, 10 s | 1,028,345 | 437.6 | 43.09% | 13.49 | 5.76 / 11.0 | 0.695 | 45.95% / 45.96% | 56.7 |
| 100,000 miners, all at level 0, 32 weak shares a share, one, 10 s | 1,028,345 | 437.6 | 42.73% | 13.41 | 5.81 / 11.1 | 0.685 | 6.48% / 6.48% | 13.6 |
| 100,000 miners, all at level 0, 32 weak shares a share, each, 5 s | 1,028,345 | 437.6 | 64.09% | 49.58 | 11.2 / 21.5 | 1.301 | 94.22% / 94.22% | 450.0 |
| 100,000 miners, all at level 0, 32 weak shares a share, per node, 5 s | 1,028,345 | 437.6 | 62.84% | 50.25 | 11.3 / 21.6 | 1.291 | 91.42% / 91.44% | 356.7 |
| 100,000 miners, all at level 0, 32 weak shares a share, one, 5 s | 1,028,345 | 437.6 | 57.90% | 52.50 | 11.6 / 21.9 | 1.042 | 13.79% / 13.80% | 31.3 |
| 100,000 miners, all at level 0, 32 weak shares a share, each, 2 s | 1,028,345 | 437.6 | 99.00% | 2055.21 | 17.8 / 31.3 | 1.818 | 235.30% / 235.30% | 392280.1 |
| 100,000 miners, all at level 0, 32 weak shares a share, per node, 2 s | 1,028,345 | 437.6 | 98.91% | 1691.95 | 17.9 / 33.1 | 1.833 | 209.40% / 210.37% | 312765.0 |
| 100,000 miners, all at level 0, 32 weak shares a share, one, 2 s | 1,028,345 | 437.6 | 78.91% | 242.81 | 28.8 / 54.6 | 1.770 | 42.11% / 42.18% | 113.3 |
| 100,000 miners, all at level 0, 32 weak shares a share, each, 1 s | 1,028,345 | 437.6 | 99.38% | 2937.43 | 36.0 / 63.4 | 1.429 | 471.33% / 471.33% | 638289.8 |
| 100,000 miners, all at level 0, 32 weak shares a share, per node, 1 s | 1,028,345 | 437.6 | 99.38% | 2465.34 | 36.2 / 67.1 | 1.857 | 405.32% / 407.87% | 522273.8 |
| 100,000 miners, all at level 0, 32 weak shares a share, one, 1 s | 1,028,345 | 437.6 | 89.38% | 761.30 | 59.3 / 111 | 1.942 | 90.35% / 91.06% | 374.4 |

Upstream is right, and the queue shows where:
- At 1,000 miners validation stays under 2% of a node's time even at
  1-second shares with 32 weak shares a share. The limit is propagation:
  4.9% stale at 10-second shares, 32% at 1-second (these runs mine 1,080
  share intervals, so their stale rates are noisier than the 8,640-interval
  rows above).
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
  with 100,000 miners is 5% busy at 10-second shares and 54% at 1-second
  (44% on token shares, 90% at level 0), and no share period overflows.

So a faster share period is bounded by propagation for small pools and by
validation, mostly of weak shares mined on different templates, for pools
paying tens of thousands of miners. At the adopted 10-second period and
default level, 100,000 miners fit with room; what does not fit is 100,000
miners at level 0 on shares faster than 5 seconds, unless the pool's nodes
build the same template.
### 6. A bigger network

| Scenario | Share bytes | Validation (ms) | Stale | To all (s) | P2Pool up, mean / max (Mbit/s) | Uncles per chain share | Validation busy, mean / max | Validation wait (ms) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 1,000 nodes, whole, 10,000 miners, no weak shares | 350,621 | 37.9 | 17.48% | 2.78 | 2.03 / 3.79 | 0.206 | 0.40% / 0.40% | 0.1 |
| 1,000 nodes, whole, 10,000 miners, 32 weak shares a share | 350,621 | 37.9 | 17.48% | 2.78 | 2.20 / 4.12 | 0.206 | 1.60% / 1.60% | 0.1 |
| 1,000 nodes, without payouts, 10,000 miners, no weak shares | 1,301 | 37.9 | 5.00% | 0.50 | 7.2e-3 / 0.014 | 0.052 | 0.40% / 0.40% | 0.1 |
| 1,000 nodes, without payouts, 10,000 miners, 32 weak shares a share | 1,301 | 37.9 | 5.00% | 0.50 | 0.185 / 0.342 | 0.052 | 1.60% / 1.60% | 0.1 |
| 3,000 nodes, whole, 10,000 miners, no weak shares | 350,621 | 37.9 | 19.54% | 3.27 | 2.03 / 4.10 | 0.230 | 0.40% / 0.40% | 0.1 |
| 3,000 nodes, whole, 10,000 miners, 32 weak shares a share | 350,621 | 37.9 | 19.54% | 3.27 | 2.21 / 4.45 | 0.230 | 1.60% / 1.60% | 0.1 |
| 3,000 nodes, without payouts, 10,000 miners, no weak shares | 1,301 | 37.9 | 5.62% | 0.56 | 7.2e-3 / 0.015 | 0.059 | 0.40% / 0.40% | 0.1 |
| 3,000 nodes, without payouts, 10,000 miners, 32 weak shares a share | 1,301 | 37.9 | 5.62% | 0.56 | 0.185 / 0.369 | 0.059 | 1.60% / 1.60% | 0.1 |
| 10,000 nodes, whole, 10,000 miners, no weak shares | 350,621 | 37.9 | 21.14% | 4.39 | 2.03 / 4.38 | 0.261 | 0.40% / 0.40% | 0.1 |
| 10,000 nodes, without payouts, 10,000 miners, no weak shares | 1,301 | 37.9 | 5.89% | 0.63 | 7.2e-3 / 0.016 | 0.062 | 0.40% / 0.40% | 0.1 |

Ten times the nodes cost little: from 1,000 to 10,000 nodes the stale rate
of shares sent without payouts goes from 5.0% to 5.9%, and of whole shares
from 17.5% to 21.1%. Hops grow with the logarithm of the node count, so the
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
