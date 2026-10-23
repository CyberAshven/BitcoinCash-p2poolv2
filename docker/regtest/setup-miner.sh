#!/bin/sh
# Give the regtest CPU miner its Stratum identity. POSIX shell glue; it runs
# in the p2poolv2 image, talks to BCHN's JSON-RPC with curl and converts the
# share key's address with p2poolv2_cli.
#
# It takes two fresh addresses from the BCHN wallet "miner": one receives the
# miner's PPLNS payouts, the other's key owns its shares. Neither key leaves
# BCHN's wallet, and nothing here makes a key. It writes
#   MUJINA_POOL_USER=<bchreg: payout address>.<worker>
#   MUJINA_POOL_PASS=p2p=<rp2pool: share address>
# to $OUTPUT, which the miner service reads at start. An existing file is
# kept, so the miner keeps its identity across restarts.
set -eu

RPC_URL=${RPC_URL:-http://bchn:18443}
RPC_USER=${RPC_USER:-p2pool}
RPC_PASSWORD=${RPC_PASSWORD:-p2pool}
OUTPUT=${OUTPUT:-/regtest/miner.env}
WORKER=${WORKER:-cpu}

if [ -s "$OUTPUT" ]; then
  echo "Keeping the miner identity in $OUTPUT:"
  cat "$OUTPUT"
  exit 0
fi

# rpc METHOD PARAMS [PATH]: print the "result" of a JSON-RPC call.
rpc() {
  curl -sS --fail-with-body --user "$RPC_USER:$RPC_PASSWORD" \
    -H 'content-type: text/plain;' \
    --data-binary "{\"jsonrpc\":\"1.0\",\"id\":\"setup\",\"method\":\"$1\",\"params\":$2}" \
    "$RPC_URL${3:-}"
}

# string_result: the string "result" of a JSON-RPC reply on stdin.
string_result() {
  sed -n 's/.*"result":"\([^"]*\)".*/\1/p'
}

until rpc getblockcount '[]' > /dev/null 2>&1; do
  echo "Waiting for BCHN RPC at $RPC_URL..."
  sleep 1
done

rpc loadwallet '["miner"]' > /dev/null 2>&1 || true

payout=$(rpc getnewaddress '["payout"]' /wallet/miner | string_result)
owner=$(rpc getnewaddress '["share-owner"]' /wallet/miner | string_result)
if [ -z "$payout" ] || [ -z "$owner" ]; then
  echo "Could not get addresses from the BCHN wallet \"miner\"." >&2
  exit 1
fi

# The CLI logs to stdout; keep only the address it prints.
share=$(RUST_LOG=off p2poolv2_cli bch-share-address --network regtest "$owner")
case "$share" in
  rp2pool:*) ;;
  *) echo "p2poolv2_cli did not return an rp2pool: address: $share" >&2; exit 1 ;;
esac

umask 022
{
  echo "MUJINA_POOL_USER=$payout.$WORKER"
  echo "MUJINA_POOL_PASS=p2p=$share"
} > "$OUTPUT"

echo "Miner identity written to $OUTPUT:"
cat "$OUTPUT"
