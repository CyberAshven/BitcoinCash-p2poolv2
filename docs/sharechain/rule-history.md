# Share-chain rule history

The code describes each Bitcoin Cash share-chain rule as it is now. This
page keeps the history: which protocol string segment, store format,
constant, genesis share or job token layout changed, when, and in which
pull request. `CHANGELOG.md` lists the same changes from an operator's side.
The Bitcoin share chain follows upstream P2Poolv2 and is not listed here.

Every rule change so far has been a flag day: every node upgrades together
and starts from a fresh share-chain store. The share-version vote
(`shares::share_version`) is in place for later changes, but during the
alpha each change has shipped with a new segment and a fresh store instead.

Dates are when the rule was made; the PR column names the pull request that
brought it to `alpha`.

## The protocol string

A Bitcoin Cash node's libp2p protocol string is

```text
/p2pool/<network>/<genesis tag>/<rule tag>/<message tag>/1.0.0
```

for the Noise prologue and Identify; request-response offers it ending in
`/1.1.0` and in `/1.0.0`, and Kademlia's ends in `/kad/1.0.0`. The genesis
tag is the first eight hex characters of the genesis share's hash. The rule
tag joins one segment per share-chain rule (`share_chain_rule_tag` in
`p2poolv2_lib/src/node/behaviour/request_response.rs`), each built from the
rule's own constants, so changing a rule changes the string. The message tag
(`BCH_MESSAGES_TAG`) names the P2P messages beyond upstream's. Nodes whose
strings differ fail the handshake instead of rejecting each other's shares.

The rule tag's segments today, in order:

| Segment | Rule | Constant |
| --- | --- | --- |
| `pplns-k3-m100` | PPLNS window of three blocks of work, 100-share maturity | `BCH_PPLNS_WINDOW_BLOCKS`, `BCH_COINBASE_MATURITY` |
| `asert-parent` | share ASERT anchored at each share's parent | `BCH_ASERT_ANCHOR_TAG` |
| `trade-htlc` | share transaction rules and the share-trading contract | `BCH_SHARE_TRADE_RULE_TAG` |
| `payout-holder` | each share paid to the holder of its payout right | `BCH_PAYOUT_HOLDER_RULE_TAG` |
| `payout-hybrid-t3072-f500-r1to3-z8-cov1` | hybrid payouts and the payout tree | `bch_payout_rule_tag()` |
| `cuts-per-share` | donation and fee off each share's own weight | `BCH_PER_SHARE_CUTS_RULE_TAG` |
| `uncle-once` | an uncle has one nephew on a chain | `BCH_UNCLE_ONCE_RULE_TAG` |
| `body-1000000-204800` | share body limit | `BCH_SHARE_BODY_RULE_TAG` |
| `payee-p2pkh-p2sh20-p2sh32` | payout scripts | `BCH_PAYOUT_SCRIPT_RULE_TAG` |
| `merkle32` | coinbase merkle branch length | `bch_merkle_rule_tag()` |
| `depths-m100` | depths from the 100-share maturity | `bch_depth_rule_tag()` |
| `sigchecks3000-1452` | SigChecks per transaction and per share | `bch_sig_checks_rule_tag()` |
| `no-segwit` | no witness commitment in a coinbase | `BCH_NO_SEGWIT_RULE_TAG` |
| `weight-work` | shares weighted by exact work | `BCH_WORK_WEIGHT_RULE_TAG` |
| `mm2-roots-asert8-ticks` | merge-mining format v2 and the token ASERT check | `BCH_MERGE_MINING_RULE_TAG` |
| `version-vote` | the share-version vote | `BCH_SHARE_VERSION_RULE_TAG` |
| `bch-prev-1off-ticks` | mining on the current Bitcoin Cash chain | `BCH_CURRENT_CHAIN_RULE_TAG` |
| `genesis-last-resort` | genesis paid only when nothing else is | `BCH_GENESIS_LAST_RESORT_RULE_TAG` |
| `band-c64` | the difficulty band | `BCH_BAND_RULE_TAG` |
| `levels-546x2-k1` | payout levels | `BCH_PAYOUT_LEVELS_RULE_TAG` |
| `weak4-640-100-48-256-320-64-4096-64-32-summary` | weak shares | `BCH_WEAK_SHARE_RULE_TAG` |

The message tag today is `weak-by-hash/without-payouts/headers-resume`.

## Rule segments

The weak-share segment has its own table below.

