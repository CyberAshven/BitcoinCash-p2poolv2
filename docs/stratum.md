# Stratum server and vardiff

> Ported from upstream P2Poolv2's documentation: `stratum.adoc` and
> `pool_difficulty_adjustment.adoc` in
> [p2poolv2/docs](https://github.com/p2poolv2/docs) at commit `623e4c1`,
> Copyright 2024-2026 P2Poolv2 Developers, MIT or Apache-2.0. Adapted to
> Bitcoin Cash by this fork; see the [port map](upstream-port-map.md).

Each P2Poolv2 node has its own Stratum server. It turns Bitcoin Cash block
templates into work for the mining rigs and keeps each rig at a target share
rate.

The server follows CKPool's parameters: one share about every 3.3 seconds
per rig, and CKPool's vardiff algorithm, which years of use have tested. It
is written in Rust in `p2poolv2_lib/src/stratum/`. Stratum V1 is served on
`[stratum] port`, and Stratum V2 on `sv2_port` when it is set.

## Components

```text
BCHN or Template Provider --templates----------> Template fetcher --> Job builder
Share chain ---------------payouts, tip--------> Job builder
Job builder <--------------difficulty----------> Vardiff
Job builder ---------------mining.notify-------> Miners
Miners --------------------new share-----------> Share validator
Share validator -----------pool shares---------> Share chain
Share validator -----------blocks--------------> BCHN or Template Provider
```

**Template fetcher.** Asks BCHN for a template with `getblocktemplate`
(without Bitcoin's SegWit rules) when BCHN announces a block over ZMQ
(`zmqpubhashblock`), right after this node submits a block, and otherwise
every 10 seconds (`GBT_POLL_INTERVAL` in `p2poolv2_node/src/lib.rs`), so new
transactions make it into the work.

- With `node_p2p_address` it also fetches one when BCHN announces a block on
  its P2P port, as jtoomim's p2pool did (`stratum/node_p2p_listener.rs`):
  the node connects as a peer that relays no transactions, answers pings
  and reconnects after errors. That works with any Bitcoin Cash node,
  including BCHD and Knuth, which have no ZMQ.
- With `template_source = "rpc_light"` it asks for `getblocktemplatelight`
  (the transactions' merkle branch instead of the transactions) and falls
  back to `getblocktemplate` (`stratum/work/gbt.rs`).
- With `template_source = "sv2"` templates come from a Stratum V2 Template
  Provider, checked as BCHN's are; BCHN still gives the height each builds
  on (`stratum/sv2/template_distribution.rs`).

Setup: [Running P2Poolv2 on Bitcoin Cash](bch-quickstart.md).

**Minimum-difficulty mark (test networks).** On chipnet a block dated more
than twice its parent's next interval after the parent (2 minutes after a
one-minute block, 20 after a ten-minute one) must carry the proof-of-work
limit's nBits, `1d00ffff`, and a block dated up to that mark must carry
ASERT's; anything else is `bad-diffbits`. The template relay
(`relay_templates_learning_prevs` in `node/bch_header_feeder.rs`) bounds
each template by its parent's mark (`BchHeaderRules::min_difficulty_mark`):
a job with ASERT's nBits carries a time no later than the mark, one with the
limit's a time after it, and a share dated on the wrong side is refused
(`nTime out of range` on SV1, `invalid-ntime` on SV2) instead of becoming a
block the node refuses. A second after the mark, while the latest template
still carries ASERT's nBits, the relay switches it itself: the same
template, with the limit's nBits from `BchHeaderRules::required_bits` (the
header check's), their target, and `mintime` and `curtime` past the mark
(`BlockTemplate::past_min_difficulty_mark`), published as a new template.
New nBits on the same parent clean the SV1 jobs, also for a session whose
clean template the watch channel replaced, and give SV2 channels a
`SetNewPrevHash`, so miners move to the easier work at once with either
template source; a Template Provider cannot be asked for a template. A
block on a switched provider template goes to the node over RPC, since
`SubmitSolution` carries no nBits. With getblocktemplate the relay also
asks the node for a template, which replaces the switched one, and a
template that arrives with ASERT's nBits after the mark, from a node or
provider whose clock is behind, is switched before it is published.
Mainnet has no such rule; regtest never retargets, and scalenet's nBits are
not checked here, so none of them is bounded or switched.

**Job builder.** Turns each template into a mining job whose coinbase pays
miners by the PPLNS window, after the donation and the operator fee. It also
carries the merge-mining commitment as output 0
([Auxiliary work proofs](bch-auxpow.md#merge-mining-tokens-on-a-public-node)),
a padding output and the share commitment, and every job commits to the
share chain tip ([Share commitments](sharechain/share-commitments.md)). A
coinbase scriptSig is at most 100 bytes (`MAX_COINBASE_SCRIPTSIG_SIZE` in
BCHN's `src/consensus/consensus.h`), and it names the pool, the operator,
the node and the share chain ([The coinbase tag](#the-coinbase-tag)). The coinbase pays every miner directly
within 1,000,000 bytes, or 3,072 on a job that merge-mines tokens, and the
rest through a payout tree ([Coinbase payouts](payouts/coinbase.md)).

Stratum V1 miners get the whole coinbase in `mining.notify`, so large jobs
cost bandwidth and the firmware must handle them. An
[SV2 extended job](https://stratumprotocol.org/specification/05-mining-protocol/)
carries its coinbase prefix and suffix as `B0_64K` fields of at most 65,535
bytes, about 1,922 P2PKH payouts. A channel whose first job is larger gets
`OpenMiningChannelError` with `coinbase-too-large`; a later oversized job
is skipped for that channel only, logged, and resumes when a coinbase
fits. Standard channels get a merkle root and keep working. These are
transport limits, not payout limits. Job Declaration has the same field
limit, and a found block too large for `SubmitSolution` goes to BCHN over
RPC. See [Pool size limits](sharechain/pool-size.md).

**Vardiff.** Keeps each miner at about one share every 3.33 seconds
(`TARGET_DRR` 0.3 in `stratum/difficulty_adjuster/mod.rs`), within
`minimum_difficulty` and `maximum_difficulty`. It follows CKPool's algorithm
and waits for enough shares, or enough time, before changing difficulty, so
it does not overreact to luck. See
[Difficulty adjustment in CKPool](difficulty_adjustment/ckpool.md).

**Share difficulty cap.** A miner's difficulty never goes above the share
difficulty of its job (`share_difficulty_ceiling` in
`stratum/difficulty_adjuster/mod.rs`), whatever vardiff,
`minimum_difficulty`, `d=`, `th=` or `mining.suggest_difficulty` ask for: a
miner submits only hashes that meet its own difficulty, and a higher one
would throw away shares. Before each job the node lowers a session above the
job's share difficulty with `mining.set_difficulty` (SV1) or `SetTarget`
(SV2). The cap is the share difficulty rounded down, so its target is never
harder than the share target. P2Pool v1 did the same
([What P2Pool v1 knew about Bitcoin Cash](p2pool-v1-bch-lessons.md), V8).
Once ASERT has settled, vardiff's own choice stays below the share
difficulty, so the cap binds mainly on explicit settings.

**Share validator.** Checks every submitted share against the miner's
current difficulty and rejects duplicates. Then:

- a share that meets the **pool target** goes to the share chain, which
  stores it and relays it to peers;
- a share that also meets the **Bitcoin Cash target** is a block, submitted
  straight away: `submitblock` to BCHN, `submitblocklight` (header and
  coinbase) to the node that made a light template, or back to the Template
  Provider whose template it used.

Shares below the pool target still count towards the miner's measured
hashrate and vardiff, but do not enter the share chain.

**Found blocks** (`stratum/block_submit.rs`). A block never depends on one
RPC call. BCHN gets it again after a connection error, a time-out (30
seconds per attempt) or a busy node (HTTP 503), with waits doubling from half
a second to 10 seconds, for up to two minutes. BCHN's answer is final: `null`
(accepted), `duplicate` (BCHN already has it), `inconclusive` (stored without
a verdict, for example because another block at this height came first) or a
reject reason. Every `[[submit_nodes]]` node also gets the block, with its
own retries, even when it went to a Template Provider. Results are counted in
`found_block_submissions_total{node,outcome}` and retries in
`found_block_submit_retries_total{node}` ([Metrics](architecture/metrics.md)).
The Template Provider's RPC fallback and the simulation retry the same way
but count nothing.

Version rolling: the node offers the BIP 320 general-purpose bits
(`version_mask = "1fffe000"`), which ASIC firmware rolls on Bitcoin Cash as
on Bitcoin.

## The coinbase tag

Every coinbase a node builds names P2Pool v2, the operator, the node and
the share chain in one data push of its scriptSig, the fifth, after the
height, the aux flags, the extranonce and the nanosecond time:

```text
P2Poolv2 0x0F <operator> 0x0F <node> 0x0F <chain>
```

The byte 0x0F separates the lines, as DATUM separates its primary and
secondary tags, so explorers that read DATUM tags split the push at 0x0F,
show line 1, the operator, and group every P2Pool v2 block as one pool.
Every line is always written, so the positions are fixed: [0] `P2Poolv2`,
[1] operator, [2] node, [3] chain. Lines 1 and 2 may be empty; line 3 is
always there. Later versions may add lines after the chain line, and a
parser ignores the lines it does not know.

| Line | Comes from | Example |
|---|---|---|
| [0] | The software; never configured | `P2Poolv2` |
| [1] operator | `[stratum] pool_signature`. Unset, empty or `P2Poolv2` (the old default) leaves it empty, so the node line never stands in its place | `CyberAshven` |
| [2] node | The node software and version from the `subversion` of `getnetworkinfo` on the `[bitcoinrpc]` node, read at start, then `[stratum] node_label` after a space | `BCHN 29.2.1 node1` |
| [3] chain | Computed from the share chain; never configured | `q7x2k8` |

Three tags, with `q7x2k8` as an example chain line (each node logs its own
tag at start as `Coinbase tag: ...`, with ` | ` for 0x0F):

| Case | Tag |
|---|---|
| Full | `P2Poolv2 0x0F CyberAshven 0x0F BCHN 29.2.1 node1 0x0F q7x2k8` |
| No operator | `P2Poolv2 0x0F 0x0F BCHN 29.2.1 node1 0x0F q7x2k8` |
| No node line | `P2Poolv2 0x0F CyberAshven 0x0F 0x0F q7x2k8` |

**The node line.** The first user agent of the subversion (BIP 14), without
its parenthesised comments, named as operators know it:
`/Bitcoin Cash Node:29.2.1(EB32.0)/` becomes `BCHN 29.2.1`, `/Knuth:1.7.0/`
becomes `Knuth 1.7.0` and `/bchd:0.22.4/` becomes `BCHD 0.22.4`. Other
software keeps its own name and version. The label follows after a space.
The node software is not the operator to fix, so it never stops the start:
a node without `getnetworkinfo`, or a software line that breaks the name
rules or does not fit beside the label, leaves the software out, with a
warning, and the node line is the label alone.

**The chain line.** Six characters of the CashAddr alphabet
`qpzry9x8gf2tvdw0s3jn54khce6mua7l`, five bits a character, most
significant first, in two parts:

- the family, 4 characters: the first 20 bits of SHA-256 over the genesis
  share hash (its 32 bytes in the order the share chain serializes them).
  Every fork of a share chain keeps it, so explorers group forks by the
  first four characters;
- the fork, 2 characters: the first 10 bits of SHA-256 over the P2P
  protocol string the node connects with, the one it logs at start as
  `P2P protocol string: ...`. A share chain that splits off with other
  rules, or another implementation, gets another fork part.

For example, `q7x2k8` could be an original chain, `q7x2m3` a rule fork of
it and `t9dw4c` an unrelated chain. Every node on one share chain writes the
same line. A rule change that also re-pins the genesis share, as the flag
days of this project usually do, changes the family too.

**The name rules.** No address or email goes on chain. The operator line is
at most 24 characters, `node_label` at most 8 and the node line at most 23;
the node software takes what the label and the space before it leave. A
name holds only letters, digits, spaces, `.`, `_` and `-`, which keeps out
`@`, `:` and `/`: no emails, no `bitcoincash:` or `bchtest:` addresses and
no URLs. A word of 20 characters or more made only of base58, hex or
CashAddr characters is refused too, as it reads as an address pasted without
its prefix. A `pool_signature` or `node_label` that breaks a rule stops the
node at start with the reason (`Refusing to start: [stratum] pool_signature
...`); a name is refused, never cut short.

**The 100-byte scriptSig.** The tag gets what the largest of every other
push leaves (`MAX_COINBASE_TAG_BYTES` in `stratum/work/coinbase_tag.rs`):

| Push | Bytes at most |
|---|---|
| Height: a four-byte script number from 8,388,608 up to the 500,000,000 a lock time allows | 5 |
| Aux flags: BCHD pushes `/bchd/`; BCHN and Knuth send none, one zero byte | 8 |
| Extranonce, enonce1 and enonce2 | 13 |
| Nanosecond time | 9 |
| Tag push opcode | 1 |
| Tag: `P2Poolv2`, three separators, an operator line of 24, a node line of 23 and the chain line of 6 | 64 |
| Total | 100 |

Merge mining adds nothing to the scriptSig: its commitment is output 0. A
declared job writes the height and its own tag in a prefix of at most 88
bytes, 70 with the longest tag, then the 12-byte extranonce.

**Not a share-chain rule.** Validation reads the push from the coinbase of
each share and rebuilds the coinbase with it as it is, so nodes with other
tags, and older nodes with a plain signature, accept the shares of one
another.

**Declared jobs.** A Stratum V2 Job Declaration client builds the template,
so the operator and node lines of the tag in its scriptSig prefix are its
own. The share lands on this share chain, though: the prefix must carry,
after the height push, a data push that starts with the `P2Poolv2` line and
has the chain line of this node, with every line under the name rules. The
chain line is in the tag of every job the node sends. Otherwise the job
gets `invalid-coinbase-tag`, and the reason is logged as a warning
([Job Declaration](bch-job-declaration.md#rules-a-declared-job-must-meet)).

## The pool difficulty

The share chain's own target is set by ASERT. Upstream chose it after
simulations of several difficulty algorithms against oscillating hashrate,
a sudden 90% drop and noisy hashrate
([methodology](https://github.com/p2poolv2/pool_difficulty_adjustment_analysis/blob/6e6ce1a8ac50e0257a9839a9744fc3e52b0f3ecd/daa_evaluation.md),
[results](https://github.com/p2poolv2/pool_difficulty_adjustment_analysis/blob/6e6ce1a8ac50e0257a9839a9744fc3e52b0f3ecd/daa_simulation_report.md)),
with a 10-second target interval, uncles at most 3 deep and PPLNS
accounting. Upstream says those simulations were run with LLMs; they are
modelled results, not measurements on a live pool.

ASERT is also Bitcoin Cash's own block difficulty algorithm since November
2020 ([aserti3-2d](https://upgradespecs.bitcoincashnode.org/2020-11-15-asert/)).
This fork computes share targets with BCH's integer formula, a 10-second
interval and a 600-second half-life, anchored at each share's parent rather
than at genesis. Why: [Share difficulty: ASERT](difficulty_adjustment/asert.md).

## Miner options

Usernames, the `p2p=`, `d=` and `th=` password options, and Stratum V2
settings are on [Miner options](miner-options.md).
