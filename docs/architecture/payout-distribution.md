**The architecture files are LLM generated. The goal is make it easier
for new comers to the code base and other LLMs to quickly understand
the design and remain consistent with the design choices.**

# Payout Distribution

How a share that finds a bitcoin block splits the reward, in share chain
(P2Poolv2) mode. Hydrapool (`accounting/payout/simple_pplns/`) is out of
scope.

## Rules

`N` = coinbase maturity: `COINBASE_MATURITY` (6,048) on Bitcoin,
`BCH_COINBASE_MATURITY` (100) on Bitcoin Cash, as Satoshi's 100-block
coinbase maturity; `W` = `PPLNS_WINDOW_SHARES` (120,960),
`K` = `BCH_PPLNS_WINDOW_BLOCKS` (3), `p` = paying share height, `D` = the
difficulty of the paying share's own bitcoin header (`bitcoin_header.bits`,
through `bitcoin_block_difficulty`).

1. **Outputs:** donation cut, then fee cut, then the remainder split over
   the window by weighted difficulty.
2. **Anchor:** the window is walked from the paying share's
   `prev_share_blockhash`, never from a node's tip.
3. **Maturity:** a share at height `h` is paid only if `h < N` or
   `p - h >= N`. The walk skips
   `min(N - 1, p.saturating_sub(N))` shares from the anchor, then takes the
   window.
