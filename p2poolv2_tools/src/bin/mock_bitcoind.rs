// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Mock node JSON-RPC server for stratum load tests.

use clap::{Parser, ValueEnum};
use p2poolv2_config::BitcoinCashNetwork;
use p2poolv2_tools::mock_bitcoind::{
    DEFAULT_MAX_CONNECTIONS, DEFAULT_PORT, MockNode, NodeKind, serve,
};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use tokio::net::TcpListener;

/// The node to answer as.
#[derive(Clone, Copy, ValueEnum)]
enum Node {
    /// Bitcoin Cash Node: a BCHN template, and validateaddress decodes a
    /// CashAddr for --network.
    Bchn,
    /// Bitcoin Core: the Bitcoin signet template, and validateaddress calls
    /// every address a valid witness address. For the JMeter comparison with
    /// CKPool, which mines Bitcoin.
    BitcoinCore,
}

/// A Bitcoin Cash network.
#[derive(Clone, Copy, ValueEnum)]
enum CashNetwork {
    Regtest,
    Chipnet,
    Mainnet,
    Scalenet,
}

/// Mock node JSON-RPC 1.0 server: answers getblocktemplate with an
/// easy-target template, accepts every submitblock, and answers
/// getdifficulty, getbestblockhash and validateaddress. Authentication is
/// ignored. By default it answers as BCHN on regtest.
#[derive(Parser)]
#[command(name = "mock_bitcoind")]
struct Arguments {
    /// Port to listen on.
    #[arg(long, default_value_t = DEFAULT_PORT)]
    port: u16,
    /// Address to listen on.
    #[arg(long, default_value = "0.0.0.0")]
    bind: String,
    /// The node to answer as.
    #[arg(long, value_enum, default_value_t = Node::Bchn)]
    node: Node,
    /// The Bitcoin Cash network a BCHN mock is on: it picks the built-in
    /// template (bitcoindrpc/test_data/bchn-<network>-template.json; mainnet
    /// and scalenet have none) and the CashAddr prefix validateaddress accepts.
    #[arg(long, value_enum, default_value_t = CashNetwork::Regtest)]
    network: CashNetwork,
    /// Block template JSON to serve instead of the built-in one. Its bits
    /// and target are replaced with an easy target either way.
    #[arg(long)]
    template: Option<PathBuf>,
    /// Connections served at once; more wait to be accepted.
    #[arg(long, default_value_t = DEFAULT_MAX_CONNECTIONS)]
    max_connections: usize,
}

fn node_kind(node: Node, network: CashNetwork) -> NodeKind {
    match node {
        Node::BitcoinCore => NodeKind::BitcoinCore,
        Node::Bchn => NodeKind::Bchn(match network {
            CashNetwork::Regtest => BitcoinCashNetwork::Regtest,
            CashNetwork::Chipnet => BitcoinCashNetwork::Chipnet,
            CashNetwork::Mainnet => BitcoinCashNetwork::Mainnet,
            CashNetwork::Scalenet => BitcoinCashNetwork::Scalenet,
        }),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let arguments = Arguments::parse();
    let kind = node_kind(arguments.node, arguments.network);
    let node = match MockNode::load(kind, arguments.template.as_deref()) {
        Ok(node) => Arc::new(node),
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
        "Mock node JSON-RPC server ({kind:?}) listening on port {}",
        arguments.port
    );
    serve(listener, node, arguments.max_connections).await;
    ExitCode::SUCCESS
}
