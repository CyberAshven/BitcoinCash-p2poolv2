// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared test helpers: rendering to text and sample data.

use crate::model::{
    BchnChain, BchnMempool, BchnNetwork, BchnPeer, BchnStatus, ChainInfo, FoundBlock, Hashrate,
    HashratePoint, LocalMiner, Miner, MinerWindow, Miners, Peer, Share, Site, Worker,
};
use crate::views::config::ConfigRow;
use ratatui::{Frame, Terminal, backend::TestBackend};
#[cfg(feature = "client")]
use std::fs;
#[cfg(feature = "client")]
use std::path::{Path, PathBuf};

/// Unix time the samples are taken at.
pub const NOW: u64 = 1_761_220_800;

/// A chipnet node's config.toml, with a comment to keep and an RPC
/// password that must never be shown.
#[cfg(feature = "client")]
pub const BCH_CHIPNET_CONFIG: &str = r#"# A chipnet node, for tests.
[network]
listen_address = "/ip4/127.0.0.1/tcp/6884"
dial_peers = []
max_pending_incoming = 10
max_pending_outgoing = 10
max_established_incoming = 50
max_established_outgoing = 50
max_established_per_peer = 2
max_workbase_per_second = 10
max_userworkbase_per_second = 10
max_miningshare_per_second = 100
max_inventory_per_second = 100
max_transaction_per_second = 100
max_requests_per_second = 100
dial_timeout_secs = 30

[store]
path = "./store.db"
pplns_ttl_days = 7

[stratum]
hostname = "0.0.0.0"
port = 3333 # the SV1 port
start_difficulty = 10000
minimum_difficulty = 100
bootstrap_address = "bchtest:qze95tc2dqrnltvxfa5yhunwx5952f34fv28vandn2"
zmqpubhashblock = "tcp://127.0.0.1:28332"
network = "signet"
version_mask = "1fffe000"
pool_signature = "P2Poolv2"
chain = "bitcoin-cash"
bitcoin_cash_network = "chipnet"
bch_share_chain = "mini"

[bitcoinrpc]
url = "http://127.0.0.1:48332"
username = "p2pool"
password = "chipnet-rpc-secret"

[logging]
level = "info"

[api]
hostname = "127.0.0.1"
port = 46884
"#;

/// Writes `text` as `config.toml` in `dir` and returns its path.
#[cfg(feature = "client")]
pub fn write_config(dir: &Path, text: &str) -> PathBuf {
    let path = dir.join("config.toml");
    fs::write(&path, text).unwrap();
    path
}

/// Draw with `draw` on a `width` by `height` test terminal and return the
/// screen as text, one line per row with trailing spaces cut.
///
/// Box drawing and block characters become ASCII (`-`, `|`, `+`, `#`) so
/// snapshots stay ASCII like the sources; anything else non-ASCII becomes
/// `?`.
pub fn screen_text(width: u16, height: u16, draw: impl FnOnce(&mut Frame)) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(draw).unwrap();
    let buffer = terminal.backend().buffer();
    let mut text = String::with_capacity(usize::from(width + 1) * usize::from(height));
    for row in 0..height {
        let mut line = String::with_capacity(usize::from(width));
        for column in 0..width {
            line.push_str(&ascii(buffer[(column, row)].symbol()));
        }
        text.push_str(line.trim_end());
        text.push('\n');
    }
    text
}

fn ascii(symbol: &str) -> String {
    symbol
        .chars()
        .map(|character| match character {
            '\u{2500}' | '\u{2501}' => '-',
            '\u{2502}' | '\u{2503}' => '|',
            '\u{250c}' | '\u{2510}' | '\u{2514}' | '\u{2518}' | '\u{251c}' | '\u{2524}'
            | '\u{252c}' | '\u{2534}' | '\u{253c}' | '\u{256d}' | '\u{256e}' | '\u{256f}'
            | '\u{2570}' => '+',
            '\u{2581}'..='\u{2588}' => '#',
            character if character.is_ascii() => character,
            _ => '?',
        })
        .collect()
}

pub fn sample_bchn_status() -> BchnStatus {
    BchnStatus {
        chain: BchnChain {
            chain: "chip".into(),
            blocks: 284_512,
            headers: 284_512,
            best_block_hash: "000000000a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5"
                .into(),
            difficulty: 1_234_567.0,
            median_time: Some(NOW - 600),
            verification_progress: Some(0.999_99),
            initial_block_download: Some(false),
            size_on_disk: Some(2_345_678_901),
            pruned: Some(false),
            warnings: Some(String::new()),
        },
        network: BchnNetwork {
            subversion: "/Bitcoin Cash Node:28.0.1(EB32.0)/".into(),
            protocol_version: 70016,
            connections: 2,
            network_active: Some(true),
            relay_fee: Some(0.00001),
        },
        mempool: BchnMempool {
            size: 12,
            bytes: 3_400,
            usage: 9_000,
            max_mempool: 300_000_000,
            min_fee: 0.00001,
            loaded: Some(true),
        },
        peers: vec![
            BchnPeer {
                id: 1,
                address: "203.0.113.5:48333".into(),
                user_agent: "/Bitcoin Cash Node:28.0.1(EB32.0)/".into(),
                inbound: false,
                synced_blocks: Some(284_512),
                ping_time: Some(0.052),
                connected_at: Some(NOW - 7_200),
            },
            BchnPeer {
                id: 2,
                address: "198.51.100.7:51234".into(),
                user_agent: "/Bitcoin Cash Node:27.1.0(EB32.0)/".into(),
                inbound: true,
                synced_blocks: Some(284_511),
                ping_time: None,
                connected_at: Some(NOW - 45),
            },
        ],
        network_hashrate: Some(8_800_000_000_000.0),
    }
}

