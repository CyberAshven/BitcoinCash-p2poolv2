// SPDX-FileCopyrightText: 2024 PDM Authors
// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Plain data the views draw: what the Bitcoin Cash P2Poolv2 node's API and
//! BCHN's JSON-RPC answer.
//!
//! The types mirror only the fields shown; serde ignores the rest, so new
//! fields on the node or in BCHN never break the views. PDM's `ChainInfo`,
//! `ShareInfo` and `PeerInfo` became the fork's `/chain_info`, `/shares`
//! and `/peers` shapes; the rest are new endpoints of the fork.

use serde::Deserialize;

/// Most BCHN peers kept from one `getpeerinfo`.
pub const MAX_BCHN_PEERS: usize = 256;

/// Most found blocks kept from one `/found_blocks`.
pub const MAX_FOUND_BLOCKS: usize = 200;

/// Most share chain peers kept, from `/peers` and live events together.
pub const MAX_SHARE_CHAIN_PEERS: usize = 256;

/// `/network`: the chain the node mines.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Site {
    /// In full, e.g. `Bitcoin Cash chipnet`.
    pub network: String,
    /// The short name, e.g. `chipnet`.
    pub badge: String,
}

/// `/chain_info`: the share chain's tip.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct ChainInfo {
    #[serde(default)]
    pub genesis_blockhash: Option<String>,
    #[serde(default)]
    pub chain_tip_height: Option<u32>,
    #[serde(default)]
    pub chain_tip_blockhash: Option<String>,
    /// Hex, e.g. `0x1a2b`.
    #[serde(default)]
    pub total_work: String,
    #[serde(default)]
    pub top_candidate_height: Option<u32>,
    #[serde(default)]
    pub top_candidate_blockhash: Option<String>,
}

/// One sample of `/hashrate`'s history, in hashes per second.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct HashratePoint {
    pub time: u64,
    #[serde(default)]
    pub pool_hashrate: Option<f64>,
    #[serde(default)]
    pub local_hashrate: Option<f64>,
}

/// `/hashrate?window=`: hashrates in hashes per second.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Hashrate {
    #[serde(default)]
    pub pool_hashrate: Option<f64>,
    #[serde(default)]
    pub local_hashrate: Option<f64>,
    #[serde(default)]
    pub network_hashrate: Option<f64>,
    /// Oldest first.
    #[serde(default)]
    pub history: Vec<HashratePoint>,
}

/// An uncle a confirmed share includes.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Uncle {
    pub blockhash: String,
    #[serde(default)]
    pub miner_bitcoin_address: String,
    #[serde(default)]
    pub timestamp: u64,
    #[serde(default)]
    pub height: Option<u32>,
}

/// A confirmed share, from `/shares` or a `/ws` share event.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Share {
    pub blockhash: String,
    pub height: u32,
    /// The CashAddr the share's PPLNS payout goes to.
    pub miner_bitcoin_address: String,
    /// The share chain address owning the share.
    #[serde(default)]
    pub miner_address: String,
    pub timestamp: u64,
    /// Compact share target.
    pub bits: u32,
    #[serde(default)]
    pub uncles: Vec<Uncle>,
}

/// `/shares?num=`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Shares {
    #[serde(default)]
    pub shares: Vec<Share>,
}

/// One of `/found_blocks`, newest first.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct FoundBlock {
    pub bitcoin_height: u64,
    pub bitcoin_blockhash: String,
    pub share_blockhash: String,
    pub bitcoin_time: u64,
    pub miner_bitcoin_address: String,
}

/// A worker of a miner on this node.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Worker {
    pub name: String,
    pub active: bool,
    pub accepted_work: u64,
    pub best_share: u64,
    pub last_share_at: u64,
}

/// A miner on this node.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct LocalMiner {
    pub address: String,
    #[serde(default)]
    pub workers: Vec<Worker>,
    pub accepted_work: u64,
    pub best_share: u64,
    pub last_share_at: u64,
    /// Its fraction of the next block's PPLNS payout, when known.
    #[serde(default)]
    pub payout_fraction: Option<f64>,
}

/// `/miners?page=`: the miners on this node.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Miners {
    pub miners_online: usize,
    pub workers_online: usize,
    #[serde(default)]
    pub total: usize,
    pub page: usize,
    pub pages: usize,
    #[serde(default)]
    pub miners: Vec<LocalMiner>,
}

/// The PPLNS window as a whole, in `/miner`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct MinerWindow {
    pub share_count: usize,
    #[serde(default)]
    pub newest_height: Option<u32>,
    #[serde(default)]
    pub oldest_height: Option<u32>,
}

/// `/miner?address=`: one miner's place in the PPLNS window.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Miner {
    pub address: String,
    /// `payout` or `share`.
    pub address_type: String,
    pub anchor_height: u32,
    pub window: MinerWindow,
    pub share_count: usize,
    pub uncle_count: usize,
    pub share_fraction: f64,
    pub payout_fraction: f64,
}

