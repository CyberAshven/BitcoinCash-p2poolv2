# P2Pool v2 CLI

Command-line utility for querying a P2Pool v2 node or its database
directly.

## Modes

### API mode (requires a running node)

Query the node's HTTP API. Requires a config file with the API section
(`hostname`, `port`, and optional auth credentials).

```sh
p2poolv2_cli --config /path/to/config.toml <command>
```

You can also use an environment variable to provide the config:

```sh
P2POOL_CONFIG=/path/to/config.toml p2poolv2_cli <command>
```

### Direct database mode (offline)

Query the RocksDB database directly without a running node. Pass
`--db-path` pointing to the store directory:

```sh
p2poolv2_cli --db-path /path/to/store.db <command>
```

All commands except `peers` work in this mode.

## Commands

### info

Display chain state (tip height, total work, candidate info, share count).

```sh
p2poolv2_cli info
```

### shares

Display confirmed shares and their uncles for a height range.

```sh
p2poolv2_cli shares                 # last 10 shares up to chain tip
p2poolv2_cli shares --num 20        # last 20 shares
p2poolv2_cli shares --to 500 --num 5  # 5 shares ending at height 500
p2poolv2_cli shares --num 5 --share-block-transactions  # include transactions (API mode)
```

Use `--dot` with `--db-path` to output shares as a Graphviz DOT DAG:

```sh
p2poolv2_cli --db-path /path/to/store.db shares --num 20 --dot
```

### candidates

Display candidate shares and their uncles for a height range.

```sh
p2poolv2_cli candidates             # last 10 candidates
p2poolv2_cli candidates --num 20    # last 20 candidates
p2poolv2_cli candidates --to 500 --num 5
```

Use `--dot` with `--db-path` to output candidates as a Graphviz DOT DAG:

```sh
p2poolv2_cli --db-path /path/to/store.db candidates --num 20 --dot
```

### share

Look up a single share by blockhash or height.

```sh
p2poolv2_cli share --hash <blockhash>
p2poolv2_cli share --height 42
p2poolv2_cli share --height 42 --full   # include transaction list
```

### pplns-shares

Query PPLNS shares with optional time filtering.

```sh
p2poolv2_cli pplns-shares --limit 50
p2poolv2_cli pplns-shares --limit 100 --start-time 1700000000 --end-time 1700100000
```

### peers

Peer management commands. Requires a running node (`--config`).

```sh
p2poolv2_cli peers info              # list connected peers
p2poolv2_cli peers blocked           # list blocked IPs
p2poolv2_cli peers block 1.2.3.4     # block an IP address
p2poolv2_cli peers unblock 1.2.3.4   # unblock an IP address
```

### db

Database maintenance commands. Requires `--db-path` and the node must
be stopped (RocksDB does not support concurrent access).

```sh
p2poolv2_cli --db-path /path/to/store.db db cleanup-dense-heights
```

#### cleanup-dense-heights

Walk the height index and invalidate excess HeaderValid blocks at
heights with more than 20 blocks. Retains Confirmed, Candidate, and
BlockValid blocks, plus HeaderValid blocks that are referenced as
uncles by other blocks.

### address

Print the share chain address for a taproot output key. Does not
require a running node or config file.

The input is the `witness_program` field of `bitcoin-cli
getaddressinfo`: the already tweaked 32 byte taproot output key, in hex.
No tweak is applied here, because the wallet applied it. The key is
checked to be a real curve point, so a typo cannot produce an address
whose shares nobody can ever spend.

```sh
ADDR=$(bitcoin-cli -signet getnewaddress "p2pool share owner" bech32m)
bitcoin-cli -signet getaddressinfo "$ADDR" \
  | jq -r .witness_program \
  | p2poolv2_cli address --network signet
```

The output key can also be passed as an argument instead of on stdin:

```sh
p2poolv2_cli address --network signet a3ea4c1c5f042ae7fbd086bc7f30e2a08c8b6753c4631db7d4b1fcb3ab941900
```

Networks are named as Bitcoin Core names them: `main`, `testnet4`,
`signet` or `regtest`. Check `"ismine": true` and `"solvable": true` in
the same `getaddressinfo` output; those are what tell you the wallet can
sign for the key. Use the result as `[stratum] miner_address`, or as the
stratum password `p2p=<address>`. See
`docs/architecture/address-format.md`.

### bch-trade

Trade Bitcoin Cash shares for BCH. Builds the contracts and signs the
lock, claim and refund transactions on either chain offline; does not
require a running node or config file, and never broadcasts. The protocol,
timeouts and failure cases are in `docs/atomic-swap/bch-share-trading.md`.