| Date | Segment | Change | PR |
| --- | --- | --- | --- |
| 2026-10-18 | `pplns-k3-m100` | The PPLNS window is sized by the paying block's difficulty, behind a 100-share maturity. | v0.0.1-alpha, 2026-10-19 refresh |
| 2026-10-19 | `asert-parent` | Share ASERT is anchored at each share's parent instead of genesis. | v0.0.1-alpha, 2026-10-22 refresh |
| 2026-10-22 | `trade-htlc` | Share transactions may spend the share-trading contract; outputs must be P2PKH or P2SH32 above dust; a share transaction must be final at its share's height. | #20 |
| 2026-10-22 | `payout-holder` | A share's weight is paid to the holder of its payout right, not always to its miner. | #26 |
| 2026-10-23 | `payout-cap11` | At most 11 payout outputs in a coinbase, the rest paid to one payee drawn by weight. Replaced by the hybrid payout a day later. | #30 |
| 2026-10-23 | `uncle-once` | An uncle may have only one nephew on a chain. | #32 |
| 2026-10-24 | rule batch | `payout-hybrid-t3072-f500-r1to3-z8-cov1` replaces `payout-cap11`, and `body-1000000-204800`, `payee-p2pkh-p2sh20-p2sh32`, `merkle32`, `depths-m100`, `sigchecks3000-1452`, `no-segwit`, `weight-work`, `mm2-roots-asert` and `version-vote` are added. The donation and fee caps become 10% each. | #33 |
| 2026-10-29 | `cuts-per-share` | Donation and fee come off each share's own weight; added after the hybrid payout. | #33 |
| 2026-10-29 | `mm2-roots-asert8` | The test networks' ASERT band for token shares widens to an eighth to 8 times the computed target; its easy side was 1.05. | #33 |
| 2026-10-30 | `bch-prev-1off` | Mining on the current chain: shares carry the Bitcoin Cash headers a peer needs, and a share is paid only for work on the paying share's BCH chain or one block off it. | #37 |
| 2026-11-05 | `genesis-last-resort` | The genesis share is paid only when nothing else in the window is. | #68 |
| 2026-11-09 | `band-c64` | One share chain per network, the former mini chain, and the difficulty band in each share header. | #82 |
| 2026-11-09 | `levels-546x2` | Payout levels: each share header carries its miner's level, and a payee below its level is paid by a draw. | #83 |
| 2026-11-09 | `weak-64-500-6` | Weak shares. | #84 |
| 2026-11-12 | `levels-546x2-k1` | Payout level 0 is not valid: its 546 satoshis fell below dust in the payout tree. | #102 |
| 2026-11-15 | `bch-prev-1off-ticks`, `mm2-roots-asert8-ticks` | Each known BCH header carries its CHIP-2025-03 ticks and next interval, worked out from its own branch; the required nBits, the window sizing and the token ASERT band follow them. Before, each node learned one-minute blocks while running, so two nodes could require different nBits of one header. | #136 |

## Weak-share segment

| Date | Segment | Change | PR |
| --- | --- | --- | --- |
| 2026-11-09 | `weak-64-500-6` | Weak shares: at most 64 a share, a 500 basis point includer cut, a depth of 6. | #84 |
| 2026-11-12 | `weak2-64-500-6` | The weak root commits each carried weak share's whole encoding, not only its proof-of-work hash, which had let a relaying peer alter a weak share and get an honest share marked invalid. | #91 |
| 2026-11-12 | `weak2-64-100-48-256` | Study #85: the includer cut falls to 100 basis points, the depth rises to 48, and `R_MAX` falls from 4096 to 256 and joins the segment. | #98 |
| 2026-11-12 | `weak3-64-100-48-256` | A carried weak share is held to its carrier's time, not each node's clock. | #103 |
| 2026-11-15 | `weak4-640-100-48-256-320-64-4096` | The weak ratio follows the weak shares that ask: headers count them (`weak_asking`), `wanted_bits` may be easier than the share's target, and a share may carry 640 weak shares in 960 KiB. The segment gains `WEAK_FILL_MAX`, `WEAK_ASK_DIVISOR` and `R_ASK_MAX`. | #118 |
| 2026-11-16 | `weak4-640-100-48-256-320-64-4096-64-32` | Before any node ran `weak4`: more than 64 weak shares a share only while more than one in 64 of them ask (`WEAK_IDLE_CAP`), and a genesis weak ratio of 32. | #118 |
| 2026-11-16 | `weak4-640-100-48-256-320-64-4096-64-32-summary` | The weak root also commits the summary of the carried weak shares, so a node can prune whole weak shares and serve their summary. | #129 |

## Message segment

Not a share-chain rule: shares and stores are the same either side of it,
but nodes that cannot read each other's messages must not peer.

