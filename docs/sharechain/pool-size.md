# Pool size limits

> Ported from upstream P2Poolv2's documentation:
> `architecture/pool_scalability_limits.adoc` ("Pool Size Constraints") in
> [p2poolv2/docs](https://github.com/p2poolv2/docs) at commit `623e4c1`,
> Copyright 2024-2026 P2Poolv2 Developers, MIT or Apache-2.0. Upstream's page
> is a title with no text yet; this page states the Bitcoin Cash situation.
> See the [port map](../upstream-port-map.md).

Upstream's requirement is a pool of up to a thousand nodes, each serving
thousands of ASIC miners, without running into coinbase size limits
([Requirements](../requirements.md#payouts)). Every miner with a share in the
PPLNS window is owed part of each block, so the question is how many payees a
coinbase can carry, and who has to handle that coinbase.

## What does not limit it on Bitcoin Cash

- **Block space.** Since May 2024 the block size limit follows ABLA
  ([CHIP-2023-04](https://gitlab.com/0353F40E/ebaa/-/blob/main/README.md)): a
  32 MB floor on mainnet that grows with use. BCHN also refuses blocks over
  2,000 MB (`MAX_CONSENSUS_BLOCK_SIZE` in `src/consensus/consensus.h`), a
  guard of its own rather than part of ABLA. A coinbase with a thousand P2PKH
  payouts is about 34 KB, a small part of even the floor.
- **Fees.** At BCHN's default minimum relay fee of 1 satoshi per byte, the
  space a thousand payouts take would otherwise hold about 34,000 satoshis
  of fees. Upstream's concern that block space is too expensive for many
  payouts does not hold here.

P2Poolv2 never sets block limits of its own: it takes its templates from BCHN,
which applies ABLA, and only governs what it builds itself.

## What does limit the coinbase

| Limit | Size | Kind |
|---|---|---|
| A Bitcoin Cash transaction, coinbase included | 1,000,000 bytes (`MAX_TX_SIZE` in BCHN's `src/consensus/consensus.h`, checked for coinbases too in `src/consensus/tx_check.cpp`), about 29,000 P2PKH payouts | Bitcoin Cash consensus. The 100 KB standard transaction size is relay policy and does not apply to a coinbase, which is never relayed alone. |
| A share body: its share transactions plus the Bitcoin Cash coinbase it carries | 1,204,800 bytes (`BCH_SHARE_BODY_SIZE_LIMIT`): a coinbase of up to 1,000,000 bytes beside 200 KiB of share transactions, so it never binds before the transaction limit above | Share-chain rule (this project's own; can be changed with a new protocol tag) |
| A coinbase in Stratum V2 messages: the extended job's prefix and suffix, Job Declaration, `SubmitSolution` to a Template Provider | 65,535 bytes per field, about 1,900 P2PKH payouts | Stratum V2 protocol. A larger coinbase is refused for that extended channel with a clear error; SV1 and SV2 standard channels keep working. A block the provider cannot take goes to BCHN over RPC instead. |
| What a Stratum V1 device accepts in `coinb1` and `coinb2` | Not measured on this pool's devices. Upstream reports stock Antminer firmware takes "just over a dozen outputs" (discussion [#668](https://github.com/p2poolv2/p2poolv2/discussions/668): an OCEAN block with 16 payouts in a 784-byte coinbase); P2Pool v1 warned that S9s may crash near 226 payees. OCEAN's own gateway sets caps per device, from 755 bytes for older S19 stock firmware to 16 KB, and none for Braiins firmware ([audit](../bch-btc-assumptions-audit.md), item A1). | Device firmware. Every Stratum V1 device hashes the whole coinbase, also behind an SV2 translator; Stratum V2 standard channels never see it. |
| A token covenant that checks a merge-mining proof | The covenant hashes the whole Bitcoin Cash coinbase in the VM, and a stack element is at most 10,000 bytes since May 2025 ([CHIP-2021-05 VM limits](https://upgradespecs.bitcoincashnode.org/2025-05-15-upgrade/)), so such a coinbase must stay under 10,000 bytes. | Bitcoin Cash VM limits, for shares that merge-mine tokens ([Auxiliary work proofs](../bch-auxpow.md)) |

The P2PKH counts assume 34 bytes per payout and leave room for the rest of the
coinbase; they are arithmetic, not measurements.

## The hybrid payout

Plan decision 14 ([bch-plan.md](../bch-plan.md)) replaced the earlier cap of
11 payouts in 784 bytes, which took Bitcoin's firmware report as a Bitcoin
Cash limit:

- **Shares that merge-mine no tokens** (the exact empty merge-mining
  commitment, as on every pool-built job) pay every payee directly, within
  the 1,000,000-byte transaction limit: 29,399 P2PKH payees, or 29,398 beside
  a tree output. Block space is ABLA's concern.
- **Shares that merge-mine tokens** keep their coinbase within 3,072 bytes
  (77 P2PKH payees, 78 without a tree output), so the token covenant can
  still hash it in the VM.
- **Past a share's limit,** payees are taken smallest first; the payee that
  straddles the limit and every payee after it go to one covenant payout
  tree output, whose keyless transactions of up to 500 outputs pay each of
  them exactly, less their share of the fee at 1 to 3 sat/byte. Every P2Pool
  node broadcasts the trees of found blocks once their coinbase is mature.

It is a share-chain rule, carried in the Bitcoin Cash protocol strings as
`payout-hybrid-t3072-f500-r1to3-z8-cov1`, and never changes Bitcoin Cash
consensus. What stock firmware really accepts is still to be measured on
chipnet over Stratum V1 and through an SV2 translator. Full rule:
[Payout distribution](../architecture/payout-distribution.md) and
[Hybrid payouts on Bitcoin Cash](../architecture/bch-hybrid-payouts.md).

## Other limits on pool size

- **Share interval and uncles.** The share chain targets one share every 10
  seconds. A share that reaches peers late becomes an uncle, which earns 90%
  of its difficulty, so latency costs miners a little but does not lose work.
- **Small miners' variance.** A miner too small to have a share in most
  windows is paid rarely. The mini share chain, with a target ceiling 256
  times easier, serves them ([Small miners](../small-miners.md)), and shares
  can be sold for BCH ([Selling shares](../payouts/selling-shares.md)).
- **Node resources.** Each node keeps the share headers forever (about 1 KB a
  share, 3.1 GB a year per share chain) and block bodies for about 29 days,
  mostly coinbases, which grow with the number of payees
  ([Requirements](../requirements.md#disk), [Pruning](../architecture/pruning.md)).
  Memory and CPU have not been measured.