4. **Window:** on Bitcoin, `W` shares. On Bitcoin Cash, the shares up to and
   including the first one that brings the window's work to `K * D`, at most
   `W` shares. A share's work is its difficulty plus its uncles'. If
   `D <= 1` (the header is at the network's proof-of-work limit) the window
   is `W` shares. See "Bitcoin Cash window" below.
5. **Weights:** share difficulty x 10. Each uncle of a paid share: the
   uncle's miner gets 9/10 of its difficulty and the nephew's miner 1/10.
   Uncles use no window slots.
6. **Payee:** on Bitcoin, each share's and uncle's `miner_bitcoin_address`.
   On Bitcoin Cash a share's own weight, nephew bonus included, goes to
   whoever holds the share's payout right at the anchor: its
   `miner_bitcoin_address` while the holder is the share's own
   `miner_address` key hash, else the P2PKH address of the holder's key hash.
   An uncle's weight always goes to the uncle's `miner_bitcoin_address`. See
   "Bitcoin Cash payout rights" below.
7. **Split:** addresses sorted by string, `amount * weight / total`
   truncated, and the last address gets the remainder.
8. **Coinbase budget (Bitcoin Cash):** at most `C` = 11 payout outputs
   (`BCH_MAX_COINBASE_PAYOUTS`) holding at most `B` = 374 bytes
   (`BCH_MAX_PAYOUT_OUTPUT_BYTES`), no payout below BCHN's dust threshold.
   Payees that fit are paid by rule 7; past the budget the largest payees
   are paid their exact share and the rest of the amount goes to one of the
   other payees, drawn by weight. See "Bitcoin Cash coinbase budget" below.
   Bitcoin pays every payee.

## Invariants

- **Deterministic.** The distribution is a function of the chain below
  the anchor and, on Bitcoin Cash, the paying share's own header and the
  share transactions in the anchor's ancestry only. No config, no difficulty
  from a node's own bitcoin node, no local tip, and no validation status.
  The producer and every validator must build identical outputs, so any
  change to the walk, the window, the weights or the payee is a consensus
  change.
- **One code path.** The notifier (`Payout::fill_distribution_from_shares`)
  and the validator (`validate_bitcoin_payout`) call the same
  `PplnsWindow::get_distribution_from_start_hash` on the same shared window,
  and both turn bits into `D` with `bitcoin_block_difficulty`. The notifier
  reads the bits from its template. They are the bits the share's header
  carries, because a stratum miner cannot change them and a declared job must
  keep them (`validate_custom_job` checks `nbits`). The validator reads them
  from the header. Both then split the weights with `build_pplns_outputs`'s
  `append_payout_split` under `PayoutSplit::for_chain` of the same anchor,
  so the Bitcoin Cash coinbase budget (rule 8) is applied identically.
- **Payee is `miner_bitcoin_address`** on Bitcoin, and on Bitcoin Cash for
  every share whose payout right its own miner holds. Never `miner_address`
  as an address: on Bitcoin Cash a transferred right pays the P2PKH address
  of its holder's key hash, which may be another key than the miner's.
- **No fallback payee.** An empty window, zero total weight, an unresolved
  anchor or a store error is an `Err`. Never pay `bootstrap_address`: it
  is node-local.
- **Candidate shares count like confirmed shares** towards both the skip
  and the window.
- **Immature shares are a prefix of the walk.** Heights fall by one per
  parent step, so one skip count is exact. Do not filter per share.
- **The first `N` shares are always mature for payout only.** Spending
  still needs depth `N` (`check_prevouts`).
- **Uncles need no maturity rule.** An uncle is at most `MAX_UNCLES_DEPTH`
  below its nephew, so it is never shallower than it.
- **Each uncle is paid once.** The walk credits every uncle reference it
  meets, so an uncle named by two nephews on one chain would be paid twice,
  with two nephew bonuses (`test_window_pays_an_uncle_once_for_each_nephew_naming_it`).
  Validation keeps that shape off every valid chain: an uncle may have only
  one nephew on a chain, so `validate_uncles` rejects a share naming an uncle
  that one of its ancestors already names (see `share-processing-pipeline.md`,
  "Uncle selection vs uncle acceptance"). This is a share-chain consensus
  rule on both chains.
- **A truncated walk is refused.** If the cache runs out before `W` shares
  and the cache does not reach height 0, return `InsufficientEntries`.
  Validation maps it to `Unresolvable`.

## Bitcoin Cash window

Bitcoin Cash sets `D` every block with ASERT, so `K * D` is `K` blocks' worth
of work for the pool. The window therefore spans about `K` of the pool's own
blocks at any pool size: short for a large pool, long for a small one, and
shorter by itself if BCH blocks get faster (CHIP-2025-03). At 10-second
shares and 10-minute blocks the window is about `K * 60 / f` shares for a
pool holding fraction `f` of BCH's hashrate, so pools under about `K / 2016`
(0.15%) reach the cap and pay exactly upstream's fixed window.

| Pool share of BCH | Window (K = 3) |
|---|---|
| 0.05% | capped at 120,960 shares, about 14 days |
| 1% | about 18,000 shares, 50 hours |
| 10% | about 1,800 shares, 5 hours |
| 30% | about 600 shares, 100 minutes |

- **Why `D` comes from the paying header.** BCH accepts a block only with the
  correct ASERT bits, so a coinbase that reaches the chain was sized by the
  real difficulty. A share that is not a block can carry any bits, but its
  coinbase never reaches the chain, so mis-sizing it pays nobody. A node's
  own BCHN, template or config would differ between nodes; upstream dropped
  its earlier window sized by local difficulty and `difficulty_multiplier`
  for that reason.
- **Why `K = 3`.** It is P2Pool v1's value on BCH (below). A share is paid
  by the blocks found while it is in the window, a Poisson count with mean
  `K`. With 3, a share misses every block 5% of the time and its payout
  varies by 58%; with 2, 13.5% and 71%. The window, the delay to be paid and
  how long a miner is paid after stopping grow linearly with `K`. In the
  offline simulation (`pplns_compare`), 3 instead of 2 moves a 0.1% miner's
  daily income variation from 0.47 to 0.43 at 10% of BCH and from 0.44 to
  0.41 at 30%, close to the spread across seeds (about 0.03), for under 1.5
  hours more delay there and about 8 hours more at 1%. Either value is
  defensible; 3 is the one v1 ran on BCH.
- **Prior art.** P2Pool v1 for BCH
  ([jtoomim/p2pool](https://github.com/jtoomim/p2pool)) sized its window the
  same way. `p2pool/networks/bitcoincash.py` sets `SHARE_PERIOD = 60`,
  `REAL_CHAIN_LENGTH = 3*24*60` (4,320 shares, three days) and `SPREAD = 3`.
  `generate_transaction` in `p2pool/data.py` walks back from the previous
  share for at most `min(height, REAL_CHAIN_LENGTH) - 1` shares and stops at
  a desired weight of `65535 * SPREAD * target_to_average_attempts(block_target)`,
  three expected blocks of work, where `block_target` is the paying share's
  own header bits. Differences here: v1 scales the share that crosses the
  target down to hit it exactly, while this rule pays that share whole, so a
  window holds at most one share's work more than `K * D`; v1 had no
  maturity skip, a 0.5% finder bonus and a three-day cap at 60-second
  shares, where this fork keeps the maturity skip and a 14-day cap at
  10-second shares. The idea is reimplemented here; no v1 code is copied
  (plan decision 10).
- **Minimum-difficulty headers.** Chipnet and testnet accept a difficulty-1
  block 20 minutes after its parent, and every regtest block is at the
  proof-of-work limit. Such a header says nothing about the pool's block
  rate, so a `D` of 1 or less pays `W` shares, as on Bitcoin, instead of
  shrinking the window to one share. Falling back to an older header was
  rejected: only a found block's bits are checked by BCH, so the bits in any
  other share are the miner's choice. Mainnet never has such a block. The
  load-test sim runs on regtest, so it still pays `W` shares.
- **Truncation.** A walk that reaches `K * D` inside the cache is a property
  of the chain even when the cache no longer reaches height 0. Only a walk
  that runs out of cached entries before either bound is refused.
- **Activation.** The rule is consensus, and nodes on different rules
  rebuild different coinbases for every share and fork the share chain. The
  BCH chains are alpha and run by their operators, so the change ships as a
  flag day: every node upgrades in one release and starts from a new store
  path, as the upstream sync did, so no share on the new chain was made
  under the old rule. Miners lose only their share-chain position, because
  payouts are coinbase outputs on BCH. Mainnet starts with the rule from its
  first share, so it needs no activation height.
- **Peers on another rule cannot connect.** On Bitcoin Cash every protocol
  string (request-response, Identify, Kademlia and the Noise prologue)
  carries `pplns-k<K>-m<N>` (now `pplns-k3-m100`) after the genesis tag,
  built from `BCH_PPLNS_WINDOW_BLOCKS` and `BCH_COINBASE_MATURITY` by
  `share_chain_rule_tag`. A node on the old fixed window, or on another `K`
  or maturity, fails the handshake instead of exchanging shares the other
  rejects, and changing either constant changes the tag by itself. The
  share difficulty rule follows it as `asert-parent`
  (`docs/difficulty_adjustment/asert.md`), the share transaction rules as
  `trade-htlc`, the payout to the holder of each share's payout right as
  `payout-holder` (`docs/atomic-swap/bch-share-trading.md`) and the coinbase
  budget as `payout-cap11` (below). Bitcoin's protocol strings are
  unchanged.

## Bitcoin Cash coinbase budget

The owner rejected this budget as a Bitcoin assumption; its proposed
replacement, not yet implemented, is
[bch-hybrid-payouts.md](bch-hybrid-payouts.md).

Rule 8. Upstream's gap: discussion
[#668](https://github.com/p2poolv2/p2poolv2/discussions/668) and issue
[#708](https://github.com/p2poolv2/p2poolv2/issues/708) ("Limit coinbase
size to 25 outputs"); `bch-plan.md` decision 14.

### Why a budget

- **Firmware.** Stock Antminer firmware accepts a coinbase of only "just
  over a dozen outputs" (#668). The evidence cited there is an OCEAN block
  mined on that firmware: 16 payouts in a 784-byte coinbase. P2Pool v1
  warned at 200 payees that "Antminer S9s may crash when this is close to
  226" (`generate_transaction` in jtoomim's `p2pool/data.py`), so the limit
  differs by model and firmware version. A device only hashes
  coinb1, the extranonce and coinb2, so the limit is on bytes, not outputs.
- **Who sees the coinbase.** Every Stratum V1 device gets coinb1 and coinb2,
  including SV1 devices behind a Stratum V2 translator such as Pickaxe's
  adapter. SV2 extended channels and declared jobs carry the coinbase prefix
  and suffix too. Only SV2 standard (header-only) channels never see it.
- **Before this rule** a Bitcoin Cash coinbase paid every window payee: a
  window with 50 miners put 50 outputs, about 1,700 bytes, into every job.

### The rule

Inputs: the payees and their weights after rules 5 and 6 (so a share's
weight is already credited to the holder of its payout right), the amount
`A` left after the donation and the fee, and the anchor. Bitcoin Cash only.

1. Each payee `i` has weight `w_i` out of `W`, a pro-rata amount
   `a_i = floor(A * w_i / W)`, an output of `s_i` bytes (34 for P2PKH, 32
   for P2SH) and BCHN's dust threshold `d_i` for that output (546 for
   P2PKH, 540 for P2SH; `bch_script::dust_threshold`).
2. **Fits.** If there are at most `C` payees, their outputs take at most `B`
   bytes and every `a_i >= d_i`, rule 7 pays every payee. A small pool, such
   as the chipnet pool today, sees no change.
3. **Paid set.** Otherwise rank the payees by weight, largest first, ties by
   address string. Walking the ranking, a payee is paid its `a_i` when
   `a_i >= d_i`, the paid set stays within `C - 1` outputs, and their bytes
   stay within `B - S`, where `S` is the largest output any payee takes. The
   reserved slot and `S` bytes are for the tail's output. Every other payee
   is in the tail.
4. **Tail draw.** The tail's value, `A` less the paid amounts (its pro-rata
   share plus every rounding remainder), goes to one tail payee drawn by
   weight. The tail is laid out in address-string order, each payee
   covering its weight, and the draw picks the payee covering
   `r mod W_tail`, where `r` is the first 16 bytes, little-endian, of
   `SHA256("P2Poolv2/bch-payout-tail-draw" || anchor)` and `anchor` is the
   paying share's `prev_share_blockhash` in its byte order.
5. **No dust.** If the tail's value is below the drawn payee's dust
   threshold, it goes to the first payee of the paid set (the largest)
   instead. If the paid set is empty, the drawn payee takes all of `A`.
6. **Order.** Outputs follow the donation and the fee, sorted by address
   string as in rule 7.

So a Bitcoin Cash coinbase has at most `C` payout outputs and `B` bytes of
them, sums to exactly the coinbase value, and is a pure function of the
weights, `A` and the anchor, which the producer and every validator share.

### Why a weighted draw

| Option | Verdict |
|---|---|
| Leave the tail out, as P2Pool v1 does past its 4,000 largest payees | Loses the tail's value. Rejected. |
| Spread the tail over the paid set, pro rata | Exact and simple, but every tail payee's expected income is zero: small miners pay large ones. A rich-list Sybil (upstream #647: a large miner splitting into enough addresses to fill the paid set) would capture the tail. Rejected. |
| Carry the tail to a later coinbase | Needs consensus state of what each address is owed, keyed on blocks the share chain cannot tell apart from BCH orphans; PPLNS already carries a share over about `K` blocks. Rejected. |
| Draw one tail payee by weight | Upstream's proposal (#708 and its 2026-10-15 meeting notes: sum the tail and send it to one miner chosen at random, weighted by accumulated weight). Every tail payee's expected income equals its pro-rata share and no value is lost. Adopted. |

The cost is variance for tail payees. Share trading (decision 13) lets a
small miner sell its shares to a holder in the paid set, and the covenant
payout tree (below) removes the draw. A Sybil that crowds others out of the
paid set only moves them into the tail, where their expected income is
unchanged.

### Why the anchor seeds the draw

- Every validator reads the anchor from the share header, and the producer
  holds it when it builds the job.
- Nobody knows it until the anchor share is mined, and the payout rights at
  an anchor are fixed by the anchor's own ancestry. So nobody can move a
  right to an address placed to cover a known draw point. A seed known in
  advance, such as the BCH previous block hash, would allow exactly that for
  the length of a BCH block.
- **Grinding.** A miner may build on an older anchor to get another draw.
  It gains only when its own share is a block, its stale shares lose
  share-chain income (an uncle earns 9/10, an orphan nothing), and each
  extra anchor adds one draw at its tail weight: with `k` anchors, a tail
  payee holding fraction `p` of the tail wins about `1 - (1 - p)^k` of its
  own blocks' tails instead of `p`. For a small miner that is second order
  (its blocks times its tail fraction).
- **Withholding.** A finder that loses the draw still gains by publishing:
  its block is also a share, which enters later windows.
- **Bias.** `r mod W_tail` with a 128-bit `r` is off by at most
  `W_tail / 2^128`; window weights stay far below `2^64`.

### Bytes

- `BCH_COINBASE_BYTE_BUDGET` = 784 bytes, the coinbase cited in #668. It is
  evidence from another pool, not a measurement of this one's devices.
- The largest coinbase this fork builds, other than its payouts, is 408
  bytes: 51 bytes of transaction fields (version, input count, outpoint,
  script length, sequence, output count and locktime), a 100-byte scriptSig
  (BCH's limit; a declared prefix is at most 88 bytes plus the 12-byte
  extranonce), the donation (34), a fee output of any address type (at most
  51, a 40-byte witness program), the merge-mining output (55), the padding
  output (at most 74) and the commitment output (43). That leaves 376 bytes,
  11 P2PKH outputs (374): hence `C` = 11 and `B` = 11 * 34, and the largest
  Bitcoin Cash coinbase is 782 bytes. A compile-time assertion keeps the sum
  within the budget.
- **Claimability.** A merge-mining (AuxPoW) claim by a token covenant
  carries the whole BCH coinbase, the 80-byte header and the coinbase's
  merkle branch (at most 25 hashes, `MAX_COINBASE_MERKLE_BRANCH_LENGTH`) in
  its unlocking bytecode. With the budget that is at most
  784 + 80 + 800 = 1,664 bytes, far under the 2026 P2S standard of about
  10,000 bytes, which keeps the claim's hashing cost low too.
- **Output positions.** The bound counts the merge-mining output by size,
  not position. Where it sits is decided in one place
  (`place_merge_mining_output` in `stratum/work/coinbase.rs`), today after
  the payouts; the merge-mining format may move it to output 0 without
  touching this rule. The padding and share-commitment outputs stay last,
  the commitment output the very last.
- `B` is part of the rule, not only `C`, because a share header may name any
  address type: a witness-program script decodes as an address and takes up
  to 51 bytes. Honest Bitcoin Cash nodes produce only CashAddr P2PKH and
  P2SH addresses, so in practice `C` binds.
- The job builder (the Stratum V1 notify and the Job Declaration
  preparation) measures each coinbase it hands out and refuses one over the
  budget. Under the rule that can only happen if BCHN's `coinbaseaux` flags
  push the scriptSig past BCH's 100 bytes, a block BCH rejects anyway.
- The validator rebuilds the coinbase exactly, so a share paying more
  outputs than the rule allows is invalid. It does not check the byte
  budget itself: beyond the payouts the bytes are the producer's own
  scriptSig, which only its own miners hash.

### What does not change

- The donation (1.5% by default, to the fixed key), the operator fee and
  the 10% cap on the two come off before the split, as before, and their
  outputs do not count towards `C`.
- CTOR orders a block's transactions. The coinbase stays first and its
  output order is this rule's, so CTOR is untouched; share transactions are
  not BCH transactions.
- Payout holders (rule 6): the ranking is by each address's total weight
  after the holder rule, so buying rights can lift a holder into the paid
  set, and a holder's tail weight is the sum of its rights in the tail.
  Uncles keep their own address.
- Bitcoin pays every payee, upstream's rule. Hydrapool (`simple_pplns`) is
  unchanged.
- The API's `/miner` view reports each address's weight, which is the
  expected value of its payout; a tail payee is paid by the draw.

### The covenant payout tree (M2) replaces the draw

BCH's introspection opcodes can already check a spending transaction's
outputs, so a keyless covenant tree can commit to many payouts in one
P2SH32 output (`bch-plan.md` decision 4). The tail's output would become
the root of such a tree: each node, spent by anyone, must create its two
children (or pay a leaf to its address), so every tail payee withdraws
exactly its `a_i` and the draw goes away. The paid set, `C` and `B` stay as
they are. Withdrawals then pay BCH fees and a leaf below dust needs
batching, which is why it is a separate design (gap analysis M2) and not
built here.

### Activation

A consensus change: a node on the old rule builds another coinbase whenever
a window has more than 11 payees, more than 374 bytes of payouts or a payee
below dust. As for decisions 11 to 13 it ships as a flag day on a fresh
store, and Bitcoin Cash protocol strings carry `payout-cap11` after
`payout-holder` (`bch_payout_cap_rule_tag`, built from
`BCH_MAX_COINBASE_PAYOUTS`, so changing the cap changes the tag). Mainnet
starts with the rule.

### Still to measure

The 784-byte budget needs confirming on a real device: mine on chipnet with
stock Antminer firmware (S19 and S21 class), over Stratum V1 directly and
behind an SV2 translator, with coinbases from about 600 to 1,200 bytes, and
record where jobs start to fail. Raising `C` and the budget together is a
consensus change with its own tag.

## Bitcoin Cash payout rights

The full rule and its reasoning are in
[bch-share-trading.md](../atomic-swap/bch-share-trading.md#payout-rights).
In short:

- A share's payout right starts on its coinbase output, held by its own
  `miner_address`. A share transaction spending an output carrying a right
  through input `i` passes the right to its output `i`; with no output `i`
  the right stays with its holder for good. A P2PKH output's key hash
  becomes the holder; a P2SH32 output (the trade contract) keeps the holder
  it had, so a seller holds a locked share until the claim or the refund.
- The holder at the paying share counts every share transaction on the
  anchor's own chain up to and including the anchor. Uncles' transactions
  never count, and an uncle's own weight never moves.
- `PplnsWindow` keeps the index (`payout_rights.rs`) beside its confirmed
  entries: moved rights by share, by outpoint, and each cached share's
  coinbase txid, all evicted with their share. `update` applies newly
  confirmed shares in height order and undoes reorged-out ones from per-share
  undo records kept for up to the newest `RIGHTS_UNDO_DEPTH` (1,210) shares
  and `RIGHTS_UNDO_MAX_CHANGES` (100,000) changed entries; a deeper reorg
  reloads. A walk from an anchor below the cached tip undoes the cached
  shares above its fork point in an overlay, then applies its branch shares;
  an anchor forking deeper than the undo log is `RightsHistoryTooShallow`,
  dropped without a verdict like `InsufficientEntries`.
- Each output carries at most one right, so a share moves at most one right
  per input and the index never holds more than one entry per cached share.

## Depths

All sized from `PAYOUT_DEPTH = N + W` (127,008). If you change `N` or `W`,
these move with it. The Bitcoin Cash window is at most `W` shares, so it
needs no more depth than the fixed window.

| Quantity | Value | Where |
|---|---|---|
| Window cache | `N + W + W/100` | `PplnsWindow::cache_capacity` |
| Full-validation zone | `PAYOUT_DEPTH` | `is_in_pplns_zone` |
| Output expiry | `PAYOUT_DEPTH` | `validate_prevouts` (`min_coinbase_root_height`) |
| Body retention | `2 * PAYOUT_DEPTH` | `PRUNE_DEPTH` |

See `pruning.md` for the two-window retention model.

## Code map

| What | Where |
|---|---|
| Constants, weights | `accounting/payout/sharechain_pplns/pplns_window.rs` |
| Anchor walk | `PplnsWindow::resolve_start_hash`, `collect_candidate_headers` |
| Paying height | `PplnsWindow::paying_height` |
| Maturity skip | `PplnsWindow::immature_share_count` |
| Window rule | `PplnsWindow::for_chain`, `resolve_window` (`work_target`), `BCH_PPLNS_WINDOW_BLOCKS`, `MIN_SIZING_BLOCK_DIFFICULTY` |
| Bits to `D` | `bitcoin_block_difficulty` |
| Peer isolation by rule | `share_chain_rule_tag`, `protocol_string` (`node/behaviour/request_response.rs`) |
| Window walk and weights | `walk_window_entries` (with `ConfirmedEntry::work` for the BCH bound), `for_each_credit` (the only place the weights are applied) |
| BCH payee of a share's own weight | `PplnsWindow::transferred_payees`, `ResolvedWindow::share_payee`; the index in `sharechain_pplns/payout_rights.rs` (`PayoutRights`, `RightsOverlay`) |
| Entry point | `PplnsWindow::get_distribution_from_start_hash` |
| Every miner's credits (API `/miner`, one walk per anchor) | `PplnsWindow::get_window_credits_from_start_hash`, same walk and weights; on BCH the API sizes it from the anchor share's own header |
| Producer | `Payout::fill_distribution_from_shares` (`sharechain_pplns/payout.rs`), from `build_output_distribution` (`stratum/work/notify.rs`); the window is built by `build_payout_for_mode` (`accounting/payout/mod.rs`) |
| Cuts and split | `include_address_and_cut`, `append_proportional_distribution` (`accounting/payout/payout_distribution.rs`) |
| BCH coinbase budget | `PayoutSplit`, `append_payout_split`, `BCH_MAX_COINBASE_PAYOUTS`, `BCH_MAX_PAYOUT_OUTPUT_BYTES`, `bch_payout_cap_rule_tag` (`accounting/payout/coinbase_budget.rs`); `BCH_COINBASE_BYTE_BUDGET`, `check_bch_coinbase_budget`, `place_merge_mining_output` (`stratum/work/coinbase.rs`) |
| Validator | `validate_bitcoin_payout` (`shares/validation/mod.rs`) |

## Tests that pin the rules

Use `PplnsWindow::new_with_window_sizes(network, maturity_skip, window)` to
test with small values (for example 3 and 2).

| Rule | Test (`pplns_window.rs` unless noted) |
|---|---|
| Skip, first paid share at depth `N` | `test_distribution_skips_immature_shares_from_anchor` |
| Skip spans candidate and confirmed | `test_distribution_skip_spans_candidate_and_confirmed_entries` |
| No skip while `p < N` | `test_distribution_does_not_skip_before_maturity_height` |
| Early shares stay mature | `test_distribution_keeps_early_shares_mature_after_bootstrap` |
| Full skip at `p = 2N - 1` | `test_distribution_skips_full_maturity_after_early_shares_age_out` |
| Truncated walk refused | `test_distribution_refuses_when_skip_and_window_exceed_cache` |
| Miner credits equal the distribution | `test_window_credits_match_the_distribution_for_every_payout_address` |
| BCH: large pool pays `K` blocks of work | `test_bch_window_large_pool_pays_three_blocks_of_work` |
| BCH: the share reaching `K * D` is paid | `test_bch_window_includes_the_share_that_reaches_the_work_target` |
| BCH: small pool capped at `W` | `test_bch_window_small_pool_is_capped_at_window_shares` |
| BCH: minimum-difficulty block pays `W` | `test_bch_window_minimum_difficulty_block_pays_the_capped_window`, `test_bch_window_zero_difficulty_block_pays_the_capped_window`, `test_chipnet_minimum_difficulty_bits_have_difficulty_one` |
| BCH: uncles count towards `K * D` | `test_bch_window_counts_uncle_work_towards_the_target` |
| BCH: truncation | `test_bch_window_refuses_when_work_target_runs_past_the_cache`, `test_bch_window_reached_inside_the_cache_is_paid` |
| Bitcoin keeps `W` | `test_bitcoin_window_ignores_paying_block_difficulty` |
| Producer and validator agree | `test_bch_notifier_and_validator_build_identical_outputs`, `test_bch_validator_with_other_header_bits_builds_other_outputs` (`sharechain_pplns/payout.rs`) |
| Output expiry | `test_validate_prevouts_expires_coinbase_roots_below_payout_depth` (`shares/validation/mod.rs`) |
| BCH: a claim pays the buyer, a refund the seller, a reorg undoes a claim, the holder depends on the anchor only, Bitcoin unchanged | `p2poolv2_tests/src/share_payout_rights_test.rs` |
| BCH: rights move input `i` to output `i`, freeze, keep the holder through P2SH32, undo, evict, overlay | `payout_rights.rs` tests |
| BCH budget: fits, paid set, tail draw, dust, bytes, determinism | `coinbase_budget.rs` tests |
| BCH budget: producer and validator agree past the cap | `test_bch_notifier_and_validator_agree_past_the_coinbase_budget` (`sharechain_pplns/payout.rs`) |
| BCH budget: largest coinbase fits 784 bytes, a 30-payee one is refused | `test_the_largest_bch_coinbase_fits_the_byte_budget`, `test_a_bch_coinbase_past_the_byte_budget_is_refused` (`stratum/work/coinbase.rs`) |
| Fixture coinbases match | `test_fixture_coinbase_reconstruction_matches_bitcoin_merkle_root` (`shares/share_block/mod.rs`, mirrors the walk by hand) |

The offline `pplns_compare` binary (`p2poolv2_sim`, `pplns-compare` feature) pays one
drawn share chain per pool size with both window rules through the real
`PplnsWindow` and prints window length, payout delay and income variance; see
`docs/simulation/overview.md`.

End to end, the sim `late-miner` scenario checks the maturity rule against real
regtest coinbases (`cargo sim maturity-check`, RUNBOOK Test G). The
sim build compiles shorter depths: `COINBASE_MATURITY` 20 and
`PPLNS_WINDOW_SHARES` 80 under `cfg(feature = "sim")`, so a short run reaches
the maturity skip. Production builds use 6,048 and 120,960. Bitcoin Cash's
`BCH_COINBASE_MATURITY` (100) is the same in both builds.

## Example (N = 6,048)

| `p` | skip | newest paid height |
|---|---|---|
| 5,000 | 0 | 4,999 |
| 9,000 | 2,952 | 6,047 |
| 12,096 | 6,047 | 6,048 |
| 20,000 | 6,047 | 13,952 |

Between `p` = 6,048 and 12,095 the newest paid share stays at 6,047 while
newer shares mature.

## Related

- `mine-on-bitcoin-chain.md`: a window share whose bitcoin prev is not
  eligible keeps its slot and is weighted zero.
- `share-processing-pipeline.md`: where payout validation sits in the
  pipeline.
