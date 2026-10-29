#!/bin/sh
# Start BCHN on the network NETWORK names. POSIX shell glue.
#
# Regtest keeps the public login in bitcoin-regtest.conf: it is a throwaway
# dev chain. Chipnet and mainnet never use a known password. Their RPC
# password is, in order:
#   1. BCHN_RPC_PASSWORD, when set;
#   2. the one in $RPC_SECRET_DIR/password-$NETWORK, from an earlier start;
#   3. a new random one, written there on first start.
# The password is written to $RPC_SECRET_DIR/password-$NETWORK, which the
# compose stack mounts read-only into P2Poolv2, and to a config file in the
# data directory that only the bchn user can read. It is passed to bitcoind
# through that config file, never on the command line, and never logged.
set -eu

NETWORK=${NETWORK:-regtest}
RPC_SECRET_DIR=${RPC_SECRET_DIR:-/run/bchn-rpc}

case "$NETWORK" in
  regtest)
    exec bitcoind -conf=/etc/bchn/bitcoin-regtest.conf -datadir=/data "$@"
    ;;
  chipnet) section=chip ;;
  mainnet) section=main ;;
  *)
    echo "NETWORK must be regtest, chipnet or mainnet, not '$NETWORK'." >&2
    exit 1
    ;;
esac

umask 077
secret="$RPC_SECRET_DIR/password-$NETWORK"
if [ -n "${BCHN_RPC_PASSWORD:-}" ]; then
  password=$BCHN_RPC_PASSWORD
  origin="from BCHN_RPC_PASSWORD"
elif [ -s "$secret" ]; then
  password=$(cat "$secret")
  origin="reused"
else
  # 32 random bytes as 64 hex digits.
  password=$(od -An -tx1 -N32 /dev/urandom | tr -d ' \n')
  origin="new and random"
fi

# bitcoind refuses a "#" in rpcpassword, and a line break would end the
# config line early.
case "$password" in
  *'#'* | *'
'*)
    echo "The BCHN RPC password must not contain '#' or a line break." >&2
    exit 1
    ;;
esac

mkdir -p "$RPC_SECRET_DIR"
printf '%s\n' "$password" > "$secret.tmp"
mv "$secret.tmp" "$secret"

conf="/data/bchn-$NETWORK.conf"
{
  cat "/etc/bchn/bitcoin-$NETWORK.conf"
  printf '\n# Written at start by entrypoint.sh; see %s.\n' "$secret"
  printf '[%s]\nrpcpassword=%s\n' "$section" "$password"
} > "$conf.tmp"
mv "$conf.tmp" "$conf"

echo "BCHN RPC password for $NETWORK ($origin) is in $secret."
exec bitcoind -conf="$conf" -datadir=/data "$@"
