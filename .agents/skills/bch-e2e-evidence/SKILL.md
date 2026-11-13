---
name: bch-e2e-evidence
description: Use when a change alters what the pool does -- shares, sync, payouts, templates, block submission, stratum jobs, the node connection -- and before saying such a change works. Proves it with a before-and-after run on Bitcoin Cash regtest, the sim or chipnet, and keeps unit tests few and aimed at the real risk. Do NOT use for docs-only or formatting-only changes.
---

# Prove behaviour end to end

A unit test written next to new code tends to confirm the code rather than
the behaviour. A behaviour change is shown by running the pool: once
without the change, once with it, measuring the difference.

## The run

1. **Pick the check that exercises the behaviour.** In order of cost:
   - a regtest run against a real node (BCHN 29.2.0, the version CI pins;
     BCHD or Knuth when the change touches the node, bch-node-agnostic);
   - the sim (`cargo sim`, `docs/simulation/RUNBOOK.md`), for propagation,
     uncles, reorgs, PPLNS and found blocks across many nodes;
   - the nightly scenarios (`cargo sim nightly <scenario>`, the
     `sim-nightly` workflow);
   - a copy of a live node's store on a throwaway node, for a bug a live
     chain shows. The copy is used, never the live store.
2. **Run it before the change** and record what fails or what number is
   measured: a stalled height, how often the genesis key was paid, time to
   sync.
3. **Run it after the change** with the same inputs and record the same
   numbers.
4. **Report both in the PR**: the command, the inputs, the before and after
   result, and what the run did not cover.

Regtest and the sim always use their own throwaway node. They never point
at a node that serves a live pool or a wallet.

## Unit tests

Unit and store tests stay few and aim at what would hurt: a cache that
wrongly refuses, a boundary that cuts the wrong share, a parse of each
node's real answer. A test that only restates the implementation adds
nothing.

## Test hygiene

- `cargo nextest run`, never `cargo test`: nextest enforces the 30-second
  limit in `.config/nextest.toml`, and plain `cargo test` hides a slow test
  that CI then fails. A test that runs clearly slower than usual is a
  failure to explain.
- After merging `alpha` into a branch, the merged head is tested (CI or
  nextest locally) before the branch is merged.
- Each worktree uses its own `CARGO_TARGET_DIR`. The sim harness finds the
  repository through its compile-time `CARGO_MANIFEST_DIR`, so a shared
  target directory can run another worktree's node. The node log's `git:`
  line shows which build ran.
