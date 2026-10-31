# Bitcoin assumptions in the Bitcoin Cash port

Baseline: audit of `alpha` at `71231a42` (2026-10-23), finished on PR #33.
The research behind it is six sets of Bitcoin Cash facts, each checked
against BCHN source, BCHN documentation, the upgrade specifications and the
CHIPs, and eight sweeps of the code that produced 106 raw findings. This page
merges the duplicates, records what was checked again by hand, applies the
owner's decisions, and proposes the order of work.

"Share-chain rule" means a rule of P2Pool's own share chain, which BCHN never
sees. "BCH consensus" means the rules BCHN enforces on Bitcoin Cash blocks,
which this project never changes.

## Status update: 2026-10-30

Checked against the code at `7ebc0892` on `feat/rust-only-tooling`.
This table is the current status. The original audit below remains the
baseline, including its old findings and proposed work. "Fixed" means the
old code problem is gone. It does not mean hardware or large-block tests
were run. M1-M9 remain measurements to make; this update does not verify
them.

In the evidence column, `L/` means `p2poolv2_lib/src/` and `C/` means
`p2poolv2_config/src/`. Line numbers refer to the checked commit.

| Item | Status | Evidence and remaining work |
|---|---|---|
| A1: 11 payouts, 784 bytes and lottery | Fixed | `L/accounting/payout/coinbase_budget.rs:50-122, 267-299` selects a 1,000,000-byte no-token budget or a 3,072-byte token budget and sorts payees for the hybrid split. `L/stratum/work/coinbase.rs:100-131` checks that budget. Device limits remain unmeasured. |
| A2: 200 KiB combined body and 1 MiB frame | Fixed | `L/shares/validation/mod.rs:208-228` allows a 1,204,800-byte BCH body: a 1,000,000-byte coinbase plus 200 KiB of share transactions. `L/node/messages.rs:17-30` sets a 1,280 KiB message limit. Propagation at that size remains unmeasured. |
| A3: Hydrapool's 784-byte shutdown | Fixed | `L/accounting/payout/simple_pplns/payout.rs:45-57` pays every payee without the share-chain hybrid split. The old 784-byte check is gone. A8 is a separate shutdown path. |
| A4: Fixed Template Provider block limit | Fixed | `L/stratum/sv2/template_distribution.rs:428-436, 1004` uses BCHN's reported next block size limit, with a floor only as fallback. Live behavior after ABLA growth remains unverified. |
| A5: 32-bit size and SigChecks fields | Fixed | `L/stratum/work/block_template.rs:51-57` uses `u64` for `sigoplimit` and `sizelimit`. |
| A6: Full templates and unused decoded cache | Partly fixed | The unused decoded cache is gone. `bitcoindrpc/src/lib.rs:400-421` still requests full GBT JSON. `L/stratum/work/gbt.rs:46-49` parses it again, and `L/stratum/work/block_template.rs:149-180` decodes and hashes every transaction. No light-template RPC path exists. Large-template cost remains unmeasured. |
| A7: SV2 large-block limits | Open | `L/stratum/sv2/session.rs:66, 566-647, 813-816` retains a 64 MiB declared-transaction budget, `u16` transaction positions and `u24` frame lengths. `L/stratum/sv2/template_distribution.rs:319-329, 370-407` retrieves full transaction lists. Extended coinbase parts are limited to 65,535 bytes at `L/stratum/sv2/mining.rs:96`. |
| A8: Stop on an oversized final block | Fixed (2026-10-30) | `L/stratum/work/block_template.rs` still checks the completed block against BCHN's limit, and the error now gives the sizes and tells the operator to lower `-blockmaxsize` or `-percentblockmaxsize`. `L/stratum/work/notify.rs` no longer stops: it withdraws the work, which closes SV1 and SV2 mining connections so no miner keeps hashing a stale job, and publishes the next template that fits with clean jobs. BCHN's template transactions are never trimmed. |
| A9: Provider coinbase reserve | Partly fixed (2026-10-30) | `L/stratum/sv2/template_distribution.rs` now reserves 0 BCH SigChecks in the legacy-named sigops field, since a coinbase executes no scripts. It still reserves 65,382 output bytes from a 65,535-byte coinbase field, which cannot carry the full BCH coinbase budget; widening SV2 fields is separate work. |
| A10: BCH payout scripts and CashAddr | Fixed (2026-10-30) | `C/cashaddr.rs` decodes P2PKH, P2SH20 and P2SH32, token-aware or not. Payouts are held as `PayoutAddress` (`p2poolv2_wallet/src/payout_address.rs`), a locking script and network class, from config and username through share headers, PPLNS weights and the coinbase to the API and CLI. Share header bytes are unchanged. |
| A11: Two-week window cap | Open | `L/accounting/payout/sharechain_pplns/pplns_window.rs:50-74` retains the 120,960-share cap beside the three-BCH-block work window. A different cap still needs measurement and a share-chain rule decision. |
| A12: Bitcoin-derived depths | Fixed | `L/accounting/payout/sharechain_pplns/pplns_window.rs:128-191` derives BCH depths from 100-share maturity. `L/store/prune_blocks.rs:441-446` pins payout depth 121,063 and prune depth 242,126. A11 still makes those depths large. |
| A13: BIP141 share sigop budget | Fixed | `L/shares/validation/mod.rs:240-285` uses BCH SigChecks: at most 3,000 per transaction and 1,452 per share. `L/shares/bch_script.rs:82-86` defines the per-transaction limit. |
| A14: 25-level coinbase branch | Fixed | `L/shares/coinbase_proof.rs:36-59` allows 32 levels for BCH shares. `L/node/p2p_message_handlers/mod.rs:38-45` sizes responses at 600 headers. This is a finite bound. Merge-mining claims keep their separate 25-level proof format. |
| A15: SAFA's 800-byte branch check | Fixed | `p2poolv2_safa/safa.cash:188-202` folds the branch without the old 800-byte check. `p2poolv2_safa/README.md:155-162` records a 7,173-byte coinbase allowance at the deepest branches. That is a recorded VM measurement, not a new measurement made for this update. |
| A16: Namecoin commitment layout | Fixed | `L/shares/merge_mining.rs:41-62` defines common format v2. `L/shares/auxpow.rs:5-17, 40-54` defines its salted slots and proof bounds. `L/shares/share_block/mod.rs:125-131` carries the pool-built token root fields. These are deliberate share-chain choices. |
| A17: Rounded difficulty weights | Fixed | `L/accounting/payout/sharechain_pplns/pplns_window.rs:824-827, 1217-1223` selects `ExactWork` for BCH and weights each header by `get_work()`. |
| A18: BCH witness commitment acceptance | Fixed | `L/shares/validation/mod.rs:827-835, 1595` rejects a witness commitment in a BCH coinbase. `L/stratum/work/block_template.rs:158-165` rejects witness transactions in BCH templates. |
| A19: Missing BCH test networks | Partly fixed | `C/lib.rs:217-295` includes scalenet, but no selectable testnet4 network. `L/shares/bch_asert.rs:126-133` skips the anchored target check on scalenet and regtest. Scalenet operation has not been verified here. Testnet4 remains open, not decided against. |
| A20: Base58 payout metric labels | Fixed | `L/stratum/work/tracker/parse_coinbase.rs:5, 39` uses the shared `payout_address_string` encoder with the BCH network. |
| A21: Bitcoin payout-rights fixture | Fixed | `p2poolv2_tests/src/share_payout_rights_test.rs:46-64` uses BCH regtest difficulty, P2PKH payouts and 312,500,000 satoshis. This update reads the test; it does not rerun it. |
| A22: Bitcoin-only simulation and tools | Partly fixed | `p2poolv2_tools/src/sim/swarm.rs:246` selects BCH, `sim/nightly.rs:584` requires BCHN, and `docker/bchn/Dockerfile:19` pins BCHN 29.2.0. `p2poolv2_sim/src/pplns_compare.rs:288-292` now compares the BCH work window with the fixed window. It remains an offline four-miner model. `p2poolv2_tools/src/jmeter.rs:201-205` deliberately keeps Bitcoin signet for CKPool. None of these source checks establishes large-block capacity. |
| A23: Bitcoin sample and default network | Fixed | `config.sample.toml:1, 38-46` is a BCH chipnet sample. `C/lib.rs:494-515` derives the effective Bitcoin library network from `bitcoin_cash_network`. The upstream Bitcoin sample remains separate. |
| A24: ECDSA-only share trades | Fixed | `L/shares/bch_trade.rs:18-28, 145-150` signs with BCH Schnorr and appends the FORKID sighash byte. The fixed signature size also sizes the fee. |
| A25: Confirmed-input-only share pool | Open | `L/shares/transaction_pool.rs:860-879` refuses unconfirmed parent transactions. This is a local share-transaction pool restriction, not a BCHN mempool rule. |
| A26: Miner keys depend on address spelling | Fixed | `L/stratum/validate_username.rs:113-118` derives `user_key` from the decoded payout script. Its tests at lines 297-327 cover alternate spellings. |
| A27: Bitcoin metric and API names | Partly fixed | `p2poolv2_api/src/api/server.rs:415-428` exposes `found_blocks_total` and `found_block_time_seconds`; `prometheus/grafana/dashboards-public/pool-public.json` uses those names. Worker dashboards use `payout_address`. JSON fields at `p2poolv2_api/src/api/endpoints/found_blocks.rs:18-22` retain `bitcoin_height` and `miner_bitcoin_address` for API compatibility. Renaming them is deferred, not a BCH arithmetic fix. |

