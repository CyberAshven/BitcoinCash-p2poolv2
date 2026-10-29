# PHOTON payout covenant feasibility prototype (phase 2a)

This developer-only crate implements the funded fungible-token payout
covenant proposed in [the architecture document](../docs/architecture/photon-pool.md).
It has no node integration, RPC client, network transport, wallet, private-key
generator or broadcast path. All transactions and funding fixtures are offline.
The crate is AGPL-3.0-or-later, unpublished and excluded from distribution.

The pinned `libauth` revision is the workspace's
`be3ed16396c9eb9c0e57343474d36079a37c8dae`. Its BCH 2026 VM is the execution
authority for this prototype, in standard and non-standard mode. BCHN
transaction admission, relay policy and real funding signatures require
separate integration evidence; no BCHN was called for this prototype.

The root is parameterized as P2SH20 or P2SH32. Children are P2SH32. The
336-byte state is prefixed by minimal `OP_PUSHDATA2`; the context digest
starts at redeem byte 43. The root reads the reward from its active input;
descendants pin the original reward and use global cumulative intervals.
Merkle-sum commitments authenticate midpoint leaf counts and unsigned
64-byte little-endian weights. Terminal buckets have at most eight leaves.

The covenant enforces a singleton payout input at index zero, one through
four token-free funding inputs, ordered positive allocation outputs,
optional token-free change last, version 2, locktime zero and final
sequences. Token allocation uses exact telescoping division. Each token
output receives its committed BCH minimum; their aggregate retains all
inherited BCH. The executor may assign excess BCH among those outputs.
This aggregate-only rule is the frozen prototype policy, pending the
owner's deployment decision.

## Reproduce

Use the build environment specified by the repository coordinator and a
target directory dedicated to this worktree. No network or wallet setup
is needed by the prototype.

```text
cargo fmt --all -- --check
cargo clippy -p p2poolv2_photon_payout --all-targets --all-features -- -D warnings
cargo nextest run -p p2poolv2_photon_payout
cargo nextest run -p p2poolv2_photon_payout --release --profile photon-heavy --test heavy --run-ignored ignored-only --nocapture
cargo deny check
cargo photon-payout-measure
```

Heavy acceptance tests are ignored by default so routine CI does not run
the full 4,096-leaf execution or 100,000 seeded random manifests. Run them
explicitly using the command above. The named `photon-heavy` nextest profile
allows up to an hour for these explicit stress runs and leaves the default
30-second timeout unchanged. The measurements artifact and the
architecture document record the actual run results and their limits.

## Phase 2b TODO: group C

Use the unmodified PHOTON v3.2 covenant, in a separate claim-side change:

- Verify P2SH20 pool claims at ages 0, 17, 128 and 32,767 BCH blocks.
- Show locktime variants preserve the reconstructed proof.
- Mutate `R`, key, nonce, datasig through signing nonce, age, sequence and
  BCH values; require changed digest or rejection.
- Publish claim-side golden vectors for `R` and `HASH160(R)`.
- Measure `HASH160(R)` per seed on CPU and GPU; this crate has no GPU engine.

BCHN regtest `testmempoolaccept` and full relay/dust/finality testing remain
a separate gate. This phase does not authorize a share chain, node,
chipnet spend, mainnet spend or deployment.
