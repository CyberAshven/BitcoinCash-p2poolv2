---
name: bch-node-agnostic
description: Use when writing, changing or reviewing code that talks to the Bitcoin Cash node -- JSON-RPC calls in `bitcoindrpc`, templates and block submission in `stratum/work/`, ZMQ or the node P2P listener, the start-up preflight, the payout-tree executor -- or log lines, errors, metrics and docs that name the node. Keeps P2Pool working the same on BCHN, BCHD and Knuth.
---

# One pool, any Bitcoin Cash node

P2Pool is neutral software. It runs on whichever Bitcoin Cash node the
operator chose: BCHN, BCHD or Knuth. No code path works on one node only,
and no miner client gets special treatment from the pool either.

## Rules

- **A node-specific call is used only when the node offers it, with the
  standard call as the fallback.** The node tells which: an RPC answered
  with `RPC_METHOD_NOT_FOUND` (`BitcoindRpcError::is_method_not_found`), or
  a field missing from an answer (an `Option` with `#[serde(default)]`).
  Examples in `bitcoindrpc/src/lib.rs` and `stratum/work/gbt.rs`:
  `getblocktemplatelight` falls back to `getblocktemplate`, per template, so
  a node that gains or loses light templates is followed;
  `in_initial_block_download` asks for a template when `getblockchaininfo`
  has no `initialblockdownload`.
- **No node's internals are assumed.** Room for the pool's coinbase, for
  example, is whatever the template leaves: BCHN leaves room by default,
  while BCHD fills a template to 1,000 bytes below the limit unless
  `blockmaxsize` is set.
- **Text says "the node", with BCHN only as an example.** Log lines, errors,
  metric names and docs: "Error polling block template from the node", not
  "from BCHN".
- **Nothing sits between P2Pool and the node.** No script, proxy or sidecar
  in the template, submit or header path. Templates and submissions are
  latency-critical: a slow template is stale work, and a slow submit is an
  orphaned block. A missing capability is closed in P2Pool's Rust, or in the
  node itself.
- **Each node's real answer is the test input.** A parser test uses the
  exact shape a node sends, captured on regtest and named after it (the
  `KNUTH_SHAPED_LIGHT_TEMPLATE` test in `gbt.rs`). A behaviour change that
  touches the node is run against each node on regtest before it is called
  done.

## Known differences

Checked against BCHN 29.2.0, BCHD 0.22.4 and Knuth. Knuth gives no template
P2Pool can use yet, so its column is the least settled.

| Behaviour | BCHN | BCHD | Knuth |
|---|---|---|---|
| `getblocktemplatelight` | Yes | No (method not found) | Yes, but without `merkle` today, so unusable |
| `getblocktemplate` | Answers the same whatever the capabilities | Acts on the capabilities: asking for `coinbasetxn` gives no `coinbasevalue`, so the pool asks for `coinbasevalue` (BIP 22) | None, so P2Pool waits for a template and logs it once |
| `getmininginfo` `miningblocksizelimit` | Yes | No | No |
| `getblockchaininfo` `initialblockdownload` | Yes | No: a template request answers -10 while downloading, -9 without peers | |
| Network names in `chain` | `main`, `chip`, `regtest` | `mainnet`, `chipnet`, `regtest` | |
| New-block notice | ZMQ `hashblock` | No ZMQ: the node P2P listener (`node_p2p_address`) | No ZMQ: the node P2P listener |
| `sendrawtransaction` of a mempool duplicate | Accepted again | -26 `txn-already-in-mempool`, read as accepted | |
| `getblockheader` of a side-chain block | `confirmations: -1` | Internal error | |
| `getblockhash` past the tip | -8, out of range | -1 | |
| RPC transport | HTTP | TLS with a self-signed `rpc.cert` that rustls refuses (`CaUsedAsEndEntity`); plain HTTP only on localhost (`notls`) | |

An empty cell has not been checked. Fill it in when a change depends on it,
from a regtest run.

On regtest BCHD may pick P2Pool's no-services P2P listener as its sync peer;
elsewhere it syncs only from `NODE_NETWORK` peers. A regtest BCHD that
does not sync is checked for this first.

## Where this fork records it

- `docs/bch-quickstart.md`: running the pool on BCHN or BCHD.
- `docs/bch-btc-assumptions-audit.md`, item A6: light templates.
- The doc comments on `LightFallback` in `stratum/work/gbt.rs` and on
  `is_method_not_found` in `bitcoindrpc/src/lib.rs`.

A newly found difference goes into the table above and into the doc
comment of the code that handles it.
