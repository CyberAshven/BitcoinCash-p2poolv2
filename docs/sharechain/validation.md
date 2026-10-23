# Share validation

> Ported from upstream P2Poolv2's documentation: `architecture/validation.adoc`
> in [p2poolv2/docs](https://github.com/p2poolv2/docs) at commit `623e4c1`,
> Copyright 2024-2026 P2Poolv2 Developers, MIT or Apache-2.0. Adapted to
> Bitcoin Cash by this fork; see the [port map](../upstream-port-map.md).

A share claims a proof of work and is paid for it. Every node validates every
share it adds to its share chain, so that no proof of work is paid twice:
without these checks a node could reuse one proof of work in several shares
and collect several payouts for the same work. Nodes also check each share's
transactions and timestamp, which makes the share chain's transactions
verifiable.

These are share-chain rules. BCHN never sees them, and none of them changes
what Bitcoin Cash accepts as a block. The code is
`p2poolv2_lib/src/shares/validation/mod.rs`; the order in which a node runs
the checks is in
[Share processing pipeline](../architecture/share-processing-pipeline.md).

## The proof of work meets the share difficulty

A share's declared target must equal the target ASERT computes from its
ancestors, and its hash must meet it. On Bitcoin Cash the share chain's ASERT
is BCH's own integer formula (aserti3-2d), anchored at each share's parent
rather than at genesis (plan decision 12; see
[Share difficulty: ASERT](../difficulty_adjustment/asert.md)). A target may
never be easier than the chain's ceiling: `MAX_POOL_TARGET` on the main chain
and a ceiling 256 times easier on the mini chain
([Small miners](../small-miners.md)).

Share headers carry everything needed to check the target, but only once all
of a header's ancestors are known. During the first sync a node therefore
checks proof of work in order, after the headers have arrived; in normal
operation it waits for any missing ancestor.

## Timestamp limits

ASERT sets difficulty from share times and heights, so miners must not be
able to move timestamps to make their shares easier. A share's time must be
later than the median of its 11 ancestors' times (`MTP_WINDOW`) and at most
120 seconds ahead of the node's clock (`MAX_FUTURE_TIME_SECS`). Header sync
applies the same limits. Keep the node's clock synced
([Requirements](../requirements.md#the-p2poolv2-node)).

## One proof of work, one share

No two shares can use the same Bitcoin Cash proof of work. Each share commits
to its header fields inside the Bitcoin Cash coinbase that the hashed header
commits to, so a header with proof of work fits only the share header it
commits to. See [Share commitments](share-commitments.md).

## Token shares pass the ASERT header check

A Bitcoin Cash share whose merge-mining commitment is not the exact empty
one is a token share. Its Bitcoin Cash header's nBits must be what BCH's
aserti3-2d gives at the coinbase's height, within the band agreed with
Pickaxe: from half to 1.05 times the computed target on mainnet, from an
eighth to 1.05 times, or the proof-of-work limit, on chipnet. A token claim
checks the same band, so a share outside it could not claim a token. Header
sync and block receipt run this check. Regtest has no ASERT anchor and skips
it, and no-token shares are never checked: BCHN alone judges their blocks.
See [Auxiliary work proofs](../bch-auxpow.md#the-asert-header-check).

## The body matches the header

The share block hash covers the header and not the body, so a peer could pair
a valid header with an altered body. Before it stores a share, a node checks
the body against the header using the share alone:

1. Each non-coinbase share transaction appears once, and together they give
   the share transaction root the header's coinbase proof carries. Repeats
   are refused because a merkle tree pairs the last node of an odd level
   with itself, so a repeated last transaction leaves the root unchanged
   (CVE-2012-2459).
2. The share coinbase is exactly the one the header implies.
3. The Bitcoin Cash coinbase has the transaction id the coinbase proof gives,
   and carries no witness.

A body that fails is a bad copy, not a bad share: the node discards it and can
fetch the share again from another peer. Once a body passes, the proof of
work fixes it, and any later failure rejects the share.

## Uncles follow the rules

Uncles are paid, so they pass the same checks as every share, plus:

- a share names at most 3 uncles (`MAX_UNCLES`);
- each uncle is at most 3 shares below the share that names it
  (`MAX_UNCLES_DEPTH`);
- an uncle may have only one nephew on a chain: a share may not name an
  uncle that one of its ancestors already names. Without this, an uncle
  named twice would be paid twice. On Bitcoin Cash this rule is tagged
  `uncle-once` in the protocol strings.

How uncles are paid is on [PPLNS accounting](pplns-accounting.md#uncles).

## Bitcoin Cash payouts follow the PPLNS window

Every Bitcoin Cash coinbase a share carries must pay the share chain's PPLNS
window. The node rebuilds the whole coinbase from its own copy of the window,
anchored at the share's parent, and compares it with the coinbase the share
carries. The rebuild covers:

- the donation and the operator fee, each at most 10,000 basis points and
  together at most 10% (1,000 basis points) on Bitcoin Cash;
- the window itself: on Bitcoin Cash the shares holding three Bitcoin Cash
  blocks' worth of work, sized by the difficulty in the share's own header,
  behind a 100-share maturity skip and at most 120,960 shares;
- who is paid: on Bitcoin Cash the holder of each share's payout right
  ([Trading shares for BCH](../atomic-swap/bch-share-trading.md#payout-rights));
- output 0, the merge-mining v2 commitment: the exact empty one on a
  pool-built share, the declared root, `h` and salt on a Job Declaration
  share, rebuilt byte for byte ([Auxiliary work proofs](../bch-auxpow.md));
- no SegWit: a Bitcoin Cash coinbase that carries a BIP141 witness
  commitment output is invalid. BCHN's templates never have one;
- how the payouts become outputs. Today that is the coinbase budget of plan
  decision 14 (at most 11 payouts, the rest paid by a draw). This is an open
  design question and the rule is being redesigned; see
  [Coinbase payouts](../payouts/coinbase.md).

A share whose coinbase pays anyone a different amount is invalid. The window
rules are on [PPLNS accounting](pplns-accounting.md).

Payouts from the share chain are Bitcoin Cash coinbase outputs. Bitcoin Cash
itself makes them spendable after 100 blocks (`COINBASE_MATURITY` in BCHN's
`src/consensus/consensus.h`); the share chain adds no wait of its own to them.

## Share transactions are valid

Every share transaction, the share coinbase included, must be valid in the
value it spends, its scripts and the outputs it spends. As on Bitcoin Cash,
nodes check for double spends and for inputs worth at least the outputs, so
no one creates share units from nothing. On Bitcoin Cash:

- scripts run in this fork's Bitcoin Cash interpreter
  (`shares/bch_script.rs`), which accepts P2PKH spends and the share-trading
  contract behind P2SH32, and nothing else;
- new outputs must be P2PKH or P2SH32 and at or above BCHN's dust threshold;
- a transaction must be final at its share's height;
- a share coinbase output can be spent once it is 100 shares deep, and no
  output can be spent once the coinbase it descends from is more than
  127,008 shares deep (`PAYOUT_DEPTH`);
- a share holds at most 100 transactions (`TXS_COUNT_LIMIT`), its body (its
  share transactions plus the Bitcoin Cash coinbase it carries) at most
  200 KiB (`BLOCK_TXS_SIZE_LIMIT`);
- the share transactions execute at most 3,000 SigChecks each
  (`MAX_TX_SIGCHECKS`, BCH consensus) and 1,452 in all
  (`BCH_MAX_SHARE_SIGCHECKS`: 200 KiB at BCH's one SigCheck per 141 bytes).
  Each accepted input runs exactly one signature check; outputs and the
  coinbase count none. This replaced Bitcoin's BIP141 sigop cost.

See [Share transactions](transactions.md) and
[Spending share outputs](spending-outputs.md).

## Mining on the current chain

Upstream adds a rule that a coinbase pays a share only if the Bitcoin Cash
block the share builds on sits on the paying share's chain, or one block off
it. **This fork has not built it yet.** See
[Mining on the current chain](current-chain.md) for the design and what it
needs on Bitcoin Cash.

## Bitcoin Cash blocks are not validated

Nodes do not validate the contents of the Bitcoin Cash block a share builds
on. They check the proof of work in the header the share carries, and they
check the Bitcoin Cash coinbase in full: each share carries the coinbase and
its merkle branch, so a node can check the share commitment and compare the
payouts with its own window. Validating whole blocks would slow share relay,
raise the uncle rate, and so lower the number of nodes the pool can serve.

What does get checked against BCHN:

- a node's own templates come from its own BCHN, or from a Template Provider
  it chose, and are checked before use (canonical transaction order, no
  witness data, target and bits);
- a miner's declared Full-Template job is checked by the node's BCHN as a
  block proposal before any share is accepted on it; a Coinbase-Only job
  hides its transactions and cannot be
  ([Job Declaration](../bch-job-declaration.md)).

A found block that turns out invalid is rejected by BCHN and every other
Bitcoin Cash node, and its finder loses the block. The share itself still
earns in the share chain; [Known attacks](../known-attacks.md) covers the
withholding attacks this allows.

## How deep validation goes

Shares within `PAYOUT_DEPTH` (127,008 shares) of the tip are fully validated:
every share a coinbase near the tip can pay. Deeper shares, met while a new
node syncs, get every check above except the payout, uncle-body and share
transaction checks (`is_within_payout_depth`, `validate_below_pplns_depth`).

A check that cannot be decided is never a verdict. A share whose parent or
uncle has not arrived waits for it. A share whose PPLNS window reaches below
what this node still holds is dropped without a verdict, because another
node holding more may judge it valid.
