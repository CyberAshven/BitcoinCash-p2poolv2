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

# Bitcoin Cash design

- [Plan and decisions](bch-plan.md)
- [Design checks](bch-design.md)
- [Job Declaration](bch-job-declaration.md)
- [Auxiliary work proofs](bch-auxpow.md)

# Architecture

- [Overview](architecture/README.md)
  - [Share processing pipeline](architecture/share-processing-pipeline.md)
  - [Async flow and concurrency](architecture/async-flow.md)
  - [Store schema](architecture/store-schema.md)
  - [Store architecture](architecture/store-architecture.md)
  - [Pruning](architecture/pruning.md)
  - [Payout distribution](architecture/payout-distribution.md)
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
