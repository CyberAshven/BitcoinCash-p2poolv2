#!/bin/sh
# Healthy once BCHN can hand out block templates.
#
# Regtest: the bootstrap has mined REGTEST_INITIAL_BLOCKS blocks, so the chain
# is past its first heights and the dev wallet has mature coins.
# Chipnet and mainnet: the node has left initial block download, before which
# getblocktemplate answers "Bitcoin is downloading blocks...".
set -eu

if [ "${NETWORK:-regtest}" = "regtest" ]; then
  height=$(bcli getblockcount)
  [ "$height" -ge "${REGTEST_INITIAL_BLOCKS:-101}" ]
else
  bcli getblockchaininfo | grep -q '"initialblockdownload": false'
fi
