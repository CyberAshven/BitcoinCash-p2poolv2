# P2Pool payouts for standard SV2 Job Declaration clients

Extension `0x5052`, "Requested Payout Outputs", lets a standard Stratum V2
Job Declaration client mine on a P2Pool node and be paid by P2Pool's
direct PPLNS outputs. Before each job the client asks its Job Declaration
connection for the job's exact coinbase outputs, then declares the job on
them. The node answers with the bytes `POST /jd_prepare` builds
([BCH P2Pool Job Declaration](../bch-job-declaration.md#read-only-preparation-for-an-external-declarator)),
so the client needs no HTTP call and no knowledge of how P2Pool builds a
coinbase. The extension changes no share-chain rule.

A Bitcoin Cash node whose listener serves mining grants `0x5052` to every
Job Declaration connection that asks for it, in either mode. No setting
turns it on or off. A Mining connection and a Bitcoin node do not get it.
The code is `stratum::sv2::payout_outputs`.

## Why a standard client cannot build the coinbase

Every node rebuilds a share's coinbase and refuses the share if it differs
([Coinbase payouts](../payouts/coinbase.md)):

1. **Output 0** is the merge-mining commitment, the exact empty one when no
   token is merge-mined ([Auxiliary work proofs](../bch-auxpow.md)).
2. **The PPLNS outputs** split the template's revenue over the window, with
   each share's donation and fee as weights in the same split. Every payee
   is paid; past the coinbase limit the largest are paid through one payout
   tree output ([Hybrid payouts](bch-hybrid-payouts.md)).
3. **Padding, then the share commitment** last. It binds the previous
   share, uncles, bits, time, donation and fee, the miner's scriptSig
   prefix, payout address and share owner
   ([Share commitments](../sharechain/share-commitments.md)).

The split and the previous share change with every share, about every 10
seconds. The payout tree and the share commitment depend on the template's
revenue and the client's scriptSig prefix. So the outputs cannot exist
before the pool sees those, and a client without the share chain cannot
compute them.

## Why not extension 0x0003

Extension 0x0003, "Coinbase Transaction Payouts"
([sv2-spec PR #203](https://github.com/stratum-mining/sv2-spec/pull/203)),
has the pool publish weights in `SetPayoutDistribution`. The client
computes `floor(weight * R / W)` for its own template, gives the remainder
to the pool-controlled first output, `pool_payout`, and drops payouts below
each dust limit. Against a P2Pool share:

| Point | 0x0003 | P2Pool |
|---|---|---|
| Custody | `pool_payout` is controlled by the pool | No operator; every payee is paid directly |
| Output order | `pool_payout`, payouts, fixed outputs, client outputs | Merge-mining commitment first, share commitment last |
| Weight size | 64 bits | 128 bits: a mainnet window's total is around 10^21, past the 64-bit limit of 1.8 × 10^19 |
| Share commitment | No place for it | Per miner and per share, binding the client's scriptSig prefix |
| Update rate | Not per share; a new distribution applies to later jobs | A new split and previous share every share; work on old ones ends as uncles or orphans |
| Payout tree | Fixed output scripts, weights only | The tree output's script holds its exact value, so no distribution can express it; it applies past about 29,400 P2PKH payees on a no-token share |
| Client outputs | The client may append zero-value outputs | Any extra output is refused |

Adopting 0x0003 would take four share-chain rule changes (output order,
64-bit weights, a commitment that no longer binds the prefix, client
outputs) and would still fail at the payee counts BCH should reach. The
rounding remainder is not a blocker: with the largest payee as
`pool_payout`, every amount is the same and only that output moves
([an example](../payouts/coinbase.md#an-example)).

The request/response design changes no rule, and the payout tree, the share
commitment and the 10-second updates work as they do for Pickaxe. It costs
one round trip per job, beside the one `DeclareMiningJob` or
`SetCustomMiningJob` already costs; the client keeps hashing its current
job meanwhile, and on a node runner's own P2Pool node the call is local.

The messages are those of sv2-spec PR #202 ("non-custodial pool payouts",
`RequestPayoutOutputs`, closed in favour of `0x0003`; PR #195, "Dynamic
Coinbase Outputs", had the same shape with `RequestCoinbaseOutputs`). P2Pool adds the template fields and
the scriptSig prefix to the request, exact output placement with the
coinbase version, sequence and lock time to the reply, a push when the
share chain moves, and a check by a second node. Proposed upstream as
sv2-spec PR #249 with id `0x0004`, it was closed unmerged; P2Pool keeps the
experimental id `0x5052`.

## Negotiation

`0x5052` is from SV2's experimental range `0x4000`–`0x7fff`. The client asks
for it with `RequestExtensions` (extension `0x0001`) after `SetupConnection`
on its Job Declaration connection, in either mode, together with `0x4C54`
([large templates](sv2-large-templates-extension.md)) when it wants both.
The Mining connection needs nothing new: the job is a standard
`SetCustomMiningJob`. A `0x5052` message on a connection that did not
negotiate it closes the connection.

## Messages

Every frame carries `extension_type = 0x5052` with the channel bit clear.
Integers are little endian, and `prev_hash` is in the byte order of
`SetCustomMiningJob.prev_hash`.

| Type | Message | Direction |
|---|---|---|
| `0x00` | `RequestPayoutOutputs` | client to pool |
| `0x01` | `RequestPayoutOutputs.Success` | pool to client |
| `0x02` | `RequestPayoutOutputs.Error` | pool to client |
| `0x03` | `PayoutOutputsStale` | pool to client |
| `0x04` | `VerifyPayoutOutputs` | client to pool |
| `0x05` | `VerifyPayoutOutputs.Success` | pool to client |
| `0x06` | `VerifyPayoutOutputs.Error` | pool to client |

`RequestPayoutOutputs`:

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
| `flags` | U32 | Bit 0, `EXACT_OUTPUTS`; other bits zero |
| `coinbase_tx_version` | U32 | Required coinbase version when `EXACT_OUTPUTS` is set |
| `coinbase_tx_input_nSequence` | U32 | Required input sequence when `EXACT_OUTPUTS` is set |
| `coinbase_tx_locktime` | U32 | Required lock time when `EXACT_OUTPUTS` is set |
| `coinbase_tx_outputs` | B0_64K | A CompactSize count, then that many serialized outputs, as in `SetCustomMiningJob` |

`RequestPayoutOutputs.Error` is `request_id` (U32) and `error_code`
(STR0_255). `PayoutOutputsStale` has no fields. `VerifyPayoutOutputs` has
the request's fields with `user_identifier` (STR0_255, as in
`AllocateMiningJobToken`) after the token, which here is a token another
node issued. Its Success and Error have the fields of
`RequestPayoutOutputs.Success` and `.Error`.

## Rules

- **Allocation.** `AllocateMiningJobToken.user_identifier` names whom the
  outputs pay, and the token remembers it. On P2Pool it is a mining
  identity, as for a mining channel
  ([Miner options](../miner-options.md#stratum-v2)): the payout CashAddr
  and worker, then `p2p=` with the share address unless the node
  configures one, and an optional `donation=`. The mining channel that
  installs the job must use the same identity. A token whose identity names
  no payout address or share owner is still allocated; a request for its
  outputs gets `invalid-request`.
- **Reservation.** On a connection that negotiated `0x5052`, the
  allocation's zero-value outputs list output 0, the payout scripts, the
  largest padding and the commitment, so their size, reserved for
  `CoinbaseOutputConstraints`, bounds every answered set. Other connections
  get the payout scripts alone.
- **One set per job.** The client asks after it has a token and a
  template, before each `DeclareMiningJob` or `SetCustomMiningJob`, and uses
  a set for one job. It keeps hashing its current job while it waits, and
  may ask ahead for a predicted template. A connection may hold sets for
  eight open tokens; asking for a ninth closes it, as a ninth pending
  declaration does.
- **The outputs.** The node pays `available_payout_value` over the PPLNS
  window at the token's share-chain anchor, at the template's height,
  previous hash and bits, with the share commitment over `coinbase_prefix`:
  the exact empty merge-mining commitment as output 0, the PPLNS outputs,
  padding, and the share commitment last. It always sets `EXACT_OUTPUTS`,
  with version 2, sequence `0xfffffffe` and lock time `height - 1`.
- **`EXACT_OUTPUTS`.** The coinbase's outputs are exactly these, in this
  order (only a witness commitment may follow, and BCH has none), with the
  given version, sequence and lock time, and its scriptSig is
  `coinbase_prefix` followed directly by the extranonce.
  `available_payout_value` must equal the template's
  `coinbase_tx_value_remaining`. Without the flag, #202's rules apply: the
  coinbase contains every output, in any order, and may add outputs the
  base protocol allows. A client whose template needs other outputs of its
  own cannot use the set and moves to its next pool.
- **Validation.** The job is checked as any declared job
  ([Rules a declared job must meet](../bch-job-declaration.md#rules-a-declared-job-must-meet)).
  Its `coinbase_prefix` carries a [coinbase tag](../stratum.md#the-coinbase-tag)
  with the chain line of the node, or the custom job gets
  `invalid-coinbase-tag`; the outputs answer does not check it.
  A coinbase that differs from the set is refused with the usual
  `DeclareMiningJob.Error` or `SetCustomMiningJob.Error`; for a custom job
  the code is `invalid-custom-job`, with the reason, such as
  `invalid-pplns-outputs` or `invalid-coinbase-layout`, in the node's log.
  A token is used once, so the node recomputes the set rather than store
  it.
- **Merge-mined tokens** still need Full-Template with the leaves in
  `excess_data` ([Declared token leaves](../bch-job-declaration.md#declared-token-leaves));
  this extension serves only the exact empty commitment.

Token lifetimes and counts are those of every Job Declaration token
([Job tokens](../bch-job-declaration.md#job-tokens)), and so are installed
jobs ([Installed jobs](../bch-job-declaration.md#installed-jobs)).

## Staleness

The node sends `PayoutOutputsStale` to each connection that negotiated
`0x5052` whenever the share it anchors new tokens on, or the Bitcoin Cash
block its template builds on, changes: every few seconds on a live pool.
Sets for earlier tokens are then stale: the client allocates a new token
and asks again.

`stale-payout-outputs` answers a token anchored on an earlier share, and a
request whose height, previous block or bits are not those of the token's
template. Neither is a refusal. For a token allocated after the last
notice, even again and again, it means the client's node and the pool's
node are on different Bitcoin Cash tips or disagree on the bits, which is
ordinary for a moment after each block and ends by itself. The client keeps
asking, no faster than once every few seconds, keeps mining its last
installed job or the pool's own jobs meanwhile, and does not fail over for
this alone. At the job itself the same race shows as `stale-chain-tip` or
`template-unavailable` ([Full-Template](../bch-job-declaration.md#full-template)),
and the client declares again.

## Errors

| Code | When |
|---|---|
| `invalid-mining-job-token` | The token is unknown, expired or used; for a verify, it does not decode |
| `stale-payout-outputs` | The token's anchor or template no longer matches ([above](#staleness)) |
| `invalid-request` | The identity does not resolve, the height is 0, the prefix lacks the height push, the revenue is past BCH's money range, or the builders refuse the request |
| `payout-outputs-too-large` | The outputs do not fit the coinbase or the 65,535-byte `B0_64K` field |
| `payout-outputs-unavailable` | The template has no PPLNS weights; for a verify, the window at that anchor pays no one or the node does not pay from a share-chain window |
| `unknown-anchor` | Verify only ([below](#verifying-with-a-second-node)) |
| `verify-rate-limited` | Verify only: the connection's verify budget is spent |

None of these closes the connection; a malformed message does. Apart from
`stale-payout-outputs`, `unknown-anchor` and `verify-rate-limited`, a
refusal is an ordinary SV2 error, and the client fails over to its next
pool, ending in solo mining.

## Known limit

`coinbase_tx_outputs` is a B0_64K field here and in `SetCustomMiningJob`,
so a declared coinbase carries at most 65,535 bytes of outputs, about 1,900
P2PKH payees. A no-token share pays every payee directly up to BCH's
1,000,000-byte transaction limit
([the coinbase limit](bch-hybrid-payouts.md#33-the-coinbase-limit-c)), so
past about 1,900 payees a declared job cannot carry its set and gets
`payout-outputs-too-large`, as `/jd_prepare` refuses it. Lifting that needs
a share-chain rule, such as a declared-job limit at which the payout tree
takes over; none is set.

## Privacy and censorship resistance

- The messages travel on the Noise-encrypted Job Declaration connection:
  no HTTP, no sidecar. `/jd_prepare` stays for clients that use it.
- The request carries no transactions, so in Coinbase-Only mode the pool
  learns nothing of the miner's transaction set. Full-Template reveals what
  base Job Declaration reveals.
- The pool learns the token, the template's height, previous hash and bits
  (public anyway), the scriptSig prefix, and the revenue, which hints at the
  template's total fees.
- The pool chooses no transactions and cannot change the client's
  template; a refusal moves the client to its next pool.

## Checking the outputs

The outputs are a function of the share chain, the request and the
identity. A client checks the commitment output itself, and the whole set
with a second node. A miner who doubts a node switches to another or runs
their own.

### The local commitment check

A client rebuilds the share commitment hash from the token
([its layout](../bch-job-declaration.md#job-tokens)) and the identities it
allocated with, and checks that the last output is value 0, `OP_RETURN`, a
32-byte push and that hash. That shows the set credits those identities; it
does not show that the PPLNS split is the window's. The hash is SHA256 of
these bytes, in order, integers little endian:

1. The previous share hash, the compact-size uncle count, the uncle hashes
   and the share bits: the token's bytes after the nonce, through the bits.
2. The token's time.
3. Donation, then fee. Each is a byte `0` (no address) or `1` followed by
   the address's text as a compact-size length and its bytes, then the
   basis points as a `u16` (`0` without an address). The text is the
   address's display form (the Bitcoin spelling of its hash under its
   network class, or CashAddr for a P2SH32 script), not the network class
   and script the token stores.
4. The share witness root: 32 zero bytes, since a job token carries no
   share transactions.
5. For a Bitcoin Cash P2PKH share owner: `0x22` and the share version (two
   `u32`); `0x24`, the token's bits, the band cap (`u32`) and the token's
   bits again (wanted, cap, minimum); `0x25` and the payout level byte;
   `0x26`, the weak ratio (`u32`), two zero `u16` (no weak shares, none
   asking) and 32 zero bytes (their empty root).
6. The declared coinbase: `0x21`, the scriptSig prefix as a compact size
   and its bytes, then the merge-mining commitment (32-byte root, height
   byte, salt `u32`). Here it is the exact empty one: 32 zero bytes, `0`,
   `0`. A hash that leaves this out does not match.
7. No `0x23`: that tag is a pool-built job's token root.
8. The miner's payout: network class (`0` mainnet, `1` test, `2` regtest),
   then script, the address encoding the token uses.
9. The share owner: `0x20`, a length byte `20`, and the 20-byte hash.

### Verifying with a second node

`VerifyPayoutOutputs` asks a node that did not issue the token, on a Job
Declaration connection that negotiated `0x5052`, to recompute the set. The
node stores and allocates nothing. It decodes the token, resolves
`user_identifier` as it resolves `AllocateMiningJobToken.user_identifier`,
computes the PPLNS weights at the token's anchor with the request's
`nbits` and `prev_hash` as the paying block, as share validation does, and
builds with the same builder. The success payload is the bytes the issuing
node would send; the client compares the two byte for byte. A client that
sends a pool-to-client message (`0x01` to `0x03`, `0x05`, `0x06`) is
closed.

Limits keep one connection from making the node do unbounded work:

- The SV2 field types bound the token, the identity, the prefix and the
  answer, and the token decode refuses more than 3 uncles.
- An anchor the node does not hold, one more than 100 shares below its
  confirmed tip (`MAX_VERIFY_ANCHOR_DEPTH`), one whose reads fail, or one
  whose Bitcoin Cash header the window needs and the node does not know yet
  is `unknown-anchor`, before the window is touched. A token anchors on its
  issuer's tip, so an honest verify names a share at or near the
  verifier's. Within that depth the paid set stops at the window's own
  bound: the share-count cap, or three blocks of the paying block's work
  when that difficulty is above 1.
- One verify at a time per connection, and at most 4 a second; one more
  gets `verify-rate-limited` and the connection stays up. Unlike a ninth
  `RequestPayoutOutputs`, it does not close the connection.

`unknown-anchor` is an honest race while a share propagates: the client
retries or asks another node. It is not agreement, and not a reason to
leave the pool.

### What the two checks trust

Matching payloads from the issuing node and one other node that holds the
same share chain mean both computed the same outputs. The local check shows
that the last output credits the client's identities, also while it waits
for the second answer. Two nodes that collude can still agree on a false
set, and a client that hashes the first set before the second answer
arrives is exposed for that one round trip.

## Tests

The tests and how to run them are on
[BCH P2Pool Job Declaration](../bch-job-declaration.md#tests). They run the
extension with this repository's client, on regtest and against the
chipnet pool; a run with an independent client is still open.
