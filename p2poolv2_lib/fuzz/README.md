<!-- SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS) -->
<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# Wire decoder fuzzing

The harness and workflows are ported from Juwon Ogunseye's upstream
P2Poolv2 commits da18b991, 6a885d31, 39113c33, 04d4a057 and 543d6240.
They retain the upstream MIT OR Apache-2.0 licence. The harness decodes
arbitrary framing, then puts the input into a frame with the correct length
and SHA256d checksum. This reaches this fork's message payload decoder,
including `ShareHeaderBatch`, without changing any share-chain rule.

This is a panic smoke test. Successful decoding does not establish that the
decoded values are correct; generated-message round-trip tests are separate.

Install `cargo-fuzz` and a nightly Rust toolchain. Set
`RUSTUP_TOOLCHAIN=nightly` before running the following commands from
`p2poolv2_lib`, so cargo-fuzz's nested Cargo process uses nightly despite the
repository's stable toolchain pin:

```text
cargo metadata --manifest-path fuzz/Cargo.toml --locked --format-version 1
cargo +nightly fuzz build wire_message_decode
cargo +nightly fuzz run wire_message_decode -- -max_total_time=60 -max_len=1310721 -rss_limit_mb=2048
```

A prebuilt cargo-fuzz, as CI installs, may default to the musl target, where
AddressSanitizer cannot run; CI passes `--target x86_64-unknown-linux-gnu`.
Cargo-fuzz has no `--locked` option; CI checks the lockfile before and after
the run. On Windows, cargo-fuzz
requires the platform setup described in the
[Rust Fuzz Book](https://rust-fuzz.github.io/book/cargo-fuzz/windows/setup.html).
The fuzz crate has its own workspace and lockfile; ordinary workspace checks
do not build it.

`seed_corpus/wire_message_decode` contains payloads for this fork's message
format: empty inventories and share-header batches, block requests, an
acknowledgement, a handshake, and invalid discriminants and lengths. Copy
these files into `corpus/wire_message_decode` before a local run to start
with the same seeds as CI. The length limit includes the current maximum
P2P payload plus one byte to exercise its rejection boundary. It is a fuzz
input limit, not a new node limit.

The independent smoke workflow runs for 60 seconds on alpha pushes and pull
requests. The scheduled/manual workflow runs for one hour. It restores and
saves evolving inputs with GitHub Actions cache; the checked-in seeds remain
available if the cache expires. Crashes are uploaded as workflow artifacts.
No external corpus repository or access token is used. As with all scheduled
GitHub workflows, the schedule starts only after the workflow exists on the
default branch; scheduled runs explicitly check out alpha.
