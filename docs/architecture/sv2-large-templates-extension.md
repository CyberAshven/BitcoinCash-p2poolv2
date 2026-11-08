# SV2 extension 0x4C54: BCH large templates

**Status: approved by the owner on 2026-11-05. The pool side is built
(`stratum::sv2::large_templates`); Knuth's client side is on its branch
`sv2/jd-large-templates`. Not yet run end to end between the two.** It
changes no share-chain rule and no standard SV2 message. A pool or client
without it never sees it.

**Template Distribution (section 7): the P2Pool client is built
(`stratum::sv2::template_distribution`); Knuth's Template Provider is being
written to the same wire contract. Not yet run end to end between the
two.**

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

## 7. Template Distribution

Standard Template Distribution has the same limits: one
`RequestTransactionData.Success` lists at most 65,535 transactions in one
`U24` frame. With the extension a Template Provider pushes every template's
transactions in pages, and a template that shares transactions with the last
one pushed comes as a delta. No transaction count or frame limits a template
then, only the block size limit, and each transaction travels once per
connection. The client never has to ask.

### Negotiation

- After `SetupConnection.Success` for protocol 2 (Template Distribution) and
  before `CoinbaseOutputConstraints`, the client sends `RequestExtensions`
  (`0x0001`, `0x00`: `request_id U16 | requested_extensions SEQ0_64K[U16]`)
  listing `0x4C54`. A P2Pool node uses request id 1.
- The provider answers `RequestExtensions.Success` (`0x01`:
  `request_id U16 | supported_extensions SEQ0_64K[U16]`), listing `0x4C54`
  when it implements it, or `RequestExtensions.Error` (`0x02`:
  `request_id U16 | unsupported_extensions SEQ0_64K[U16] |
  required_extensions SEQ0_64K[U16]`) as the extensions specification says.
- A provider may ignore `RequestExtensions`. The client waits a bounded time
  for the answer, then carries on with standard Template Distribution and
  sends `CoinbaseOutputConstraints`.
- Without the extension the connection is standard. A template too large for
  `RequestTransactionData.Success` is still announced, and a request for its
  transactions gets `RequestTransactionData.Error` `template-too-large`
  (Knuth #804).

### Messages

Extension type `0x4C54`, channel bit clear, provider to client:

```
0x03 RequestTransactionData.Transactions: template_id U64 | transaction_list SEQ0_64K[B0_16M]
0x04 RequestTransactionData.Kept:         template_id U64 | base_template_id U64 | kept B0_16M
```

- `0x03` is one page of a template's transactions, in order: at most 65,535
  that fit one frame. A Bitcoin Cash transaction is at most 1,000,000 bytes,
  so every page holds at least one.
- `0x04` is a bitmap over the base template's transactions, in block order
  without the coinbase. Bit `i` is `(kept[i / 8] >> (i % 8)) & 1`, least
  significant first, and is 1 when the base's `i`-th transaction is also in
  this template. It is exactly `ceil(base_count / 8)` bytes, and the unused
  high bits of its last byte are 0.

### Push

Only on a connection that negotiated `0x4C54`. A template's transactions
follow its `NewTemplate`, and the `SetNewPrevHash` that activates it when one
comes with it, and arrive before the next `NewTemplate`:

- **Full:** zero or more `0x03` pages, then the core
  `RequestTransactionData.Success` (`excess_data` empty, `transaction_list`
  the remaining transactions, possibly none). The list is every page in
  order, then the Success's list. The first template on a connection, and
  the one after a push the provider gave up, come in full.
- **Delta:** a `0x04` `Kept` against the base, then zero or more `0x03` pages
  of added transactions, then the core Success with the rest of them. The
  list is the base's kept transactions and the added ones, sorted by txid
  ascending (CTOR, the Bitcoin Cash block order after the coinbase).
- **Base:** the last template whose transactions the provider pushed on this
  connection, full or delta. Both sides track it, and a delta names no
  other.
- **Integrity:** the client computes the coinbase merkle path (position 0)
  of a placeholder coinbase followed by the list, and checks it equals
  `NewTemplate.merkle_path`. A mismatch means the list is wrong: the client
  drops it, logs it and treats the connection as broken (it reconnects).
- **Backpressure:** the provider never makes other clients or the template
  flow wait on one slow client. It makes pages as the connection's bounded
  output queue drains, never all at once in memory. When a push cannot keep
  up before the next template is ready, the provider gives it up (sends
  nothing more for it) and pushes the next template in full. The client sees
  a `NewTemplate` before the previous template's Success, drops that partial
  list, and expects a full push next.
- A client that negotiated the extension may still send the core
  `RequestTransactionData`. The provider answers with the full list as
  `0x03` pages and the core Success, or the usual error codes. That does not
  change the base.
- `SubmitSolution` is unchanged.

### How a P2Pool node takes it

- `template_distribution::negotiate_large_templates` asks after
  `SetupConnection.Success` and sends nothing else until the answer, as the
  extensions specification requires. It waits twice the time
  `SetupConnection` took to be answered, the specification's suggested
  strategy, at least 250 ms and at most the 10-second setup timeout.
  Messages that arrive meanwhile are kept. A Success that lists `0x4C54`
  puts the connection in push mode. An Error, a Success without it, or no
  answer leaves it standard, exactly as before; an answer that comes later is
  ignored, with one warning.
- A provider that does not answer in time (bchn-sv2-bridge dispatches on the
  message type alone and ignores the request, which the specification
  allows) is remembered by address while the node runs. Later connections
  to it skip `RequestExtensions` and are standard from the start. A provider
  that answered, with a Success or an Error, is asked again on every
  connection. A provider that closes the connection on the request breaks
  the specification, which has unknown extension messages ignored: that is
  an ordinary provider error, and the node reconnects with its usual
  backoff.
- In push mode the node sends no `RequestTransactionData`. It keeps two
  lists only: the base, and the template being received. Both hold shared
  references to the transactions, so a delta's kept transactions and the
  published template are not copies.
- A complete list is checked against the template's merkle path before it
  becomes the base, whether or not the template is still wanted (a template
  for an old parent is not published, but its list is still the base).
- A failed check, a malformed message, a page or Success for a template other
  than the one announced, a `Kept` after pages or against anything but the
  base, or a bitmap of the wrong size ends the connection. The node logs the
  reason and reconnects, as for any broken provider, with the usual backoff.
- A `NewTemplate` before the previous Success drops that partial list and
  the base with it, so a `Kept` before the next full push ends the
  connection.
- Each published template is then checked as before: against the block
  size limit the node reports for its parent, CTOR and the rest of
  `validate_bch`.