pub fn sample_site() -> Site {
    Site {
        network: "Bitcoin Cash chipnet".into(),
        badge: "chipnet".into(),
    }
}

pub fn sample_chain_info() -> ChainInfo {
    ChainInfo {
        genesis_blockhash: Some("aa11".into()),
        chain_tip_height: Some(1_204),
        chain_tip_blockhash: Some("bb22".into()),
        total_work: "0x1f3a".into(),
        top_candidate_height: Some(1_205),
        top_candidate_blockhash: Some("cc33".into()),
    }
}

pub fn sample_hashrate() -> Hashrate {
    Hashrate {
        pool_hashrate: Some(1_230_000_000_000.0),
        local_hashrate: Some(615_000_000_000.0),
        network_hashrate: Some(8_800_000_000_000.0),
        history: vec![
            HashratePoint {
                time: NOW - 120,
                pool_hashrate: Some(1_000_000_000_000.0),
                local_hashrate: Some(500_000_000_000.0),
            },
            HashratePoint {
                time: NOW - 60,
                pool_hashrate: Some(1_230_000_000_000.0),
                local_hashrate: Some(615_000_000_000.0),
            },
        ],
    }
}

pub fn sample_share(height: u32) -> Share {
    Share {
        blockhash: format!("{height:064x}"),
        height,
        miner_bitcoin_address: "bchtest:qze95tc2dqrnltvxfa5yhunwx5952f34fv28vandn2".into(),
        miner_address: "tp2pool:qq9q4yfh".into(),
        timestamp: NOW - u64::from(height % 60),
        bits: 0x1c38_4bd7,
        uncles: Vec::new(),
    }
}

pub fn sample_miners() -> Miners {
    Miners {
        miners_online: 1,
        workers_online: 1,
        total: 1,
        page: 1,
        pages: 1,
        miners: vec![LocalMiner {
            address: "bchtest:qze95tc2dqrnltvxfa5yhunwx5952f34fv28vandn2".into(),
            workers: vec![Worker {
                name: "avalon".into(),
                active: true,
                accepted_work: 512_000,
                best_share: 90_000,
                last_share_at: NOW - 20,
            }],
            accepted_work: 512_000,
            best_share: 90_000,
            last_share_at: NOW - 20,
            payout_fraction: Some(0.5),
        }],
    }
}

pub fn sample_miner() -> Miner {
    Miner {
        address: "bchtest:qze95tc2dqrnltvxfa5yhunwx5952f34fv28vandn2".into(),
        address_type: "payout".into(),
        anchor_height: 1_204,
        window: MinerWindow {
            share_count: 900,
            newest_height: Some(1_104),
            oldest_height: Some(205),
        },
        share_count: 450,
        uncle_count: 3,
        share_fraction: 0.5,
        payout_fraction: 0.497,
    }
}

pub fn sample_found_block() -> FoundBlock {
    FoundBlock {
        bitcoin_height: 284_400,
        bitcoin_blockhash: "00000000deadbeef00000000deadbeef00000000deadbeef00000000deadbeef"
            .into(),
        share_blockhash: "cc33".into(),
        bitcoin_time: NOW - 3_600,
        miner_bitcoin_address: "bchtest:qze95tc2dqrnltvxfa5yhunwx5952f34fv28vandn2".into(),
    }
}

/// A network choice, a secret and an unset optional setting.
pub fn sample_config_rows() -> Vec<ConfigRow> {
    vec![
        ConfigRow {
            section: "stratum",
            key: "bitcoin_cash_network",
            description: "The network switch",
            type_hint: "mainnet | chipnet | regtest",
            value: Some("chipnet".into()),
            default: None,
            sensitive: false,
        },
        ConfigRow {
            section: "bitcoinrpc",
            key: "password",
            description: "BCHN RPC password",
            type_hint: "text",
            value: Some("hunter2".into()),
            default: None,
            sensitive: true,
        },
        ConfigRow {
            section: "api",
            key: "public_dashboard",
            description: "Serve the dashboard without a login",
            type_hint: "true | false",
            value: None,
            default: Some("false"),
            sensitive: false,
        },
    ]
}

pub fn sample_peer() -> Peer {
    Peer {
        peer_id: "12D3KooWQz9YwqT3dCk2FzE7m1dUoRZpWg7yQpRjzNkPz1P8Q2aB".into(),
        address: Some("/ip4/127.0.0.1/tcp/6885".into()),
        direction: Some("Outbound".into()),
        connected_secs: Some(3_725),
    }
}