Every command takes `--chain share` or `--chain bch` and `--network
mainnet|chipnet|regtest`. Share-chain addresses are share addresses
(`p2pool:`, `tp2pool:`, `rp2pool:`, from `bch-share-address`) and Bitcoin
Cash addresses P2PKH CashAddr, so an address meant for one chain is refused
on the other.

```sh
# Seller: hash the secret (pick it with e.g. `openssl rand -hex 32`).
p2poolv2_cli bch-trade secret-hash --secret-file secret.hex

# Both: build each contract from the agreed terms and compare.
p2poolv2_cli bch-trade contract --chain share --network chipnet   --secret-hash <hash> --claim-address tp2pool:q<buyer>   --refund-address tp2pool:q<seller> --refund-height <share height>
p2poolv2_cli bch-trade contract --chain bch --network chipnet   --secret-hash <hash> --claim-address bchtest:q<seller>   --refund-address bchtest:q<buyer> --refund-height <bch height>
p2poolv2_cli bch-trade inspect --chain bch --network chipnet --redeem-script <hex>

# Lock coins into a contract (the seller on the share chain, the buyer on
# Bitcoin Cash, who can also pay the printed P2SH32 address from a wallet).
p2poolv2_cli bch-trade lock --chain share --network chipnet   --redeem-script <hex> --coin <txid>:<vout>:<units> --amount <units>   --change-address tp2pool:q<seller> --wif-file seller.wif

# Claim with the secret, read it from a counterparty's claim, or refund.
p2poolv2_cli bch-trade claim --chain bch --network chipnet --redeem-script <hex>   --coin <txid>:0:<sats> --to bchtest:q<seller> --wif-file seller.wif --secret-file secret.hex
p2poolv2_cli bch-trade extract-secret --redeem-script <hex> --transaction <claim hex>
p2poolv2_cli bch-trade refund --chain share --network chipnet --redeem-script <hex>   --coin <txid>:0:<units> --to tp2pool:q<seller> --wif-file seller.wif
```

Keys are WIF exports from the owner's wallet, read from `--wif-file` or a
hidden prompt and never taken on the command line; the secret is read the
same way. A mainnet key is refused on chipnet or regtest and the other way
round. Fees default to 1 satoshi per byte on Bitcoin Cash and 0 on the
share chain (`--fee-rate`). Outputs below BCHN's dust threshold (546 for
P2PKH, 576 for a contract) are refused. Share-chain transactions are not
relayed or mined by nodes yet; see the status section of the design.

### gen-auth

Generate API authentication credentials. Does not require a running
node or config file.

```sh
p2poolv2_cli gen-auth myuser           # auto-generate password
p2poolv2_cli gen-auth myuser mypass    # use provided password
```

An auto-generated password is written to a new file, `api-password.txt`
by default (`--password-file` to choose another), readable only by its
owner on Unix. It is never printed, and an existing file is never
overwritten.

## Viewing DOT output

The `--dot` flag (available on `shares` and `candidates` in `--db-path`
mode) outputs a Graphviz DOT digraph. Confirmed shares are green and
uncle blocks are orange. Each node shows the truncated blockhash,
height, miner address, difficulty, and timestamp.

### View interactively with xdot

```sh
xdot <(p2poolv2_cli --db-path /path/to/store.db shares --num 20 --dot)
```

### Render to PNG

```sh
p2poolv2_cli --db-path /path/to/store.db shares --num 20 --dot | dot -Tpng -o shares.png
```

### Render to SVG

```sh
p2poolv2_cli --db-path /path/to/store.db shares --num 20 --dot | dot -Tsvg -o shares.svg
```

## Formatting output with jq

Commands output JSON by default. When `--dot` is used, output is
Graphviz DOT instead. Use `jq` to extract fields or build tables from
JSON output.

### Shares as a tab-separated table

```sh
p2poolv2_cli shares --num 5 \
| jq -r '.shares[] | [.height, .blockhash[:8], .miner_pubkey[:8], .difficulty] | @tsv'
```

### Shares table with column headers

```sh
p2poolv2_cli shares --num 5 \
  | jq -r '["Height","Hash","Miner","Difficulty"], (.shares[] | [.height, .blockhash[:8], .miner_pubkey[:8], .difficulty]) | @tsv' \
  | column -t
```

### List uncle blockhashes for a share

```sh
p2poolv2_cli share --height 42 \
  | jq -r '.[0].uncles[]'
```

### Filter shares by miner pubkey

```sh
p2poolv2_cli shares --num 100 \
  | jq '.shares[] | select(.miner_pubkey | startswith("02aabb"))'
```

### Candidate heights only

```sh
p2poolv2_cli candidates --num 20 \
  | jq -r '.shares[].height'
```

### Peer IDs as a plain list

```sh
p2poolv2_cli peers info \
  | jq -r '.[].peer_id'
```
