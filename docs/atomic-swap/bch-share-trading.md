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
[htlc_scripts.md](htlc_scripts.md). This document replaces the Lightning
leg with an on-chain BCH contract and fixes the share-chain side for
Bitcoin Cash.

## In short

- The **seller** (a miner) holds share units on the share chain and picks a
  32-byte secret. The **buyer** holds BCH.
- One contract template serves both chains: a hash time locked contract
  (HTLC) behind a P2SH32 output. Whoever knows the secret and holds the
  claim key can spend it; after a refund height the refund key can take it
  back.
- The seller locks share units first, refundable late. The buyer then locks
  BCH under the same secret hash, refundable earlier. The seller claims the
  BCH, which publishes the secret on Bitcoin Cash; the buyer reads it there
  and claims the share units.
- Either both claims can happen or both refunds can: the secret that pays
  the seller is the secret that pays the buyer.
- What the buyer gets is the share's **payout right**: from the claim on,
  every BCH block the pool finds while that share is in its PPLNS window
  pays the share's weight to the buyer's key hash (see "Payout rights").
- Nodes accept, relay and mine signed share transactions (see "Share
  transactions on the network"), so a trade reaches the share chain from any
  node: `p2poolv2_cli transactions submit` or `POST /share_transaction`.

## What a trade sells

A share coinbase output pays one share unit (10^8 base units) to the
miner's P2PKH script, `miner_address` in the header. Upstream's design
([P2Poolv2 docs, payouts](https://github.com/p2poolv2/docs/tree/main/payouts))
is that the holder of a share output earns that share's PPLNS payouts, so a
buyer who bought shares collects their future coinbase payouts. Upstream
has not implemented it; on Bitcoin Cash this fork does, as the payout right
below. The units and the right are separate things: the units are
share-chain currency and stay divisible down to the dust floor, while the
right is whole and travels as one, so a trade that locks the share output
at input 0 into the contract at output 0 sells the right, whatever part of
the units it locks.

## Payout rights

On Bitcoin Cash every share carries one payout right. A coinbase that pays
the PPLNS window credits each window share's weight to whoever holds that
share's right at the paying share. Bitcoin keeps upstream's rule and pays
`miner_bitcoin_address`.

### The holder rule

1. **Origin.** A share's right starts on its coinbase output (output 0, one
   share unit), held by the share's own `miner_address` key hash.
2. **Transfer: input i to output i.** When a share transaction spends an
   output carrying a right through its input `i`, the right passes to output
   `i` of that transaction. A transaction with no output `i` leaves the right
   with its current holder, and the right can no longer move ("frozen"): it
   sits on a spent output. Each output therefore carries at most one right,
   and a merge of several share outputs into one output transfers only the
   right on input 0. The trade tools put the sold coin at input 0 and the
   contract at output 0, so a lock moves the right into the contract.
3. **Holder.** A right on a P2PKH output is held by that output's key hash.
   A right on a P2SH32 output, the trade contract or any other, keeps the
   holder it had before: **the seller holds a locked share until the claim
   or the refund**. The share chain cannot name anyone else at lock time,
   because a P2SH32 output shows only the hash of its redeem script; the
   refund key is revealed only when the refund spends it. A claim passes the
   right to the buyer's P2PKH output, a refund to the refund key's.
4. **Which transactions count.** The transactions of the shares on the
   paying share's own chain, from the share chain's start up to and
   including the paying share's parent (the PPLNS anchor, the same anchor
   the window walks from). Uncles' transactions never count: an uncle adds
   nothing to chain state, and its coinbase output can never be spent on the
   chain.
5. **Payee.** Each window share's own weight (its difficulty times 10, plus
   the nephew bonus for each uncle it includes) goes to its holder:
   - the holder is the share's own `miner_address` key hash (never
     transferred, or transferred back): `miner_bitcoin_address`, exactly as
     before;
   - any other key hash: the Bitcoin Cash P2PKH address of that key hash.
     The holder spends it with the same key it signs share transactions
     with.

   An uncle's weight (9/10 of its difficulty) always goes to the uncle's own
   `miner_bitcoin_address`.

