# Trading Bitcoin Cash shares for BCH

How a miner sells share-chain value to a buyer for BCH, atomically, with
Bitcoin Cash scripts only. This is decision 13 of
[bch-plan.md](../bch-plan.md): shares trade through BCH covenants, every
share output stays above the dust limit, and the Lightning Network is not
used. Mainnet, chipnet and regtest run the same rules; the code branches on
the chain (Bitcoin or Bitcoin Cash), never on the Bitcoin Cash network.

Upstream's design for Bitcoin, with an HTLC on the share chain and a
Lightning invoice on the other side, is in
[p2pool-2-lightinig-example.md](p2pool-2-lightinig-example.md) and
[htlc_scripts.md](htlc_scripts.md). This page replaces the Lightning leg
with an on-chain BCH contract. The user view is on
[Selling shares](../payouts/selling-shares.md).

## In short

- The **seller** (a miner) holds share units and picks a 32-byte secret; the
  **buyer** holds BCH. One hash time locked contract (HTLC) template behind
  P2SH32 serves both chains.
- The seller locks share units first, refundable late; the buyer locks BCH
  under the same secret hash, refundable earlier. The seller's BCH claim
  publishes the secret, and the buyer uses it to claim the shares. Either
  both claims can happen or both refunds.
- The buyer gets the share's **payout right**: from the claim on, every BCH
  block the pool finds while that share is in its PPLNS window pays the
  share's weight to the buyer's key hash.

## What a trade sells

A share coinbase output pays one share unit (10^8 base units) to the miner's
P2PKH script, `miner_address` in the header. In upstream's design
([P2Poolv2 docs, payouts](https://github.com/p2poolv2/docs/tree/main/payouts))
the holder of a share output earns that share's PPLNS payouts; upstream has
not built it, and this fork does, as the payout right. Units are share-chain
currency, divisible down to the dust floor; the right is whole and travels
as one. A trade that locks the share output at input 0 into the contract at
output 0 sells the right, whatever part of the units it locks.

## Payout rights

On Bitcoin Cash every share carries one payout right. A coinbase credits
each window share's weight to whoever holds that share's right at the paying
share. Bitcoin keeps upstream's rule and pays `miner_bitcoin_address`.

### The holder rule

1. **Origin.** A share's right starts on its coinbase output (output 0, one
   share unit), held by the share's own `miner_address` key hash.
2. **Transfer: input i to output i.** A share transaction spending an output
   that carries a right through its input `i` passes the right to its output
   `i`. With no output `i`, the right stays with its holder for good
   ("frozen"). So each output carries at most one right, and a merge of
   several share outputs into one passes only the right on input 0.
3. **Holder.** A right on a P2PKH output is held by its key hash. A right on
   a P2SH32 output, the trade contract or any other, keeps the holder it
   had, since a P2SH32 output shows only a script hash: **the seller holds a
   locked share until the claim or the refund**. A claim passes the right to
   the buyer's P2PKH output, a refund to the refund key's.
4. **Which transactions count.** Those of the shares on the paying share's
   own chain, up to and including its parent (the PPLNS anchor). Uncles'
   transactions never count.
5. **Payee.** Each window share's own weight (its work times 10, plus the
   nephew bonus for each uncle it includes) goes to `miner_bitcoin_address`
   while the holder is the share's own `miner_address` key hash, otherwise to
   the Bitcoin Cash P2PKH address of the holder's key hash, spendable with
   the key it signs share transactions with. An uncle's weight (9/10 of its
   work) always goes to the uncle's own `miner_bitcoin_address`.

The rule is a function of the share chain alone: the window, the anchor and
the transactions in the anchor's ancestry.

### Why input i to output i

