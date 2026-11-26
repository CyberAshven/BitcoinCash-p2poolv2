# Known attacks and threat model

Well-studied attacks on mining pools, and how they apply to P2Poolv2 on
Bitcoin Cash. Adapted from upstream's draft "Handling Known Attacks" (its
docs, `handling_known_attacks.adoc`). Status is one of **mitigated**,
**partial** or **open**; nothing here is a proof, and mainnet has not
started.

At stake are the miners' payouts, the share chain's agreement (a node that
rebuilds a different coinbase forks off) and traded shares
([Trading shares for BCH](atomic-swap/bch-share-trading.md)). P2Poolv2 never
forks Bitcoin Cash: its blocks are ordinary BCH blocks.

## Attacks on the reward scheme

| Attack | Status | Why |
|---|---|---|
| **Pool hopping:** join early in a round, leave late (Rosenfeld 2011, [arXiv:1112.4980](https://arxiv.org/abs/1112.4980)) | mitigated | PPLNS, with a window of `K = 3` blocks of work at the paying block's difficulty (plan decision 11), so share difficulty changes do not bring timing back. |
| **Block withholding:** submit shares, discard block-grade hashes (Rosenfeld 2011, section 6.2) | open | A share carries its coinbase and merkle branch, not the block's transactions, so only the finder's node can broadcast the block. A block-finder bonus was deferred (decision 11): it costs a large withholder little. The dashboard's effort panel shows a pool finding fewer blocks than its work predicts. |
| **Miner's dilemma, pool infiltration:** mine inside a victim pool and withhold there (Eyal 2015, [arXiv:1411.7099](https://arxiv.org/abs/1411.7099)) | open | As block withholding. |
| **Fork after withholding (FAW), power adjusting withholding (PAW):** release a found block only when a competitor appears (Kwon et al., CCS 2017, [arXiv:1708.09790](https://arxiv.org/abs/1708.09790)), or shift hashrate between infiltration and own mining (Gao et al., CCS 2019) | open | A late block-grade share still earns 9/10 as an uncle. Cutting uncle credit for block-grade shares stale when peers first saw them would also cost honest orphans, and needs modelling first. Per-address hashrate in Grafana helps spot PAW. |

## Attacks on the share chain

**Selfish mining** (Eyal and Sirer 2013, [arXiv:1311.0243](https://arxiv.org/abs/1311.0243);
Sapirshtein et al., [arXiv:1507.06183](https://arxiv.org/abs/1507.06183)):
withholding shares to orphan honest ones. Ten-second shares give latency more
weight. **Partial:** an orphaned honest share is still paid 9/10 as an
uncle, and fast relay keeps the attacker's edge small. Uncle rewards can also
make selfish mining pay in some ranges, as Ethereum analyses found; an MDP
model of the 9/10 split is still to be done (upstream issue #162).

**Share withholding and timestamp games** (Chang, SecureComm 2020): timing
submissions, or skewing timestamps to lower the share difficulty.
**Partial:** full validation, header sync and block receipt all require a
share's time to be after the median of its 11 ancestors and at most 120
seconds in the future. The share chain's ASERT is anchored at each share's
parent (decision 12), which makes these rules matter more; integer
fixed-point aserti3 gives every node the same target.

**Stubborn mining, eclipse and partitioning** (Nayak et al. 2016,
[eprint 2015/796](https://eprint.iacr.org/2015/796); Heilman et al. 2015,
[eprint 2015/263](https://eprint.iacr.org/2015/263); Apostolaki et al. 2017,
[arXiv:1605.07524](https://arxiv.org/abs/1605.07524)): isolating nodes on a
stale tip, or splitting the network by routing. **Partial:** nodes cap
connections (`max_established_*`, `max_connections`), block addresses at run
time (`/blocked_ips`), and ask peers for headers on a stale tip. Outgoing
peers are spread over IPv4 /16 and IPv6 /32 groups, and saved anchors are
redialed first after a restart ([Outgoing peers](p2p-network.md#outgoing-peers)).
With no DNS seeds, peers come from `dial_peers` and Nostr relays of
different operators: list several `dial_peers` on different networks. There
is no alert when the tip stops; the Grafana node dashboard shows the peer
count in red at zero.

### Resource exhaustion

**Partial.**

- Message rates are capped per peer (`max_*_per_second`).
- Shares whose parent is unknown wait in memory, at most 1,024 blocks and
  32 MiB, each for at most 10 minutes, oldest out first. At the low share
  target ceiling they are cheap, so a flood can push out honest shares
  waiting for a parent.
- Validated shares wait for the organise worker in a queue of at most
  512 shares and 64 MiB (validation waits for room); those whose parent is
  not yet validated wait in a buffer of at most 1,024 shares and 128 MiB,
  oldest evicted first. An evicted share stays stored and is re-validated
  when a share arrives on top of it.
- The share-transaction pool holds at most 1,000 transactions and 4 MiB,
  ranked by share units burned per byte, then arrival (decision 15). A full
  pool evicts the lowest only for one that outranks it; a conflicting spend
  is refused (first seen wins).
- Vardiff sums saturate, and `maximum_difficulty` defaults to 2^32, so a
  very large `d=` cannot overflow.

## Attacks on miners

**Stratum hijacking and eavesdropping** (Recabarren and Carbunar, PETS
2017, [arXiv:1703.06545](https://arxiv.org/abs/1703.06545)) on plaintext
Stratum V1. **Mitigated for Stratum V2:** Noise encryption under the
operator's authority key, which miners pin (`sv2_authority_public_key`). Run
Stratum V1 on the miners' LAN, P2Poolv2's intended setup.

**Coinbase size and stock firmware.** Upstream reports that stock Antminer
firmware accepts only about a dozen outputs, and many tiny addresses could
push small miners out of a capped coinbase (the "rich list" attack).
**Open** for firmware, not yet measured on Bitcoin Cash hardware; Stratum
V2 standard channels hash headers only and avoid any firmware limit. The
rich list attack finds no capped set: the coinbase pays every miner directly
up to 1,000,000 bytes (3,072 on a share that merge-mines tokens; plan
decision 14), smallest first, and pays those past the limit exactly through
a covenant payout tree ([Coinbase payout rule](architecture/bch-hybrid-payouts.md)).
Why there is no firmware cap: [Pool size limits](sharechain/pool-size.md)
and [the Bitcoin assumptions audit](bch-btc-assumptions-audit.md), A1 and M1.

## Bitcoin Cash chain risks

**A made-up or hostile BCH chain** fed to a node through its templates.
**Mitigated, not yet run on chipnet:** shares carry the BCH headers they
build on, every node checks them (aserti3-2d and chipnet's 20-minute rule),
and a coinbase pays only work on its block's BCH chain, give or take one
block ([Mining on the current chain](sharechain/current-chain.md)). A hostile
fork is paid for at most its first two block intervals, and BCHN finalizes
blocks ten deep. Still run your own BCHN, or a Template Provider you trust,
well connected: your templates choose the chain your coinbases pay.

**Share-chain reorgs and traded shares.** **Partial:** payouts skip the
newest 100 shares, so a block's payout differs between forks only when they
split deeper than that, and moving payout rights are undone on reorgs. A
block spending an output the node never fetched is stored with an unknown
coinbase root instead of stalling a syncing node
([Pruning](architecture/pruning.md#missing-input-at-write-time)).

## For operators

- Keep BCHN's RPC and ZMQ on localhost.
- Set `maximum_difficulty` on a public node.
- Use Stratum V2 with authority keys for miners outside your LAN.
- List several `dial_peers`, and watch the peer count and the share chain
  tip in Grafana.
- Keep the clock in sync.
- Put a rate-limiting reverse proxy in front of a public API
  (`public_dashboard = true`); the Ansible nginx role does this.
