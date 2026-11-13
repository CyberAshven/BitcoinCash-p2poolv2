---
name: bch-share-chain-rules
description: Use when a change could make two P2Pool nodes disagree -- which shares or share headers are valid, how chain work or fork choice is counted, what a share coinbase or the Bitcoin Cash block's coinbase pays (PPLNS window, weights, cuts, donation, genesis), the share header or message format, or the share-version vote -- and when reviewing such a change. Covers naming it, the plain-words explanation, the agreement before merge, the protocol tag, the store format and the rollout. Do NOT use for Bitcoin Cash's own rules, which this project never changes (bch-not-btc).
---

# Share-chain rule changes

P2Pool's share-chain rules decide which shares a node accepts and how the
coinbase of a found Bitcoin Cash block is split. Every node must apply the
same rules, or the share chain splits and miners are paid differently by
different nodes. The Bitcoin Cash node never sees these rules, except
through the coinbase of the block P2Pool builds.

## Is it a rule change?

It is if two nodes, one with the change and one without, could disagree
about the same share or the same coinbase. Examples:

- share and header validation (`docs/sharechain/validation.md`);
- chain work, uncle rules, fork choice;
- the PPLNS window, weights, cuts, donation, the genesis payout;
- the share header, share messages, merkle commitments;
- what goes into the Bitcoin Cash coinbase.

It is not if only what a node sends, asks for, logs or stores locally
changes. Sync serving, RPC fallbacks, metrics and performance work are not
rule changes. Example: answering header requests with the confirmed share
first at a crowded height changed what is sent, not what is valid.

## Naming

The text says "share-chain rules", never "consensus" alone. "Bitcoin Cash
consensus" means Bitcoin Cash's own rules, which this project never
changes. Each rule change says plainly that the Bitcoin Cash node does not
see it, or what it sees in the coinbase.

## Agreement before merge

The PR explains the change in plain words before it is merged, and the
fork's maintainer agrees to it:

- what changes, in one or two sentences a miner would follow;
- what the Bitcoin Cash node sees (usually nothing, or the coinbase);
- what could go wrong, and for whom;
- who must upgrade, and whether the chain restarts.

A reviewer's or another agent's agreement does not replace the
maintainer's. A change to what goes into the Bitcoin Cash coinbase needs
the same agreement even when no share-chain rule moves.

## How it ships

The correct rule is preferred over a compatibility shim. There are two
ways to switch:

1. **The share-version vote**, the default (`shares/share_version.rs`). The
   new rules become share version `n + 1`, `MAX_SUPPORTED_SHARE_VERSION`
   rises, and the old rules stay for shares of the old version. Nodes on
   both versions keep peering while the vote runs. A share switches only
   when 60% of the work in the vote span, about three days below its
   parent, desires the new version; a node produces it at 95%.
2. **A flag day**, an exception the maintainer approves case by case: a new
   rule tag segment in the protocol string, so old and new nodes no longer
   negotiate, and every node upgrades together. It fits a change the vote
   cannot carry, such as share messages old nodes cannot read, or a rule
   that matters only in a new chain's first hours (the vote cannot switch
   before three days).

For a flag day:

- A `pub const BCH_..._RULE_TAG: &str` sits beside the rule it names. It is
  added to `share_chain_rule_tag` in `node/behaviour/request_response.rs`,
  and that function's doc comment says what the rule is and why it is a
  flag day.
- A test there shows the new string differs from the one before
  (`test_bitcoin_cash_protocol_string_differs_from_the_string_before_...`).
- If a node could not sync the existing chain under the new rule, or a
  store holds data validated under the old one, `STORE_FORMAT_VERSION` in
  `store/mod.rs` rises, with a dated line in its doc comment, and the
  `OutdatedStore` message says what changed. Old stores are then refused
  at start-up and the chain is synced from scratch.
- Chipnet and mainnet keep the same rules.

## Evidence and docs

- A run before and after the change, on regtest or in the sim, measures
  the difference it makes (bch-e2e-evidence). Example: the genesis
  last-resort rule was shown by the share of blocks that paid the genesis
  key, before and after.
- The rule's page under `docs/sharechain/` and the CHANGELOG entry say
  that share-chain rules change and how nodes upgrade.
