// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use clap::Parser;
use p2poolv2_lib::config::{BchShareChain, Config};
use p2poolv2_lib::logging::setup_logging;
use std::process::ExitCode;
use tracing::info;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long, env("P2POOL_CONFIG"))]
    config: String,
    /// Run the vardiff Bitcoin Cash share chain, overriding
    /// `bch_share_chain` from the config file. Rejected on a Bitcoin chain,
    /// like a configured vardiff chain.
    #[arg(long)]
    vardiff_sharechain: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();

    let mut config = match Config::load(&args.config) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("Failed to load config: {err}");
            return ExitCode::FAILURE;
        }
    };

    // Hold guards to keep non-blocking writers alive
    let _guards = match setup_logging(&config.logging) {
        Ok(guards) => guards,
        Err(err) => {
            // no logger yet
            eprintln!("Failed to load config: {err}");
            return ExitCode::FAILURE;
        }
    };

    if args.vardiff_sharechain && config.stratum.bch_share_chain != BchShareChain::Vardiff {
        info!(
            "--vardiff-sharechain overrides [stratum] bch_share_chain = {:?} with vardiff",
            config.stratum.bch_share_chain
        );
        config.stratum.bch_share_chain = BchShareChain::Vardiff;
    }

    info!(
        "Running on {} network (git: {})",
        &config.stratum.network,
        env!("GIT_VERSION")
    );

    let (_handles, runner) = match p2poolv2_node::build_node(config).await {
        Ok(result) => result,
        Err(exit_code) => return exit_code,
    };

    runner.run().await
}