/// A share chain peer, from `/peers`, or from a live event that names only
/// its id.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Peer {
    pub peer_id: String,
    #[serde(default)]
    pub address: Option<String>,
    /// `Inbound` or `Outbound`.
    #[serde(default)]
    pub direction: Option<String>,
    #[serde(default)]
    pub connected_secs: Option<u64>,
}

/// Whether a peer connected or disconnected, in a `/ws` peer event.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub enum PeerStatus {
    Connected,
    Disconnected,
}

/// A `/ws` peer event.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct PeerEvent {
    pub peer_id: String,
    pub status: PeerStatus,
}

/// A `/ws` frame: `{"topic":"Share","data":{...}}` or
/// `{"topic":"Peer","data":{...}}`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "topic", content = "data")]
pub enum LiveEvent {
    Share(Share),
    Peer(PeerEvent),
}

/// BCHN's `getblockchaininfo`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct BchnChain {
    /// `main`, `chip`, `test`, `test4`, `scale` or `regtest`.
    pub chain: String,
    pub blocks: u64,
    pub headers: u64,
    #[serde(rename = "bestblockhash")]
    pub best_block_hash: String,
    pub difficulty: f64,
    #[serde(default, rename = "mediantime")]
    pub median_time: Option<u64>,
    #[serde(default, rename = "verificationprogress")]
    pub verification_progress: Option<f64>,
    #[serde(default, rename = "initialblockdownload")]
    pub initial_block_download: Option<bool>,
    #[serde(default)]
    pub size_on_disk: Option<u64>,
    #[serde(default)]
    pub pruned: Option<bool>,
    #[serde(default)]
    pub warnings: Option<String>,
}

/// BCHN's `getnetworkinfo`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct BchnNetwork {
    /// E.g. `/Bitcoin Cash Node:28.0.1(EB32.0)/`.
    pub subversion: String,
    #[serde(rename = "protocolversion")]
    pub protocol_version: u64,
    pub connections: u64,
    #[serde(default, rename = "networkactive")]
    pub network_active: Option<bool>,
    /// BCH per kilobyte.
    #[serde(default, rename = "relayfee")]
    pub relay_fee: Option<f64>,
}

/// BCHN's `getmempoolinfo`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct BchnMempool {
    /// Transactions.
    pub size: u64,
    pub bytes: u64,
    /// Memory the mempool uses, in bytes.
    pub usage: u64,
    #[serde(rename = "maxmempool")]
    pub max_mempool: u64,
    /// BCH per kilobyte.
    #[serde(rename = "mempoolminfee")]
    pub min_fee: f64,
    #[serde(default)]
    pub loaded: Option<bool>,
}

/// One of BCHN's `getpeerinfo`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct BchnPeer {
    pub id: u64,
    #[serde(rename = "addr")]
    pub address: String,
    #[serde(default, rename = "subver")]
    pub user_agent: String,
    #[serde(default)]
    pub inbound: bool,
    #[serde(default)]
    pub synced_blocks: Option<i64>,
    /// Seconds.
    #[serde(default, rename = "pingtime")]
    pub ping_time: Option<f64>,
    /// Unix seconds.
    #[serde(default, rename = "conntime")]
    pub connected_at: Option<u64>,
}

/// BCHN's `getmininginfo`, for the network hashrate.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct BchnMining {
    #[serde(default, rename = "networkhashps")]
    pub network_hashrate: Option<f64>,
}