The 100,000-byte standard transaction size is BCHN node policy. It is not
a BCH consensus rule or a P2Pool block limit. The BCH consensus transaction
maximum is 1,000,000 bytes. Knuth has no lower transaction-size limit.

## Summary

P2Poolv2 was written for Bitcoin, and this fork runs it on Bitcoin Cash. Some
limits in the code still come from Bitcoin, or from Bitcoin pools, rather
than from Bitcoin Cash. This audit lists them, says which ones matter, and
says what to do about each.

1. **The coinbase payout cap.** A Bitcoin Cash coinbase pays at most 11
   miners and draws one of the rest by lottery, because one Bitcoin pool's
   block suggested that old Antminer firmware takes about 784 bytes. That is
   a lower bound for one device, not a limit, and Bitcoin Cash allows a
   1,000,000-byte coinbase. The owner has decided that the cap goes: every
   miner is paid directly, and a payout tree is used only for shares that
   carry merge-mining token commitments past a threshold. That design is the
   separate proposal
   [Hybrid payouts on Bitcoin Cash](architecture/bch-hybrid-payouts.md).
2. **The next limit after the cap.** Every share carries the whole Bitcoin
   Cash coinbase inside a 200 KiB share body, a number taken from upstream.
   With every miner paid directly, that body limit stops the pool at about
   4,000 to 6,000 miners, well short of what Bitcoin Cash allows. It is a
   share-chain rule. The owner has directed that it allow a full
   1,000,000-byte coinbase, and the hybrid proposal sizes it so (its section
   8.1).
3. **A bug in Hydrapool mode.** On Bitcoin Cash, Hydrapool pays every miner,
   but the 784-byte check still applies, so the node stops itself once about
   15 miners are in its window. It goes away with the cap.
4. **Block size.** BCHN decides the block size: a 32 MB floor and no fixed
   maximum. The `getblocktemplate` path already takes BCHN's limit. The
   Stratum V2 Template Provider path used a fixed 32 MB floor, and the
   template's size fields were 32-bit. Both are fixed on PR #33.
5. **Template handling.** The node fetches and re-decodes the whole block
   template every 10 seconds and moves every transaction through Stratum V2
   messages sized for Bitcoin (16 MiB frames, 65,535 transactions). That
   costs nothing at today's Bitcoin Cash blocks of tens of kilobytes, but it
   grows with the block size. It needs measuring, and BCHN's
   `getblocktemplatelight`, before blocks grow.
6. **Smaller share-chain rules from Bitcoin.** A two-week window cap, depths
   sized from Bitcoin's maturity, Bitcoin's sigop counting, a merkle branch
   limit tied to BCHN's temporary 2 GB guard, payout addresses limited to
   what a Bitcoin library can hold, and a Namecoin layout for the
   merge-mining commitment. They are proposed below as one batch for the
   owner.
7. **Documentation.** Several pages still describe Bitcoin, or the code
   before 23 October. The plainly wrong ones are fixed on PR #33.

