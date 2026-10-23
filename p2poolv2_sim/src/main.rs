// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! No-PoW load-test simulation binary for P2Poolv2.
//!
//! Builds a full node via `p2poolv2_node::build_node`, then wires a synthetic
//! share emitter that models one miner at a configured hashrate. Must be built
//! with `--features sim`.
//!
//! The sim swarm runs Bitcoin Cash: `chain = "bitcoin-cash"` with
//! `bitcoin_cash_network = "regtest"`, a `bchreg:` CashAddr payout address
//! and an `rp2pool:` share address, against a BCHN regtest node. A Bitcoin
//! config still runs, since the node parses both chains.
//!
//! See docs/simulation/load-test-plan.md for the full design.

#[cfg(feature = "sim")]
mod config;
#[cfg(feature = "sim")]
mod identity;

#[cfg(feature = "sim")]
use clap::Parser;
#[cfg(feature = "sim")]
use config::SimNodeConfig;
#[cfg(feature = "sim")]
use identity::ChainSelection;
#[cfg(feature = "sim")]
use p2poolv2_lib::accounting::payout::sharechain_pplns::pplns_window::{
    ChainDepths, PPLNS_WINDOW_SHARES, PRUNE_INTERVAL,
};
#[cfg(feature = "sim")]
use p2poolv2_lib::logging::setup_logging;
#[cfg(feature = "sim")]
use p2poolv2_lib::sim::emitter::SimEmitter;
#[cfg(feature = "sim")]
use p2poolv2_lib::sim_overrides;
#[cfg(feature = "sim")]
use p2poolv2_node::signal::ShutdownReason;
#[cfg(feature = "sim")]
use tracing::{error, info};

use std::process::ExitCode;

#[cfg(feature = "sim")]
#[derive(Parser, Debug)]
#[command(author, version, about = "P2Poolv2 no-PoW load-test simulation")]
struct Args {
    #[arg(short, long, env("P2POOL_CONFIG"))]
    config: String,
}

#[cfg(feature = "sim")]
#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();

    let sim_node_config = match SimNodeConfig::load(&args.config) {
        Ok(config) => config,
        Err(err) => {
            error!("Failed to load config: {err}");
            return ExitCode::FAILURE;
        }
    };

    let node_config = sim_node_config.node.clone();
    let sim_cfg = sim_node_config.sim;

    let _guards = match setup_logging(&node_config.logging) {
        Ok(guards) => guards,
        Err(e) => {
            error!("Failed to set up logging: {e}");
            return ExitCode::FAILURE;
        }
    };

    info!(
        "Running on {} network, chain {:?} (sim mode, git: {})",
        &node_config.stratum.network,
        node_config.stratum.chain,
        env!("GIT_VERSION")
    );

    // Initialize sim overrides before build_node (ASERT anchor, genesis).
    sim_overrides::init_ideal_block_time(sim_cfg.ideal_block_time_secs.unwrap_or(10));
    sim_overrides::init_genesis_overrides(
        sim_cfg.asert_anchor_time.unwrap_or(0),
        sim_cfg.network_hashrate.unwrap_or(0),
    );
    sim_overrides::init_propagation_delay(sim_cfg.propagation_delay_ms.unwrap_or(0));
    // The depths are compile-time sim constants. Each chain keeps its own
    // maturity: Bitcoin's is the sim's 20 shares, Bitcoin Cash's stays 100,
    // a share-chain rule the sim build does not change, and each chain's
    // prune depth follows from its maturity and the sim window (Bitcoin
    // Cash: 2 * (100 + 80 + 3) = 366). `cargo sim maturity-check` and the
    // prune scenarios read them from this line.
    let depths = ChainDepths::for_chain(node_config.stratum.chain);
    info!(
        "Sim depths: coinbase maturity {} shares, PPLNS window {PPLNS_WINDOW_SHARES} shares, \
         prune depth {} shares, prune interval {PRUNE_INTERVAL} shares",
        depths.coinbase_maturity,
        depths.prune_depth()
    );

    let (handles, runner) = match p2poolv2_node::build_node(node_config.clone()).await {
        Ok(result) => result,
        Err(exit_code) => return exit_code,
    };

    // Wire the sim emitter using the node handles.
    if sim_cfg.enabled {
        let stratum = &node_config.stratum;
        let resolved = identity::resolve(
            ChainSelection {
                chain: stratum.chain,
                network: stratum.network,
                bitcoin_cash_network: stratum.bitcoin_cash_network,
            },
            &sim_cfg.miner_address,
            sim_cfg.share_address.as_deref(),
            stratum.miner_address.as_deref(),
        );
        let rpc = bitcoindrpc::BitcoindRpcClient::new(
            &node_config.bitcoinrpc.url,
            &node_config.bitcoinrpc.username,
            &node_config.bitcoinrpc.password,
        );
        match (resolved, rpc) {
            (Ok(identity), Ok(sim_rpc)) => {
                let emitter = SimEmitter::new(
                    handles.emissions_tx.clone(),
                    handles.template_rx.clone(),
                    identity.payout,
                    identity.share_owner,
                    sim_cfg,
                    sim_rpc,
                );
                let mut sim_exit_rx = handles.shutdown_tx.subscribe();
                tokio::spawn(async move {
                    tokio::select! {
                        _ = emitter.run() => {}
                        _ = sim_exit_rx.wait_for(|r| *r != ShutdownReason::None) => {
                            info!("Sim emitter shutting down");
                        }
                    }
                });
                info!("Sim emitter spawned");
            }
            (Err(reason), _) => error!("{reason}, not starting sim emitter"),
            (_, Err(e)) => error!("Failed to build sim bitcoind rpc client: {e}"),
        }
    }

    runner.run().await
}

#[cfg(not(feature = "sim"))]
fn main() -> ExitCode {
    eprintln!("p2poolv2_sim must be built with: cargo build -p p2poolv2_sim --features sim");
    ExitCode::FAILURE
}
