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
| Donation and fee | Weights in one split | Already weights in one split on BCH | Fits, no change |
| Dust | Drops a payout below its `dust_limits[i]` | Pays every payee | Fits: the pool publishes dust limit 0 |
| Rounding remainder | To `pool_payout`, the first output | To the largest payee, the last | Same amounts if the largest payee (or the tree output) is published as `pool_payout`; only its position moves (section 4) |
| Output order | `pool_payout`, payouts, fixed outputs, client outputs | Merge-mining commitment first, share commitment last | Rule change |
| Weight size | 64 bits (the 8-byte amount field) | 128 bits: difficulty times 10^9 (uncle tenths, then basis points squared) over 3 BCH blocks of work | Rule change: at a mainnet difficulty of a few hundred billion, a window's total is around 10^21, past the 64-bit limit of 1.8 × 10^19 |
| Share commitment | No place for it; a client adds only zero-value outputs it chooses | Per miner, per share, binds the client's own scriptSig prefix | The pool could send it only per connection, and only if it stopped binding the prefix: a rule change, and a distribution that is no longer pool-wide |
| Update rate | MUST NOT publish per share; the client uses a new distribution only for later jobs | New split and previous share every share, about 10 seconds | Conflict: publishing less often makes clients mine on old shares, which end as uncles or orphans |
| Payout tree | Fixed output scripts, weights only | The tree output's script holds its exact value and outputs, which depend on the template's revenue | Blocker: no distribution can express it. It applies past about 29,400 P2PKH payees on a share without tokens, the scale BCH is built for |
| Client outputs | The client may append zero-value outputs | Validators reject any extra output | Rule change |

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
outputs. Two departures from the extension's intent: a distribution per
connection, and one per share. Even then the payout tree cannot be
expressed, so it fails at the payee counts BCH should reach. A covenant
that computes tree amounts from weights would fix that, but it is a
redesign of the hybrid payout. Not recommended.

**B. A request/response extension.** For each template, the client asks
the pool for its exact coinbase outputs, and the pool answers with the
bytes `/jd_prepare` already builds:

- The client sends the mining job token, the template's revenue, height,
  previous block hash, bits and its own scriptSig prefix.
- The pool answers with the exact output vector, or an error such as a
  stale token or a mismatched tip.
- The pool also says when the share chain moves, so the client allocates a
  new token and asks again, and how many coinbase bytes it needs, for the
  client's `CoinbaseOutputConstraints`.

No share-chain rule changes, and the share commitment, the payout tree and
the 10-second updates all work as they do for Pickaxe today. It costs one
round trip per template and per share-chain move, and the client trusts
the pool's arithmetic; any node can still check a share's coinbase against
its own share chain. The wording can stay pool-neutral: "the pool computes
the coinbase outputs for the client's template", which suits any pool whose
outputs depend on the template.

| | A: 0x0003 | B: request/response |
|---|---|---|
| Share-chain rule changes | Four | None |
| Payout tree (large windows) | Not expressible | Works |
| Share commitment | Per connection, prefix unbound | As today |
| Round trips | None per template | One per template |
| Standard | Proposed, approved, not merged | New; needs an allocated id |
| Client code | 0x0003 | This extension |

## 6. Recommendation

Build B. Option A would change four share-chain rules and still break at
BCH scale; option B changes none. On the kth side, the row 24 job engine
already takes its coinbase outputs as an input, so its Job Declaration
client gets one more source of outputs: ask the pool per template.

Before building:

- **Extension id.** SV2 reserves `0x4000`–`0x7fff` for experimental
  extensions that must not ship in production equipment; a production id
  is allocated through stratumprotocol.org. Chipnet tests can use an
  experimental id; asking for an allocation is the owner's call.
- **Messages.** Exact fields and error codes, following the Job
  Declaration and 0x0003 conventions, and whether both the Job Declaration
  and Mining connections negotiate it, as 0x0003 requires.
- **Evidence.** A chipnet run in which a standard client, given only this
  extension, gets shares accepted in both Job Declaration modes.
