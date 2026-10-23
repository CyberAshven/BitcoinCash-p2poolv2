// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Mock bitcoind JSON-RPC server for stratum load tests.

use clap::Parser;
use p2poolv2_tools::mock_bitcoind::{DEFAULT_MAX_CONNECTIONS, DEFAULT_PORT, load_template, serve};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use tokio::net::TcpListener;

/// Mock bitcoind JSON-RPC 1.0 server: answers getblocktemplate with an
/// easy-target template, accepts every submitblock, and answers
/// getdifficulty, getbestblockhash and validateaddress. Authentication is
/// ignored.
#[derive(Parser)]
#[command(name = "mock_bitcoind")]
struct Arguments {
    /// Port to listen on.
    #[arg(long, default_value_t = DEFAULT_PORT)]
    port: u16,
    /// Address to listen on.
    #[arg(long, default_value = "0.0.0.0")]
    bind: String,
    /// Block template JSON to serve instead of the built-in one
    /// (p2poolv2_tests/test_data/validation/stratum/b/template.json). Its
    /// bits and target are replaced with an easy target either way.
    #[arg(long)]
    template: Option<PathBuf>,
    /// Connections served at once; more wait to be accepted.
    #[arg(long, default_value_t = DEFAULT_MAX_CONNECTIONS)]
    max_connections: usize,
}

#[tokio::main]
async fn main() -> ExitCode {
    let arguments = Arguments::parse();
    let template = match load_template(arguments.template.as_deref()) {
        Ok(template) => Arc::new(template),
        Err(error) => {
            eprintln!("mock_bitcoind: {error}");
            return ExitCode::FAILURE;
        }
    };
    let listener = match TcpListener::bind((arguments.bind.as_str(), arguments.port)).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!(
                "mock_bitcoind: cannot listen on {}:{}: {error}",
                arguments.bind, arguments.port
            );
            return ExitCode::FAILURE;
        }
    };
    println!(
        "Mock bitcoind JSON-RPC server listening on port {}",
        arguments.port
    );
    serve(listener, template, arguments.max_connections).await;
    ExitCode::SUCCESS
}
