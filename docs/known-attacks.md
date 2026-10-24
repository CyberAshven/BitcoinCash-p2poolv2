# Known attacks and threat model

Well-studied attacks on mining pools, and how they apply to P2Poolv2 on
Bitcoin Cash. Adapted from upstream's draft "Handling Known Attacks" (its
docs, `handling_known_attacks.adoc`), with Bitcoin Cash notes and this fork's
status. Status is one of **mitigated**, **partial** or **open**; nothing
here is a proof, and mainnet has not started.

## What is at stake

- **Miners' payouts.** Each BCH block the pool finds pays the PPLNS window in
  its coinbase. An attacker wins by taking a larger slice of the window, by
  making blocks get lost, or by stopping honest nodes from mining.
- **The share chain's agreement.** Every node must rebuild the same coinbase
  for every share. A node that disagrees forks off and its miners are paid
  on a chain nobody else follows.
- **Traded shares.** A share's payout right can be sold for BCH; the share
  chain's script and payout-right rules guard it
  ([atomic-swap/bch-share-trading.md](atomic-swap/bch-share-trading.md)).

Bitcoin Cash itself is never forked by P2Poolv2: blocks the pool finds are
ordinary BCH blocks, and every share-chain rule changes only the share chain.

## Attacks on the reward scheme

### Pool hopping

