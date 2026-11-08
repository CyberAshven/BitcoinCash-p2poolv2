# SV2 extension 0x4C54: BCH large templates

**Status: approved by the owner on 2026-11-05. The pool side is built
(`stratum::sv2::large_templates`); Knuth's client side is on its branch
`sv2/jd-large-templates`. Not yet run end to end between the two.** It
changes no share-chain rule and no standard SV2 message. A pool or client
without it never sees it.

## 1. Why

Standard Job Declaration was sized for Bitcoin:

- `DeclareMiningJob.wtxid_list` is a `SEQ0_64K`: at most 65,535
  transactions.
- `ProvideMissingTransactions.unknown_tx_position_list` is
  `SEQ0_64K[U16]`: no position above 65,535 can be named.
- Every SV2 message has a `U24` length, so one
  `ProvideMissingTransactions.Success` carries at most `2^24 - 1` bytes.

A Bitcoin Cash block may be 32 MB, the floor of its adaptive block size
limit (ABLA), and larger as ABLA raises it. With typical BCH transactions
65,535 of them come to about 20 MB. A node that asks for every transaction
in one reply tops out at about 16 MiB. Both are below the 32 MB floor.
ASICs are not affected: they only hash the 80-byte header.

## 2. Negotiation

- Extension id `0x4C54` ("LT"), experimental, on a Job Declaration
  connection. The client asks for it with `RequestExtensions` (`0x0001`)
  right after `SetupConnection.Success`, together with `0x5052` when it
  wants both.
- Only in Full-Template mode (`DECLARE_TX_DATA` set). A Coinbase-Only
  connection does not ask for it, and a P2Pool node does not grant it
  there.
- The node answers `RequestExtensions.Success` with the extensions it
  grants. Knuth turns an extension on only when the Success lists it, and
  a `RequestExtensions.Error` would turn every extension it asked for off.
  The node answers `RequestExtensions.Error` only when it grants none of
  them, as `0x0001` requires, which leaves Knuth with none, as before.
- The extension's messages have the channel bit clear, little-endian
  fields, and `extension_type` `0x4C54` in the frame header.
- A `0x4C54` message on a connection that did not negotiate it closes the
  connection, as any unknown extension does.

## 3. Messages

**`0x01` `DeclareMiningJob.Transactions`** (client to node)

```
request_id U32 | txid_list SEQ0_64K[U256]
```

- Zero or more pages, each sent **before** the `DeclareMiningJob` with
  the same `request_id`. A page carries at most 65,535 ids.
- The declared list is every page in the order sent, then
  `DeclareMiningJob.wtxid_list`. The core `DeclareMiningJob` ends the list.
- A template of at most 65,535 transactions sends no page, so its
  declaration is byte for byte standard.
- On Bitcoin Cash the ids are txids in the template's CTOR order, as in
  the core message.

**`0x02` `ProvideMissingTransactions.Positions`** (node to client)

```
request_id U32 | unknown_tx_position_list SEQ0_64K[U32]
```

- It means the same as the core `ProvideMissingTransactions`, but its
  positions are `U32`. They index the whole declared list, pages first,
  from 0, without the coinbase.
- The client answers with the core `ProvideMissingTransactions.Success`.

## 4. Rules

1. **Partial replies.** On a connection that negotiated `0x4C54`, when the
   requested transactions do not all fit one frame, the client's
   `ProvideMissingTransactions.Success` carries the longest prefix of the
   requested list that fits. The node asks for the rest with another
   request. BCH caps a transaction at 1,000,000 bytes (BCHN `consensus.h`
   `MAX_TX_SIZE`), so every reply holds at least one transaction and the
   exchange always moves on.
2. **One request at a time.** The node has at most one outstanding
   missing-transactions request per declaration, so each Success answers
   the latest request.
3. **Either request form.** The core `U16` request stays valid while every
   position fits a `U16`. Past that the node uses `0x02`, in batches of at
   most 65,535 positions.
4. **Bounds on the node.** A declaration lists at most `size_limit / 65`
   transactions. 65 bytes is BCH's smallest transaction since Upgrade 9
   (BCHN `MIN_TX_SIZE_UPGRADE9`), and `size_limit` is the block size limit
   of the template the token was allocated against (BCHN's `sizelimit`, so
   ABLA's limit), not a fixed number. Pages beyond that bound, or for a
   `request_id` that already has its `DeclareMiningJob`, close the
   connection.
5. **Bounds on the client.** Knuth keeps the template's transactions for
   its replies, bounded by the template's own size limit.

## 5. How a P2Pool node answers

- `Sv2Connection::grant_large_templates` grants the extension to a
  Full-Template Job Declaration connection on a Bitcoin Cash node. The
  listener grants it next to `0x5052` in one `RequestExtensions.Success`.
- Pages are buffered by `request_id`, for at most eight request ids at a
  time per connection. They are bounded by `size_limit / 65` ids at the
  published template's size limit. `DeclareMiningJob` takes them, whatever
  its answer.
- The node holds no transactions of its own, so it asks for all of them, a
  batch of at most 65,535 at a time, in order. It uses the core request
  while the batch's positions fit a `U16`, and `0x02` past that.
- A reply may be a prefix of the batch (at least one transaction) only on
  a connection that negotiated `0x4C54`. Without it, a reply must hold the
  whole batch, as before. Each transaction must match its declared id.
  When the whole list has arrived, it is sorted into CTOR and the token is
  ready, as for a standard declaration.
- Declared transaction memory, in waiting and ready tokens, is bounded per
  registry by two blocks at the published template's size limit, and at
  least 64 MiB, so the bound follows ABLA. It replaces the fixed 64 MiB
  this node had before. A declaration's revealed transactions may not pass
  its own template's size limit. The same bound applies to the
  declarations kept for `PushSolution`.

## 6. Without the extension

Standard behaviour is unchanged. When a template cannot be declared within
standard limits, Knuth logs why and mines that one template solo; it never
sends a malformed message. Coinbase-Only declarations have no such limit:
the pool gets a merkle path, not the transaction list.
