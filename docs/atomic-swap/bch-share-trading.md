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

## What a share unit is worth today

A share coinbase output pays one share unit (10^8 base units) to the
miner's P2PKH script, `miner_address` in the header. Upstream's design
([P2Poolv2 docs, payouts](https://github.com/p2poolv2/docs/tree/main/payouts))
is that the holder of a share output earns that share's PPLNS payouts, so a
buyer who bought shares collects their future coinbase payouts. That rule
is not implemented upstream or here: a found block pays each share's
`miner_bitcoin_address` ([payout-distribution.md](../architecture/payout-distribution.md),
"Payee is `miner_bitcoin_address`"). Until a payout-to-holder rule exists,
a trade moves share-chain units, not the BCH income behind them. The
trading mechanism below does not depend on that rule and does not change
when it arrives.

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
  script code, as for any Bitcoin Cash P2SH spend. The tools sign ECDSA
  with `SIGHASH_ALL | SIGHASH_FORKID`.

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
wallets do. The share chain has no fee, so sell a whole output or leave at
least 546 units of change there.

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
   deep):

   ```sh
   p2poolv2_cli bch-trade lock --chain share --network chipnet \
     --redeem-script <share contract> --coin <txid>:<vout>:100000000 \
     --amount 40000000 --change-address tp2pool:q<seller> --wif-file seller.wif
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
   ```

Refunds, if the trade stops: the seller runs `bch-trade refund --chain
share`, valid in shares above the share refund height; the buyer runs
`bch-trade refund --chain bch`, valid in blocks above the BCH refund
height. Every command prints the transaction as hex and JSON and never
broadcasts it.

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

Keys come from a WIF file (`--wif-file`) or a hidden prompt, and the secret
from `--secret-file` or a prompt; neither is ever taken on the command line
or generated. The library side is `shares::bch_htlc` (template),
`shares::bch_trade` (transaction building and signing) and
`shares::bch_script` (share-chain validation).

## Status and limits

- **Share transactions are not relayed or mined yet.** Every node builds
  its shares with the coinbase alone (`handle_stratum_share`, upstream's
  open TODO), so the share-chain leg can be built, signed and checked, and
  the in-process tests mine it, but no live node includes it yet. That needs
  a share-transaction pool that the work notify path draws from and
  revalidates on every new parent, a separate change.
- **Payout to the holder of a share output** is upstream's design and not
  implemented (see "What a share unit is worth today").
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
