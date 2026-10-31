# aserti3-2d test vectors

The twelve runs of the aserti3-2d specification's test vectors, copied
unchanged from Bitcoin Cash Node's QA assets:
<https://gitlab.com/bitcoin-cash-node/bchn-sw/qa-assets/-/tree/master/test_vectors/aserti3-2d>
at commit `1e153d75` (2020-08-13, "Add runs 11 and 12"). That repository is
MIT licensed (Copyright (c) 2018 the Bitcoin Cash Node developers).

Each run states its anchor (height, parent time, nBits) in its `##` lines.
Every data line is `iteration height time target`: `target` is the nBits of
the block after one at `height` and `time`, with a two-day half-life and the
proof-of-work limit `00000000ffff…ff`. `bch_asert::tests` checks every line.