Each right's destination depends only on the input that spends it. A node
tracks only the shares it can still pay (its PPLNS window cache); under a
rule where rights shift outputs by counting each other ("the k-th right goes
to output k"), a right this node never tracked could move one it does, and
nodes that started at different times would pay differently. Pairing also
caps each output at one right, so work and memory per share are linear in
its inputs, and nobody can build one output holding thousands of rights. A
proportional rule (each output gets a slice of each right, by value) would
need a list of slices per output that grows with every split.

### How a node looks a holder up

`PayoutRights` (`accounting/payout/sharechain_pplns/payout_rights.rs`) lives
in the PPLNS window cache (`PplnsWindow`) that the notifier, validator and
API share:

- **Index:** share to moved right (where, holder, owner), outpoint to share,
  and each cached share's coinbase txid to the share; a right still on its
  coinbase output has no entry. Entries leave with their share
  (`cache_capacity`, 122,270 shares on Bitcoin Cash).
- **Confirm:** `PplnsWindow::update` applies newly confirmed shares'
  transactions in height order, reading only shares whose
  `share_witness_root` is not that of an empty set. A share confirmed
  header-only more than `PRUNE_DEPTH` below the candidate tip during sync is
  applied as carrying none: it could only move rights no payout in the
  PPLNS zone reaches. A branch share's missing body is retried.
- **Reorg:** each applied share records the previous entry of each right it
  changed, kept for the newest 1,210 shares (`RIGHTS_UNDO_DEPTH`) and at most
  100,000 changed entries (`RIGHTS_UNDO_MAX_CHANGES`, about 10 MB); a deeper
  reorg reloads the cache from the store.
- **Per anchor:** an anchor at the cached tip reads the index as it is; one
  on a fork or above the tip gets an overlay (`RightsOverlay`) that undoes
  the cached shares above its fork point and applies its branch. A fork
  deeper than the undo log is dropped without a verdict
  (`RightsHistoryTooShallow`).
- **Cost:** a payout scans the moved rights and the window once. A share
  moves at most one right per input (about 5,000 inputs fit
  `BLOCK_TXS_SIZE_LIMIT`), and a node that started late places every cached
  right where a node running since genesis does.

### Consequences

- A lock of part of a share output (0.4 share in the example below) sells
  the whole right with those units; the change keeps the rest of the units
  and no right. To sell several rights, lock each share output to its own
  contract output, input `i` to output `i`.
- A wallet that merges share outputs keeps receiving their payouts: rights
  beyond the last output freeze with their holder and can no longer be sold.
- A transfer counts from the next coinbase on. A right is worth what is left
  of the share's time in the window (up to 120,960 shares, about two weeks,
  behind the 100-share maturity).

## Share transactions on the network

A Bitcoin Cash node keeps a bounded pool of signed share transactions,
relays them and mines them; a Bitcoin node has no pool. Validity rules are
on [Share transactions](../sharechain/transactions.md). Pool, relay and
inclusion are node policy, not share-chain rules.

### Pool (`shares/transaction_pool.rs`)

- **Admission:** valid on the confirmed chain at the next height under every
  share-chain rule (context-free checks, the interpreter for every input,
  P2PKH or P2SH32 outputs above dust, inputs at least outputs, finality,
  maturity, expiry). Every input must spend an unspent output of a
  confirmed transaction, so there are no unconfirmed chains; a second spend
  of an input is refused (first seen wins).
- **Limits:** 1,000 transactions and 4 MiB, each at most 50 KiB
  (`MAX_POOL_TRANSACTIONS`, `MAX_POOL_BYTES`, `MAX_POOL_TRANSACTION_SIZE`);
  a transaction expires 3 hours after arrival. Peers' new transactions are
  admitted at 10 a second on average, bursts of 100, across all peers; the
  API is not limited.
- **Priority:** the fee is the share units burned, inputs less outputs, and
  nobody receives it. Priority is fee per byte, compared exactly as a
  fraction, then arrival. A new transaction that does not fit evicts the
  lowest-priority ones only if every one ranks below it; otherwise it is
  refused ("the pool is full ..., burn more units per byte"). One burning
  nothing still gets in while there is room.
- **Confirmed and reorged:** at each new job the pool is rechecked. A
  confirmed transaction moves to a list of the last 200
  (`MAX_RECENTLY_CONFIRMED`) and returns if a reorg removes it and it is
  still valid. An invalid one is dropped; one not yet mature or final waits
  until it is ready or expires.
- **Locking:** one mutex, never held across an await, a store read or
  another lock.

### Relay

Accepted transactions go to every peer except the sender as the
`Transaction` request (message 7). A peer answers `Ack` and relays a
transaction only the first time it accepts it. The relay channel (capacity
256) drops on overflow; the transaction stays in the pool and is mined
anyway.

### Inclusion

The notifier takes pool transactions in priority order that are valid on
the mining base (the confirmed tip or a validated share above it), skipping
ones that do not fit, up to 99 transactions (`TXS_COUNT_LIMIT` less the
coinbase), 64 KiB (`MAX_SHARE_TRANSACTION_BYTES`) and 1,452 SigChecks; the
job builder then drops from the end until they and the share coinbase fit
`BLOCK_TXS_SIZE_LIMIT` (200 KiB). The set is fixed for the job: its
`share_witness_root` is in the share commitment, and a found share carries
exactly those transactions, valid on its parent. Declared Stratum V2 jobs
carry no share transactions.