The rule is a function of the share chain alone: the window, the anchor and
the transactions in the anchor's ancestry. No node-local state, BCH node or
configuration enters it, and mainnet, chipnet and regtest run the same code.

### Why input i to output i

Each right's destination depends only on the input that spends it, never on
the other inputs, so no right's position depends on the history of another
right. A node tracks only the shares it can still pay (its PPLNS window
cache); a rule where rights shift outputs by counting each other ("the k-th
right goes to output k") would let a right this node never tracked move a
right it does, and two nodes that started at different times would pay
differently. Pairing inputs and outputs also caps each output at one right,
so one spend moves at most one right per input: the work and memory per
share are linear in its inputs, and nobody can build one output holding
thousands of rights and spend it over and over. A proportional rule (each
output gets a slice of each right, by value) would need a list of slices
per output that grows with every split.

### How a node looks a holder up

`PayoutRights` (`accounting/payout/sharechain_pplns/payout_rights.rs`) lives
in the PPLNS window cache (`PplnsWindow`), which the notifier, the validator
and the API already share:

- **Index.** A map from share to its moved right (where the right is, its
  holder, its owner), a reverse map from outpoint to share, and a map from
  each cached share's coinbase txid to the share. A right still on its
  coinbase output has no entry. Entries are evicted with their share when it
  leaves the cache (`cache_capacity`, 122,270 shares on Bitcoin Cash), so
  the index holds at most one entry per cached share.
- **Confirm.** `PplnsWindow::update` applies each newly confirmed share's
  transactions in height order. Only shares whose header commits to share
  transactions are read from the store: `share_witness_root` differs from
  the root of an empty set. A confirmed share whose body was never fetched
  (header-only confirmation more than `PRUNE_DEPTH` below the candidate tip,
  during sync) is applied as carrying none: its transactions could only move
  rights of shares that deep, which no payout validated in the PPLNS zone
  reaches. A branch share's missing body is a read failure instead, retried
  later.
- **Partial history.** A node tracks only the shares in its cache. Because
  a right moves by its own input index alone, a right this node never
  tracked cannot shift one it does, so a node that started late places every
  cached right exactly where a node running since genesis does.
- **Reorg.** Every applied share records the previous entry of each right it
  changed. A confirmed reorg pops the shares above the fork, newest first,
  and restores those entries. Undo records are kept for up to the newest
  1,210 shares (`RIGHTS_UNDO_DEPTH`, the window's eviction buffer, so as deep
  as the window serves an anchor) and at most 100,000 changed entries in all
  (`RIGHTS_UNDO_MAX_CHANGES`, about 10 MB); past either, the oldest records go.
  A reorg deeper than the log reloads the cache, rights included, from the
  store.
- **Per anchor.** A payout anchored at the cached tip reads the index as it
  is. An anchor on a fork or above the cached tip gets an overlay: undo the
  cached shares above its fork point, then apply its branch shares. An
  anchor whose fork point lies deeper below the cached tip than the undo log
  reaches cannot be resolved and is dropped without a verdict, like a window
  truncated by eviction.
- **Cost.** A payout scans the moved rights once and the window once;
  nothing is read per window share. One share moves at most one right per
  input (at most about 5,000 inputs fit `BLOCK_TXS_SIZE_LIMIT`), so applying
  or undoing a share and the undo log are bounded too.

### Consequences

- A lock of part of a share output (0.4 share in the example below) sells
  the whole right with those units; the change keeps the rest of the units
  and no right. To sell several rights, lock each share output to its own
  contract output, input `i` to output `i`.
- A wallet that merges share outputs keeps receiving their payouts: rights
  beyond the last output freeze with their holder. They can no longer be
  sold.
- A transfer counts from the next coinbase on: the paying share's parent is
  the newest share whose transactions count. The share itself is paid only
  while it is in the window, so a right is worth what is left of the share's
  time there (up to 120,960 shares, about two weeks, behind the 100-share
  maturity).

## Share transactions on the network

A node keeps a bounded pool of signed share transactions, relays them to its
peers and puts them in the shares it builds. Bitcoin Cash only: a Bitcoin
node has no pool and ignores relayed transactions, as before.

### Pool (`shares/transaction_pool.rs`)

- **Admission.** A transaction is accepted only if it is valid on top of
  the confirmed chain at the next height, under every share-chain rule: the
  context-free checks of a share block (inputs and outputs present, no
  repeated input, values within the money supply), the Bitcoin Cash
  interpreter for every input, P2PKH or P2SH32 outputs above dust, inputs
  at least outputs, finality, coinbase maturity and output expiry. Every
  input must spend an output of a confirmed transaction that the confirmed
  chain has not spent, so the pool holds no chains of unconfirmed
  transactions. A transaction spending an input another pool transaction
  spends is refused (first seen wins).
- **Limits.** At most 1,000 transactions and 4 MiB, each at most 50 KiB
  (`MAX_POOL_TRANSACTIONS`, `MAX_POOL_BYTES`, `MAX_POOL_TRANSACTION_SIZE`).
  A transaction expires 3 hours after it arrived.
- **Priority.** A transaction's fee is the share units it burns, its inputs
  less its outputs (the share coinbase pays a fixed unit, so nobody receives
  them). Its priority is that fee per byte, compared exactly as a fraction,
  then its arrival: at equal rates the first seen ranks higher. When a new
  transaction does not fit, the lowest-priority ones are evicted, lowest
  first, but only if every one of them ranks below it; otherwise it is
  refused ("the pool is full ..., burn more units per byte") and the pool is
  unchanged. A transaction burning nothing still gets in while there is
  room. This is node policy, not a share-chain rule (see "Why a priority,
  not a fee rule" below). New transactions from peers
  are admitted at most 10 a second on average, in bursts of up to 100, across
  all peers (copies of one the pool already has are not counted);
  transactions over the API are not limited.
- **Confirmed and reorged.** Every time the node builds work it rechecks the
  pool against the confirmed chain. A transaction now on the confirmed chain
  leaves the pool for a list of the last 200 confirmed ones
  (`MAX_RECENTLY_CONFIRMED`); if a reorg takes it off the confirmed chain and
  it is still valid, it returns. A transaction that became invalid (an input
  spent by another transaction, a spent output no longer stored, an expired
  output) is dropped. One whose input is not yet mature, whose source a
  reorg took off the confirmed chain, or whose lock time is not yet reached
  waits until it is ready or expires.
- **Locking.** The pool's state sits behind one mutex that is never held
  across an await, a store read or another lock.

### Relay

Accepted transactions go to every connected peer except the one that sent
them, as the `Transaction` request (message 7) the protocol already had,
through a bounded channel the node actor drains. A peer answers with `Ack`
and relays a transaction only the first time it accepts it, so relay ends at
nodes that already have it. Per-peer request limits apply as to every
request, and the relay channel drops on overflow (capacity 256): the
transaction stays in the pool and is mined anyway.

### Inclusion

When the notifier builds work it selects pool transactions in priority
order (highest fee per byte first, then arrival) that are valid on the
mining base (the confirmed tip or a validated share
above it): inputs not spent on the confirmed chain nor by a share between
the confirmed tip and the mining base, final and mature at the share's
height. It skips a transaction that does not fit and stops adding at 99
transactions (`TXS_COUNT_LIMIT` less the coinbase), 64 KiB
(`MAX_SHARE_TRANSACTION_BYTES`) and the SigChecks budget (1,452); the job builder then
drops transactions from the end until the share's transactions, its share
coinbase and its bitcoin coinbase fit `BLOCK_TXS_SIZE_LIMIT` (200 KiB).

The chosen set is fixed for the job: its `share_witness_root` goes into the
share commitment every miner on the job hashes, the job keeps the set, and a
found share is built with exactly those transactions after its coinbase. A
share is valid if the set is valid on its parent, whenever it is found.
Declared Stratum V2 jobs carry no share transactions: their terms travel in
a 255-byte token.

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
`"already_in_pool"`) and relays it, or answers 400 with the rule the
transaction breaks. A Bitcoin node answers 503. The endpoint sits behind
the API login, also on a node whose dashboard is public. When the pool is
full the 400 says so; build the transaction again with a higher
`--fee-rate` (share units per byte, 0 by default on the share chain).

### Why a priority, not a fee rule

Upstream plans share-transaction fees that go to the share's miner
([transactions.adoc](https://github.com/p2poolv2/docs), "Handling Fees")
and rise linearly with an output's depth, from 1 unit per byte at depth 1
to an amount to be decided at depth 120,960
([#646](https://github.com/p2poolv2/p2poolv2/issues/646)), against a holder
spamming with outputs about to expire. ([#247](https://github.com/p2poolv2/p2poolv2/issues/247)
is about weighting shares by the fees of the bitcoin template they mine,
not share transactions.) On Bitcoin Cash this fork keeps the pool rule as
policy instead:

- **Units carry no payout here.** A share's BCH income follows its payout
  right, which is whole and moves input `i` to output `i`; units only pay
  fees and keep outputs above the 546-unit dust floor. A fee in units
  therefore costs every holder the same nothing in payouts, whatever its
  depth, so #646's depth scale would tax honest sellers of older shares
  without raising an attacker's cost: the attacker can burn fresh units as
  easily as dying ones.
- **Paying it to the share's miner** needs a fee output in the canonical
  share coinbase that keeps its inputs' root height (or fees would turn
  expiring units into fresh ones), a change to the store's root-height
  bookkeeping, and it would give miners units with no right attached.
- **What bounds the attack is priority.** Shares carry the highest fee rate
  first, so keeping honest trades out costs the attacker more units per
  byte than the honest trades bid, for every transaction in every share,
  while a trader can bid up to nearly all of an output's units without
  losing any payout.
- **Policy, not consensus.** Admission, eviction and selection are this
  node's; a share may carry any valid transaction, with or without a fee,
  so nothing changes for validity and no protocol tag moves. A
  consensus fee (minimum, depth scale or recipient) can follow if units
  ever carry value, for instance with partial payout rights.

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
(1 to 4):

```text
63 82 01 20 88 a8 20 <32-byte secret hash> 88 76 a9 14 <20-byte claim key hash>
67 <n> <refund height> b1 75 76 a9 14 <20-byte refund key hash> 68 88 ac
```

The script is 93 + `n` bytes: 96 for any height from 32,768 to 8,388,607.
It is locked behind a P2SH32 output:

```text
aa 20 <HASH256 of the redeem script> 87      OP_HASH256 <32 bytes> OP_EQUAL
```

| Path | Unlocking script | Spendable |
|---|---|---|
| Claim | `<signature> <public key> <32-byte secret> OP_1 <redeem script>` | at once, by the claim key, with the secret |
| Refund | `<signature> <public key> OP_0 <redeem script>` | by the refund key, in a transaction whose lock time is a height at or above the refund height, with a non-final input sequence, mined in a block or share above the refund height |

Choices, and why:

- **P2SH32, not P2SH.** Both parties choose parts of the script. A 20-byte
  script hash lets a party who chooses its own parameters search for a
  collision with about 2^80 work; P2SH32 (Bitcoin Cash, May 2023) raises
  that to 2^128. BCHN treats P2SH32 as standard, and wallets pay it to a
  `p` CashAddr with a 32-byte payload (`bitcoincash:p...`, version byte
  `0x0b`). The share chain accepts no P2SH20 spend.
- **The secret is exactly 32 bytes.** `OP_SIZE <32> OP_EQUALVERIFY` stops a
  secret that is valid on one chain and unspendable on the other, the
  known cross-chain swap attack where the secret holder picks a preimage
  one chain cannot push.
- **SHA256.** One hash, the one wallets and upstream's design use.
- **Absolute height locks (`OP_CHECKLOCKTIMEVERIFY`), not times or
  `OP_CHECKSEQUENCEVERIFY`.** On the share chain a height counts the shares
  in which a claim could be mined, so an outage in share production does
  not eat the buyer's window, where a time lock would expire during it.
  Relative locks would need each output's confirmation height inside script
  validation (BIP68), which the share chain does not track. Bitcoin Cash
  uses heights too, so both refunds are read the same way.
- **Key hashes, not public keys.** The parties name each other by the
  addresses they already have: a share address (`p2pool:q...`) or a P2PKH
  CashAddr (`bitcoincash:q...`), both a HASH160 of a key.
- **Minimal pushes and `OP_1`/`OP_0` selectors.** These are what Bitcoin
  Cash's minimal-data, minimal-if, push-only and clean-stack rules require,
  so the same unlocking script passes BCHN and the share chain.
- **Signatures.** `SIGHASH_FORKID` ECDSA (strict DER, low S) or Bitcoin Cash
  Schnorr, over the replay-protected digest with the redeem script as the
  script code, as for any Bitcoin Cash P2SH spend. The tools sign Bitcoin
  Cash Schnorr with `SIGHASH_ALL | SIGHASH_FORKID`, with BCHN's
  deterministic nonce (RFC6979 with the `Schnorr+SHA256` tag), so the same
  key and transaction give the signature BCHN's wallet would. A Schnorr
  signature is always 65 bytes with its sighash byte, so the fee is sized
  exactly before signing.

### Covenants and introspection

Decision 13 says the trade goes through BCH covenants. The contract above
is one: a P2SH32 contract that the BCH VM, and the share chain's
interpreter for the same bytes, enforce. Native introspection (BCH, May
2022) was considered for two jobs:

1. A claim that needs no signature and pins its output to the claimer's
   P2PKH script (`OP_INPUTINDEX OP_OUTPUTBYTECODE ... OP_EQUALVERIFY`,
   `OP_OUTPUTVALUE` against `OP_UTXOVALUE`), so anyone could complete a
   claim once the secret is public.
2. A cancel signed by the buyer alone that can only pay the seller back,
   so the seller need not wait for the refund height when a buyer walks
   away.

Neither is needed for atomicity, which rests on the hash lock and the two
height locks. The first is available without new opcodes: a signature
does not cover the unlocking script, so the buyer can sign the share-chain
claim in advance with `SIGHASH_ALL` and hand it, without the secret, to any
watcher, who inserts the secret when it appears on Bitcoin Cash. The
second saves the seller time, not funds, and costs seven more opcodes and
script arithmetic in the share-chain interpreter. Both can be added later
as a second template under a new rule tag.

## Dust

Every output a trade creates, on either chain, holds at least BCHN's dust
threshold for that output:

- BCHN's `GetDustThreshold` (`policy.cpp`) is three times the fee, at the
  dust relay fee, for the output's serialized size plus a 148-byte input to
  spend it. The default dust relay fee is 1,000 satoshis per kilobyte
  (`DUST_RELAY_TX_FEE`), so the threshold is 3 x (output size + 148):
  - P2PKH: 3 x (34 + 148) = **546**;
  - P2SH32: 3 x (44 + 148) = **576**.
- **Share chain (consensus).** Every output of a non-coinbase share
  transaction must be P2PKH or P2SH32 and hold at least its threshold, in
  share base units (`bch_script::verify_outputs`). Anything else, an
  `OP_RETURN` or P2SH20 output included, invalidates the share. A share
  coinbase pays one whole share unit, 10^8 base units.
- **Bitcoin Cash (policy).** BCHN does not relay a transaction with a dust
  output. The tools refuse to build one: the contract output needs at least
  576 satoshis, and a claim or refund must leave at least 546 after its fee.
  With the default 1 satoshi per byte a claim costs about 300 satoshis, so
  the smallest practical BCH price is about 850 satoshis.

Lock change below its threshold is added to the fee, as Bitcoin Cash
wallets do. The share chain needs no fee, so sell a whole output or leave at
least 546 units of change there; pay one (`--fee-rate`, share units per
byte, burned) only when a node's pool is full.

## The trade, step by step

Example: the seller sells 0.4 share for 0.0005 BCH on chipnet. The share
chain is at height 50,000 and Bitcoin Cash at height 300,000.

1. **Secret.** The seller picks 32 random bytes from their own tool, for
   example `openssl rand -hex 32 > secret.hex`, and prints its hash:
   `p2poolv2_cli bch-trade secret-hash --secret-file secret.hex`.
2. **Addresses.** The buyer sends a share address to receive the shares
   (`tp2pool:q...`, from `bch-share-address`) and a BCH refund address. The
   seller sends a BCH address to be paid at and a share address to refund
   to. The seller also names the share output being sold.
3. **Contracts.** Both build both contracts from the agreed terms and
   compare the redeem scripts. Same terms, same bytes.

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

   `bch-trade inspect --redeem-script <hex>` decodes a script a
   counterparty sends.
4. **Seller locks the shares.** From a mature share output (100 shares
   deep). The coin is input 0 and the contract output 0, so the lock moves
   the share's payout right into the contract, where the seller keeps
   holding it:

   ```sh
   p2poolv2_cli bch-trade lock --chain share --network chipnet \
     --redeem-script <share contract> --coin <txid>:<vout>:100000000 \
     --amount 40000000 --change-address tp2pool:q<seller> --wif-file seller.wif
   p2poolv2_cli --config p2pool.toml transactions submit <lock hex>
   ```

5. **Buyer checks the share lock.** The contract output exists with the
   contract's locking script and 40,000,000 units; it has at least 100 share
   confirmations; the refund height is far enough ahead (see Timeouts); and
   the output does not expire first. An output expires when its root
   coinbase falls more than `PAYOUT_DEPTH` (127,008) shares below the
   spending share, so the refund height plus a day of shares must stay
   below the root height plus 127,008.
6. **Buyer locks BCH.** Pay 50,000 satoshis to the BCH contract's P2SH32
   address from any wallet that pays `p` CashAddr, or from a P2PKH coin:

   ```sh
   p2poolv2_cli bch-trade lock --chain bch --network chipnet \
     --redeem-script <bch contract> --coin <txid>:<vout>:<satoshis> \
     --amount 50000 --change-address bchtest:q<buyer> --wif-file buyer.wif
   ```

   and broadcast the hex with BCHN's `sendrawtransaction` or a wallet.
7. **Seller claims the BCH** once the BCH lock has the confirmations the
   seller wants (one for small amounts, six for large ones). This publishes
   the secret.

   ```sh
   p2poolv2_cli bch-trade claim --chain bch --network chipnet \
     --redeem-script <bch contract> --coin <bch lock txid>:0:50000 \
     --to bchtest:q<seller> --wif-file seller.wif --secret-file secret.hex
   ```

8. **Buyer claims the shares.** The buyer watches the BCH contract output.
   When the seller's claim spends it (in the mempool or a block), the buyer
   reads the secret and claims on the share chain:

   ```sh
   p2poolv2_cli bch-trade extract-secret --redeem-script <bch contract> \
     --transaction <seller's claim hex> | jq -r .secret > secret.hex
   p2poolv2_cli bch-trade claim --chain share --network chipnet \
     --redeem-script <share contract> --coin <share lock txid>:0:40000000 \
     --to tp2pool:q<buyer> --wif-file buyer.wif --secret-file secret.hex
   p2poolv2_cli --config p2pool.toml transactions submit <claim hex>
   ```

   From the share after the claim's share, every BCH block the pool finds
   while the sold share is in its window pays that share's weight to
   `bchtest:q<buyer>`, the P2PKH address of the buyer's key hash.

Refunds, if the trade stops: the seller runs `bch-trade refund --chain
share`, valid in shares above the share refund height, which returns the
payout right to the seller's refund address; the buyer runs `bch-trade
refund --chain bch`, valid in blocks above the BCH refund height. Every
`bch-trade` command prints the transaction as hex and JSON and never
broadcasts it; `transactions submit` hands a share transaction to a node.

## Timeouts

Two rules make the trade safe for both sides:

1. **The BCH refund comes well before the share refund, in time.** The
   seller can reveal the secret as late as the BCH refund height. From then
   the buyer needs enough time to see the secret and get a share-chain claim
   mined before the share refund height. The gap covers the buyer being
   briefly offline and slow share-chain inclusion.
2. **Both refunds are far enough ahead** for the other side to confirm the
   locks and claim: the buyer waits for share confirmations before locking
   BCH, and the seller waits for BCH confirmations before claiming.

Recommended defaults, taken from the heights at the time of each lock:

| Value | Default | About |
|---|---|---|
| Share refund height | share lock height + 17,280 | 48 hours at 10-second shares |
| Share confirmations before the buyer locks BCH | 100 | 17 minutes; the coinbase maturity depth |
| BCH refund height | BCH lock height + 72 | 12 hours at 10-minute blocks |
| BCH confirmations before the seller claims | 1 to 6 | by amount |
| Gap between the refunds | at least 24 hours | |

The share chain targets 10-second shares and BCH 10-minute blocks. Convert
from a duration, not a fixed block count: if CHIP-2025-03's one-minute
blocks activate, 12 hours is 720 BCH blocks. The share output being sold
must also not expire before its contract does (step 5), which leaves about
eleven days of a share's fourteen-day life to sell it in with these
defaults.

## Failure cases

| What happens | Result | What to do |
|---|---|---|
| Seller never locks | Nothing is locked | Nothing |
| Buyer never locks BCH | Shares stay locked | Seller refunds after the share refund height |
| Seller never claims the BCH | Both locked | Buyer refunds BCH after the BCH refund height; seller refunds shares after theirs |
| Seller claims after the BCH refund height, racing the buyer's refund | If the claim wins, the secret is public | Buyer refunds the moment the BCH refund height passes, and claims the shares whenever the secret appears |
| Buyer offline when the secret appears | Shares still claimable until the share refund height | Claim later, or hand a watcher a pre-signed claim (see Covenants) |
| Wrong or guessed secret | Claim invalid on both chains | None: the hash lock holds |
| Secret of another length | Claim invalid on both chains | None: `OP_SIZE` pins 32 bytes |
| Claim signed by another key, or redirected | Invalid | None: the claim key's signature covers the outputs |
| Refund before its height | Invalid: not final, and `OP_CHECKLOCKTIMEVERIFY` fails | Wait |
| Share-chain reorg removes the share lock after the buyer paid | The seller could double-spend the shares and still claim the BCH | Buyer waits for 100 share confirmations, more for large trades |
| BCH reorg or double spend of the buyer's lock after the seller revealed the secret | The buyer could claim the shares and keep the BCH | Seller waits for BCH confirmations before claiming |
| Share output expires before its contract | Neither party can spend it | Buyer checks the root height before paying (step 5) |
| Dust output | Refused by the tools; invalid on the share chain; not relayed by BCHN | Trade at least 576 units and about 850 satoshis |

## Why not Lightning

- Decision 13 rules it out, and Bitcoin Cash has no Lightning Network in
  use to route payments over.
- It is not needed: an on-chain BCH contract costs a few hundred satoshis at
  1 satoshi per byte and confirms in one block, and blocks are not full.
- It would add channels, liquidity, routing, invoice expiry and an
  always-online node to a trade that two on-chain contracts settle, and its
  hash lock would sit outside any chain the share chain's own interpreter
  can check.
- Both legs here are Bitcoin Cash scripts: BCHN runs one, and the share
  chain runs the same bytes through its interpreter.

## Share-chain consensus changes

On Bitcoin Cash only; Bitcoin keeps upstream's rules. They change share
validity, so nodes restart on a fresh store as for decisions 11 and 12.

1. **Accepted spends.** P2PKH as before, and the contract above behind
   P2SH32: the unlocking script must be push-only with minimal pushes, its
   last push must hash (HASH256) to the output, the redeem script must be
   byte for byte the template, and the arguments must be exactly the claim
   or the refund form. The interpreter then makes the checks BCHN makes for
   those bytes: secret size and hash, key hash, signature over the redeem
   script, and for a refund BIP65's lock time rules. Everything else is
   rejected (`shares/bch_script.rs`).
2. **Outputs.** Every non-coinbase output is P2PKH or P2SH32 and at or
   above its dust threshold.
3. **Finality.** A non-coinbase transaction must be final at the height of
   the share that includes it: a zero lock time, all sequences final, or a
   height lock time below the share's height. A time lock time with a
   non-final sequence is never final on the share chain
   (`validate_prevouts`).
4. **Protocol string.** Bitcoin Cash protocol strings gain `trade-htlc`
   after `asert-parent`, so nodes with and without these rules fail the
   handshake instead of exchanging shares the other rejects.
5. **Payout to the holder** (added after the trading rules). A coinbase pays
   each window share's weight to the holder of its payout right (see
   "Payout rights"). Every node rebuilds every coinbase from the window, so
   this is a payout rule like decision 11's window: a node on the old rule
   rebuilds another coinbase once any right has moved. Share and transaction
   validity do not change.
6. **Protocol string.** Bitcoin Cash protocol strings gain `payout-holder`
   after `trade-htlc` (`BCH_PAYOUT_HOLDER_RULE_TAG`).

## Tools

| Command | Does |
|---|---|
| `p2poolv2_cli bch-trade contract` | Prints a contract for agreed terms: redeem script, locking script, and on Bitcoin Cash its P2SH32 CashAddr |
| `p2poolv2_cli bch-trade inspect` | Decodes a redeem script into its terms |
| `p2poolv2_cli bch-trade secret-hash` | Prints the SHA256 of a secret |
| `p2poolv2_cli bch-trade lock` | Signs a lock of P2PKH coins into a contract |
| `p2poolv2_cli bch-trade claim` | Signs a claim with the secret |
| `p2poolv2_cli bch-trade refund` | Signs a refund after the refund height |
| `p2poolv2_cli bch-trade extract-secret` | Reads the secret from a counterparty's claim |
| `p2poolv2_cli transactions submit` | Hands a signed share transaction to a node, which checks, relays and mines it (`POST /share_transaction`) |

Keys come from a WIF file (`--wif-file`) or a hidden prompt, and the secret
from `--secret-file` or a prompt; neither is ever taken on the command line
or generated. The library side is `shares::bch_htlc` (template),
`shares::bch_trade` (transaction building and signing) and
`shares::bch_script` (share-chain validation).

## Status and limits

- **Relay and mining** of share transactions and **payout to the holder**
  are implemented (see "Payout rights" and "Share transactions on the
  network") and tested in process: a claim moves a share's payout to the
  buyer's key hash in the next coinbase, a refund keeps it with the seller,
  a reorg that removes a claim gives the payout back, an anchor sees the same
  holders whatever the window's cached tip, a Bitcoin window keeps paying
  the miner, and a transaction submitted to one node reaches another over the
  real P2P stack (`p2poolv2_tests/src/share_payout_rights_test.rs`,
  `share_transaction_relay_test.rs`). A job built with pool transactions
  commits to them and its found share passes the coinbase proof and body
  checks (`handle_stratum_share`). Not yet run on chipnet, and no live node
  has mined a share transaction yet.
- **Partial rights.** A right is whole. Selling part of a share's payout
  needs a proportional rule, which the index above cannot bound (see "Why
  input i to output i").
- **Fee market.** The pool ranks by share units burned per byte and evicts
  the lowest (see "Priority"). It is local policy: there is no consensus
  minimum fee, no fee recipient and no depth scale (see "Why a priority,
  not a fee rule").
- **Evidence.** Unit tests sign real Bitcoin Cash transactions for every
  path (claim, Schnorr claim, refund at the height, wrong and short secret,
  early refund, final sequence, time lock, wrong key, redirected output,
  other scripts, dust) through the share-chain interpreter. The same spends
  also run on a small test-only stack machine (`test_utils::bch_vm`) that
  executes the contract's bytes opcode by opcode, and both verdicts agree.
  End-to-end tests (`p2poolv2_tests/src/share_trading_test.rs`) run a
  whole trade, a refund and an immature lock through the real store and
  validator. BCHN itself has not executed these scripts yet; a regtest run
  with `testmempoolaccept` should precede mainnet use.