| Date | Segment | Change | PR |
| --- | --- | --- | --- |
| 2026-11-15 | `weak-by-hash` | A new share lists the weak shares it carries by hash (`ShareWithWeakHashes`), and a peer asks for the ones it lacks (`GetCarriedWeakShares`, `CarriedWeakShares`). | #121 |
| 2026-11-16 | `without-payouts` | A new share with large coinbase outputs is relayed without them (`ShareWithoutPayouts`). | #131 |
| 2026-11-18 | `headers-resume` | Upstream's request-response protocol 1.1.0: a header request carries the last header the requester got (`resume_after`), and the answer serves every share at a height, paging inside a height, where 1.0.0 cuts each height at 20 shares. The segment keeps Bitcoin Cash nodes on 1.1.0 with each other. | #152 |

## Store format

`STORE_FORMAT_VERSION` in `p2poolv2_lib/src/store/mod.rs`. A node refuses a
store of another format at start-up; delete the store directory and restart
to resync. Before format 1 a store had no version: earlier rule changes
needed a new store all the same, and #32 added a byte to each stored output.

| Format | Date | What changed | PR |
| --- | --- | --- | --- |
| 1 | 2026-10-30 | Chain work counts the work of each uncle a share names (upstream `604fe3ce`). | #37 |
| 2 | 2026-11-05 | Genesis is paid only as the last resort; a chain's first hours were validated under the old rule. | #68 |
| 3 | 2026-11-09 | One share chain, and headers carry the difficulty band. | #82 |
| 4 | 2026-11-09 | Headers carry a payout level. | #83 |
| 5 | 2026-11-09 | Headers carry a weak ratio and commit weak shares; the store keeps weak shares and their summary. | #84 |
| 6 | 2026-11-12 | The weak root commits whole weak shares. | #91 |
| 7 | 2026-11-12 | Weak-share constants of study #85. | #98 |
| 8 | 2026-11-12 | Payout level 0 is not valid. | #102 |
| 9 | 2026-11-12 | A carried weak share is held to its carrier's time. | #103 |
| 10 | 2026-11-15 | Headers count the weak shares that ask, and the weak ratio follows them. | #118 |
| 11 | 2026-11-16 | The weak root commits the summary; `WeakSummaries` keeps the leaf root beside it, and whole weak shares are pruned one payout depth below the confirmed tip. | #129 |
| 12 | 2026-11-16 | Known BCH headers carry their CHIP-2025-03 ticks and next interval. | #136 |

## Constants

| Constant | Was | Now | Date | PR |
| --- | --- | --- | --- | --- |
| `INCLUDER_CUT` | 500 basis points | 100 basis points | 2026-11-12 | #98 |
| `WEAK_DEPTH` | 6 shares | 48 shares | 2026-11-12 | #98 |
| `WEAK_RATIO_MAX_MULTIPLE` (`R_MAX`) | 4096 | 256 | 2026-11-12 | #98 |
| `WEAK_CAP` | 64 | 640, more than `WEAK_IDLE_CAP` (64) only while asks open it | 2026-11-15 | #118 |
| `WEAK_BYTES` | 96 KiB | 960 KiB | 2026-11-15 | #118 |
| `MAX_P2P_MESSAGE_SIZE` | 1,280 KiB | 2,144 KiB: up by the 864 KiB `WEAK_BYTES` gained | 2026-11-15 | #118 |
| `GENESIS_WEAK_RATIO_MULTIPLE` | `R_MAX` (4096, then 256) | 32 | 2026-11-16 | #118 |
| `BCH_PRUNE_DEPTH` | 2 x `BCH_PAYOUT_DEPTH` | 2 x `BCH_PAYOUT_DEPTH` + `WEAK_DEPTH`, so a syncing node fetches the summaries its payouts read | 2026-11-16 | #132 |

## Genesis shares

The genesis share's hash names the share chain in every protocol string.
The pinned hashes (`bch_genesis` tests) changed with these flag days:

| Date | Why | PR |
| --- | --- | --- |
| 2026-11-09 | The share header gained the band, the payout level and the weak ratio and weak shares; the merged chain keeps the mini chain's anchor. | #82, #83, #84 |
| 2026-11-12 | The genesis weak ratio is `R_MAX`, which fell from 4096 to 256. | #98 |
| 2026-11-12 | The genesis share's payout level is the lowest valid one, 1. | #102 |
| 2026-11-15 | The header gained `weak_asking`. | #118 |
| 2026-11-16 | The genesis weak ratio is 32, not `R_MAX`. | #118 |

## Job Declaration token layout

`BCH_JOB_TOKEN_VERSION` in `p2poolv2_lib/src/shares/share_commitment.rs`.

| Layout | Change | Date | PR |
| --- | --- | --- | --- |
| 2 | The share version follows the fee. | 2026-10-24 | #33 |
| 3 | The band cap follows the share version. | 2026-11-09 | #82 |
| 4 | The payout level follows the band cap. | 2026-11-09 | #83 |
| 5 | The weak ratio follows the payout level. | 2026-11-09 | #84 |
