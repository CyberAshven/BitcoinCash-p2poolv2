#!/bin/sh
# One-shot bootstrap for the regtest dev chain. POSIX shell glue; it only
# drives bitcoin-cli.
#
# On regtest it creates the wallet "miner" and mines REGTEST_INITIAL_BLOCKS
# blocks to it with generatetoaddress, so the chain is past its first heights
# and the wallet holds mature coins (coinbase outputs mature after 100 blocks)
# for share-trading and transaction tests. On chipnet and mainnet it does
# nothing: those chains come from the network.
set -eu

NETWORK=${NETWORK:-regtest}
TARGET_HEIGHT=${REGTEST_INITIAL_BLOCKS:-101}
RPC_HOST=${RPC_HOST:-bchn}

if [ "$NETWORK" != "regtest" ]; then
  echo "NETWORK=$NETWORK: nothing to bootstrap."
  exit 0
fi

cli() {
  bitcoin-cli -conf=/etc/bchn/bitcoin-regtest.conf -rpcconnect="$RPC_HOST" "$@"
}

echo "Waiting for BCHN RPC at $RPC_HOST..."
until cli getblockcount > /dev/null 2>&1; do
  sleep 1
done

cli createwallet miner > /dev/null 2>&1 || cli loadwallet miner > /dev/null 2>&1 || true

height=$(cli getblockcount)
if [ "$height" -ge "$TARGET_HEIGHT" ]; then
  echo "Regtest chain at height $height, nothing to mine."
  exit 0
fi

address=$(cli -rpcwallet=miner getnewaddress bootstrap)
needed=$((TARGET_HEIGHT - height))
echo "Mining $needed regtest blocks to $address..."
cli generatetoaddress "$needed" "$address" > /dev/null
echo "Regtest chain at height $(cli getblockcount)."
