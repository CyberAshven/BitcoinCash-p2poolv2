# SAFA AuxPoW covenant (merge-mining common format v2)

This is a merge-mined variant of [2qx's SAFA draft](https://bitcoincashresearch.org/t/safas-a-sha256-asic-minable-automated-token-market/2123):
a token whose threads are mined by any SHA-256 work that meets the token's
target, proved with the merge-mining common format v2 that BCH P2Pool and
Pickaxe agreed (Pickaxe PR #70, `docs/merge-mining.md`, and
[`docs/bch-auxpow.md`](../docs/bch-auxpow.md)). It is a reference
covenant, not the author's token; tSAFA is deployed on chipnet below. A focused
independent review found the ASERT-band and thread-count issues fixed
below; a complete independent review remains open.

The crate compiles the covenant and runs it in the Bitcoin Cash 2026 VM of
[libauth-rs](https://github.com/mainnet-pat/libauth-rs), a Rust port of
`@bitauth/libauth` 3.1.0-next.8, pinned to commit `be3ed16396c9`. It
replaced the Node tool `tools/safa-auxpow` on 2026-10-29, after both printed
the same bytes for every command over the fixtures
([`docs/tooling.md`](../docs/tooling.md)).

```sh
cargo nextest run -p p2poolv2_safa
cargo safa params deployment.json
```

| File | What it is |
|---|---|
| `safa.cash` | The covenant in CashASM, with per-deployment placeholders |
| `src/covenant.rs` | Network parameters; compiles a deployment and finds its fee constant K |
| `src/targets.rs` | Token targets, BCHN's compact nBits and aserti3-2d, the bands and the DAA |
| `src/codec.rs` | Thread state, leaves, slots, trees, output 0, splits and `AuxProof` v2 |
| `src/transaction.rs` | Leaves, claims and the genesis as transactions, and the VM check |
| `src/cli.rs`, `src/main.rs` | The `safa` command line; it never signs |
| `src/claimer/` | Operator-side chain following, win journal and verified claim broadcasts |
| `tests/` | The tests, all in libauth-rs's standard BCH 2026 VM |
| `test_data/` | The tSAFA fixtures and the recorded parity cases |

## Threads and their state

A deployment has 1 to 63 `threads` vaults. Compilation and the genesis
command reject 64 or more. With 288 threads the capped easing factor never
exceeds 1, so a hardened vault cannot recover; the stricter 63-thread
maximum leaves room for recovery before the 288-block cap. Each vault is
a P2SH32 output of the covenant holding fungible tokens, satoshis, and one
mutable NFT of the token category whose 16-byte commitment is the thread's
state:

```text
off len field
0   4   thread         u32 LE, fixed at genesis
4   4   state_height   u32 LE, the coinbase height of the last claim
8   4   target_bits    u32 LE, the token's canonical compact target
12  4   ref_bits       u32 LE, nBits of the last claim's header (the fallback's reference)
```

Every spend returns the vault to the output with its own index: the same
locking bytecode, category and mutable capability, and a 16-byte state.
No minting NFT exists, so no thread can be added after genesis.

## The claim

A claim spends exactly one input, the vault, with three unlocking pushes
before the redeem script: the split preimage (69 bytes, or empty for none),
`payout_hash` (32 bytes) and the proof (canonical `AuxProof` v2:
`"CTMP" 02 | header | h | salt | aux_branch | cb_head | cb_tail | d |
cb_branch`, `h ≤ 16`, `d ≤ 25`, nothing trailing). The covenant:

1. reads the BIP34 height `H` from the fixed-shape coinbase head (a minimal,
   positive push of 1 to 5 bytes), requires `H < 500,000,000` and runs
   `<H> OP_CHECKLOCKTIMEVERIFY`, so the claim's lock time is a block height
   at or above `H` and the coinbase cannot claim a height the chain has not
   reached. It requires `state_height < H`, so a height is never reused;
2. eases the state's target by the blocks since the last claim, the SAFA
   draft's DAA with its age capped at 288 blocks:
   `target × (⌊min(H − state_height, 288) × 5000 / threads⌋ + 5000) / 10000`,
   clamped to 1 and `0x7fffff << 232`, rounded down to canonical token
   bits (`3 ≤ exp ≤ 32`, `0 < mantissa < 0x800000`). These bits are the
   claim's target and the next state's;
3. rebuilds the 169-byte leaf from introspection: the category (wire
   order), `HASH256` of the vault's locking bytecode, `HASH256` of its
   current commitment, `payout_hash`, the claim's target bits and `ext`
   (zero, or `SHA256` of the revealed split preimage, checked canonical);
4. finds the slot `u32le(SHA256(02 ‖ salt ‖ category)[0..4]) mod 2^h`,
   folds the aux branch with `SHA256(01 ‖ left ‖ right)`, rebuilds the
   53-byte output 0 `OP_RETURN "CTMM" 02 root h salt`, hashes
   `cb_head ‖ output 0 ‖ cb_tail` (a tail of at least 4 bytes), folds the
   coinbase branch at index 0 to the header's merkle root, and requires
   `HASH256(header)`, read as an unsigned number, to be at most the claim's
   target. The header's own nBits never set the token's target;
5. checks the header's nBits against BCH's aserti3-2d at `H`, as BCHN's
   `CalculateASERT` computes it (a truncating division for the exponent, a
   flooring shift for its integer part, BCHN's cubic, then the clamps), the
   header's time standing in for its parent's:
   - mainnet: `computed / 2 ≤ target ≤ computed × 1.05`;
   - chipnet: `computed / 8 ≤ target ≤ computed × 8`, or nBits exactly
     the proof-of-work limit's `0x1d00ffff`;
   - anchors and half-lives from BCHN's chain parameters: mainnet 661,647 /
     `0x1804dafe` / 1,605,447,844 / 172,800 s; chipnet 16,844 / `0x1d00ffff`
     / 1,605,451,779 / 3,600 s. Chipnet needs the wider easy side because
     the header's timestamp may precede its parent's by hours; two hours
     span two chipnet half-lives. Mainnet keeps its original band;
   - the DAA-change fallback: once `H − state_height ≥ 4,032`, a header
     that fails the band is also accepted when
     `ref >> 4 ≤ target ≤ ref × 1.1`, `ref` being the state's `ref_bits`:
     four doublings on the hard side and ×1.1 on the easy side, as
     `docs/merge-mining.md` says. Its `min(⌊Δh / 288⌋, 4)` is always 4 from
     `Δh = 4,032` (where `⌊Δh / 288⌋` is already 14), so the covenant shifts
     by a constant 4;
6. writes the next state: the same thread, `H`, the claim's target bits and
   the header's nBits;
7. pays the reward `R = ⌊vault tokens / 420,000⌋` (above 0); the vault
   keeps the rest. The split follows `docs/merge-mining.md`:
   `d = ⌊R × d_bps / 10000⌋`, `f = ⌊(R − d) × f_bps / 10000⌋`; a share worth
   under 1,000 satoshis at the AMM price `share × (cash + virtual) / tokens`
   goes to the miner, the fee stays as computed from what the raw donation
   left, and an AMM with no tokens prices nothing;
8. requires exactly the outputs vault, miner, donation and fee, each of the
   last three only when above 0, each with exactly 800 satoshis of fungible
   tokens of the vault's category (no NFT, amount at least 1) to a script
   of at most 35 bytes (P2PKH, P2SH20 or P2SH32, as K assumes) whose
   `HASH256` is `payout_hash`, `d_hash` or `f_hash`;
9. pays the claim's fee from the vault's own cash: `S` to `3S` satoshis
   (there is no other input), and the vault output keeps at least the
   800-satoshi token dust, so a claim never leaves a vault that will not
   relay. `S = K + |proof|`, with
   `K = 580 + redeem script length`: the most bytes the claim has besides
   the proof (version, input, outpoint, a 3-byte unlocking length,
   sequence, output count, the 104-byte vault output, three 87-byte token
   outputs to scripts of at most 35 bytes, lock time, the split, payout
   and proof push overheads, the redeem script and its push) plus the
   53-byte commitment output the proof leaves out, as agreed. The claim's
   real size is always below `S`, so 1 sat/byte relays.

Work jobs and donation work carry `ext = 0`: their leaves pay one script
(the donation's or the operator's) as the miner, with no split.

## The market

A spend that leaves the state unchanged is a trade with the vault's AMM:
`(out_cash + virtual) × out_tokens ≥ (in_cash + virtual) × in_tokens`,
with `virtual` a deployment constant. Any other inputs and outputs are
free. A trade replaces the vault's outpoint but not its state, so a proof
mined for that state still claims afterwards.

## Measured size and cost

Measured by `tests/claims.rs` in the standard VM (chipnet, 4 threads); the
Node tool measured the same values with JavaScript Libauth, and the tests
pin them:

| Claim | Unlocking bytes | Op-cost / limit | Hash iterations / limit |
|---|---|---|---|
| `h` 0, `d` 0, no split | 1,595 | 121,745 / 1,308,800 | 40 / 818 |
| `h` 1, `d` 1, split | 1,729 | 136,035 / 1,416,000 | 53 / 885 |
| `h` 16, `d` 25, split | 2,977 | 257,638 / 2,414,400 | 153 / 1,509 |
| `h` 16, `d` 25, split, fallback band | 2,977 | 267,247 / 2,414,400 | 153 / 1,509 |
| `h` 16, `d` 25, split, 10,000-byte unlock | 10,000 | 370,058 / 8,032,800 | 263 / 5,020 |

The redeem script is 1,364 bytes on chipnet and 1,363 on mainnet (with 4
threads and 84,000,000,000 virtual satoshis; it varies by a few bytes with
the constants), so K is 1,944 and 1,943. The worst case uses 11.1% of the
operation-cost limit and 10.1% of the hashing limit. The largest coinbase a
claim can carry is the 10,000-byte unlocking limit less the redeem script,
the other pushes and the proof's other parts: 7,173 bytes at the deepest
branches (`h` 16, `d` 25), measured. P2Pool's token shares keep the
coinbase within 3,072 bytes.

## What the tests check

- **Golden vectors** (`tests/vectors.rs`), the ones `p2poolv2_lib` checks
  (Pickaxe PR #70; a test pins them to its test sources): leaf A and B, the
  two-token root `5b61ecfe…1c38d0`, output 0 and the empty commitment, the
  224-byte proof (its BIP34 height 325,909, its coinbase txid, and that it
  verifies for token A and for no other state hash), the split preimage,
  `ext` and leaf `c4027136…033e41`, every split share in the table,
  `MIN_SHARE_SATS` and the 1,000,000-reward claim (980,100 / 10,000 /
  9,900). Each runs in the covenant's own VM functions.
- **ASERT** (`tests/asert.rs`): all 14,000 aserti3-2d specification vectors
  (BCHN's QA runs vendored in `p2poolv2_lib`), the 9 mainnet and 9 chipnet
  real header pairs, chipnet block 329,001's nBits with a timestamp two
  hours before its parent, the exact x8 and x8 + 1 boundaries, unchanged
  mainnet edges, invalid nBits, heights below the anchor, Bitcoin nBits
  and the fallback band.
- **Claims and attacks** (`tests/claims.rs`): whole claims with and without
  a split, a whole donation, a dropped donation, the deepest proof and a
  10,000-byte unlock, on chipnet and mainnet. Refused: redirected payouts
  (with or without a matching `payout_hash`), extra tokens, satoshis,
  outputs or inputs, a changed vault, another thread or category, every
  kind of wrong split, a hidden or rewritten split, Bitcoin nBits, a reused
  or older height, a replayed proof, lock times below `H` or in the time
  domain, a time-domain coinbase height, fee overpay above `3S` or under
  `S`, a vault left under 800 satoshis, a 36-byte payout script, a wrong
  next state, an easier target in the leaf, and a fallback claim one block
  early or just harder than `ref >> 4`; and the VM cost above.
- **Command line** (`tests/cli.rs`, `tests/fixture.rs`): every command,
  the recorded parity cases (`test_data/parity/cases.json`, what the Node
  tool printed), thread-count refusals and byte-for-byte fixture
  reproduction. The printed claim and both four-thread and 63-thread
  genesis transactions are decoded and checked in the standard VM. Genesis
  uses an `OP_1` funding-script stand-in for the owner's signed input.
- **Against P2Pool's Rust side**, checked once by hand on 2026-10-24: for a
  claim the covenant accepts (chipnet, three tokens, a split),
  `p2poolv2_cli bch-auxpow verify --network chipnet` accepts its proof with
  the `token` JSON `leaf` prints and refuses it with another state hash,
  and `p2poolv2_cli bch-auxpow build` gives the same root, `h` and salt.

## Command line

Every command reads JSON files of at most 1 MiB and prints JSON. Amounts
are whole numbers (strings for large values), scripts and hashes are hex,
txids and the category are in display order, and bits are eight hex digits.
`cargo safa` runs the `safa` binary.

```sh
cargo safa params  deployment.json            # the covenant, K and the vault address
cargo safa leaf    deployment.json job.json   # a miner's leaf at one height (also: work)
cargo safa claim   deployment.json claim.json # a verified, complete claim transaction
cargo safa unlock  deployment.json claim.json # only that claim's unlocking bytecode
cargo safa verify  deployment.json claim.json # checks that claim in the VM
cargo safa genesis genesis.json               # an unsigned chipnet genesis
cargo safa claimer claimer.json               # follow a vault and claim pool wins
```

`deployment.json` is what `genesis` prints as `deployment`:
`{"network":"chipnet","threads":4,"virtual_sats":"84000000000","category":"<txid>"}`.

`leaf` takes `{"vault_commitment","height","payout_script","split"}`, where
`split` is optional: `{"donation_bps","donation_script","fee_bps","fee_script"}`.
Its `token` object is the JSON that `p2poolv2_cli bch-auxpow build` and
`verify` read. The target eases with the height, so a miner asks for the
leaf of the height it mines.

`claim` takes `{"outpoint":{"txid","vout"},"vault":{"value_sats","token_amount","commitment"},"payout_script","split","proof","fee_rate","locktime"}`
(`fee_rate` 1 to 3, default 1; `locktime` defaults to the coinbase
height). It refuses a claim the covenant would refuse. The transaction needs
no signature; whoever broadcasts it decides when.

## Claimer

`cargo safa claimer claimer.json` runs separately from P2Pool. It follows
one vault, reads the pool's public wins log, resolves the recipients, and
uses the same claim builder and complete BCH 2026 VM verification as
`safa claim` before sending anything. It needs no signing key. The pool
and its miners can keep running their existing software.

```json
{
  "deployment": "deployment.json",
  "vault": {
    "txid": "8c5ecd6d4ce039490c233c9399cc1e591720218fd9712dc8c00edc172a06c471",
    "vout": 3
  },
  "token_file": "merge-tokens.json",
  "wins_log": "merge-tokens.wins.jsonl",
  "payout_scripts": ["76a914c40a24bf5b3cfe561cf36afea1b59fe61383e80688ac"],
  "split_scripts": ["76a914b25a2f0a68073fad864f684bf26e350b4526354b88ac"],
  "p2pool_config": "config.toml",
  "fee_rate": 1,
  "delay_seconds": [10, 120],
  "poll_seconds": 5,
  "journal": "claimer-journal.json"
}
```

The example selects tSAFA thread 3. All paths are relative to
`claimer.json`. Configurations and token files are limited to 1 MiB.
`deployment.json` supplies the network, thread count, virtual satoshis and
category in **display order**, as in the deployment section below. The
pool's token file and wins log use **hashed (wire) order** for categories;
the claimer reverses the deployment category when matching them.

Use exactly one RPC source. `p2pool_config` reads only `[bitcoinrpc]`
`url`, `username` and `password` from the existing P2Pool TOML. Alternatively,
replace it with:

```json
"rpc": {
  "url": "http://127.0.0.1:48332",
  "username": "operator",
  "password_file": "rpc-password.txt"
}
```

The password file contains one line; a trailing newline is allowed.
Passwords are never printed or stored in the journal. RPC URLs cannot
contain embedded credentials. The RPC node's network must match the
deployment. Use a node with historical block data and `txindex` when
starting from an already-spent vault: BCHN's
[`getrawtransaction`](https://docs.bitcoincashnode.org/doc/json-rpc/getrawtransaction/)
needs that index for a historical transaction without its block hash.

`payout_scripts` lists miners' locking bytecodes, matched by `HASH256`
against each win's `payout_hash`. `split_scripts` lists the donation and
operator fee locking bytecodes named by the 69-byte split preimage. A
zero-rate split slot needs no script. Unknown recipients are logged and
skipped permanently; add all intended scripts before running. Other token
categories and malformed lines are ignored, with malformed lines logged.

Each pass checks `gettxout(txid, vout, true)`, including mempool spends.
Every vault must have the deployment's category, a mutable NFT, the exact
covenant locking bytecode and the same thread (the commitment's first
four bytes, little endian). A spent vault is followed through mempool
transactions or blocks since the saved scan height, always to output 0.
This follows Pickaxe's claims and AMM trades too; an AMM trade can move the
outpoint without changing the commitment. An unseen spender is polled
again. The mempool and block scan is practical for chipnet; scanning the
larger mainnet mempool and blocks costs more.

Only proof coinbase heights `H` above the current state height and at or
below the chain tip are considered, oldest height first. The leaf binds
`HASH256` of the exact 16-byte commitment. A proof for another state fails
the builder or VM and is journaled as dropped, never retried. Wins made
obsolete by a newer state are dropped too. A win's random delay is chosen
once, measured from its logged Unix time and saved across restarts. An
older valid win keeps its place until its delay expires. Defaults are
10 to 120 seconds, a 5-second poll and 1 sat/byte; fee rates must be 1 to 3.
A vault that temporarily lacks cash defers its win until an AMM trade or
other valid move adds enough cash; this does not drop a same-state proof.

The claimer saves a prepared transaction before `sendrawtransaction`.
Success immediately moves its vault to the claim's output 0 and atomically
updates only the matching token entry's `safa.state`, without waiting for
confirmation. Every externally observed commitment change also updates
that state. Other tokens and fields are preserved. Writes use a flushed
temporary file in the same directory and atomic replacement, so P2Pool's
mtime reload never reads a partial file. A stable token-file sidecar lock
serializes updates from claimers for different categories. A rejection triggers chain
following on the next pass. An uncertain send is looked up after restart;
any retry is the same locally verified transaction, never another claim
against that pending input.

The atomic journal stores the current vault and commitment, pinned thread,
block scan cursor, wins byte offset, queued wins and delays, claimed and
dropped identities, and pending transaction IDs and raw transactions. It
is bound to the deployment and configured starting vault. One broadcaster
can hold its journal lock at a time. Complete log lines advance the offset;
a shrinking log resets it to zero after rotation, and win identities
prevent duplicate claims. Each pass reads up to 8 MiB, draining an oversized
malformed line to its next newline if needed; the journal has a
64 MiB safety limit. Keep the journal across ordinary restarts. A changed
block scan anchor stops claims until the operator reviews the reorg and
starts a fresh journal from a verified current vault.

Every event is one JSON line on stdout: vault following, state writes,
skipped/dropped wins, built claims, broadcasts and rejections. Built claim
events include the verified transaction. `--once` makes one pass and sends
at most one claim, useful for scheduled runs. `--dry-run` prints verified
claims without broadcasting or changing the token file or journal; it
still reads the configured node to follow the actual vault. Combine both
flags for an inspection pass.

Mainnet uses the same command and code. Supply a mainnet `deployment.json`
and corresponding vault, token file, recipients, node and fresh journal
in the config; no chipnet deployment or outpoint is built into the claimer.
The unchanged public live-win fixture `test_data/tsafa-live-win.jsonl`
is exercised offline against the deployed tSAFA parameters in nextest.

## Chipnet tSAFA deployment

Deployed on 2026-10-29 with the owner's approval. `genesis` still refuses
mainnet until the covenant has an independent review and the owner's
approval.

| | |
|---|---|
| Genesis txid | `8c5ecd6d4ce039490c233c9399cc1e591720218fd9712dc8c00edc172a06c471` (607 bytes, 609-satoshi fee) |
| Category | `1e642d4ed9a0a44b3153a3f1fda841906eb1c9a2ce9daa2f85641ddd3e8700fe` |
| Vaults | genesis outputs 0 to 3, threads 0 to 3, at `bchtest:rwhk4prfungulj3qwcdfcvf8khjk94nmw9t262mju2gevuae7n0qwnf98a02d` |
| First state | start height 330,001, target bits `1d00ffff`, reference bits `1a457dba` (thread 0's commitment: `0000000011090500ffff001dba7d451a`) |

`deployment.json`:

```json
{"network":"chipnet","threads":4,"virtual_sats":"84000000000","category":"1e642d4ed9a0a44b3153a3f1fda841906eb1c9a2ce9daa2f85641ddd3e8700fe"}
```

The covenant is the regenerated fixture's, byte for byte (1,364-byte redeem
script, K 1,944). The genesis was built with `cargo safa genesis`, signed by
the owner's chipnet BCHN wallet and accepted by its `testmempoolaccept`
before broadcast. The owner kept PHOTON's emission: each claim pays
1/420,000 of its vault's remaining tokens.

The rest of this section is how it was prepared, for a later deployment.

**Parameters.** 4 threads; 525,000,000,000,000 base units per
thread (21,000,000 tSAFA at 8 decimals in all), so the first reward is
1,250,000,000 units (12.5 tSAFA); 84,000,000,000 virtual satoshis, so a
first reward is worth about 200,000 satoshis at the AMM price and a 1%
donation or fee (about 2,000 satoshis) clears the 1,000-satoshi minimum;
2,000,000 satoshis seeded per thread; initial target bits `1d00ffff`
(difficulty 1; the DAA hardens it by up to a factor of 0.625 per claim).

`test_data/tsafa-chipnet-fixture.json` holds what `cargo safa params`
prints for these parameters, with a placeholder funding outpoint (txid
`11` × 32, output 0, which would be the category), so Pickaxe can check the
redeem script, K, locking bytecode and `lock_hash` before any funding; the
tests check it reproduces byte for byte. The covenant does not depend on
the category, so the real funding txid changes only the leaves.

The corrected ASERT band changed the chipnet redeem script, locking
bytecode, `lock_hash` and address. Use the regenerated fixture before any
genesis: a deployed covenant cannot be replaced. Proofs built for the old
`lock_hash` must be rebuilt and mined again, because the leaf, root,
coinbase and header commitment all change.

The supplied Pickaxe synthetic claim was rebuilt for this `lock_hash`,
preserving its deployment, outpoint, vault, split, branches and coinbase
head and tail. Its original proof fails the corrected covenant. The
rebuilt input is [`test_data/tsafa-chipnet-claim.json`](test_data/tsafa-chipnet-claim.json);
[`test_data/tsafa-chipnet-claim-result.json`](test_data/tsafa-chipnet-claim-result.json)
records the original proof and rebuilt leaf, root and transaction. Copy
the fixture's `deployment` object to `deployment.json`, then run:

```sh
cargo safa claim deployment.json p2poolv2_safa/test_data/tsafa-chipnet-claim.json
```

The command verifies the complete transaction in the standard BCH 2026 VM:
txid `89771ec35e517ee27478de8a4639c8847dffbcff5a44db0c5156d180e1274c09`,
2,105 bytes, a 2,200-satoshi fee within a 2,200-byte size bound, height
329,001 and target `2001bffe`. Its 1,250,000,000-unit reward pays
1,225,125,000 to the miner, 12,500,000 to the donation and 12,375,000 to
the fee recipient. This is an offline VM check; it was not broadcast or
accepted by BCHN.

**Funds.** One chipnet UTXO that is **output 0** of its transaction, worth
at least 8,001,155 satoshis: 4 × 2,000,000 for the vaults, a 609-satoshi
genesis fee at 1 sat/byte (609 bytes, estimated with the largest P2PKH
signature, a 73-byte ECDSA one, and its key) and 546 satoshis of change.
0.081 tBCH (8,100,000 satoshis) leaves 99,391 in change. Its txid becomes
the token category. Each claim then costs its vault `S` (1,944 plus the
proof, so about 5,300 satoshis for a 3,000-byte coinbase) plus 800
satoshis per token output: about 250 claims per thread before the seed
runs out, estimated, unless buyers add cash through the market.

**Keys.** Only the owner's own chipnet wallet key for that one UTXO, used
once to sign the genesis input in the owner's BCHN wallet. The covenant has
no key, no admin and no minting baton; after genesis nobody holds any power
over the vaults beyond the covenant's rules.

**Steps for the owner** (BCHN chipnet RPC):

1. Make the output-0 UTXO with a self-send whose change goes second:
   `createrawtransaction '[]' '{"<own address>":0.081}'`, then
   `fundrawtransaction <hex> '{"changePosition":1}'`,
   `signrawtransactionwithwallet` and `sendrawtransaction`. Note the txid,
   output 0's `scriptPubKey`, the current tip height (`getblockcount`) and
   its nBits (`getblockheader <tip hash>`, field `bits`).
2. Write `genesis.json`:

   ```json
   {
     "network": "chipnet", "threads": 4, "virtual_sats": "84000000000",
     "supply_per_thread": "525000000000000", "seed_sats": "2000000",
     "start_height": 0, "target_bits": "1d00ffff", "ref_bits": "1d00ffff",
     "funding": { "txid": "<txid>", "vout": 0, "value_sats": "8100000", "script": "<scriptPubKey>" },
     "change_script": "<a P2PKH scriptPubKey of the owner's wallet>",
     "fee_rate": 1
   }
   ```

   with `start_height` the tip height and `ref_bits` the tip's nBits.
3. `cargo safa genesis genesis.json` prints the unsigned genesis, the vault
   address, each thread's state and the `signrawtransactionwithwallet` call
   (with the funding output as `prevtxs`). Review the outputs.
4. Sign with that call. Broadcasting it with `sendrawtransaction` is the
   owner's decision.
5. Publish the printed `deployment` object. Miners and pools then ask
   `cargo safa leaf` for each height they mine, and claim with
   `cargo safa claim`.

Operators can use the claimer above for pool wins. Its real-win test is an
offline BCH 2026 VM check; a live run and claim broadcast are separate
operator steps.

## Changes from the v1 variant

The v1 variant (in `tools/safa-auxpow`'s history, up to commit `34deb329`)
verified the whole coinbase with the `fabe6d6d` output and Namecoin slots,
kept an 80-byte baton with a recipient hash, timestamp and nonce, bound the
claim to a Unix-time lock and a CSV block age on a version-2 transaction,
and allowed a sponsor input with a 1,500-satoshi vault contribution. All of
that is gone: the leaf binds the payout and the state, the coinbase height
binds the time, and the vault pays its own fee.

## Open

- An independent review of the covenant, and a run against BCHN (regtest
  or chipnet) with a real P2Pool coinbase.
- The economic parameters: the virtual reserve, the seed, the supply and
  the reward divisor are a proposal for a test token. As in the draft,
  selling rewards back to a vault spends its cash, which also pays claim
  fees; a vault without cash cannot be claimed until someone buys.
- The residual risks in `docs/merge-mining.md`: any SHA-256 work with a
  header that passes these format checks can mine a token, and eCash
  headers share BCH's ASERT anchor, so only the height bound stops them.

## Regtest evidence and reproduction

This evidence is for the **v1** variant and format. On 2026-10-17 two
isolated BCHN 29.2.0 nodes accepted P2Pool Job Declaration blocks carrying
v1 SAFA work, and a v1 withdrawal
(`d1d879194e1ca5df8a2dad35ebdf6436841b903a29ad6a3e484beb47d05c79be`, block
`4aa8b6848906eba9df5d8293e08e00a047812f43b1003c569a3fdafbeda46d2e`) was
rejected one block before its relative age matured and accepted after it;
BCHN rejected a changed merkle sibling. The harness (`live-regtest.mjs`)
and its fixture are in `tools/safa-auxpow`'s history at `34deb329`. The v2
covenant has not yet been run against BCHN.
