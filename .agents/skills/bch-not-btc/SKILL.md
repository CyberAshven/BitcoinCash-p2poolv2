---
name: bch-not-btc
description: Use when porting code or a design from upstream P2Poolv2 or any Bitcoin source into this Bitcoin Cash fork, or when choosing a constant, limit, integer width, script, address, signature, transaction or block rule. Checks each Bitcoin assumption against Bitcoin Cash's own rules before it is adopted. Do NOT use for P2Pool's share-chain rules themselves (bch-share-chain-rules) or for code that talks to the node (bch-node-agnostic).
---

# Bitcoin Cash is not Bitcoin

Upstream P2Poolv2 is designed around Bitcoin: scarce block space, high
fees, SegWit. Much of that does not hold on Bitcoin Cash. A Bitcoin
assumption carried over unchecked gives a design that is wrong on BCH, and
it usually still compiles and passes its tests.

This project never changes Bitcoin Cash's own rules. It follows them.

## The check

Every constant, limit, type width and design argument that comes from
upstream or from Bitcoin is checked against Bitcoin Cash's primary sources
before it is adopted:

- the node's source, BCHN first (`src/consensus/`, `src/policy/`,
  `src/validation.cpp`);
- the upgrade specifications (upgradespecs.bitcoincashnode.org) and the
  CHIPs they name;
- the protocol reference (reference.cash).

The source is cited in the doc comment, the commit or the PR. Evidence from
another chain or another pool is labelled as such and measured on Bitcoin
Cash (regtest, chipnet) before it shapes a rule.

## Facts that most often break ported code

| Area | Bitcoin Cash | What it means here |
|---|---|---|
| SegWit | None: no witness, no wtxid, no weight, no witness commitment in the coinbase | Block and transaction size is counted in bytes. A share carries no witness commitment (rule tag `no-segwit`) |
| Block size | ABLA (CHIP-2023-04, May 2024): a fixed 32 MB floor and no fixed maximum; the limit moves with demand | Never hard-code 32 MB as a maximum. Bound block-sized data by the node's current limit: the template's `sizelimit`, or `ablastate.nextblocksizelimit` from BCHN's verbose `getblockheader`. Sizes and counts are `u64`, not `u32` |
| Transaction order | CTOR (Nov 2018): after the coinbase, transactions are sorted by txid, ascending | Keep the node's order. Adding a transaction to a block means sorting again |
| Signatures | Every input signs with `SIGHASH_FORKID` over a BIP143-style digest. Schnorr and ECDSA are both valid in `OP_CHECKSIG`, `OP_CHECKDATASIG` and `OP_CHECKMULTISIG` (2019) | A signer sets the fork id. Tests that check scripts cover both signature types |
| Signature counting | SigChecks (May 2020) replace sigops: 3,000 per transaction, and a block limit of one SigCheck per 141 bytes of the block size limit | Count SigChecks, never sigops |
| Addresses | CashAddr (`bitcoincash:`, `bchtest:`, `bchreg:`), for P2PKH, P2SH20 and P2SH32 (May 2023), with token-aware variants | No bech32 segwit addresses. A 32-byte script hash is a valid payee |
| Difficulty | ASERT, `aserti3-2d` (Nov 2020), retargets every block | No 2016-block retarget anywhere. The share chain uses ASERT too (`docs/difficulty_adjustment/`) |
| Script VM | CashTokens (May 2023), the VM limits and big integers (May 2025), and changes each May | Script code targets the Bitcoin Cash VM of the current upgrade, not Bitcoin's interpreter |
| Transaction size | 1,000,000 bytes per transaction is a rule, the coinbase included. 100,000 bytes is BCHN's standardness policy | A coinbase is not held to the 100 kB policy. Never present policy as a rule |
| Transaction version | Since May 2023 only versions 1 and 2 are valid | Never build a transaction with another version |
| Fees and mempool | About 1 sat/byte; no replace-by-fee, first seen wins | An argument that rests on scarce block space or high fees is derived again for Bitcoin Cash |

## Scaling is the test

A design is judged by its cost when blocks are large: who pays (the node,
the pool, a miner, a wallet) and how that cost grows as blocks grow.
Designs that stay small on every side are preferred. Design for what the
rules allow today, at least 32 MB or about 128,000 transactions of 250
bytes, not for what is usually mined. "Today's blocks are small" is not an
argument.

Example: SV2's full-template messages are sized for Bitcoin (lists of up to
64K entries, 16-bit positions, one message of at most 16 MiB) and cannot
carry a 32 MB block. A coinbase-only job with a merkle path, as BCHN's
`getblocktemplatelight` gives, scales.

## Where this fork records it

- `docs/bch-btc-assumptions-audit.md`: every Bitcoin assumption found,
  how it was checked and what was done.
- `docs/p2pool-v1-bch-lessons.md`: what P2Pool v1 on Bitcoin Cash learned.
- `docs/sharechain/pool-size.md`: what limits the coinbase and the pool.
- `docs/bch-design.md`: decisions and the checklist.

A new finding goes into the audit page, with its source.