/// One refresh of BCHN's status.
#[derive(Clone, Debug, PartialEq)]
pub struct BchnStatus {
    pub chain: BchnChain,
    pub network: BchnNetwork,
    pub mempool: BchnMempool,
    /// At most [`MAX_BCHN_PEERS`].
    pub peers: Vec<BchnPeer>,
    pub network_hashrate: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chain_info_parses_the_nodes_answer() {
        let body = r#"{"genesis_blockhash":"aa","chain_tip_height":42,"total_work":"0x10",
            "chain_tip_blockhash":"bb","top_candidate_height":43,"top_candidate_blockhash":"cc"}"#;
        let info: ChainInfo = serde_json::from_str(body).unwrap();
        assert_eq!(info.chain_tip_height, Some(42));
        assert_eq!(info.top_candidate_blockhash.as_deref(), Some("cc"));
        assert_eq!(info.total_work, "0x10");
    }

    #[test]
    fn test_share_parses_with_uncles() {
        let body = r#"{"blockhash":"ab","prev_blockhash":"cd","height":7,
            "miner_bitcoin_address":"bchtest:q","miner_address":"tp2pool:q","timestamp":5,
            "bits":486604799,"uncles":[{"blockhash":"ef","prev_blockhash":"aa",
            "miner_bitcoin_address":"bchtest:u","miner_address":"tp2pool:u","timestamp":4,
            "height":6}]}"#;
        let share: Share = serde_json::from_str(body).unwrap();
        assert_eq!(share.bits, 0x1d00_ffff);
        assert_eq!(share.uncles[0].height, Some(6));
    }

    #[test]
    fn test_live_share_event_parses() {
        let frame = r#"{"topic":"Share","data":{"blockhash":"ab","height":7,
            "miner_bitcoin_address":"bchtest:q","miner_address":"tp2pool:q","timestamp":5,
            "bits":486604799,"uncles":[]}}"#;
        let event: LiveEvent = serde_json::from_str(frame).unwrap();
        assert!(matches!(event, LiveEvent::Share(share) if share.height == 7));
    }

    #[test]
    fn test_live_peer_event_parses() {
        let frame = r#"{"topic":"Peer","data":{"peer_id":"12D3KooW","status":"Disconnected"}}"#;
        let event: LiveEvent = serde_json::from_str(frame).unwrap();
        assert_eq!(
            event,
            LiveEvent::Peer(PeerEvent {
                peer_id: "12D3KooW".into(),
                status: PeerStatus::Disconnected,
            })
        );
    }

    #[test]
    fn test_live_event_of_an_unknown_topic_is_an_error() {
        assert!(serde_json::from_str::<LiveEvent>(r#"{"topic":"Other","data":{}}"#).is_err());
    }

    #[test]
    fn test_peer_parses_the_peers_route() {
        let body = r#"[{"peer_id":"12D3","ip":"10.0.0.2","address":"/ip4/10.0.0.2/tcp/6884",
            "direction":"Outbound","connected_secs":90}]"#;
        let peers: Vec<Peer> = serde_json::from_str(body).unwrap();
        assert_eq!(peers[0].direction.as_deref(), Some("Outbound"));
        assert_eq!(peers[0].connected_secs, Some(90));
    }

    #[test]
    fn test_miners_page_parses() {
        let body = r#"{"miners_online":1,"workers_online":2,"total":1,"page":1,"pages":1,
            "page_size":25,"miners":[{"address":"bchtest:q","workers":[{"name":"rig",
            "active":true,"accepted_work":10,"best_share":5,"last_share_at":9}],
            "accepted_work":10,"best_share":5,"last_share_at":9,"payout_fraction":null}]}"#;
        let miners: Miners = serde_json::from_str(body).unwrap();
        assert_eq!(miners.miners[0].workers[0].name, "rig");
        assert_eq!(miners.miners[0].payout_fraction, None);
    }

    #[test]
    fn test_miner_parses_without_its_share_list() {
        let body = r#"{"address":"bchtest:q","address_type":"payout","anchor":"aa",
            "anchor_height":100,"window":{"newest_height":90,"oldest_height":1,"share_count":90,
            "weight":"900"},"share_count":9,"uncle_count":1,"weight":"95","share_fraction":0.1,
            "payout_fraction":0.105,"shares":[]}"#;
        let miner: Miner = serde_json::from_str(body).unwrap();
        assert_eq!(miner.window.share_count, 90);
        assert_eq!(miner.payout_fraction, 0.105);
    }

    #[test]
    fn test_found_block_parses() {
        let body = r#"[{"bitcoin_height":300,"bitcoin_blockhash":"00ab","share_blockhash":"cd",
            "bitcoin_time":1700000000,"miner_bitcoin_address":"bchtest:q"}]"#;
        let blocks: Vec<FoundBlock> = serde_json::from_str(body).unwrap();
        assert_eq!(blocks[0].bitcoin_height, 300);
    }

    #[test]
    fn test_bchn_chain_parses_getblockchaininfo() {
        let body = r#"{"chain":"chip","blocks":250000,"headers":250001,
            "bestblockhash":"0000ab","difficulty":12.5,"mediantime":1700000000,
            "verificationprogress":0.9999,"initialblockdownload":false,"chainwork":"00ff",
            "size_on_disk":1234,"pruned":false,"warnings":""}"#;
        let chain: BchnChain = serde_json::from_str(body).unwrap();
        assert_eq!(chain.chain, "chip");
        assert_eq!(chain.best_block_hash, "0000ab");
        assert_eq!(chain.initial_block_download, Some(false));
    }

    #[test]
    fn test_bchn_peer_parses_getpeerinfo() {
        let body = r#"[{"id":3,"addr":"203.0.113.5:48333","subver":"/Bitcoin Cash Node:28.0.1/",
            "inbound":false,"synced_blocks":250000,"pingtime":0.05,"conntime":1700000000}]"#;
        let peers: Vec<BchnPeer> = serde_json::from_str(body).unwrap();
        assert_eq!(peers[0].address, "203.0.113.5:48333");
        assert_eq!(peers[0].ping_time, Some(0.05));
    }

    #[test]
    fn test_bchn_mempool_parses_getmempoolinfo() {
        let body = r#"{"loaded":true,"size":12,"bytes":3400,"usage":9000,"maxmempool":300000000,
            "mempoolminfee":0.00001,"minrelaytxfee":0.00001}"#;
        let mempool: BchnMempool = serde_json::from_str(body).unwrap();
        assert_eq!(mempool.size, 12);
        assert_eq!(mempool.min_fee, 0.00001);
    }
}
