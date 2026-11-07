---
name: bch-upstream-sync
description: Use before writing a fix or a feature (check whether upstream P2Poolv2 or P2Pool v1 already has it), when bringing upstream p2poolv2/p2poolv2 changes into this fork by merge or cherry-pick, when resolving upstream merge conflicts, and when writing code in an area upstream also maintains. Keeps the fork close to upstream's code and idioms so later merges stay easy.
---

# Staying close to upstream

This fork tracks upstream P2Poolv2 (`p2poolv2/p2poolv2`, remote `upstream`)
and adds Bitcoin Cash on top. Code that reads like upstream's merges
cleanly and stays readable to upstream's maintainers. Drift costs every
later merge.

## Look upstream first

Before a fix or a feature is written, upstream is checked for one:

- `git grep` on `upstream/main` for the area, and its log for the files.
- For Bitcoin Cash behaviour, P2Pool v1 (`jtoomim/p2pool`, which mined
  Bitcoin Cash), read with `gh api repos/jtoomim/p2pool/contents/<path>`.
  `docs/p2pool-v1-bch-lessons.md` lists what was already taken from it.

An existing fix is ported rather than rewritten, and still checked against
Bitcoin Cash's rules (bch-not-btc).

## Bringing upstream in

- **A full sync** merges `upstream/main` into `alpha` with `git merge`
  (`docs/upstream-port-map.md`, "Code from upstream"). A modify/delete
  conflict on a shell script this fork replaced is resolved by keeping the
  deletion and porting the change into the Rust module `docs/tooling.md`
  names.
- **A single commit** is taken with `git cherry-pick -x` while the fork's
  files still match upstream's parent. Upstream's author stays the author,
  and the Bitcoin Cash adaptation goes into one separate commit on top.
- **Docs pages** follow `docs/upstream-port-map.md`: each page names the
  upstream commit it was last brought up to.

Upstream changes are never retyped by hand when a cherry-pick or merge
applies.

## Writing code upstream also maintains

- Upstream's idioms for the area are followed: where state lives, doc
  comment style, naming, error handling, log wording, test layout.
- Signatures and file layout stay as upstream has them unless the change
  needs otherwise. No new module for a small thing.
- One known layout difference: test modules holding fixed nonces,
  passwords or salts live in `<module>/tests.rs` files so CodeQL's
  `paths-ignore` skips them. A file already split stays split; one still
  inline, as upstream has it, stays inline unless CodeQL flags it.
- Every deliberate difference from upstream is named in the PR with its
  reason.

## Direction

Pull requests go to this fork (`--repo CyberAshven/BitcoinCash-p2poolv2
--base alpha`), never to upstream. One PR covers one problem, with its fix
and its tests together.
