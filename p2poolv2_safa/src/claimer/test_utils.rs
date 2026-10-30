// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reusable, socket-free claimer fixtures and exact RPC transcripts.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;

use bitcoindrpc::BitcoindRpcError;
use serde_json::{Value, json};
use tempfile::{TempDir, tempdir};

use super::Claimer;
use super::chain::ChainRpc;
use super::config::{Config, RpcConfig, VaultPoint};
use super::files::{Journal, PendingClaim};
use super::wins::{Win, build_win_claim, parse_win};
use crate::{Vault, create_covenant};

pub(super) const CATEGORY: &str =
    "1e642d4ed9a0a44b3153a3f1fda841906eb1c9a2ce9daa2f85641ddd3e8700fe";
pub(super) const CATEGORY_WIRE: &str =
    "fe00873edd1d64852faa9dcea2c9b16e9041a8fdf1a353314ba4a0d94e2d641e";
pub(super) const GENESIS: &str = "8c5ecd6d4ce039490c233c9399cc1e591720218fd9712dc8c00edc172a06c471";
pub(super) const CLAIM_TXID: &str =
    "e30dc911a420cae10ba767c4eaf22ee720f56400d8099cfcd943050631d6a47f";
pub(super) const NEXT_STATE: &str = "030000001f090500fd3f021dd2872d1a";

pub(super) struct ExpectedCall {
    pub method: &'static str,
    pub parameters: Vec<Value>,
    pub result: Result<Value, BitcoindRpcError>,
    /// When set, require that this transaction was journaled before the call.
    pub persisted_pending: Option<(PathBuf, String)>,
}

pub(super) struct FakeRpc {
    calls: RefCell<VecDeque<ExpectedCall>>,
}

impl FakeRpc {
    pub fn new(calls: Vec<ExpectedCall>) -> Self {
        Self {
            calls: RefCell::new(calls.into()),
        }
    }

    pub fn assert_finished(&self) {
        assert!(
            self.calls.borrow().is_empty(),
            "unconsumed RPC expectations"
        );
    }
}

impl ChainRpc for FakeRpc {
    async fn call(&self, method: &str, parameters: Vec<Value>) -> Result<Value, BitcoindRpcError> {
        let expected = self
            .calls
            .borrow_mut()
            .pop_front()
            .expect("unexpected RPC call");
        assert_eq!(method, expected.method);
        assert_eq!(parameters, expected.parameters);
        if let Some((path, txid)) = expected.persisted_pending {
            let journal: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            assert_eq!(journal["pending"][0]["txid"], txid);
        }
        expected.result
    }
}

pub(super) struct RuntimeFixture {
    pub _directory: TempDir,
    pub claimer: Claimer<FakeRpc>,
    pub vault: Vault,
    pub win: Win,
    pub prepared: PendingClaim,
}

/// Build a verified public-data claim without installing any RPC expectations.
pub(super) fn runtime_fixture(client: FakeRpc, dry_run: bool) -> RuntimeFixture {
    let directory = tempdir().unwrap();
    let origin = VaultPoint {
        txid: GENESIS.into(),
        vout: 3,
    };
    let config = Config {
        deployment: directory.path().join("deployment.json"),
        vault: origin.clone(),
        token_file: directory.path().join("tokens.json"),
        wins_log: directory.path().join("wins.jsonl"),
        payout_scripts: vec!["76a914c40a24bf5b3cfe561cf36afea1b59fe61383e80688ac".into()],
        split_scripts: vec!["76a914b25a2f0a68073fad864f684bf26e350b4526354b88ac".into()],
        p2pool_config: None,
        rpc: Some(RpcConfig {
            url: "http://127.0.0.1:1".into(),
            username: "unused".into(),
            password_file: directory.path().join("unused-password"),
        }),
        fee_rate: 1,
        delay_seconds: [0, 0],
        poll_seconds: 5,
        journal: directory.path().join("journal.json"),
    };
    let deployment = json!({"network":"chipnet", "threads":4,
        "virtual_sats":"84000000000", "category":CATEGORY});
    let covenant = create_covenant("chipnet", 4, 84_000_000_000).unwrap();
    let vault = Vault {
        category: hex::decode(CATEGORY).unwrap(),
        value: 2_000_000,
        token_amount: 525_000_000_000_000,
        commitment: hex::decode("0300000011090500ffff001dba7d451a").unwrap(),
    };
    let win = parse_win(
        include_bytes!("../../test_data/tsafa-live-win.jsonl"),
        CATEGORY_WIRE,
    )
    .unwrap()
    .unwrap();
    let built = build_win_claim(
        &deployment,
        &win,
        &origin,
        &vault,
        &config.payout_scripts,
        &config.split_scripts,
        config.fee_rate,
    )
    .unwrap();
    let prepared = PendingClaim {
        win: win.id(),
        txid: built["txid"].as_str().unwrap().into(),
        hex: built["hex"].as_str().unwrap().into(),
        next_state: built["next_state"].as_str().unwrap().into(),
        previous: origin.clone(),
    };
    let mut journal = Journal::load(&config.journal, &origin).unwrap();
    journal.commitment = Some(hex::encode(&vault.commitment));
    journal.thread = Some(3);
    journal.wins.push(win.clone());
    journal.delays.insert(win.id(), 0);
    fs::write(
        &config.token_file,
        serde_json::to_vec(&json!({"keep":true, "tokens":[{
            "category":CATEGORY_WIRE, "other":123, "safa":{
                "state":hex::encode(&vault.commitment), "keep":"unchanged"}
        }]}))
        .unwrap(),
    )
    .unwrap();
    journal.save(&config.journal).unwrap();
    RuntimeFixture {
        _directory: directory,
        claimer: Claimer {
            config,
            deployment,
            covenant,
            category_display: CATEGORY.into(),
            category_wire: CATEGORY_WIRE.into(),
            client,
            journal,
            dry_run,
        },
        vault,
        win,
        prepared,
    }
}