### Submitting

`p2poolv2_cli bch-trade` builds transactions offline. To mine one, hand its
hex to any node:

```sh
p2poolv2_cli --config p2pool.toml transactions submit <hex>
# or
curl -u <user>:<password> -H 'Content-Type: application/json' \
  -d '{"hex":"<hex>"}' http://<api hostname>:<api port>/share_transaction
```

The node answers `{"txid": ..., "status": "accepted"}` (or
`"already_in_pool"`) and relays it, or 400 with the broken rule; a Bitcoin
node answers 503. The endpoint needs the API login even when the dashboard
is public. When the pool is full, rebuild with a higher `--fee-rate` (share
units per byte, 0 by default).

### Why a priority, not a fee rule

Upstream plans share-transaction fees paid to the share's miner
([transactions.adoc](https://github.com/p2poolv2/docs), "Handling Fees"),
rising with an output's depth up to depth 120,960
([#646](https://github.com/p2poolv2/p2poolv2/issues/646)), against spam with
outputs about to expire (#247 is about template fees, not this). Here:

- **Units carry no payout;** the payout right does. A depth scale would tax
  honest sellers of older shares, while an attacker burns fresh units as
  easily as dying ones.
- **Paying the miner** needs a fee output in the share coinbase that keeps
  its inputs' root height, and would hand out units with no right.
- **Priority bounds the attack:** keeping honest trades out costs more units
  per byte than they bid, in every share, and a trader can bid nearly all of
  an output's units without losing payout.
- A share may carry any valid transaction, fee or not, so no protocol tag
  moves. A fee rule can follow if units ever carry value (partial rights).

## The contract

The same redeem script locks share units on the share chain and BCH on
Bitcoin Cash (`shares/bch_htlc.rs`, `HtlcContract`):

```text
OP_IF
    OP_SIZE <32> OP_EQUALVERIFY
    OP_SHA256 <secret hash> OP_EQUALVERIFY
    OP_DUP OP_HASH160 <claim key hash>
OP_ELSE
    <refund height> OP_CHECKLOCKTIMEVERIFY OP_DROP
    OP_DUP OP_HASH160 <refund key hash>
OP_ENDIF
OP_EQUALVERIFY OP_CHECKSIG
```

Bytes, with the refund height as a minimal script number of `n` bytes
(1 to 4); the script is 93 + `n` bytes, 96 for any height from 32,768 to
8,388,607, locked behind a P2SH32 output:

```text
63 82 01 20 88 a8 20 <32-byte secret hash> 88 76 a9 14 <20-byte claim key hash>
67 <n> <refund height> b1 75 76 a9 14 <20-byte refund key hash> 68 88 ac

aa 20 <HASH256 of the redeem script> 87      OP_HASH256 <32 bytes> OP_EQUAL
```

| Path | Unlocking script | Spendable |
|---|---|---|
| Claim | `<signature> <public key> <32-byte secret> OP_1 <redeem script>` | at once, by the claim key, with the secret |
| Refund | `<signature> <public key> OP_0 <redeem script>` | by the refund key, in a transaction whose lock time is a height at or above the refund height, with a non-final input sequence, mined in a block or share above the refund height |

Choices, and why:

- **P2SH32, not P2SH:** both parties choose parts of the script, and a
  20-byte hash allows a collision search of about 2^80 work against 2^128.
  Wallets pay it to a `p` CashAddr with a 32-byte payload
  (`bitcoincash:p...`, version byte `0x0b`). The share chain accepts no
  P2SH20 spend.
- **A secret of exactly 32 bytes** (`OP_SIZE <32> OP_EQUALVERIFY`) stops a
  secret valid on one chain and unspendable on the other.
- **SHA256,** as wallets and upstream's design use.
- **Absolute height locks (`OP_CHECKLOCKTIMEVERIFY`),** not times or
  `OP_CHECKSEQUENCEVERIFY`: a share height counts the shares a claim could be
  mined in, so a stall in share production does not eat the buyer's window,
  and relative locks (BIP68) would need confirmation heights the share chain
  does not track in script.
- **A Bitcoin Cash lock height counts ten-minute units.** Under one-minute
  blocks (CHIP-2025-03: chipnet since block 332,451, mainnet from
  2027-05-15) a lock height `L` is final in the first block whose ticks pass
  `600 * L`, not in block `L + 1`, so "the current height plus N" opens much
  later than N blocks from now. `bch-trade refund` prints the first block a
  refund is valid in (`valid_from_height`) on either chain.
- **Key hashes:** the parties name each other by a share address
  (`p2pool:q...`) or a P2PKH CashAddr (`bitcoincash:q...`).
- **Minimal pushes and `OP_1`/`OP_0` selectors,** so the same unlocking
  script passes BCHN's minimal-data, minimal-if, push-only and clean-stack
  rules and the share chain.
- **Signatures:** `SIGHASH_FORKID` ECDSA (strict DER, low S) or Bitcoin Cash
  Schnorr over the replay-protected digest. The tools sign Schnorr with
  `SIGHASH_ALL | SIGHASH_FORKID` and BCHN's deterministic nonce (RFC6979
  with the `Schnorr+SHA256` tag), always 65 bytes, so the fee is exact.

### Covenants and introspection

The contract is a covenant that the BCH VM, and the share chain's
interpreter for the same bytes, enforce. Atomicity rests on the hash lock
and the two height locks, so two introspection uses were left out: a claim
needing no signature (the buyer can instead pre-sign the share-chain claim
with `SIGHASH_ALL`, which does not cover the unlocking script, and hand it
to a watcher who inserts the secret), and a buyer-signed early cancel
(saves the seller time, not funds). Either can come later as a second
template under a new rule tag.

## Dust

Every output a trade creates holds at least BCHN's dust threshold
(`GetDustThreshold`, `policy.cpp`): three times the fee at 1,000 satoshis
per kilobyte (`DUST_RELAY_TX_FEE`) for the output plus a 148-byte input:

- P2PKH: 3 x (34 + 148) = **546**;
- P2SH32: 3 x (44 + 148) = **576**.

On the share chain this is a rule, in share base units
(`bch_script::verify_outputs`): every non-coinbase output is P2PKH or P2SH32
at or above its threshold, and anything else, `OP_RETURN` or P2SH20
included, invalidates the share. On Bitcoin Cash, BCHN does not relay dust,
so the tools refuse it: the contract output needs 576 satoshis and a claim
or refund must leave 546 after its fee. A claim costs about 300 satoshis at
1 satoshi per byte, so the smallest practical BCH price is about 850
satoshis. Lock change below its threshold goes to the fee. The share chain
needs no fee: sell a whole output or leave at least 546 units of change.

## The trade, step by step

Example: the seller sells 0.4 share for 0.0005 BCH on chipnet, with the
share chain at height 50,000 and Bitcoin Cash at 300,000.

1. **Secret.** The seller picks 32 random bytes with their own tool
   (`openssl rand -hex 32 > secret.hex`) and prints the hash:
   `p2poolv2_cli bch-trade secret-hash --secret-file secret.hex`.
2. **Addresses.** The buyer sends a share address to receive the shares
   (`tp2pool:q...`, from `bch-share-address`) and a BCH refund address; the
   seller sends a BCH address to be paid at, a share refund address and the
   share output being sold.
3. **Contracts.** Both build both contracts and compare the redeem scripts
   (`bch-trade inspect --redeem-script <hex>` decodes one):

   ```sh
   # Share chain: the buyer claims, the seller refunds from share height 67,280.
   p2poolv2_cli bch-trade contract --chain share --network chipnet \
     --secret-hash <hash> --claim-address tp2pool:q<buyer> \
     --refund-address tp2pool:q<seller> --refund-height 67280
   # Bitcoin Cash: the seller claims, the buyer refunds from BCH height 300,072.
   p2poolv2_cli bch-trade contract --chain bch --network chipnet \
     --secret-hash <hash> --claim-address bchtest:q<seller> \
     --refund-address bchtest:q<buyer> --refund-height 300072
   ```

4. **Seller locks the shares** from a mature output (100 shares deep), the
   coin at input 0 and the contract at output 0, so the right moves into the
   contract and the seller keeps holding it:

   ```sh
   p2poolv2_cli bch-trade lock --chain share --network chipnet \
     --redeem-script <share contract> --coin <txid>:<vout>:100000000 \
     --amount 40000000 --change-address tp2pool:q<seller> --wif-file seller.wif
   p2poolv2_cli --config p2pool.toml transactions submit <lock hex>
   ```

5. **Buyer checks the share lock:** the contract output holds 40,000,000
   units under the contract's script, with at least 100 confirmations; the
   refund height is far enough ahead (Timeouts); and the output does not
   expire first. An output expires when its root coinbase falls more than
   `BCH_PAYOUT_DEPTH` (121,063) shares below the spending share, so the
   refund height plus a day of shares must stay below the root height plus
   121,063.
6. **Buyer locks BCH:** 50,000 satoshis to the BCH contract's P2SH32 address
   from any wallet that pays `p` CashAddr, or with the tool, broadcast by
   BCHN's `sendrawtransaction` or a wallet:

   ```sh
   p2poolv2_cli bch-trade lock --chain bch --network chipnet \
     --redeem-script <bch contract> --coin <txid>:<vout>:<satoshis> \
     --amount 50000 --change-address bchtest:q<buyer> --wif-file buyer.wif
   ```

7. **Seller claims the BCH** after the confirmations it wants (one for small
   amounts, six for large), publishing the secret:

   ```sh
   p2poolv2_cli bch-trade claim --chain bch --network chipnet \
     --redeem-script <bch contract> --coin <bch lock txid>:0:50000 \
     --to bchtest:q<seller> --wif-file seller.wif --secret-file secret.hex
   ```

8. **Buyer claims the shares** once the seller's claim spends the BCH
   contract (mempool or block):

   ```sh
   p2poolv2_cli bch-trade extract-secret --redeem-script <bch contract> \
     --transaction <seller's claim hex> | jq -r .secret > secret.hex
   p2poolv2_cli bch-trade claim --chain share --network chipnet \
     --redeem-script <share contract> --coin <share lock txid>:0:40000000 \
     --to tp2pool:q<buyer> --wif-file buyer.wif --secret-file secret.hex
   p2poolv2_cli --config p2pool.toml transactions submit <claim hex>
   ```

   From the share after the claim's share, every BCH block the pool finds
   while the sold share is in its window pays its weight to
   `bchtest:q<buyer>`.

Refunds: the seller runs `bch-trade refund --chain share`, valid in shares
above the share refund height, which returns the right to the seller's
refund address; the buyer runs `bch-trade refund --chain bch`, valid in
blocks above the BCH refund height. `bch-trade` prints each transaction as
hex and JSON and never broadcasts it.

## Timeouts

1. **The BCH refund comes well before the share refund, in time:** the
   seller can reveal the secret as late as the BCH refund height, and the
   buyer then needs time, even if briefly offline, to get a share-chain
   claim mined before the share refund height.
2. **Both refunds are far enough ahead** for the other side to wait for
   confirmations of the locks and claim.

| Value | Default | About |
|---|---|---|
| Share refund height | share lock height + 17,280 | 48 hours at 10-second shares |
| Share confirmations before the buyer locks BCH | 100 | 17 minutes; the coinbase maturity depth |
| BCH refund height | BCH lock height + 72 | 12 hours of ten-minute lock units |
| BCH confirmations before the seller claims | 1 to 6 | by amount |
| Gap between the refunds | at least 24 hours | |

Convert from a duration, not a block count, and check the BCH refund's
`valid_from_height`. With these defaults the share output's expiry (step 5)
leaves about eleven days of a share's fourteen-day life to sell it in.

## Failure cases

| What happens | Result | What to do |
|---|---|---|
| Seller never locks | Nothing is locked | Nothing |
| Buyer never locks BCH | Shares stay locked | Seller refunds after the share refund height |
| Seller never claims the BCH | Both locked | Each side refunds after its refund height |
| Seller claims after the BCH refund height, racing the buyer's refund | If the claim wins, the secret is public | Buyer refunds as soon as it can, and claims the shares whenever the secret appears |
| Buyer offline when the secret appears | Shares claimable until the share refund height | Claim later, or hand a watcher a pre-signed claim |
| Wrong, guessed or other-length secret | Claim invalid on both chains | None: the hash lock and `OP_SIZE` hold |
| Claim signed by another key, or redirected | Invalid | None: the claim key's signature covers the outputs |
| Refund before its height | Invalid: not final, and `OP_CHECKLOCKTIMEVERIFY` fails | Wait |
| Share-chain reorg removes the share lock after the buyer paid | The seller could double-spend the shares and still claim the BCH | Buyer waits for 100 share confirmations, more for large trades |
| BCH reorg or double spend of the buyer's lock after the secret is out | The buyer could claim the shares and keep the BCH | Seller waits for BCH confirmations before claiming |
| Share output expires before its contract | Neither party can spend it | Buyer checks the root height first (step 5) |
| Dust output | Refused by the tools; invalid on the share chain; not relayed by BCHN | Trade at least 576 units and about 850 satoshis |

## Why not Lightning

Decision 13 rules it out, and Bitcoin Cash has no Lightning Network in use.
An on-chain BCH contract costs a few hundred satoshis and confirms in one
block, and both legs are Bitcoin Cash scripts that BCHN and the share chain
run; Lightning would add channels, liquidity, routing and an always-online
node, with a hash lock outside any chain the share chain can check.

## Share-chain rule changes

On Bitcoin Cash only; Bitcoin keeps upstream's rules.

1. **Accepted spends:** P2PKH, and the contract above behind P2SH32. The
   unlocking script must be push-only with minimal pushes, its last push
   must HASH256 to the output, the redeem script must be the template byte
   for byte, and the arguments exactly the claim or the refund form; then
   the checks BCHN makes for those bytes (secret size and hash, key hash,
   signature, BIP65 lock time for a refund). Anything else is rejected
   (`shares/bch_script.rs`).
2. **Outputs:** every non-coinbase output is P2PKH or P2SH32, at or above
   its dust threshold.
3. **Finality:** a non-coinbase transaction must be final at its share's
   height: a zero lock time, all sequences final, or a height lock time
   below the share's height; a time lock time with a non-final sequence is
   never final (`validate_prevouts`).
4. **Payout to the holder:** a coinbase pays each window share's weight to
   the holder of its payout right. A payout rule: share and transaction
   validity do not change.
5. **Protocol strings** carry `trade-htlc` (rules 1 to 3) after
   `asert-parent`, then `payout-holder` (`BCH_PAYOUT_HOLDER_RULE_TAG`,
   rule 4), so nodes with and without them fail the handshake.

## Tools

| Command | Does |
|---|---|
| `p2poolv2_cli bch-trade contract` | Prints a contract for agreed terms: redeem script, locking script, and on Bitcoin Cash its P2SH32 CashAddr |
| `p2poolv2_cli bch-trade inspect` | Decodes a redeem script into its terms |
| `p2poolv2_cli bch-trade secret-hash` | Prints the SHA256 of a secret |
| `p2poolv2_cli bch-trade lock` | Signs a lock of P2PKH coins into a contract |
| `p2poolv2_cli bch-trade claim` | Signs a claim with the secret |
| `p2poolv2_cli bch-trade refund` | Signs a refund and prints `valid_from_height` |
| `p2poolv2_cli bch-trade extract-secret` | Reads the secret from a counterparty's claim |
| `p2poolv2_cli transactions submit` | Hands a signed share transaction to a node, which checks, relays and mines it (`POST /share_transaction`) |

Keys come from `--wif-file` or a hidden prompt, the secret from
`--secret-file` or a prompt, never from the command line, and the tools
generate neither. Library: `shares::bch_htlc` (template), `shares::bch_trade`
(building and signing), `shares::bch_script` (share-chain validation).

## Limits and tests

- A right is whole: selling part of a share's payout needs a proportional
  rule, which the index cannot bound. The fee market is local policy only.
- Unit tests sign real Bitcoin Cash transactions for every path (claim,
  Schnorr claim, refund at the height, wrong and short secret, early refund,
  final sequence, time lock, wrong key, redirected output, other scripts,
  dust) through the share-chain interpreter and, with the same verdicts,
  `test_utils::bch_vm`. In `p2poolv2_tests/src`, `share_trading_test.rs`
  runs a trade, a refund and an immature lock through the real store and
  validator, `share_payout_rights_test.rs` checks who is paid after a claim,
  a refund and a reorg, and `share_transaction_relay_test.rs` relays a
  transaction between two nodes.
- BCHN 29.2.0 on regtest accepted the BCH leg (the P2SH32 lock, the Schnorr
  claim, a refund at its height), refused early refunds
  (`bad-txns-nonfinal`) and forged claims
  (`mandatory-script-verify-flag-failed`), and the CLI refuses to sign
  forged ones (`WrongSecret`, `WrongKey`). Two sim nodes
  (`sim_harness run-swarm 2`) mined a trade and paid the buyer until the
  share left the window.
