# Proposal: P2Pool payouts for standard SV2 Job Declaration clients

**Status: proposal, 2026-11-03. Nothing is built.** Option B below changes
no share-chain rule. Option A would change several and is not recommended.

## 1. Goal

Anyone running a node declares their own block template through SV2 Job
Declaration, to every pool in their list, public BCH P2Pool nodes included.
A pool that cannot take their template is skipped, and the node mines solo.
Nobody has to mine on a pool's template.

Today a standard Job Declaration client cannot mine on a P2Pool node: a
P2Pool share needs an exact PPLNS coinbase, and only `POST /jd_prepare`
builds it ([BCH P2Pool Job Declaration](../bch-job-declaration.md#read-only-preparation-for-an-external-declarator)).
Pickaxe calls that endpoint over HTTP. A node such as kth should get the
same coinbase over its Job Declaration connection instead.

## 2. What a P2Pool share needs in its coinbase

Every node rebuilds a share's coinbase and rejects the share if it differs
([Coinbase payouts](../payouts/coinbase.md)):

1. **Output 0** is the merge-mining commitment, the exact empty one when no
   token is merge-mined ([Auxiliary work proofs](../bch-auxpow.md)).
2. **The PPLNS outputs**, one weighted split of the template's revenue over
   the window: smallest weight first, every payee paid even below dust,
   the rounding remainder to the largest. Each window share's donation and
   fee are already weights in that split. Past the coinbase limit, the
   largest payees are paid through one payout tree output
   ([Hybrid payouts](bch-hybrid-payouts.md)).
3. **Padding, then the share commitment** as the last output. The
   commitment binds the previous share, uncles, bits, time, donation and
   fee, the miner's scriptSig prefix, payout address and share owner
   ([Share commitments](../sharechain/share-commitments.md)).

The share chain targets a share every 10 seconds, so the window, and with
it the split and the previous share, changes about every 10 seconds.

## 3. Extension 0x0003 against these rules

Extension 0x0003, "Coinbase Transaction Payouts", is proposed in
[sv2-spec PR #203](https://github.com/stratum-mining/sv2-spec/pull/203)
(approved, not merged; file `extensions/0x0003-coinbase-transaction-payouts.md`
at `47dbedd8`). The pool publishes weights in `SetPayoutDistribution`; the
client computes `floor(weight * R / W)` for its own template, gives the
remainder to the first output, `pool_payout`, and drops payouts below each
dust limit; the pool recomputes and compares.

| Point | 0x0003 | P2Pool today | Verdict |
|---|---|---|---|
| Custody | The first output is pool-controlled and absorbs rounding, dust and miners left out of the distribution | No operator; every payee is paid directly | Conflict: P2Pool could only publish a miner's own output as `pool_payout`, which keeps payouts direct but breaks the extension's "controlled by Pool" |
| Who sets the split | "The Pool dictates scripts and weights" | The share chain does, and every peer rebuilds it | Fits if a P2Pool node publishes the share chain's split; a client alone still cannot check it, under either option |
| Donation and fee | Weights in one split | Already weights in one split on BCH | Fits, no change |
| Dust | Drops a payout below its `dust_limits[i]` | Pays every payee | Fits: the pool publishes dust limit 0 |
| Rounding remainder | To `pool_payout`, the first output | To the largest payee, the last | Same amounts if the largest payee (or the tree output) is published as `pool_payout`; only its position moves (section 4) |
| Output order | `pool_payout`, payouts, fixed outputs, client outputs | Merge-mining commitment first, share commitment last | Rule change |
| Weight size | 64 bits (the 8-byte amount field) | 128 bits: difficulty times 10^9 (uncle tenths, then basis points squared) over 3 BCH blocks of work | Rule change: at a mainnet difficulty of a few hundred billion, a window's total is around 10^21, past the 64-bit limit of 1.8 × 10^19 |
| Share commitment | No place for it; a client adds only zero-value outputs it chooses | Per miner, per share, binds the client's own scriptSig prefix | The pool could send it only per connection, and only if it stopped binding the prefix: a rule change, and a distribution that is no longer pool-wide |
| Update rate | MUST NOT publish per share; the client uses a new distribution only for later jobs | New split and previous share every share, about 10 seconds | Conflict: publishing less often makes clients mine on old shares, which end as uncles or orphans |
| Payout tree | Fixed output scripts, weights only | The tree output's script holds its exact value and outputs, which depend on the template's revenue | Blocker: no distribution can express it. It applies past about 29,400 P2PKH payees on a share without tokens, the scale BCH is built for |
| Client outputs | The client may append zero-value outputs | Validators reject any extra output | Rule change |
| Witness commitment | Kept last, from the template provider | BCH has none | No effect |

The update rule is judged on BCH's share timing, not on the extension's
reason for it (churn at a centralized pool). A new share every 10 seconds
moves the previous share every miner must build on. A client that keeps
an old distribution until its next template mines shares on an old
previous share, and those end as uncles, paid 9/10, or as orphans, paid
nothing.

## 4. Worked example: the remainder

The window of [Coinbase payouts](../payouts/coinbase.md#an-example): miner A
3 shares, B 2 and C 1, B and C with the default 1% donation, so the
weights are A 3, B 1.98, C 0.99 and the donation 0.03, total 6. The
template pays 312,500,007 satoshis.

Each payee gets its weight times 312,500,007, divided by 6, rounded down:

| Payee | Rounded down | Today | 0x0003 with A as `pool_payout` |
|---|---|---|---|
| Donation | 1,562,500 | 1,562,500 | 1,562,500 |
| C | 51,562,501 | 51,562,501 | 51,562,501 |
| B | 103,125,002 | 103,125,002 | 103,125,002 |
| A | 156,250,003 | 156,250,004, the last output | 156,250,004, the first output |

The rounded amounts add up to 312,500,006. Today the 1 satoshi left goes to
A, the largest payee. Under 0x0003, `pool_payout` is paid what the others
leave, 312,500,007 − 156,250,003 = 156,250,004: the same amount. Only A's
output moves from last to first. So the remainder alone needs no rule
change; the order, the weight size, the share commitment and the tree do.

## 5. Options

**A. Adopt 0x0003.** Four rule changes: output order, 64-bit weights, a
commitment that no longer binds the scriptSig prefix, and tolerating client
outputs. Three departures from the extension's text: a miner's output as
`pool_payout`, a distribution per connection, and one per share. Even then the payout tree cannot be
expressed, so it fails at the payee counts BCH should reach. A covenant
that computes tree amounts from weights would fix that, but it is a
redesign of the hybrid payout. Not recommended.

**B. A request/response extension, shaped like the earlier drafts.**
Upstream proposed this shape twice before 0x0003, and closed both in its
favour:

- [sv2-spec PR #195](https://github.com/stratum-mining/sv2-spec/pull/195)
  (warioishere, "Dynamic Coinbase Outputs", closed 2026-07-29) defined
  `RequestCoinbaseOutputs`. The client sends its token, the template's
  `prev_hash` and the revenue it gives the pool, and gets the exact outputs
  back. It may ask ahead for a predicted next `prev_hash`. In its review
  TheBlueMatt asked for this shape: "Is it not simpler to eat the
  round-trip?"
- [sv2-spec PR #202](https://github.com/stratum-mining/sv2-spec/pull/202)
  (GitGab19, "non-custodial pool payouts", closed 2026-07-28) defined
  `RequestPayoutOutputs`: token and `available_payout_value` in, an output
  vector summing exactly to that value out, single use per declared job,
  sized by the reservation made at token allocation, with a
  `stale-payout-outputs` error.

Option B takes #202's messages, single use, reservation and stale error
as they are, so `/jd_prepare`'s bytes travel over the Job Declaration
connection. P2Pool's allocation already returns zero-value outputs that
reserve the coinbase space for `CoinbaseOutputConstraints`. It adds three
things P2Pool needs:

- **Request fields:** height, previous block hash and bits, which anchor
  the PPLNS window and the paying block's difficulty, and the client's
  scriptSig prefix, which the share commitment binds.
- **Exact placement:** #202 validates the outputs as an unordered set and
  lets the client add its own. A P2Pool share needs the returned vector as
  the coinbase's whole output list, in order (merge-mining commitment
  first, share commitment last), paying the whole revenue. A flag in the
  success reply says so. BCH has no witness commitment, so a template
  needs no output of its own and a client can always meet it.
- **A push when the share chain moves,** so the client asks again before
  it mines on an old share. #202 only tells it at its next declaration.

No share-chain rule changes, and the share commitment, the payout tree and
the 10-second updates all work as they do for Pickaxe today. The client
trusts the pool's arithmetic; any node can still check a share's coinbase
against its own share chain.

**The round trip.** Upstream chose pushed weights to save one round trip
per declared job. For P2Pool that round trip cannot be avoided: the payout
tree and the share commitment depend on the template's revenue and the
client's scriptSig prefix, so the outputs cannot exist before the pool
sees them, and the share chain moves every 10 seconds anyway. The cost is
also small next to what a job already costs. Full-Template sends
`DeclareMiningJob` and Coinbase-only sends `SetCustomMiningJob`, each a
round trip of its own. The client keeps hashing its current job until the
new outputs arrive, and can ask ahead as #195 allowed. On a node runner's
own P2Pool node it is a local call. So the blockers in section 3 are the
case for keeping a request/response path beside 0x0003, for pools whose
outputs weights cannot express.

| | A: 0x0003 | B: request/response |
|---|---|---|
| Share-chain rule changes | Four | None |
| Payout tree (large windows) | Not expressible | Works |
| Share commitment | Per connection, prefix unbound | As today |
| Round trips | None per template | One per template |
| Standard | Proposed, approved, not merged | The shape of closed drafts #195 and #202; needs an id |
| Client code | 0x0003 | #202's messages plus three additions |

**Changing 0x0003 instead.** A decentralized pool would need three changes
in sv2-spec PR #203: `pool_payout` allowed to be any payee the
distribution names, so nothing is held in custody; pool-placed zero-value
outputs before the payouts and at the end, for the two commitments; and
updates on every share. Even then, the payout tree and the share
commitment depend on each template, so the pool has to see the template
before the outputs exist. That is option B's model: #202 with its three
additions could be proposed to #203 as a second, pool-computed mode.
Writing it up is possible; posting anything upstream is the owner's call.

**Cost to kth.** Under B, kth's Job Declaration client implements #202's
messages with the three additions, beside 0x0003 if kth also supports
that: one request per template and one push to handle. It is one
spec-shaped extension rather than a P2Pool-only protocol, but until the
spec takes the shape back, only P2Pool offers it.

## 6. Recommendation

Build B. Option A would change four share-chain rules and still break at
BCH scale; option B changes none. On the kth side, the row 24 job engine
already takes its coinbase outputs as an input, so its Job Declaration
client gets one more source of outputs: ask the pool per template.

Before building:

- **Extension id.** #195 and #202 both used `0x0003`, which #203 now
  holds. SV2 reserves `0x4000`–`0x7fff` for experimental extensions that
  must not ship in production equipment; a production id is allocated
  through stratumprotocol.org. Chipnet tests can use an experimental id;
  asking for an allocation is the owner's call.
- **Messages.** The exact layout of the three additions and their error
  codes, and whether both the Job Declaration and Mining connections
  negotiate it, as 0x0003 requires.
- **Evidence.** A chipnet run in which a standard client, given only this
  extension, gets shares accepted in both Job Declaration modes.
