# Summary

[Introduction](introduction.md)

# Running a pool

- [Running P2Poolv2 on Bitcoin Cash](bch-quickstart.md)
- [Requirements](requirements.md)
- [Docker and Ansible](deployment.md)
- [Miner options](miner-options.md)
- [Small miners](small-miners.md)
- [Trading shares for BCH](atomic-swap/bch-share-trading.md)
- [Known attacks and threat model](known-attacks.md)

# How P2Poolv2 works

- [Why P2Poolv2](why-p2poolv2.md)
- [Comparison with SV2 and DATUM](compare-datum-sv2.md)
- [P2P network of miners](p2p-network.md)
- [Node components](node-components.md)
- [Stratum server and vardiff](stratum.md)
- [Share chain](sharechain/README.md)
  - [Share commitments](sharechain/share-commitments.md)
  - [Share validation](sharechain/validation.md)
  - [Mining on the current chain](sharechain/current-chain.md)
  - [PPLNS accounting](sharechain/pplns-accounting.md)
  - [Share transactions](sharechain/transactions.md)
  - [Spending share outputs](sharechain/spending-outputs.md)
  - [Pool size limits](sharechain/pool-size.md)
- [Payouts](payouts/README.md)
  - [Coinbase payouts](payouts/coinbase.md)
  - [Selling shares](payouts/selling-shares.md)
- [Upstream port map](upstream-port-map.md)

# Bitcoin Cash design

- [Plan and decisions](bch-plan.md)
- [Design checks](bch-design.md)
- [Job Declaration](bch-job-declaration.md)
- [Auxiliary work proofs](bch-auxpow.md)
- [Bitcoin assumptions audit](bch-btc-assumptions-audit.md)
- [Lessons from P2Pool v1 on BCH](p2pool-v1-bch-lessons.md)

# Architecture

- [Overview](architecture/README.md)
  - [Share processing pipeline](architecture/share-processing-pipeline.md)
  - [Async flow and concurrency](architecture/async-flow.md)
  - [Store schema](architecture/store-schema.md)
  - [Store architecture](architecture/store-architecture.md)
  - [Pruning](architecture/pruning.md)
  - [Payout distribution](architecture/payout-distribution.md)
  - [Hybrid payouts on Bitcoin Cash (proposal)](architecture/bch-hybrid-payouts.md)
  - [Metrics](architecture/metrics.md)
  - [Share chain address format](architecture/address-format.md)
  - [Mine on the current chain](architecture/mine-on-bitcoin-chain.md)
- [Share difficulty: ASERT](difficulty_adjustment/asert.md)
- [Difficulty adjustment in CKPool](difficulty_adjustment/ckpool.md)
- [Simulation overview](simulation/overview.md)
- [Simulation runbook](simulation/RUNBOOK.md)
- [Dependency upgrades](dependency-upgrades.md)

# Upstream background (Bitcoin)

- [HTLC scripts](atomic-swap/htlc_scripts.md)
- [Shares for Lightning bitcoin](atomic-swap/p2pool-2-lightinig-example.md)
