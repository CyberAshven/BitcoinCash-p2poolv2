# p2poolv2_netsim

A discrete-event simulator of share propagation in a Bitcoin Cash P2Poolv2
network. It answers one question with numbers: as the pool grows, what limits
it first, the share chain, the coinbase or bandwidth?

The model, the results and what they mean are in
[docs/simulation/network-sim.md](../docs/simulation/network-sim.md).

## Run it

```sh
# One scenario, read as text (defaults: 100 nodes, 1,000 miners, a pool
# with 1% of BCH, 32 MB blocks, 10-second shares, hybrid payout, no tokens,
# a day of mining)
cargo netsim run

# The same with merge-mined tokens and a million miners
cargo netsim run --tokens --miners 1000000

# A sweep as a Markdown table, or every sweep
cargo netsim sweep payees
cargo netsim sweep all

# The payee counts at which a coinbase meets each limit
cargo netsim limits
```

`cargo netsim` is an alias (in `.cargo/config.toml`) for
`cargo run --release -p p2poolv2_netsim --bin netsim --`.

`netsim run --help` lists every input: nodes, peers, miners and how their
hashrate is spread, payees, the pool's share of BCH, share interval, block
size, payout rule and tokens, bandwidth and latency distributions
(`50`, `uniform:10:100`, `lognormal:50:0.8`), validation and work-switch
delays, Stratum protocol, duration and seed. A run is a pure function of its
inputs and seed.

## Credit and licence

The idea comes from upstream's ns-3 simulation,
[p2poolv2/Ns3_simulation](https://github.com/p2poolv2/Ns3_simulation)
(`ns-3.44/scratch/p2pool_v2_with_share.cc`): miners on a small-world network
gossiping shares and counting uncles and orphans. That repository has no
licence file, so no code is copied; the model is reimplemented in Rust from
its description and changed for Bitcoin Cash and for scale.

This crate is new code for the Bitcoin Cash fork and is licensed under the
GNU Affero General Public License, version 3 or later (see `LICENSE`), as
the fork decided (`docs/bch-plan.md`, decision 13).