Under proportional payouts, miners join early in a round and leave late
(Rosenfeld 2011, [arXiv:1112.4980](https://arxiv.org/abs/1112.4980)).

**Mitigated.** PPLNS resists hopping. On Bitcoin Cash the window is measured
in work, not shares: `K = 3` blocks of work by the paying block's own
difficulty (plan decision 11), so share difficulty changes do not bring
timing back into a share's value.

### Block withholding (sabotage)

The attacker submits shares but throws away any hash that also meets the BCH
block target (Rosenfeld 2011, section 6.2).

**Open.** A share carries its coinbase and merkle branch, not the block's
transactions, so other nodes cannot rebuild and broadcast the block from a
block-grade share; the finder's node must. A block-finder bonus was weighed
and deferred (decision 11): it adds little cost for a large withholder. The
dashboard's effort panel shows a pool that finds fewer blocks than its work
predicts.

### Miner's dilemma and pool infiltration

A pool mines inside a victim pool and withholds blocks there
(Eyal 2015, [arXiv:1411.7099](https://arxiv.org/abs/1411.7099)).

**Open,** with the same defences as block withholding.

### Fork after withholding (FAW) and power adjusting withholding (PAW)

The attacker holds a found block and releases it only when a competitor
appears (Kwon et al., CCS 2017,
[arXiv:1708.09790](https://arxiv.org/abs/1708.09790)), or shifts hashrate
between infiltration and its own mining (Gao et al., CCS 2019).

**Open.** A late block-grade share still earns 9/10 as an uncle. Cutting
uncle credit for block-grade shares that were stale when peers first saw
them is the candidate rule; it also costs honest orphans, so it needs
modelling first. Per-address hashrate in Grafana helps spot PAW.

## Attacks on the share chain

### Selfish mining on the share chain

The attacker withholds shares to orphan honest ones and grow its window
share (Eyal and Sirer 2013, [arXiv:1311.0243](https://arxiv.org/abs/1311.0243);
Sapirshtein et al., [arXiv:1507.06183](https://arxiv.org/abs/1507.06183)).
Ten-second shares give latency more weight.

**Partial.** Uncles blunt it: an orphaned honest share is still paid 9/10.
Uncle rewards can also make selfish mining pay in some ranges, as Ethereum
analyses found; an MDP model of the 9/10 split is still to be done
(upstream issue #162). Fast relay keeps the attacker's edge small.

### Share withholding and timestamp games

The attacker times share submissions (Chang, SecureComm 2020), or skews
timestamps to lower the share difficulty and mint cheap shares.

**Partial.** Full validation checks a share's time against the median of its
ancestors and rejects shares more than 120 seconds in the future. On Bitcoin
Cash the share chain's ASERT is anchored at each share's parent (decision
12), so a share's target follows its parent's interval directly, which makes
timestamp rules matter more. Header sync and block receipt apply the same
timestamp rules as full validation (after the median of 11 ancestors, at
most 120 seconds ahead). Targets use integer fixed-point aserti3, so every
node computes the same one.

### Stubborn mining, eclipse and partitioning

The attacker isolates nodes so they mine on a stale tip (Nayak et al. 2016,
[eprint 2015/796](https://eprint.iacr.org/2015/796); Heilman et al. 2015,
[eprint 2015/263](https://eprint.iacr.org/2015/263)), or splits the network
by routing (Apostolaki et al. 2017,
[arXiv:1605.07524](https://arxiv.org/abs/1605.07524)).

**Partial.** Nodes cap connections (`max_established_*`, `max_connections`)
and can block addresses at run time (`/blocked_ips`), and a node on a stale
tip asks peers for headers. There are no anchor peers kept across restarts,
no spread across networks, and no alert when the tip stops; the Grafana node
dashboard shows the peer count in red at zero. Bitcoin Cash has no P2Poolv2
DNS seeds yet: operators list peers in `dial_peers` by hand, so list several
on different networks.

### Resource exhaustion

**Partial.**

- Message rates are capped per peer (`max_*_per_second`).
- Shares whose parent is unknown wait in memory, at most 1,024 blocks and
  32 MiB, each for at most 10 minutes; the oldest go first. Each costs the
  attacker a share's proof of work, but on the mini chain shares are cheap,
  and a flood can push out honest shares that are waiting for a parent.
- The share-transaction pool holds at most 1,000 transactions and 4 MiB.
  Priority is the share units a transaction burns per byte, then arrival
  (decision 15); a full pool evicts the lowest only for a transaction that
  outranks it, and a conflicting spend of a pool transaction's input is
  refused (first seen wins).
- Vardiff sums saturate, and `maximum_difficulty` defaults to 2^32, so a very
  large `d=` in the Stratum password no longer overflows its accounting.

## Attacks on miners

### Stratum hijacking and eavesdropping

Stratum V1 is plaintext: an attacker on the path can read a miner's earnings
and swap jobs (Recabarren and Carbunar, PETS 2017,
[arXiv:1703.06545](https://arxiv.org/abs/1703.06545)).

**Mitigated for Stratum V2.** The node speaks Stratum V2 with Noise
encryption, signed by the operator's authority key, which miners pin
(`sv2_authority_public_key`). Stratum V1 stays plaintext: run the node on the
miners' LAN, which is P2Poolv2's intended setup. The coinbase pays the
payout address the miner chose, so a hijacker must swap the whole job.

### Coinbase size and stock firmware

A coinbase that pays every miner in the window grows with the pool. Upstream
reports that stock Antminer firmware accepts only about a dozen outputs. An
attacker with many tiny addresses could also push small honest miners out of
a capped coinbase (the "rich list" attack).

**Open.** Not yet measured on Bitcoin Cash hardware. The code pays every
miner directly in a coinbase of up to 1,000,000 bytes (plan decision 14), so
there is no paid set to crowd others out of. Only a share that merge-mines
tokens keeps its coinbase within 3,072 bytes. Payees are paid smallest
first, and the larger payees past the limit are paid exactly, less their
share of the fee at 1 to 3 sat/byte, through a covenant payout tree, which
BCH's introspection opcodes allow without a fork. Tiny addresses can move
others into the tree, not out of their income. The old 784-byte cap came
from one Bitcoin block another pool mined: a lower bound for that firmware,
not a measured limit, and P2Pool v1 on Bitcoin Cash paid hundreds of miners
per coinbase. Stratum V2 standard channels, which hash headers only, avoid
any firmware limit, and device limits are to be measured and handled per
connection. See [Pool size limits](sharechain/pool-size.md) and
[the Bitcoin assumptions audit](bch-btc-assumptions-audit.md), A1 and M1.

## Bitcoin Cash chain risks

### Mining on a made-up or hostile BCH chain

A node can be fed templates for a chain the network does not follow, and a
coinbase may pay work done on such a chain.

**Open.** Shares do not yet carry or check the BCH headers they build on
(upstream's "mine on the current chain" design). On BCH it needs aserti3-2d
and chipnet's 20-minute rule. BCHN's automatic finalization of blocks ten
deep limits how deep a BCH reorg can go, but does not replace the rule. Run
your own BCHN, or a Template Provider you trust, and keep it well connected.

### Share-chain reorgs and traded shares

**Partial.** Payouts skip the newest 100 shares, so a block's payout differs
between share-chain forks only when they split deeper than that. Moving
payout rights are undone on reorgs. Since 2026-10-23 a block spending an
output the node never fetched is stored with an unknown coinbase root instead
of stalling a syncing node
([pruning](architecture/pruning.md#missing-input-at-write-time)).

## For operators

- Keep BCHN's RPC and ZMQ on localhost.
- Set `maximum_difficulty` on a public node.
- Use Stratum V2 with authority keys for miners outside your LAN.
- List several `dial_peers`, and watch the peer count and the share chain
  tip in Grafana.
- Keep the clock in sync.
- Put a rate-limiting reverse proxy in front of a public API
  (`public_dashboard = true`); the Ansible nginx role does this.
