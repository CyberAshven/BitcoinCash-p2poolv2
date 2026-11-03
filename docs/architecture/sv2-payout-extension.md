# Proposal: P2Pool payouts for standard SV2 Job Declaration clients

**Status: option B approved by the owner on 2026-11-03, specified in
section 7. The pool side is built (`stratum::sv2::payout_outputs`); the
chipnet evidence at the end of section 7 is not done yet.** It changes no
share-chain rule. Option A would change several and is not recommended.
Nothing is posted on sv2-spec without the owner's OK.

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

One pool built the first shape: Blitzpool implements #195's
`RequestCoinbaseOutputs` in TypeScript on an open, unmerged branch
([warioishere/blitzpool PR #115](https://github.com/warioishere/blitzpool/pull/115),
last updated 2026-06-10). For #202 there is only its author's comment of
2026-06-25 that Blitzpool runs that spec; no code for `RequestPayoutOutputs`
was found.

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

## 6. Decision

The owner chose B on 2026-11-03, on three conditions: node native,
decentralized, censorship resistant. Option A would change four
share-chain rules and still break at BCH scale; option B changes none. On
the kth side, the row 24 job engine already takes its coinbase outputs as
an input, so its Job Declaration client gets one more source of outputs:
ask the pool per template.

Building a standard client also needs a fourth addition to #202. A P2Pool
share fixes the coinbase's version (2), input sequence (`0xfffffffe`) and
lock time (height − 1), which a client would otherwise choose itself, so
the success reply states them.

## 7. Specification

The wording is pool-neutral, so any pool that pays miners directly from
the coinbase can serve it. Section 7.5 says how a P2Pool node answers.

### 7.1 Negotiation

Extension type **`0x5052`**, "Requested Payout Outputs", from SV2's
experimental range `0x4000`–`0x7fff`. Experimental ids must not ship in
production equipment; a production id is allocated through
stratumprotocol.org, and asking for one is the owner's call. #195 and
#202 used `0x0003`, which #203 now holds.

The client asks for it with `RequestExtensions` (extension `0x0001`) on
its Job Declaration connection, in both modes. The Mining connection
needs nothing new: the custom job is a standard `SetCustomMiningJob`.

### 7.2 Allocation

- `AllocateMiningJobToken.user_identifier` names whom the outputs pay, in
  the pool's format. The token remembers it.
- `AllocateMiningJobToken.Success.coinbase_tx_outputs` reserves the
  coinbase space, as in base Job Declaration and #202: zero-value outputs
  whose serialized size bounds every later set for that token, for the
  client's `CoinbaseOutputConstraints`.

### 7.3 Messages

| Type | `channel_msg_bit` | Message | Direction |
|---|---|---|---|
| `0x00` | 0 | `RequestPayoutOutputs` | client to pool |
| `0x01` | 0 | `RequestPayoutOutputs.Success` | pool to client |
| `0x02` | 0 | `RequestPayoutOutputs.Error` | pool to client |
| `0x03` | 0 | `PayoutOutputsStale` | pool to client |

Every frame carries `extension_type = 0x5052`.

`RequestPayoutOutputs`, #202's message with the template fields added:

| Field | Type | Meaning |
|---|---|---|
| `request_id` | U32 | Echoed in the reply |
| `mining_job_token` | B0_255 | From `AllocateMiningJobToken.Success` |
| `available_payout_value` | U64 | Satoshis the set pays out |
| `height` | U32 | The template's block height |
| `prev_hash` | U256 | The template's previous block hash |
| `nbits` | U32 | The template's compact target |
| `coinbase_prefix` | B0_255 | The coinbase scriptSig before the extranonce, exactly as the job will declare it |

`RequestPayoutOutputs.Success`:

| Field | Type | Meaning |
|---|---|---|
| `request_id` | U32 | From the request |
| `flags` | U32 | Bit 0, `EXACT_OUTPUTS` (section 7.4); other bits zero |
| `coinbase_tx_version` | U32 | Required coinbase version when `EXACT_OUTPUTS` is set |
| `coinbase_tx_input_nSequence` | U32 | Required input sequence when `EXACT_OUTPUTS` is set |
| `coinbase_tx_locktime` | U32 | Required lock time when `EXACT_OUTPUTS` is set |
| `coinbase_tx_outputs` | B0_64K | A CompactSize count, then that many serialized outputs, as in `SetCustomMiningJob` |

`RequestPayoutOutputs.Error`: `request_id` (U32) and `error_code`
(STR0_255). `PayoutOutputsStale` has no fields.

### 7.4 Rules

- **One set per job.** The client asks after it has a token and a
  template, before each `DeclareMiningJob` or `SetCustomMiningJob`, and
  uses a set for one job only, as in #202. It keeps hashing its current
  job while it waits, and may ask ahead for a predicted template, as #195
  allowed.
- **Without `EXACT_OUTPUTS`,** #202's rules apply: the outputs sum to
  `available_payout_value`, the coinbase must contain every one of them,
  order does not matter, and the client may add outputs the base protocol
  allows.
- **With `EXACT_OUTPUTS`,** the coinbase's outputs are exactly these, in
  this order. Only a witness commitment, on a chain that has one, may
  follow them; BCH has none. Its version, input sequence and lock time
  are the ones given, and its scriptSig is `coinbase_prefix` followed
  directly by the extranonce. `available_payout_value` must equal the
  template's `coinbase_tx_value_remaining`. A client whose template needs
  other outputs of its own cannot use the set and moves to its next pool.
- **Validation.** The pool rejects a declared or custom job whose coinbase
  differs from the set, with its usual `DeclareMiningJob.Error` or
  `SetCustomMiningJob.Error`. It may recompute the set rather than store
  it; a token is used for one job.
- **Staleness.** When the pool's payouts move, it sends
  `PayoutOutputsStale`. Sets for tokens allocated before it are stale: the
  client allocates a new token and asks again. A request or job on a stale
  token may get `stale-payout-outputs`, which is an honest race, not a
  refusal: the client asks again with a new token, as 0x0003 treats its own
  stale code. Only `stale-payout-outputs` for a token allocated after the
  last `PayoutOutputsStale`, again and again, counts as a refusal.
- **Errors.** `invalid-mining-job-token`, `stale-payout-outputs` (the token
  or the template no longer matches what the pool can pay),
  `invalid-request`, `payout-outputs-too-large` and
  `payout-outputs-unavailable`. Apart from the stale race above, every
  refusal is an ordinary SV2 error, and the client fails over to its next
  pool, ending in solo mining.

### 7.5 How a P2Pool node answers

- **Identity.** `user_identifier` is a mining identity, as for a mining
  channel ([Miner options](../miner-options.md)): the payout CashAddr,
  then `p2p=` with the share address and an optional `donation=`. The
  mining channel that installs the job must use the same identity.
- **Outputs.** The PPLNS window at the token's share-chain anchor, paid
  out of `available_payout_value` at the template's height, previous hash
  and bits, with the share commitment over `coinbase_prefix`: the bytes
  `/jd_prepare` builds. That is the exact empty merge-mining commitment as
  output 0, the PPLNS outputs, padding, and the share commitment last.
  `EXACT_OUTPUTS` is always set, with version 2, sequence `0xfffffffe` and
  lock time height − 1. A job that differs is refused as any declared job
  is, for example `invalid-pplns-outputs` or `invalid-coinbase-layout`.
- **Reservation.** On a connection that negotiated the extension, the
  allocation's zero-value outputs list output 0, the payout scripts, the
  largest padding and the commitment, so they bound every answered set.
  Other connections get the payout scripts alone, as before.
- **Staleness.** The node sends `PayoutOutputsStale` whenever the share it
  anchors new tokens on, or the BCH block its template builds on, changes:
  about every 10 seconds.
- **Anyone can check it.** The outputs are a function of the share chain,
  the request and the identity, so every honest node holding the share
  chain gives the same answer, and anyone holding it can recompute one.
  A miner who doubts a node switches to another or runs their own.
- **Merge-mined tokens** still need Full-Template with the leaves in
  `excess_data` ([BCH P2Pool Job Declaration](../bch-job-declaration.md));
  this extension serves the exact empty commitment.

### 7.6 Node native, private, censorship resistant

- **In-band.** The messages travel on the Noise-encrypted Job Declaration
  connection: no HTTP, no sidecar, nothing between the node and the pool.
  `/jd_prepare` stays for clients that already use it.
- **No transactions.** The request carries none, so in Coinbase-only mode
  the pool learns nothing of the miner's transaction set. Full-Template
  reveals what base Job Declaration already reveals.
- **What the pool learns:** the token, the template's height, previous
  hash and bits (public anyway), the scriptSig prefix, and the revenue,
  which hints at the template's total fees.
- **Answer or refuse.** The pool chooses no transactions and cannot change
  the client's template; a refusal moves the client to its next pool.

### 7.7 Known limit

`coinbase_tx_outputs` is a B0_64K field here and in `SetCustomMiningJob`,
so a declared coinbase carries at most 65,535 bytes of outputs, about
1,900 P2PKH payees. A share without tokens pays every payee directly up to
BCH's 1,000,000-byte transaction limit, so past about 1,900 payees a
declared job cannot carry its set and gets `payout-outputs-too-large`, as
`/jd_prepare` already refuses it. Lifting that needs a share-chain rule,
such as a declared-job limit at which the payout tree takes over; that is
a later decision for the owner.

Evidence before this is called done: a chipnet run in which a standard
client, using only this extension, gets custom jobs accepted and shares
credited in both Job Declaration modes.