Nothing in this audit was measured on Bitcoin Cash mining devices or at
Bitcoin Cash block sizes. Byte counts are arithmetic from the code. Bitcoin
Cash facts come from BCHN source and documentation, linked in the table.
Third-party data (block explorers, OCEAN's gateway code) is marked as such.

## The owner's decisions

These override the findings.

- **BCHN is the manager.** P2Pool must not duplicate or tighten node limits.
  It governs only what it builds itself: the coinbase, the share chain, share
  transactions and merge-mining proofs.
- **Block size.** 32 MB floor, no fixed maximum. P2Pool takes the limit from
  BCHN, and its types hold any size (`u64`).
- **First-seen is respected.** No replace-by-fee anywhere.
- **The coinbase payout cap goes.** `BCH_MAX_COINBASE_PAYOUTS` (11) and
  `BCH_COINBASE_BYTE_BUDGET` (784) are replaced by a hybrid payout designed
  separately: every miner paid directly, and a payout tree only for shares
  carrying merge-mining token commitments past a threshold. The design is
  [Hybrid payouts on Bitcoin Cash](architecture/bch-hybrid-payouts.md), a
  proposal awaiting the owner's agreement. The owner has also directed that
  the share body allow a full 1,000,000-byte coinbase.
- **The 100 KB standard transaction size is BCHN's node policy**, not a
  BCH consensus rule and not a P2Pool limit. Knuth has no such limit below
  the 1,000,000-byte BCH consensus maximum (Knuth PR #728, 2026-10-27).
- **The simulation moves to BCHN separately.** Simulation findings are listed
  for that work and not acted on here.

## How the findings were checked

The 106 raw findings collapse to the 27 items in the table below. Every high
and medium finding was re-read in the code and against the fact sets. These
changed:

- **Witness-program payouts are burned, not stealable.** One sweep said a
  peer naming a SegWit-shaped payout puts value in an output anyone can take.
  On Bitcoin Cash the input script must be push-only and leave exactly one
  stack item (a BCH consensus rule since November 2018), so a bare
  `OP_0 <program>` output cannot be spent at all. A peer that names one burns
  only its own payout. Kept as part of A10, at low weight.
- **Template cost: high to medium.** The memory and bandwidth figures for
  full templates are estimates for 32 MB blocks. Today's blocks are tens of
  kilobytes, so the cost is real only as blocks grow. Kept under "Measure on
  BCH".
- **Share sigop budget: medium to low.** It counts the wrong thing, but under
  today's byte limits it can never be reached.
- **Bitcoin-sized depths: medium to low.** They keep about 6,000 shares more
  than Bitcoin Cash needs. That is harmless, though the output expiry is a
  share-chain rule.
- **Firmware evidence.** That the 784-byte cap may not even protect older
  S19s (because OCEAN uses a special job layout for them) is an inference
  from OCEAN's code, not a measurement. It is kept as measurement M1.
- **SAFA's claim bound** was 7.3 KB in the sweep's arithmetic for the v1
  covenant. The v2 covenant's tests measure 7,173 bytes of coinbase at the
  deepest branches (`p2poolv2_safa/README.md`).
- **Hydrapool shutdown** was confirmed by reading the code path, not by
  running it.

## Take away

S: changes share-chain rules. B: changes the Bitcoin Cash block P2Pool
builds. Risk is what happens if nothing is done.

| # | What | Where (alpha `71231a42`) | Bitcoin origin | Bitcoin Cash reality (source) | Bitcoin Cash replacement | S | B | Risk |
|---|---|---|---|---|---|---|---|---|
| A1 | Coinbase pays at most 11 PPLNS outputs; jobs over 784 bytes refused; the tail is paid by a weighted draw | `accounting/payout/coinbase_budget.rs:26-60, 122-276`; `stratum/work/coinbase.rs:54-131`; `stratum/work/prepared_notify.rs:463-466` | Upstream [#668](https://github.com/p2poolv2/p2poolv2/discussions/668): one OCEAN Bitcoin block, 16 payouts in 784 bytes, said to suit stock Antminer firmware. [#708](https://github.com/p2poolv2/p2poolv2/issues/708) plans 25 outputs and a random tail, relying on Lightning | A coinbase is bounded only by `MAX_TX_SIZE` = 1,000,000 bytes and a 2-100 byte scriptSig, room for about 29,000 P2PKH outputs ([consensus.h](https://gitlab.com/bitcoin-cash-node/bitcoin-cash-node/-/raw/master/src/consensus/consensus.h), [tx_check.cpp](https://gitlab.com/bitcoin-cash-node/bitcoin-cash-node/-/raw/master/src/consensus/tx_check.cpp)). A mined 784-byte coinbase is a lower bound. OCEAN's own gateway sets caps per device, from 755 B (older S19 stock firmware) to 16 KB, and none for Braiins ([datum_coinbaser.c](https://github.com/OCEAN-xyz/datum_gateway/blob/85a19fd48caed0d00e2d26045916d30f84b0668a/src/datum_coinbaser.c)). P2Pool v1 on BCH paid up to 4,000 payees and warned only near 226 for S9s ([data.py](https://github.com/jtoomim/p2pool/blob/master/p2pool/data.py)). Stratum V2 standard channels never send the coinbase | Owner's decision: remove. Pay every miner directly; a payout tree only for shares with merge-mining commitments past a threshold ([proposal](architecture/bch-hybrid-payouts.md)). Device limits are handled per connection after M1, never as a chain-wide cap. **Done on PR #33**: cap, draw and 784-byte check removed; a no-token share pays every payee within 1,000,000 bytes, a token share within T = 3,072 bytes, the rest through the payout tree (`payout-hybrid-t3072-f500-r1to3-z8-cov1`). Device limits still wait for M1 | yes | yes | High: small miners paid by lottery |
| A2 | The 200 KiB share body includes the carried Bitcoin Cash coinbase; 1 MiB P2P frame and buffers sized from it | `shares/validation/mod.rs:186-189, 1406-1415`; `node/messages.rs:17-27`; `node/p2p_message_handlers/receivers/block_receiver.rs:33-39`; `shares/transaction_pool.rs:56`; `shares/share_block/mod.rs:404` | Upstream constants (commits `19819f07`, `243c994e`) for a Bitcoin share chain with a small coinbase | With every miner paid directly the coinbase is about 408 + 34 bytes per P2PKH payee. 200 KiB holds about 4,080 payees beside 64 KiB of share transactions, about 6,000 without; BCH allows about 29,400. A 1,000,000-byte coinbase with 64 KiB of share transactions does not fit the 1 MiB frame. Every share relays and stores the whole coinbase: about 170 KB per 10-second share at 5,000 payees (arithmetic) | Budget the carried coinbase apart from share transactions, bounded by BCH's 1,000,000-byte `MAX_TX_SIZE`, or carry only the fields a peer cannot rebuild (scriptSig, aux commitment) and rebuild the payouts from the window, which validators already do (`validation/mod.rs:1194-1270`). Re-derive the frame and buffer sizes and put the limit in the protocol tag. The owner has directed room for a full 1,000,000-byte coinbase; the [hybrid proposal](architecture/bch-hybrid-payouts.md) (8.1) sizes it so. Measure M2 for the bandwidth cost. **Done on PR #33**: a coinbase of up to 1,000,000 bytes beside 200 KiB of share transactions (`BCH_SHARE_BODY_SIZE_LIMIT`, 1,204,800 bytes), a 1.25 MiB P2P message; M2 still open | yes | no | High once A1 lands |
| A3 | Hydrapool on BCH stops the node at about 15 payees | `stratum/work/notify.rs:141`; `prepared_notify.rs:463-466`; `accounting/payout/simple_pplns/payout.rs:48-50`; `notify.rs:237-245` | The 784-byte budget of A1, applied to every BCH job | Hydrapool pays every payee and is not bound by the share chain. Past 784 bytes the check fails, `start_notify` returns, and the node shuts down (read from the code, not run) | Goes with A1: the hybrid proposal removes `check_bch_coinbase_budget`. Until then do not run Hydrapool on BCH with more than about 10 miners. **Done on PR #33**: the check is gone | no | yes | High for Hydrapool operators |
| A4 | Template Provider templates checked against a fixed 32 MB / 2 MB floor | `stratum/sv2/template_distribution.rs:411-421, 488-521` | A fixed block size, true of Bitcoin; Template Distribution carries no limit | ABLA since 2024-05-15: 32 MB floor (2 MB on chipnet), no fixed maximum; BCHN's 2 GB guard is temporary ([ABLA CHIP](https://gitlab.com/0353F40E/ebaa/-/raw/main/README.md), [27.0.0 notes](https://docs.bitcoincashnode.org/doc/release-notes/release-notes-27.0.0/)). BCHN reports the next limit as `ablastate.nextblocksizelimit` in `getblockheader` ([doc](https://docs.bitcoincashnode.org/doc/json-rpc/getblockheader/), [blockchain.cpp](https://gitlab.com/bitcoin-cash-node/bitcoin-cash-node/-/raw/master/src/rpc/blockchain.cpp)) | Read the limit for the provider's parent from BCHN with the `getblockheader` call the node already makes for the height. Fall back to the last limit BCHN reported, and to the floor only while BCHN is unreachable. **Done on PR #33** | no | no | Medium: a provider pool stops getting work as BCH scales |
| A5 | Template `sizelimit` and `sigoplimit` are `u32` | `stratum/work/block_template.rs:51-52` | Bitcoin Core's 4 M weight fits 32 bits | Owner: no fixed maximum; `u32` stops at 4.29 GB, and the 2 GB guard is to go by May 2028 | `u64`, compared as `u64`. **Done on PR #33** | no | no | Low |
| A6 | Full `getblocktemplate` every 10 s, decoded several times, held per job for 15-30 minutes; a decoded copy is cached and never read | `p2poolv2_node/src/lib.rs:50`; `bitcoindrpc/src/lib.rs:259-306`; `block_template.rs:99-183`; `notify.rs:76-90, 119`; `stratum/sv2/session.rs:66-98`; `stratum/work/tracker/mod.rs:15` | Upstream and ckpool polling of Bitcoin Core, whose blocks stay under 4 M weight | A 32 MB template is about 64 MB of hex JSON (estimate). BCHN has `getblocktemplatelight`/`submitblocklight`, which return a job id and merkle branch and keep the transactions in BCHN ([doc](https://docs.bitcoincashnode.org/doc/getblocktemplatelight/)). Today's median block is about 27 KB (third-party explorer data) | Measure M3. Then a `getblocktemplatelight` mode for SV1 and GBT jobs, full templates only for Job Declaration proposals, and drop the unread cache (**dropped on PR #33**: the notifier no longer decodes and sorts every template transaction into it) | no | no | Medium, grows with ABLA |
| A7 | Stratum V2 paths keep Bitcoin-sized limits: 16 MiB frames, 65,535 transactions, every transaction requested, 64 MiB declared-transaction cap, 10 s proposal timeout | `template_distribution.rs:304-408`; `stratum/sv2/session.rs:56, 488-590`; `stratum/sv2/mining.rs:208-222`; `docs/bch-job-declaration.md:62-66` | The SV2 specification, sized for Bitcoin ([sv2-spec](https://github.com/stratum-mining/sv2-spec)) | A 32 MB block of small transactions exceeds both limits. `NewTemplate` already carries the merkle path, enough to mine. On BCH a wtxid is the txid, so declared ids match BCHN's mempool | Mine provider templates from the merkle path; fetch only unknown transactions, from the node's own BCHN; scale caps and timeouts from the size limit; document Coinbase-Only with the declarator's own BCHN as the large-block path | no | yes: limits the blocks P2Pool can build over SV2 | Medium |
| A8 | A block over the limit stops the node; the coinbase room BCHN leaves is not planned for | `block_template.rs:184-201`; `prepared_notify.rs:463-466`; `notify.rs:191-262`; `p2poolv2_node/src/lib.rs:358-361` | Upstream treats a failed template as fatal and relies on Bitcoin Core's coinbase weight reserve | BCHN reserves 1,000 bytes for the coinbase and clamps blocks to the limit less 1,000 ([miner.cpp](https://gitlab.com/bitcoin-cash-node/bitcoin-cash-node/-/raw/master/src/miner.cpp)). At `-percentblockmaxsize=100`, chipnet's default, a full mempool leaves about 1.9 KB. Enough for 784 bytes, not for A1's direct payouts | With A1: never stop the notifier on a size failure; keep the previous job and tell the operator to lower `-percentblockmaxsize` or `-blockmaxsize`. Trimming BCHN's template is the owner's call | no | yes | Medium after A1 |
| A9 | `CoinbaseOutputConstraints` declares 1,922 sigops and a 64 KiB size reserve | `template_distribution.rs:78-103, 579-590` | SV2's legacy sigop field; P2PKH outputs cost sigops on Bitcoin | Coinbase outputs cost 0 SigChecks ([SigChecks spec](https://upgradespecs.bitcoincashnode.org/2020-05-15-sigchecks/)). The size reserve should be the new payout's largest coinbase, which may exceed `SubmitSolution`'s 64 KiB (blocks then go over RPC, as now) | Declare 0; size the reserve from A1's maximum | no | yes: the provider's template | Low |
| A10 | Payouts are what `rust-bitcoin`'s `Address` can hold: no P2SH32, no token-aware CashAddr; witness programs decode from share headers; outputs ordered by Base58 text | `p2poolv2_config/src/cashaddr.rs:5-10, 197-221`; `p2poolv2_config/src/lib.rs:102-124`; `shares/share_block/mod.rs:117-162`; `stratum/validate_username.rs:73-105`; `coinbase_budget.rs:105-106, 198` | `rust-bitcoin` models Bitcoin address types | P2SH32 since 2023-05-15 ([upgrade](https://upgradespecs.bitcoincashnode.org/2023-05-15-upgrade/)); token-aware CashAddr types 2 and 3 lock with the same script as 0 and 1 ([CashTokens](https://github.com/cashtokens/cashtokens)); a bare witness program cannot be spent on BCH ([Nov 2018](https://upgradespecs.bitcoincashnode.org/2018-nov-upgrade/)), so it only burns its own payout | Now, no rule change: accept token-aware CashAddr and map it to the same script (**done on PR #33**). In the batch: a BCH payout script type (P2PKH, P2SH20, P2SH32) in share headers, other scripts rejected, outputs ordered by script bytes (**done on PR #33**: payee, donation and fee scripts must be P2PKH, P2SH20 or P2SH32, and payees are ordered by weight, ties by script bytes; the header's address type cannot yet carry a P2SH32 payee) | (b) yes | (b) yes | Medium |
| A11 | PPLNS window capped at 120,960 shares | `accounting/payout/sharechain_pplns/pplns_window.rs:35-74` | Upstream's fixed two-week window, Bitcoin's 2,016-block retarget period at 10 s shares | P2Pool v1 on BCH used `SPREAD` 3 with a 3-day cap ([bitcoincash.py](https://github.com/jtoomim/p2pool/blob/master/p2pool/networks/bitcoincash.py)). This fork's work-sized window hits the cap for any pool under about 0.15% of BCH hashrate, which is every early pool | A BCH cap of its own (`BCH_PPLNS_WINDOW_MAX_SHARES`) chosen after M5, for example 3 days (25,920 shares) | yes | yes: who is paid | Medium |
| A12 | Payout depth, output expiry, prune and reorg depths sized by Bitcoin's 6,048-share maturity | `pplns_window.rs:111-156, 188-201`; `store/prune_blocks.rs:48-90`; `validation/mod.rs:298, 1667`; `transaction_pool.rs:751, 895` | Upstream: 6,048 = 70% of a day of 10 s shares, matching Bitcoin's 100-block maturity time | The BCH path uses a 100-share maturity; its deepest payout is 121,063 shares, against 127,008 kept. Harmless over-retention, but the expiry is a share-chain rule | Derive BCH depths from BCH constants together with A11. **Done on PR #33**: payout depth 121,063 and prune depth 242,126 (`ChainDepths`, `depths-m100`), with A11's cap unchanged | yes: expiry | no | Low |
| A13 | Share transactions budgeted with Bitcoin's BIP141 sigop cost (80,000) | `validation/mod.rs:192-193, 793-832`; `transaction_pool.rs:60-62, 788-801, 937-946` | Bitcoin Core's `MAX_BLOCK_SIGOPS_COST` | SigChecks replaced sigops on 2020-05-15: at most 3,000 per transaction, outputs cost 0 ([spec](https://upgradespecs.bitcoincashnode.org/2020-05-15-sigchecks/)). The Bitcoin count charges P2PKH outputs and misses the P2SH32 contract's signature check. It cannot be reached under today's byte limits | Count SigChecks (one per accepted input, at most 3,000 per transaction), or drop the budget. **Done on PR #33**: SigChecks, 3,000 per transaction and 1,452 per share (`sigchecks3000-1452`) | yes | no | Low |
| A14 | Coinbase merkle branch limited to 25 levels | `shares/coinbase_proof.rs:36-46`; `share_block/mod.rs:602-605, 690-695`; `node/messages.rs:280-305`; `node/p2p_message_handlers/mod.rs:45` | Fixed depths of Bitcoin AuxPoW (Namecoin 30, upstream 16) | 25 levels cover BCHN's 2 GB guard at 65-byte transactions. The guard is temporary: the ABLA CHIP removes it on or before May 2028 ([CHIP](https://gitlab.com/0353F40E/ebaa/-/raw/main/README.md)). The owner allows no fixed maximum | 32 levels (2^32 transactions), with header batch sizes re-derived. **Done on PR #33** (`merkle32`; at most 600 headers a response); merge-mining proofs keep 25 | yes | no | Low now; before BCHN lifts 2 GB |
| A15 | The SAFA covenant hard-codes an 800-byte (25-level) branch limit | `tools/safa-auxpow/safa.cash:180`; `tools/safa-auxpow/lib.mjs:156-158` | Copies A14 | A covenant cannot change once tokens use it. The 10,000-byte unlocking limit and the 2025 VM cost limits already bound the work ([P2S](https://github.com/bitjson/bch-p2s), [VM limits](https://github.com/bitjson/bch-vm-limits)) | Drop the `<800>` check, keeping the multiple-of-32 check, or raise it to 1,024 before any issuance. **Done on PR #33**: the covenant moved to merge-mining v2 and has no byte limit of its own; its branches are the format's (`h ≤ 16`, `d ≤ 25`), bounded by the 10,000-byte unlocking limit | no | no | Medium: permanent once issued |
| A16 | Merge-mining commitment uses Namecoin's slot scheme, nonce and 2^30 tree, and proofs carry the whole parent coinbase | `shares/merge_mining.rs:5-71`; `shares/auxpow.rs:20-25, 48-220`; `tools/safa-auxpow/safa.cash:13-47, 167-177`; `coinbase.rs:229-244, 301-319` | Namecoin's merged-mining specification | This format is BCH-only (the commitment is an output, not scriptSig bytes), so Namecoin compatibility buys nothing, and its slot formula cannot separate colliding ids. A covenant must hash the whole coinbase in one unlocking script of at most 10,000 bytes, so a claimable parent coinbase is at most about 7.3 to 8.7 KB (sweep arithmetic) | A1's payout tree keeps merge-mining shares' coinbases under that bound, which sets the threshold. Before any covenant deployment: a dense sorted merkle tree with no nonce, or the aux commitment moved into the fixed tail after the midstate. **Done on PR #33** in P2Pool: merge-mining common format v2 agreed with Pickaxe (53-byte output 0, salted slots, tree height at most 16, AuxProof v2 with a split coinbase, ASERT header check, `mm2-asert`, since 2026-10-24 `mm2-roots-asert` with pool-built token roots in the share header, and `mm2-roots-asert8` since the test networks' easy side widened to 8 times), matching Pickaxe PR #70's vectors byte for byte. The SAFA tool (A15) moved to v2 on PR #33 too | yes | yes | Medium, before issuance |
| A17 | PPLNS weights use integer difficulty | `shares/share_block/mod.rs:192-198`; `pplns_window.rs:1037, 1447, 1575` | Upstream, where share difficulty is large | The BCH mini chain's ceiling is difficulty 4.55 (`0x1c384bd7`), which rounds to 4. Shares at difficulty 4 to 9 are weighted with errors of up to about 20% against each other | Weight by work (2^256 / (target + 1)), as chainwork does. **Done on PR #33** on Bitcoin Cash; Bitcoin keeps difficulty | yes | yes: amounts | Low |
| A18 | The BCH validator rebuilds a BIP141 witness commitment it finds in a block coinbase | `stratum/work/coinbase.rs:584-590`; `validation/mod.rs:1245-1261` | Upstream's Bitcoin coinbase shape | BCH has no SegWit, and BCHN templates carry no witness commitment ([getblocktemplate](https://docs.bitcoincashnode.org/doc/json-rpc/getblocktemplate/)). Honest builders never add one | Reject it on BCH. **Done on PR #33** (`no-segwit`) | yes | no | Low |
| A19 | No scalenet or testnet4 | `p2poolv2_config/src/lib.rs:196-265`; `p2poolv2_cli/src/commands/bch_share_address.rs:33-45`; `bch_trade.rs:56-64` | Bitcoin test practice: small-block signet and testnet | BCHN's scalenet (256 MB default) exists to test big-block applications ([test networks](https://gitlab.com/bitcoin-cash-node/bitcoin-cash-node/-/raw/master/doc/test-networks.md)) | Add scalenet, with its own share genesis, for M3 and M2 at scale. **Scalenet done on PR #33**: `bitcoin_cash_network = "scalenet"`, share addresses `sp2pool:`, a share genesis on BCHN's scalenet genesis block, a 256 MB template floor, and no ASERT header check (BCHN has no scalenet anchor). testnet4 is not added | no | no | Medium: nothing can be measured at scale |
| A20 | The `coinbase_output` metric labels BCH payouts with Base58 addresses | `stratum/work/tracker/parse_coinbase.rs:28-36`; `p2poolv2_api/src/api/server.rs:341-345` | Upstream's Bitcoin address display | Operators know their payouts as CashAddr | Render with `payout_address_string`. **Done on PR #33** | no | no | Low |
| A21 | A BCH payout test pays Bitcoin signet P2WPKH addresses, measures difficulty against signet's limit and splits a 50-coin block value | `p2poolv2_tests/src/share_payout_rights_test.rs:42-52`; `p2poolv2_lib/src/test_utils.rs` (`TestShareBlockBuilder` pays signet P2WPKH) | Upstream Bitcoin fixtures | BCH subsidy is 3.125 BCH since block 840,000 ([chainparams.cpp](https://gitlab.com/bitcoin-cash-node/bitcoin-cash-node/-/raw/master/src/chainparams.cpp)); a BCH node takes difficulty and Base58 data from its legacy network (mainnet, testnet or regtest), never signet; BCH has no SegWit | P2PKH payouts on BCH regtest's legacy network, at regtest's proof-of-work limit, splitting 312,500,000 sat. **Done on PR #33** | no | no | Low |
| A22 | The simulation, swarm, nightly and load tests ran on Bitcoin only; `pplns_compare` models 4 miners and no payout budget | `p2poolv2_lib/src/sim/**`; `p2poolv2_sim/src/pplns_compare.rs`; `p2poolv2_tools` sim and mock node | Upstream's Bitcoin Core regtest harness | The old harness did not exercise BCH paths under load | Owner: the simulation moves to BCHN separately; it should then cover A1, A2, A11 and M3. **Partly fixed, checked 2026-10-30**: the swarm selects BCH and its nightly requires BCHN; the Docker image pins BCHN 29.2.0. `pplns_compare` includes the BCH work-sized window beside the fixed window, but remains an offline four-miner model. The JMeter plan and configs deliberately remain Bitcoin signet for CKPool. Source inspection does not verify scenario results or large-block capacity | no | no | High for evidence, handled separately |
| A23 | `config.sample.toml` is a Bitcoin signet config, and a BCH config must still name a Bitcoin `network` | `config.sample.toml`; `p2poolv2_config/src/lib.rs:175-187, 1040-1046`; `README.adoc` install steps | Upstream's sample | A BCH operator starts from Bitcoin addresses | Ship a BCH sample (chipnet) as the default; derive `network` from `bitcoin_cash_network`. **Done on PR #33**: `config.sample.toml` is a chipnet node, upstream's sample is `config-bitcoin.sample.toml`, and a Bitcoin Cash config may leave `network` out | no | no | Low |
| A24 | `bch-trade` signs ECDSA only and sizes fees for 72-byte signatures | `shares/bch_trade.rs:17-18, 37-43, 117-144`; `p2poolv2_cli/src/commands/bch_trade.rs:216-258` | Bitcoin wallets' default | BCH accepts 65-byte Schnorr signatures, and the share chain verifies them | Sign with Schnorr. **Done on PR #33** (`shares/bch_schnorr.rs`): BCHN's algorithm and RFC6979 nonce, checked against BCHN's `key_tests.cpp` vectors; fees are sized for 65 bytes exactly | no | no | Low |
| A25 | The share pool refuses chains of unconfirmed share transactions | `shares/transaction_pool.rs:9-11, 133-134, 763-765, 881-883` | Bitcoin's mempool chain limits | BCH dropped its unconfirmed-chain limit on 2021-05-15 ([upgrade](https://upgradespecs.bitcoincashnode.org/2021-05-15-upgrade/)) | Allow in-pool parents, after measuring trade use | no | no | Low |
| A26 | Miner statistics are keyed by the address text as typed | `store/user.rs:11-52`; `stratum/validate_username.rs:75-102` | Bitcoin addresses have one spelling | CashAddr may omit its prefix and may be upper case, so one miner can appear twice in statistics (payouts are grouped correctly) | Key by the decoded script. **Done on PR #33**: miners are stored and counted under the canonical CashAddr of their decoded script (`user_key`), so prefix, case and token-aware spellings are one miner | no | no | Low |
| A27 | Metric and API names say `bitcoin` and `btc`; Grafana labels say `btcaddress` | `p2poolv2_api/src/api/server.rs:400-433`; `p2poolv2_api/README.md`; `prometheus/grafana/**` | Upstream names | Cosmetic, but renaming breaks dashboards | Rename in one change with the dashboards. **Metrics done on PR #33**: `found_blocks_total`, `found_block_time_seconds` and the `payout_address` label, with every dashboard in `prometheus/grafana/`. The JSON API keeps `miner_bitcoin_address`, `bitcoin_height` and the like: they are the share header's field names, read by the TUI, the WASM dashboard and outside frontends, so renaming them is a separate API change | no | no | Low |

## Measure on Bitcoin Cash

Each test runs on chipnet unless it needs block sizes chipnet cannot reach.

| # | Question | Chipnet test | Decides |
|---|---|---|---|
| M1 | How large a coinbase each mining device accepts | Run Hydrapool on chipnet with A3 fixed, and add CPU-miner connections with distinct CashAddr usernames until the coinbase reaches 1, 2, 4, 8, 16, 32 and 64 KB. At each size, point a stock S19, an S21, an Avalon, a Bitaxe and a Pickaxe at it, over SV1 and through the SV2 translator, for 30 minutes each. Record coinb1 and coinb2 lengths and whether valid shares come back | Per-connection device limits for A1 |
| M2 | What a large share body costs in relay | Three chipnet nodes in different places, connected over the internet. Add padding to the share coinbase in a test build so share bodies are 10, 50, 100 and 200 KB, and run each for 6 hours. Measure share propagation with `p2poolv2_tools share-latency` and the uncle rate from `/chain_info` | A2's body limit and whether to carry a compact coinbase |
| M3 | What full templates cost as blocks grow | Fill chipnet's mempool to past 2 MB with 1 sat/byte transactions from a test wallet (about 0.02 tBCH) and run BCHN at `-percentblockmaxsize=100`. Measure `getblocktemplate` time, node memory and notify latency, then repeat with `getblocktemplatelight`. Repeat on scalenet (A19) at 32 and 256 MB | A6, A7 |
| M4 | Whether the Template Provider path follows ABLA | Mine chipnet blocks of about 1.5 MB until `getblockheader <tip> true` shows `ablastate.nextblocksizelimit` above 2,000,000. Run the node with `template_source = "sv2"` against bchn-sv2-bridge and check that it logs BCHN's limit and accepts a template larger than 2,000,000 - 65,624 bytes | Confirms A4 on a live node |
| M5 | How often the window cap binds, and what a shorter cap does to small miners | Log the window length in shares for every block the chipnet pool finds for two weeks, with the paying difficulty. Compute the same for mainnet difficulty (508 G) at 0.01% to 1% of BCH hashrate, and compare payout variance at 3-day, 1-week and 2-week caps in the BCHN simulation | A11, A12 |
| M6 | Uncle rate at 10-second shares on BCH | Three chipnet nodes on different continents for 48 hours; count uncles against shares from the API | The share period (`bch-plan.md` decision 11) |
| M7 | Today's live block size limit | `bitcoin-cli getblockheader $(bitcoin-cli getbestblockhash) true` on a mainnet and a chipnet BCHN, reading `ablastate`. The fact sets estimate 32,000,000 bytes on mainnet but did not read a node | Confirms the floor figures in this page |
| M8 | Share-chain reorg depths | Count reorg depths in the chipnet nodes' logs for a week | `BCH_COINBASE_MATURITY` and the confirmations a share buyer waits for |
| M9 | Coinbase room at full blocks | After A1, fill chipnet's mempool past the limit with `-percentblockmaxsize=100` and a coinbase over 2 KB. The node must keep serving work and warn | A8 |

## Fix the docs

Done on PR #33:

- `docs/bch-design.md`: the window is three BCH blocks of work behind a
  100-share maturity, not upstream's 120,960 shares behind 6,048; the window
  follows faster blocks; the template check does count the coinbase, and a
  block over the limit stops the node; the coinbase is capped by the share
  chain today, with the cap being replaced; a declared job's aux choice is
  stored in the share header.
- `docs/known-attacks.md`: header sync checks timestamps; the pending buffer
  is capped; the share pool has a priority rule; `maximum_difficulty` has a
  default; a spend of an unfetched output no longer stalls a node; the
  coinbase section describes the cap, the evidence and its replacement.
- `docs/bch-plan.md`: the Template Provider limit now comes from BCHN.
- Code comments next to the fixes (Template Provider size limit, share dust
  floor units).

Still to do, after the payout redesign lands:

- `docs/small-miners.md`, `docs/requirements.md` (scalability row),
  `docs/bch-quickstart.md`, `docs/architecture/payout-distribution.md` and
  `docs/bch-plan.md` decision 14 describe either "every miner gets an output"
  or the 11-payout cap. Rewrite them once for the new payout.
  **Done on PR #33**: the payout pages describe the hybrid payout, and
  "every miner gets an output" is right again for shares that merge-mine no
  tokens.
- The 784-byte comment in `stratum/work/coinbase.rs` presents the OCEAN block
  as a firmware limit and leaves the aux branch and redeem script out of the
  claim arithmetic. It goes with A1. **Done on PR #33**: it went with the
  byte budget.
- `docs/architecture/pruning.md`, `async-flow.md` and `store-schema.md` size
  storage with Bitcoin figures (P2TR addresses, about 2,000 Bitcoin txids per
  share, 1 MB blocks). Recompute with A2.
- Several pages need explicit "share-chain rule" wording
  (`docs/bch-design.md:76-83`, `docs/difficulty_adjustment/asert.md:128-140`,
  `docs/bch-plan.md`). Say "share-chain rule".
- Comments: `BIP54_COINBASE_SEQUENCE` and the share coinbase's "witness root"
  and "1 BTC" wording (`shares/transactions/coinbase.rs:13, 118-134`) are
  Bitcoin names for share-chain choices; `node/messages.rs:20-22` cites
  Bitcoin's block relay limit; `chain_store_handle.rs:36-38`,
  `pplns_window.rs:219-221` and `pool_difficulty.rs:33-36` carry stale
  upstream remarks.
  **Checked 2026-10-30:** `node/messages.rs:17-30` now explains the BCH
  share size and message budget. It no longer derives the budget from
  Bitcoin's block relay limit.
- `p2poolv2_cli/README.md` documents the Bitcoin taproot address flow and not
  `bch-share-address`.

## Fine as is

Bitcoin-mode only, kept for upstream:

- `PayoutSplit::EveryPayee`, the 16-level Bitcoin proof limit, the taproot
  share owner, the BIP141 share commitment and witness commitment, the
  `segwit`/`signet` template rules and `libbitcoinconsensus`, all behind
  `Chain::Bitcoin`.
- The bech32m share address, `witness_program_codec.rs` and the CLI `address`
  command.
- Bitcoin's 6,048-share maturity and fixed 120,960-share window on
  `Chain::Bitcoin`.

Already correct for Bitcoin Cash:

- The `getblocktemplate` path takes `sizelimit` from BCHN, which is
  `GetNextBlockSizeLimit`, ABLA included, and checks the final block with the
  real coinbase ([mining.cpp](https://gitlab.com/bitcoin-cash-node/bitcoin-cash-node/-/raw/master/src/rpc/mining.cpp)).
- `sigoplimit` is read as BCHN's SigChecks limit (`sizelimit / 141`).
- First-seen: the share transaction pool refuses a transaction that spends an
  outpoint another pool transaction spends (`PoolRejection::Conflict`), and
  its priority rule only decides which non-conflicting transaction a full pool
  keeps. Nothing replaces a first-seen spend.
- BCHN's 100 KB standard transaction size is not used anywhere as a P2Pool
  limit. `MAX_POOL_TRANSACTION_SIZE` (50 KiB) is P2Pool's own limit on share
  transactions, which P2Pool governs.
- CTOR, the BIP34 height push, the 100-byte scriptSig, coinbase version 2,
  sequence `0xfffffffe` and lock time height - 1 (valid and final on BCH),
  BIP320 version rolling, SIGHASH_FORKID, BCH Schnorr, CashAddr encoding,
  dust thresholds (546, 540, 576), the 100-block maturity of payout outputs,
  ASERT, and the per-block window sizing (K = 3, from P2Pool v1 on BCH).
- The midstate-and-tail share proof is constant in size whatever the coinbase
  size.
- Operator advice to keep `-percentblockmaxsize` below 100% so a large
  coinbase fits: right for the coinbase the owner has decided on.

## Ruled out

- **Share transactions should follow CTOR.** A share is not a BCH block, so
  canonical order does not apply. Topological order inside a share is a
  share-chain choice.
- **A peer can steal value through a witness-program payout.** It cannot; see
  "How the findings were checked".
- **`MAX_AUXPOW_COINBASE_SIZE` (65,685) limits BCH parents.** It is the Rust
  proof's transport bound. With A1's payout tree, merge-mining shares stay
  far below it.
- **Unused rate-limit settings** (`max_workbase_per_second` and others) are
  real but not a Bitcoin assumption. Left for a separate fix.
- **The TLA+ specifications model upstream rules only.** True, but a
  specification gap, not a Bitcoin assumption in running code.
- **The 10-second share period and the 600-second ASERT half-life of the
  share chain** were weighed for BCH in decisions 11 and 12. Only the uncle
  rate measurement (M6) is open.

## Proposed order of work

### 1. Share-chain rule changes: one batch, the owner's agreement needed

All need new protocol tags and a new store, so they belong in one flag day.

1. **The hybrid payout (A1)**, designed separately in
   [Hybrid payouts on Bitcoin Cash](architecture/bch-hybrid-payouts.md):
   remove `payout-cap11` and the tail draw, pay every miner directly, use a
   payout tree only for token shares past T = 3,072 bytes (inside A16's
   bound).
2. **The share body (A2):** room for a full 1,000,000-byte coinbase plus the
   share transactions, as the owner directed and the proposal's section 8.1
   sets out, or a compact coinbase after M2; re-derive frame and buffer
   sizes; put the limit in the protocol tag. Without this the payout stops at
   about 4,000 to 6,000 miners.
3. **Payout scripts (A10 b):** P2PKH, P2SH20 and P2SH32 in share headers,
   other scripts rejected, outputs ordered by script bytes.
4. **Merkle branch limit (A14):** 32 levels.
5. **Window cap and depths (A11, A12):** a BCH cap and BCH-derived depths,
   after M5.
6. **SigChecks (A13)** in place of the BIP141 sigop cost, or no budget.
7. **Witness commitment (A18):** rejected on BCH.
8. **Weights by work (A17).**
9. **Merge-mining commitment layout (A16):** before any covenant goes live;
   with it, the SAFA branch limit (A15, not a share-chain rule).

Items 4, 6, 7 and 9 and the depths of item 5 are done on PR #33, under the
batch's single protocol tag, with the share-version vote (lessons V17) and
a 10% cap on each of the donation and the fee, and the SAFA tool moved to
the new format (A15). A11's window cap is not done.

### 2. Changes to the block P2Pool builds, outside the share chain

Need the owner's agreement, and belong with the payout work: Hydrapool's
budget (A3, removed by the proposal), never stopping on an oversized block
(A8), the provider's coinbase reserve (A9), and the Stratum V2 large-block
path (A7). The proposal's section 8.2 lists the same outside limits a large
coinbase meets (SV2's 65,535-byte coinbase fields, `-percentblockmaxsize`).

### 3. Non-rule fixes, done on PR #33

- Template Provider size limit from BCHN, the floor only while BCHN is
  unreachable (A4).
- `sizelimit` and `sigoplimit` as `u64` (A5).
- CashAddr labels on the `coinbase_output` metric (A20).
- BCH data in the payout-rights test (A21).
- The documentation fixes listed above.
- Token-aware CashAddr accepted wherever a payout address is read (A10 a).
- The unread template transaction cache dropped (A6).
- Scalenet as a network (A19).
- A BCH chipnet `config.sample.toml` as the default sample (A23).
- Schnorr signatures in `bch-trade` (A24).
- Miner statistics keyed by the decoded script (A26).
- Prometheus metric names and labels without `bitcoin`/`btc`, with the
  Grafana dashboards (A27).
- From [What P2Pool v1 knew about Bitcoin Cash](p2pool-v1-bch-lessons.md):
  stratum difficulty capped at the share difficulty (V8), found blocks
  retried and sent to extra BCHN nodes (V9, part of V10), and the start
  check that the node is BCHN on the configured network (V15).

### 4. Non-rule fixes that can follow on PR #33 without the owner

A `getblocktemplatelight` mode after M3 (A6); the JSON API field names that
still say `bitcoin` (A27); found-block relay to share-chain peers (V10).
