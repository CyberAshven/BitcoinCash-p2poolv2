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

1. **Outputs:** on BCH each window share's own donation and fee divide
   only its weighted work. The donation recipient, that share's fee
   recipient and its payee are ordinary payees, aggregated by locking
   bytecode before the hybrid split. Bitcoin retains the finding share's
   donation and fee cuts on the whole coinbase before the window split.
2. **Anchor:** the window is walked from the paying share's
   `prev_share_blockhash`, never from a node's tip.
3. **Maturity:** a share at height `h` is paid only if `h < N` or
   `p - h >= N`. The walk skips
   `min(N - 1, p.saturating_sub(N))` shares from the anchor, then takes the
   window.
4. **Window:** on Bitcoin, `W` shares. On Bitcoin Cash, the shares up to and
   including the first one that brings the window's work to
   `K * D * work_at_pow_limit`, at most `W` shares. A share contributes its
   exact work plus its uncles' exact work. `D` remains whole-number header
   difficulty; the threshold is converted to the same work unit. If
   `D <= 1` (the header is at the network's proof-of-work limit) the window
   is `W` shares. See "Bitcoin Cash window" below.
5. **Weights:** share work x 10 on BCH, difficulty x 10 on Bitcoin. BCH
   work is `floor(2^256 / (target + 1))`, the expected hash count used for
   chainwork, rather than difficulty rounded down to a whole number.
   Each uncle of a paid share: the uncle's miner gets 9/10 of its weight
   and the nephew's miner 1/10.
   Uncles use no window slots.
6. **Payee:** on Bitcoin, each share's and uncle's `miner_bitcoin_address`.
   On Bitcoin Cash a share's own weight, nephew bonus included, goes to
   whoever holds the share's payout right at the anchor: its
   `miner_bitcoin_address` while the holder is the share's own
   `miner_address` key hash, else the P2PKH address of the holder's key hash.
   An uncle's weight always goes to the uncle's `miner_bitcoin_address`. See
   "Bitcoin Cash payout rights" below.
7. **Split:** `amount * weight / total`, truncated. Bitcoin sorts addresses
   by string and gives the last address the remainder. BCH sorts by
   ascending weight, ties by locking bytecode, with address text as a final
   tie-breaker, and gives the last payee (the largest) the remainder.
8. **Hybrid payout (Bitcoin Cash):** every payee is direct while its output
   fits the share's coinbase budget: 1,000,000 bytes for the exact empty
   merge-mining commitment, 3,072 bytes otherwise. The first payee that does
   not fit and every later payee go to a P2SH32 payout tree. Direct outputs
   below dust are allowed; tree leaves are checked after fees at 3 sat/byte
   and merged into the largest tree payee when needed. See below.
9. **Payout levels (Bitcoin Cash):** between rules 7 and 8, every payee
   below the level of its newest credited entry, `546 * 2^k` satoshis, is
   pooled with the others at that level, and the pot is drawn among them,
   seeded from the anchor; the donation key and fee addresses are never
   drawn, and a payee that wins nothing gets no output
   ([Coinbase payouts](../payouts/coinbase.md#payout-levels)).

### Bitcoin Cash per-share cuts

For a share or uncle with gross credited weight `w`, donation fraction `d`
and fee fraction `f`, the weights are:

- `d * w` to the built-in P2Pool donation key;
- `f * (1 - d) * w` to that share's own fee address;
- `(1 - f) * (1 - d) * w` to its payee.

The fee follows the donation. Each cut keeps its separate 10% cap. A
nephew bonus uses the nephew's cuts; an uncle's 9/10 credit uses the
uncle's cuts. Selling a payout right moves only the remaining payee credit;
the share's donation and fee terms stay those it originally committed.
The finding share's settings affect its own later window credit only.

Why (as P2Pool v1 did it): with the finding share's donation and fee taken
off the whole coinbase, a miner choosing a 10% donation would make the whole
window donate 10% of that block, and a miner running its own node could set
a 10% fee to itself and take 10% of every block it found. Per share, each
miner's terms cost only that miner's own work.

With basis-point integers `D`, `F` and `B = 10,000`, the exact numerators
are `w * D * B`, `w * F * (B - D)` and
`w * (B - D) * (B - F)`, all over `B * B`. No per-share satoshi rounding
occurs: first sum these integer weights by locking bytecode, including
scripts shared by a miner, donation or fee recipient. Then split the whole
coinbase by the resulting weights. Round each amount down and add every
remaining satoshi to the largest payee in canonical order. Donation and
fee payees follow the same direct/tree and dust rules as any miner.

The API's share and uncle credit listing reports gross earned work before
these cuts. The exact payout weight snapshot used for job preparation
includes the cuts. A share-chain rule: its protocol tag segment is
`cuts-per-share` (`BCH_PER_SHARE_CUTS_RULE_TAG`), set into the rule batch's
tag with the others.

## Invariants

- **Deterministic.** The distribution is a function of the chain below
  the anchor and, on Bitcoin Cash, the paying share's own header and the
  share transactions in the anchor's ancestry only. No config, no difficulty
  from a node's own bitcoin node, no local tip, and no validation status.
  The producer and every validator must build identical outputs, so any
  change to the walk, the window, the weights or the payee is a share-chain
  rule change.
- **One code path.** The notifier (`Payout::fill_distribution_from_shares`)
  and the validator (`validate_bitcoin_payout`) call the same
  `PplnsWindow::get_distribution_from_start_hash` on the same shared window,
  and both turn bits into `D` with `bitcoin_block_difficulty`. The notifier
  reads the bits from its template. They are the bits the share's header
  carries, because a stratum miner cannot change them and a declared job must
  keep them (`validate_custom_job` checks `nbits`). The validator reads them
  from the header. Both then split the weights with `build_pplns_outputs`'s
  `append_payout_split` under `PayoutSplit::for_chain` of the same anchor,
  so the Bitcoin Cash hybrid payout (rule 8) is applied identically. Both
  paths also rebuild the payout tree committed by its root output.
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
  "Uncle selection vs uncle acceptance"). This is a share-chain rule on
  both chains.
- **A truncated walk is refused.** If the cache runs out before `W` shares
  and the cache does not reach height 0, return `InsufficientEntries`.
  Validation maps it to `Unresolvable`.

## Bitcoin Cash window

Bitcoin Cash sets `D` every block with ASERT, so `K * D` is `K` blocks' worth
of work for the pool. The window therefore spans about `K` of the pool's own
blocks at any pool size: short for a large pool, long for a small one, and
shorter by itself when BCH blocks get faster. At 10-second shares and the
one-minute blocks of CHIP-2025-03 (mainnet from 2027-05-15) the window is
about `K * 6 / f` shares for a pool holding fraction `f` of BCH's hashrate,
so pools under about `K / 20160` (0.015%) reach the cap and pay exactly
upstream's fixed window. With ten-minute blocks it is `K * 60 / f` shares,
ten times as long, and the cap binds under 0.15%.

| Pool share of BCH | Window (K = 3), one-minute blocks |
|---|---|
| 0.005% | capped at 120,960 shares, about 14 days |
| 0.1% | about 18,000 shares, 50 hours |
| 1% | about 1,800 shares, 5 hours |
| 10% | about 180 shares, 30 minutes |
| 30% | about 60 shares, 10 minutes |

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
  window holds at most one entry's work more than the target; v1 skipped
  one share and had a 0.5% finder bonus and a three-day cap at 60-second
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
- **Activation.** A share-chain rule: nodes on different rules rebuild
  different coinbases and fork the share chain, so a change ships as a flag
  day on a fresh store. Mainnet starts with the rule from its first share.
- **Peers on another rule cannot connect.** On Bitcoin Cash every protocol
  string (request-response, Identify, Kademlia and the Noise prologue)
  carries `pplns-k<K>-m<N>` (now `pplns-k3-m100`) after the genesis tag,
  built from `BCH_PPLNS_WINDOW_BLOCKS` and `BCH_COINBASE_MATURITY` by
  `share_chain_rule_tag` (`node/behaviour/request_response.rs`), so a node on
  another `K` or maturity fails the handshake instead of exchanging shares
  the other rejects. A segment for each later rule follows, among them
  `asert-parent` (share difficulty), `trade-htlc` (share transactions),
  `payout-holder` (payout rights), `payout-hybrid-t3072-f500-r1to3-z8-cov1`
  (rule 8, which replaced `payout-cap11`), `cuts-per-share`, `uncle-once`,
  `body-1000000-204800`, `payee-p2pkh-p2sh20-p2sh32` and `weight-work`.
  Bitcoin's protocol strings are unchanged.

## Bitcoin Cash coinbase budget

Rule 8, the hybrid payout (plan decision 14), is on its home page,
[Hybrid payouts on Bitcoin Cash](bch-hybrid-payouts.md): the coinbase
limit (1,000,000 bytes for the exact empty merge-mining commitment, 3,072
for a token share, 367 bytes reserved beside the PPLNS outputs), the
smallest-first order, the straddle rule, dust, the coinbase layout, and the
P2SH32 covenant payout tree with its fees, executor and API. Donation and
fee are PPLNS payees (rule 1), so they need no reserve; payout, donation and
fee scripts must be P2PKH, P2SH20 or P2SH32. How SV1, SV2 and Job
Declaration carry a large coinbase is on [Stratum](../stratum.md).

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
  entries, with undo records for reorgs and an overlay (`RightsOverlay`) for
  an anchor below the cached tip
  ([how a node looks a holder up](../atomic-swap/bch-share-trading.md#how-a-node-looks-a-holder-up)).
  An anchor forking deeper than the undo log is `RightsHistoryTooShallow`,
  dropped without a verdict like `InsufficientEntries`.
- Each output carries at most one right, so a share moves at most one right
  per input and the index never holds more than one entry per cached share.

## Depths

All sized from each chain's payout depth (`ChainDepths::for_chain`): on
Bitcoin `PAYOUT_DEPTH = N + W` (127,008), on Bitcoin Cash `BCH_PAYOUT_DEPTH =
100 + W + MAX_UNCLES_DEPTH` (121,063), from its own 100-share maturity. If you
change `N`, the Bitcoin Cash maturity or `W`, these move with it. The Bitcoin
Cash window is at most `W` shares, so it needs no more depth than the fixed
window.

| Quantity | Value | Where |
|---|---|---|
| Window cache | `N + W + W/100` (`N` the chain's maturity) | `PplnsWindow::cache_capacity` |
| Full-validation zone | the payout depth | `is_within_payout_depth` |
| Output expiry | the payout depth | `validate_prevouts` (`min_coinbase_root_height`) |
| Reorg depth limit | the payout depth | `ChainDepths::max_reorg_depth` |
| Body retention | 2 x the payout depth | `ChainDepths::prune_depth` (`PRUNE_DEPTH`, `BCH_PRUNE_DEPTH`) |

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
| Entry point | `PplnsWindow::get_distribution_from_start_hash`; with payout levels, `get_weights_from_start_hash` |
| Payout levels and the draw | `PayeeLevels`, `collect_distribution` (`sharechain_pplns/pplns_window.rs`); `PplnsWeights`, `draw_group`, `draw_seed` (`accounting/payout/payout_levels.rs`); `draw_payout_levels` (`accounting/payout/coinbase_budget.rs`) |
| Every miner's credits (API `/miner`, one walk per anchor) | `PplnsWindow::get_window_credits_from_start_hash`, same walk and weights; on BCH the API sizes it from the anchor share's own header |
| Producer | `Payout::fill_distribution_from_shares` (`sharechain_pplns/payout.rs`), from `build_output_distribution` (`stratum/work/notify.rs`); the window is built by `build_payout_for_mode` (`accounting/payout/mod.rs`) |
| Cuts and split | `ShareCuts`, `distribution_for_resolved` (`sharechain_pplns/pplns_window.rs`); Bitcoin/Hydrapool cuts in `include_address_and_cut` (`accounting/payout/payout_distribution.rs`) |
| BCH hybrid payout | `PayoutSplit`, `append_payout_split`, `direct_payee_count`, `bch_payout_rule_tag` (`accounting/payout/coinbase_budget.rs`); `check_bch_coinbase_limit`, `place_merge_mining_output` (`stratum/work/coinbase.rs`) |
| Tree building and covenant | `accounting/payout/payout_tree.rs`, `shares/bch_payout_covenant.rs` |
| Tree storage, execution and API | `store/payout_tree.rs`, `node/payout_tree_executor.rs`, `p2poolv2_api/src/api/endpoints/payout_trees.rs` |
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
| BCH exact-work share and uncle weights | `test_bch_window_weighs_shares_by_exact_work` and adjacent tests (`pplns_window.rs`) |
| BCH per-share donation and fee, kept through a sold right and on uncles; cut payees summed by script; producer and validator agree | `test_bch_transferred_share_and_uncle_keep_their_own_cuts`, `test_bch_cut_payees_aggregate_by_script` (`pplns_window.rs`), `test_bch_per_share_cuts_producer_and_validator_agree` (`sharechain_pplns/payout.rs`), `prepared_notify/donation_tests.rs` |
| BCH hybrid: both budgets, exact straddler, canonical order, dust, bytes, determinism | `coinbase_budget/tests.rs` |
| BCH hybrid: producer and validator agree beyond the former cap | `test_bch_notifier_and_validator_agree_paying_twenty_miners_directly` (`sharechain_pplns/payout.rs`) |
| Tree shapes, fee variants, dust, covenant execution | `payout_tree/tests.rs`, `bch_payout_covenant/tests.rs` |
| Maturity, rebroadcast and settlement; a refused child keeps a tree open; unknown blocks expire, never during initial block download | `node/payout_tree_executor/tests.rs`, `store/payout_tree.rs` |
| A found block's tree is kept before submission, over SV1, SV2 and declared jobs, and the block goes out even if the store fails | `stratum/message_handlers/submit/handle_submit_tests.rs`, `stratum/sv2/mining/tests.rs`, `stratum/sv2/custom_job/tests.rs` |
| BCHN relay and end-to-end payout execution | `p2poolv2_tests/src/payout_tree_bchn_test.rs` |
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
