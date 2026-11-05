// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Explicit opt-in checks against two disposable BCHN regtest nodes, and
//! against a running node's Stratum V2 port.

use super::payout_outputs::{
    EXACT_OUTPUTS, PAYOUT_OUTPUTS_EXTENSION, PAYOUT_OUTPUTS_STALE, REQUEST_PAYOUT_OUTPUTS,
    REQUEST_PAYOUT_OUTPUTS_ERROR, REQUEST_PAYOUT_OUTPUTS_SUCCESS, RequestPayoutOutputsError,
    RequestPayoutOutputsSuccess,
};
use super::session::encode_extension_frame;
use super::{PoolTransactionCache, Sv2Mining, bind, serve};
use crate::shares::auxpow::{
    AuxProof, AuxiliaryTree, DisclosedLeaf, Disclosure, TokenLeaf, TokenSplit,
};
use crate::shares::merge_mining::MergeMiningCommitment;
use crate::shares::share_block::ShareHeader;
use crate::stratum::merge_disclosure::{
    EXTENSIONS_NEGOTIATION, REQUEST_EXTENSIONS, REQUEST_EXTENSIONS_SUCCESS, extensions_payload,
};
use crate::stratum::work::block_template::BlockTemplate;
use crate::test_utils::{
    TestSv2Client, test_bch_custom_coinbase, test_bch_custom_job_with_aux,
    test_bch_prepared_for_template, test_bch_solve_custom_job, test_bch_stratum_context,
    test_custom_job_from_payout_outputs, test_declared_coinbase_parts, test_payout_outputs_request,
    test_sv2_authority, test_sv2_setup,
};
use binary_sv2::{B016M, B032, B064K, B0255, Deserialize, Seq064K, U256};
use bitcoin::consensus::{deserialize, serialize};
use bitcoin::hashes::Hash;
use bitcoin::script::Builder;
use bitcoin::{Address, Amount, Block, BlockHash, Network, PubkeyHash, Transaction, TxOut};
use bitcoindrpc::BitcoindRpcClient;
use common_messages_sv2::{
    MESSAGE_TYPE_SETUP_CONNECTION, MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS, Protocol,
};
use job_declaration_sv2::{
    AllocateMiningJobToken, AllocateMiningJobTokenSuccess, DeclareMiningJob,
    DeclareMiningJobSuccess, MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN,
    MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN_SUCCESS, MESSAGE_TYPE_DECLARE_MINING_JOB,
    MESSAGE_TYPE_DECLARE_MINING_JOB_SUCCESS, MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS,
    MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS_SUCCESS, ProvideMissingTransactions,
    ProvideMissingTransactionsSuccess,
};
use mining_sv2::{
    MESSAGE_TYPE_MINING_SET_NEW_PREV_HASH, MESSAGE_TYPE_NEW_EXTENDED_MINING_JOB,
    MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL, MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS,
    MESSAGE_TYPE_SET_CUSTOM_MINING_JOB, MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_SUCCESS,
    MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED, MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS, NewExtendedMiningJob,
    OpenExtendedMiningChannel, OpenExtendedMiningChannelSuccess, SetCustomMiningJob,
    SetCustomMiningJobSuccess, SetNewPrevHash, SubmitSharesExtended, SubmitSharesSuccess,
};
use p2poolv2_config::DEFAULT_VERSION_MASK;
use p2poolv2_wallet::bch_share::{BchShareNetwork, encode_payout_address};
use serde_json::{Value, json};
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Semaphore, watch};
use tokio::time::{sleep, timeout};

#[tokio::test]
#[ignore = "requires two disposable BCHN nodes and a funded jd-regtest wallet; see docs/bch-job-declaration.md"]
async fn bchn_declared_jobs_reach_both_real_nodes_over_noise() {
    let client_url = env::var("BCHN_JD_CLIENT_RPC").expect("set BCHN_JD_CLIENT_RPC");
    let pool_url = env::var("BCHN_JD_POOL_RPC").expect("set BCHN_JD_POOL_RPC");
    let client_socket: SocketAddr = client_url.strip_prefix("http://").unwrap().parse().unwrap();
    let pool_socket: SocketAddr = pool_url.strip_prefix("http://").unwrap().parse().unwrap();
    assert!(client_socket.ip().is_loopback() && pool_socket.ip().is_loopback());
    assert_ne!(client_socket, pool_socket);
    let client =
        BitcoindRpcClient::new(&client_url, "p2pool-jd-test", "local-regtest-only").unwrap();
    let pool = BitcoindRpcClient::new(&pool_url, "p2pool-jd-test", "local-regtest-only").unwrap();
    let wallet = BitcoindRpcClient::new(
        &format!("{client_url}/wallet/jd-regtest"),
        "p2pool-jd-test",
        "local-regtest-only",
    )
    .unwrap();
    let client_chain: Value = client.request("getblockchaininfo", vec![]).await.unwrap();
    let pool_chain: Value = pool.request("getblockchaininfo", vec![]).await.unwrap();
    assert_eq!(client_chain["chain"], "regtest");
    assert_eq!(pool_chain["chain"], "regtest");

    let payout = Address::p2pkh(PubkeyHash::from_byte_array([0x22; 20]), Network::Regtest);
    let identity = encode_payout_address(&payout, BchShareNetwork::Regtest).unwrap();
    let full_txid: String = wallet
        .request("sendtoaddress", vec![json!(identity), json!(1)])
        .await
        .unwrap();
    let template = timeout(Duration::from_secs(10), async {
        loop {
            let template: BlockTemplate =
                serde_json::from_str(&pool.get_bch_block_template().await.unwrap()).unwrap();
            if template
                .transactions
                .iter()
                .any(|transaction| transaction.txid == full_txid)
            {
                break template;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("transaction did not propagate to the pool node");
    let full_height = template.height;
    let full_value = template.coinbasevalue;
    let transactions = template
        .transactions
        .iter()
        .map(Transaction::try_from)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let prepared = test_bch_prepared_for_template(template);
    let (mut ctx, mut emissions, _dirs) = test_bch_stratum_context(&pool_url).await;
    ctx.bitcoindrpc_client = pool.clone();
    let cache = PoolTransactionCache::new();
    cache.set_prepared(Arc::clone(&prepared));
    let (template_sender, template_rx) = watch::channel(Some(Arc::clone(&prepared)));
    let authority = test_sv2_authority();
    let listener = bind("127.0.0.1", 0).await.unwrap();
    let address = listener.local_addr().unwrap();
    let connection_limit = Arc::new(Semaphore::new(3));
    let (shutdown_sender, shutdown_receiver) = watch::channel(());
    let server = tokio::spawn(serve(
        listener,
        Arc::clone(&connection_limit),
        cache.clone(),
        Some(MergeMiningCommitment::empty()),
        Some(Sv2Mining {
            ctx,
            version_mask: DEFAULT_VERSION_MASK,
            template_rx,
        }),
        Some(authority),
        shutdown_receiver,
    ));
    let mut mining = TestSv2Client::connect(address, authority.public_key).await;
    mining
        .send_message(
            MESSAGE_TYPE_SETUP_CONNECTION,
            test_sv2_setup(Protocol::MiningProtocol, 2),
        )
        .await;
    mining.receive(MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS).await;
    mining
        .send_message(
            MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
            OpenExtendedMiningChannel {
                request_id: 1,
                user_identity: identity.as_str().try_into().unwrap(),
                nominal_hash_rate: 1.0,
                max_target: U256::new(&[0xff; 32]).unwrap(),
                min_extranonce_size: 8,
            },
        )
        .await;
    let mut open_payload = mining
        .receive(MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS)
        .await;
    let opened = OpenExtendedMiningChannelSuccess::from_bytes(&mut open_payload).unwrap();
    let extranonce_prefix = opened.extranonce_prefix.as_bytes().to_vec();
    let channel_id = opened.channel_id;
    assert_eq!(channel_id, 1);
    mining.receive(MESSAGE_TYPE_NEW_EXTENDED_MINING_JOB).await;
    mining.receive(MESSAGE_TYPE_MINING_SET_NEW_PREV_HASH).await;
    let mut full_jdc = TestSv2Client::connect(address, authority.public_key).await;
    full_jdc
        .send_message(
            MESSAGE_TYPE_SETUP_CONNECTION,
            test_sv2_setup(Protocol::JobDeclarationProtocol, 1),
        )
        .await;
    full_jdc
        .receive(MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS)
        .await;
    full_jdc
        .send_message(
            MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN,
            AllocateMiningJobToken {
                request_id: 2,
                user_identifier: "miner".try_into().unwrap(),
            },
        )
        .await;
    let mut allocated_payload = full_jdc
        .receive(MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN_SUCCESS)
        .await;
    let allocated = AllocateMiningJobTokenSuccess::from_bytes(&mut allocated_payload).unwrap();
    let token = allocated.mining_job_token.as_bytes().to_vec();
    let transaction_bytes = transactions.iter().map(serialize).collect::<Vec<_>>();
    let wtxids = transactions
        .iter()
        .map(|transaction| transaction.compute_wtxid().to_byte_array())
        .collect::<Vec<_>>();
    // Two synthetic tokens whose target is regtest's limit, so every regtest
    // block wins both.
    let token_payout =
        Address::p2pkh(PubkeyHash::from_byte_array([0x22; 20]), Network::Regtest).script_pubkey();
    let first_work = TokenLeaf::new(
        [0xa1; 32],
        &[0x51],
        [0xa2; 32],
        token_payout.as_bytes(),
        0x207f_ffff,
        [0; 32],
    );
    let second_work = TokenLeaf::new(
        [0xb1; 32],
        &[0x52],
        [0xb2; 32],
        token_payout.as_bytes(),
        0x207f_ffff,
        [0; 32],
    );
    let auxiliary_tree = AuxiliaryTree::build(&[first_work, second_work]).unwrap();
    // The declaration discloses its leaves, which pay the miner with no
    // split: this node takes no donation or fee.
    let disclosure = Disclosure {
        leaves: vec![
            DisclosedLeaf {
                leaf: first_work,
                split: TokenSplit::default(),
            },
            DisclosedLeaf {
                leaf: second_work,
                split: TokenSplit::default(),
            },
        ],
    };
    let (mut request_bytes, declared) = test_bch_custom_job_with_aux(
        Arc::clone(&prepared),
        token.clone(),
        transactions,
        true,
        full_value,
        auxiliary_tree.commitment,
    );
    let declaration = declared.declaration.unwrap();
    full_jdc
        .send_message(
            MESSAGE_TYPE_DECLARE_MINING_JOB,
            DeclareMiningJob {
                request_id: 3,
                mining_job_token: B0255::new(&token).unwrap(),
                version: declaration.version,
                coinbase_tx_prefix: B064K::new(&declaration.prefix).unwrap(),
                coinbase_tx_suffix: B064K::new(&declaration.suffix).unwrap(),
                wtxid_list: Seq064K::new(
                    wtxids
                        .iter()
                        .map(|wtxid| U256::new(wtxid).unwrap())
                        .collect(),
                )
                .unwrap(),
                excess_data: B064K::new(&disclosure.encode()).unwrap(),
            },
        )
        .await;
    let mut missing_payload = full_jdc
        .receive(MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS)
        .await;
    let missing = ProvideMissingTransactions::from_bytes(&mut missing_payload).unwrap();
    assert_eq!(missing.request_id, 3);
    assert_eq!(
        missing.unknown_tx_position_list.into_inner(),
        (0..transaction_bytes.len())
            .map(|index| u16::try_from(index).unwrap())
            .collect::<Vec<_>>()
    );
    full_jdc
        .send_message(
            MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS_SUCCESS,
            ProvideMissingTransactionsSuccess {
                request_id: 3,
                transaction_list: Seq064K::new(
                    transaction_bytes
                        .iter()
                        .map(|bytes| B016M::new(bytes).unwrap())
                        .collect(),
                )
                .unwrap(),
            },
        )
        .await;
    let mut accepted_payload = full_jdc
        .receive(MESSAGE_TYPE_DECLARE_MINING_JOB_SUCCESS)
        .await;
    let accepted = DeclareMiningJobSuccess::from_bytes(&mut accepted_payload).unwrap();
    assert_eq!(accepted.request_id, 3);
    let new_token = accepted.new_mining_job_token.as_bytes().to_vec();
    assert_ne!(new_token, token);
    let mut request = SetCustomMiningJob::from_bytes(&mut request_bytes).unwrap();
    request.token = B0255::new(&new_token).unwrap();
    mining
        .send_message(MESSAGE_TYPE_SET_CUSTOM_MINING_JOB, request.clone())
        .await;
    let mut installed_payload = mining
        .receive(MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_SUCCESS)
        .await;
    let installed = SetCustomMiningJobSuccess::from_bytes(&mut installed_payload).unwrap();
    assert_eq!(installed.request_id, request.request_id);
    let full_header = test_bch_solve_custom_job(&request, &extranonce_prefix);
    mining
        .send_message(
            MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED,
            SubmitSharesExtended {
                channel_id,
                sequence_number: 1,
                job_id: installed.job_id,
                nonce: full_header.nonce,
                ntime: full_header.time,
                version: request.version,
                extranonce: B032::new(&[0; 8]).unwrap(),
            },
        )
        .await;
    let mut submitted_payload = mining.receive(MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS).await;
    let submitted = SubmitSharesSuccess::from_bytes(&mut submitted_payload).unwrap();
    assert_eq!(submitted.last_sequence_number, 1);
    let pool_tip: BlockHash = pool.request("getbestblockhash", vec![]).await.unwrap();
    assert_eq!(pool_tip, full_header.block_hash());
    timeout(Duration::from_secs(10), async {
        loop {
            let tip: BlockHash = client.request("getbestblockhash", vec![]).await.unwrap();
            if tip == pool_tip {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the client node did not validate the full-template block");
    let emission = emissions.try_recv().unwrap();
    assert_eq!(emission.header, full_header);
    let share = ShareHeader::from_commitment_and_header(
        emission.share_commitment,
        emission.header,
        u64::from(full_height),
        emission.coinbase_proof,
    );
    assert_eq!(
        share
            .coinbase_proof
            .merkle_root(&share, &emission.template_merkle_branches)
            .unwrap(),
        full_header.merkle_root
    );
    let full_coinbase = test_bch_custom_coinbase(&request, &extranonce_prefix);
    let full_proofs = [first_work, second_work]
        .iter()
        .zip(&auxiliary_tree.branches)
        .map(|(leaf, branch)| {
            let proof = AuxProof::new(
                full_header,
                &full_coinbase,
                &emission.template_merkle_branches,
                auxiliary_tree.commitment,
                branch,
            )
            .unwrap();
            proof.verify(leaf, 16, None).unwrap();
            hex::encode(proof.to_bytes().unwrap())
        })
        .collect::<Vec<_>>();
    println!(
        "{}",
        json!({"mode": "full-template", "height": full_height, "hash": pool_tip.to_string(), "coinbase_value": full_value, "transaction": full_txid, "tokens": 2, "aux_proofs": full_proofs})
    );

    let raw: String = wallet
        .request(
            "createrawtransaction",
            vec![json!([]), json!({identity.clone(): 1})],
        )
        .await
        .unwrap();
    let funded: Value = wallet
        .request("fundrawtransaction", vec![json!(raw)])
        .await
        .unwrap();
    let signed: Value = wallet
        .request("signrawtransactionwithwallet", vec![funded["hex"].clone()])
        .await
        .unwrap();
    assert_eq!(signed["complete"], true);
    let private_transaction: Transaction =
        deserialize(&hex::decode(signed["hex"].as_str().unwrap()).unwrap()).unwrap();
    let private_txid = private_transaction.compute_txid().to_string();
    let private_fee = Amount::from_btc(funded["fee"].as_f64().unwrap())
        .unwrap()
        .to_sat();
    let pool_mempool: Vec<String> = pool.request("getrawmempool", vec![]).await.unwrap();
    let client_mempool: Vec<String> = client.request("getrawmempool", vec![]).await.unwrap();
    assert!(!pool_mempool.contains(&private_txid));
    assert!(!client_mempool.contains(&private_txid));
    let template: BlockTemplate =
        serde_json::from_str(&pool.get_bch_block_template().await.unwrap()).unwrap();
    assert!(template.transactions.is_empty());
    let private_height = template.height;
    let private_value = template.coinbasevalue + private_fee;
    let prepared = test_bch_prepared_for_template(template);
    cache.set_prepared(Arc::clone(&prepared));
    template_sender.send(Some(Arc::clone(&prepared))).unwrap();
    mining.receive(MESSAGE_TYPE_NEW_EXTENDED_MINING_JOB).await;
    mining.receive(MESSAGE_TYPE_MINING_SET_NEW_PREV_HASH).await;
    let mut private_jdc = TestSv2Client::connect(address, authority.public_key).await;
    private_jdc
        .send_message(
            MESSAGE_TYPE_SETUP_CONNECTION,
            test_sv2_setup(Protocol::JobDeclarationProtocol, 0),
        )
        .await;
    private_jdc
        .receive(MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS)
        .await;
    private_jdc
        .send_message(
            MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN,
            AllocateMiningJobToken {
                request_id: 4,
                user_identifier: "miner".try_into().unwrap(),
            },
        )
        .await;
    let mut allocated_payload = private_jdc
        .receive(MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN_SUCCESS)
        .await;
    let allocated = AllocateMiningJobTokenSuccess::from_bytes(&mut allocated_payload).unwrap();
    let token = allocated.mining_job_token.as_bytes().to_vec();
    // A Coinbase-only job declares no leaves, so it merge-mines no token:
    // its output 0 is the empty commitment.
    let (mut request_bytes, _) = test_bch_custom_job_with_aux(
        Arc::clone(&prepared),
        token,
        vec![private_transaction.clone()],
        false,
        private_value,
        MergeMiningCommitment::empty(),
    );
    let request = SetCustomMiningJob::from_bytes(&mut request_bytes).unwrap();
    let private_header = test_bch_solve_custom_job(&request, &extranonce_prefix);
    let private_block = Block {
        header: private_header,
        txdata: vec![
            test_bch_custom_coinbase(&request, &extranonce_prefix),
            private_transaction,
        ],
    };
    client
        .validate_block_proposal(&private_block)
        .await
        .unwrap();
    mining
        .send_message(MESSAGE_TYPE_SET_CUSTOM_MINING_JOB, request.clone())
        .await;
    let mut installed_payload = mining
        .receive(MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_SUCCESS)
        .await;
    let installed = SetCustomMiningJobSuccess::from_bytes(&mut installed_payload).unwrap();
    mining
        .send_message(
            MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED,
            SubmitSharesExtended {
                channel_id,
                sequence_number: 2,
                job_id: installed.job_id,
                nonce: private_header.nonce,
                ntime: private_header.time,
                version: request.version,
                extranonce: B032::new(&[0; 8]).unwrap(),
            },
        )
        .await;
    let mut submitted_payload = mining.receive(MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS).await;
    let submitted = SubmitSharesSuccess::from_bytes(&mut submitted_payload).unwrap();
    assert_eq!(submitted.last_sequence_number, 2);
    let emission = emissions.try_recv().unwrap();
    assert!(emission.blocktemplate.transactions.is_empty());
    assert_eq!(emission.header, private_header);
    let still_pool_tip: BlockHash = pool.request("getbestblockhash", vec![]).await.unwrap();
    assert_eq!(
        still_pool_tip, pool_tip,
        "private submissions must not broadcast an incomplete block"
    );
    let share = ShareHeader::from_commitment_and_header(
        emission.share_commitment,
        emission.header,
        u64::from(private_height),
        emission.coinbase_proof,
    );
    assert_eq!(
        share
            .coinbase_proof
            .merkle_root(&share, &emission.template_merkle_branches)
            .unwrap(),
        private_header.merkle_root
    );
    let mut excessive = private_block.clone();
    excessive.txdata[0].output[0].value += Amount::from_sat(1);
    excessive.header.merkle_root = excessive.compute_merkle_root().unwrap();
    assert!(
        client
            .validate_block_proposal(&excessive)
            .await
            .unwrap_err()
            .to_string()
            .contains("bad-cb-amount")
    );
    let result: Value = client
        .request(
            "submitblock",
            vec![json!(hex::encode(serialize(&private_block)))],
        )
        .await
        .unwrap();
    assert!(result.is_null());
    timeout(Duration::from_secs(10), async {
        loop {
            let tip: BlockHash = pool.request("getbestblockhash", vec![]).await.unwrap();
            if tip == private_header.block_hash() {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the pool node did not validate the private block broadcast by the client");
    println!(
        "{}",
        json!({"mode": "coinbase-only", "height": private_height, "hash": private_header.block_hash().to_string(), "coinbase_value": private_value, "private_fee": private_fee, "transaction": private_txid, "tokens": 0})
    );
    drop(mining);
    drop(full_jdc);
    drop(private_jdc);
    shutdown_sender.send(()).unwrap();
    timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    let permits = timeout(
        Duration::from_secs(5),
        connection_limit.acquire_many_owned(3),
    )
    .await
    .unwrap()
    .unwrap();
    drop(permits);
}

/// A client that knows nothing of P2Pool's coinbase builds both jobs from
/// `RequestPayoutOutputs.Success` alone, and two real BCHN nodes accept the
/// blocks: the Full-Template one after the pool node's proposal check and
/// submission, the Coinbase-only one, on a private transaction and the
/// client's own revenue, broadcast by the client.
#[tokio::test]
#[ignore = "requires two disposable BCHN nodes and a funded jd-regtest wallet; see docs/bch-job-declaration.md"]
async fn bchn_jobs_on_requested_payout_outputs_reach_both_real_nodes() {
    let client_url = env::var("BCHN_JD_CLIENT_RPC").expect("set BCHN_JD_CLIENT_RPC");
    let pool_url = env::var("BCHN_JD_POOL_RPC").expect("set BCHN_JD_POOL_RPC");
    let client_socket: SocketAddr = client_url.strip_prefix("http://").unwrap().parse().unwrap();
    let pool_socket: SocketAddr = pool_url.strip_prefix("http://").unwrap().parse().unwrap();
    assert!(client_socket.ip().is_loopback() && pool_socket.ip().is_loopback());
    assert_ne!(client_socket, pool_socket);
    let client =
        BitcoindRpcClient::new(&client_url, "p2pool-jd-test", "local-regtest-only").unwrap();
    let pool = BitcoindRpcClient::new(&pool_url, "p2pool-jd-test", "local-regtest-only").unwrap();
    let wallet = BitcoindRpcClient::new(
        &format!("{client_url}/wallet/jd-regtest"),
        "p2pool-jd-test",
        "local-regtest-only",
    )
    .unwrap();
    let client_chain: Value = client.request("getblockchaininfo", vec![]).await.unwrap();
    let pool_chain: Value = pool.request("getblockchaininfo", vec![]).await.unwrap();
    assert_eq!(client_chain["chain"], "regtest");
    assert_eq!(pool_chain["chain"], "regtest");

    let payout = Address::p2pkh(PubkeyHash::from_byte_array([0x22; 20]), Network::Regtest);
    let identity = encode_payout_address(&payout, BchShareNetwork::Regtest).unwrap();
    let full_txid: String = wallet
        .request("sendtoaddress", vec![json!(identity), json!(1)])
        .await
        .unwrap();
    let template = timeout(Duration::from_secs(10), async {
        loop {
            let template: BlockTemplate =
                serde_json::from_str(&pool.get_bch_block_template().await.unwrap()).unwrap();
            if template
                .transactions
                .iter()
                .any(|transaction| transaction.txid == full_txid)
            {
                break template;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("transaction did not propagate to the pool node");
    let full_height = template.height;
    let full_value = template.coinbasevalue;
    let transactions = template
        .transactions
        .iter()
        .map(Transaction::try_from)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let prepared = test_bch_prepared_for_template(template);
    let (mut ctx, mut emissions, _dirs) = test_bch_stratum_context(&pool_url).await;
    ctx.bitcoindrpc_client = pool.clone();
    let cache = PoolTransactionCache::new();
    cache.set_prepared(Arc::clone(&prepared));
    let (template_sender, template_rx) = watch::channel(Some(Arc::clone(&prepared)));
    let authority = test_sv2_authority();
    let listener = bind("127.0.0.1", 0).await.unwrap();
    let address = listener.local_addr().unwrap();
    let connection_limit = Arc::new(Semaphore::new(3));
    let (shutdown_sender, shutdown_receiver) = watch::channel(());
    let server = tokio::spawn(serve(
        listener,
        Arc::clone(&connection_limit),
        cache.clone(),
        Some(MergeMiningCommitment::empty()),
        Some(Sv2Mining {
            ctx,
            version_mask: DEFAULT_VERSION_MASK,
            template_rx,
        }),
        Some(authority),
        shutdown_receiver,
    ));
    let mut mining = TestSv2Client::connect(address, authority.public_key).await;
    mining
        .send_message(
            MESSAGE_TYPE_SETUP_CONNECTION,
            test_sv2_setup(Protocol::MiningProtocol, 2),
        )
        .await;
    mining.receive(MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS).await;
    mining
        .send_message(
            MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
            OpenExtendedMiningChannel {
                request_id: 1,
                user_identity: identity.as_str().try_into().unwrap(),
                nominal_hash_rate: 1.0,
                max_target: U256::new(&[0xff; 32]).unwrap(),
                min_extranonce_size: 8,
            },
        )
        .await;
    let mut open_payload = mining
        .receive(MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS)
        .await;
    let opened = OpenExtendedMiningChannelSuccess::from_bytes(&mut open_payload).unwrap();
    let extranonce_prefix = opened.extranonce_prefix.as_bytes().to_vec();
    let channel_id = opened.channel_id;
    mining.receive(MESSAGE_TYPE_NEW_EXTENDED_MINING_JOB).await;
    mining.receive(MESSAGE_TYPE_MINING_SET_NEW_PREV_HASH).await;
    let extensions = encode_extension_frame(
        EXTENSIONS_NEGOTIATION,
        REQUEST_EXTENSIONS,
        false,
        &extensions_payload(1, &[PAYOUT_OUTPUTS_EXTENSION]),
    )
    .unwrap();
    // The client's own scriptSig prefix, which P2Pool never chose.
    let full_prefix = Builder::new()
        .push_int(i64::from(full_height))
        .push_slice(b"standard client")
        .into_bytes();

    let mut full_jdc = TestSv2Client::connect(address, authority.public_key).await;
    full_jdc
        .send_message(
            MESSAGE_TYPE_SETUP_CONNECTION,
            test_sv2_setup(Protocol::JobDeclarationProtocol, 1),
        )
        .await;
    full_jdc
        .receive(MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS)
        .await;
    full_jdc.send(extensions.clone()).await;
    assert_eq!(
        full_jdc
            .receive_extension(EXTENSIONS_NEGOTIATION, REQUEST_EXTENSIONS_SUCCESS)
            .await,
        extensions_payload(1, &[PAYOUT_OUTPUTS_EXTENSION])
    );
    full_jdc
        .send_message(
            MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN,
            AllocateMiningJobToken {
                request_id: 2,
                user_identifier: identity.as_str().try_into().unwrap(),
            },
        )
        .await;
    let mut allocated_payload = full_jdc
        .receive(MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN_SUCCESS)
        .await;
    let allocated = AllocateMiningJobTokenSuccess::from_bytes(&mut allocated_payload).unwrap();
    let token = allocated.mining_job_token.as_bytes().to_vec();
    full_jdc
        .send(
            encode_extension_frame(
                PAYOUT_OUTPUTS_EXTENSION,
                REQUEST_PAYOUT_OUTPUTS,
                false,
                &test_payout_outputs_request(
                    3,
                    &token,
                    prepared.template(),
                    full_value,
                    &full_prefix,
                ),
            )
            .unwrap(),
        )
        .await;
    let mut success_payload = full_jdc
        .receive_extension(PAYOUT_OUTPUTS_EXTENSION, REQUEST_PAYOUT_OUTPUTS_SUCCESS)
        .await;
    let success = RequestPayoutOutputsSuccess::from_bytes(&mut success_payload).unwrap();
    assert_eq!(success.flags, EXACT_OUTPUTS);
    let mut request_bytes = test_custom_job_from_payout_outputs(
        &success,
        &token,
        prepared.template(),
        &full_prefix,
        &transactions,
    );
    let mut request = SetCustomMiningJob::from_bytes(&mut request_bytes).unwrap();
    let (coinbase_prefix, coinbase_suffix) = test_declared_coinbase_parts(&request);
    let transaction_bytes = transactions.iter().map(serialize).collect::<Vec<_>>();
    let wtxids = transactions
        .iter()
        .map(|transaction| transaction.compute_wtxid().to_byte_array())
        .collect::<Vec<_>>();
    full_jdc
        .send_message(
            MESSAGE_TYPE_DECLARE_MINING_JOB,
            DeclareMiningJob {
                request_id: 4,
                mining_job_token: B0255::new(&token).unwrap(),
                version: request.version,
                coinbase_tx_prefix: B064K::new(&coinbase_prefix).unwrap(),
                coinbase_tx_suffix: B064K::new(&coinbase_suffix).unwrap(),
                wtxid_list: Seq064K::new(
                    wtxids
                        .iter()
                        .map(|wtxid| U256::new(wtxid).unwrap())
                        .collect(),
                )
                .unwrap(),
                excess_data: B064K::new(&[]).unwrap(),
            },
        )
        .await;
    let mut missing_payload = full_jdc
        .receive(MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS)
        .await;
    let missing = ProvideMissingTransactions::from_bytes(&mut missing_payload).unwrap();
    assert_eq!(missing.request_id, 4);
    full_jdc
        .send_message(
            MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS_SUCCESS,
            ProvideMissingTransactionsSuccess {
                request_id: 4,
                transaction_list: Seq064K::new(
                    transaction_bytes
                        .iter()
                        .map(|bytes| B016M::new(bytes).unwrap())
                        .collect(),
                )
                .unwrap(),
            },
        )
        .await;
    let mut accepted_payload = full_jdc
        .receive(MESSAGE_TYPE_DECLARE_MINING_JOB_SUCCESS)
        .await;
    let accepted = DeclareMiningJobSuccess::from_bytes(&mut accepted_payload).unwrap();
    let new_token = accepted.new_mining_job_token.as_bytes().to_vec();
    request.token = B0255::new(&new_token).unwrap();
    mining
        .send_message(MESSAGE_TYPE_SET_CUSTOM_MINING_JOB, request.clone())
        .await;
    let mut installed_payload = mining
        .receive(MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_SUCCESS)
        .await;
    let installed = SetCustomMiningJobSuccess::from_bytes(&mut installed_payload).unwrap();
    let full_header = test_bch_solve_custom_job(&request, &extranonce_prefix);
    mining
        .send_message(
            MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED,
            SubmitSharesExtended {
                channel_id,
                sequence_number: 1,
                job_id: installed.job_id,
                nonce: full_header.nonce,
                ntime: full_header.time,
                version: request.version,
                extranonce: B032::new(&[0; 8]).unwrap(),
            },
        )
        .await;
    let mut submitted_payload = mining.receive(MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS).await;
    let submitted = SubmitSharesSuccess::from_bytes(&mut submitted_payload).unwrap();
    assert_eq!(submitted.last_sequence_number, 1);
    let pool_tip: BlockHash = pool.request("getbestblockhash", vec![]).await.unwrap();
    assert_eq!(pool_tip, full_header.block_hash());
    timeout(Duration::from_secs(10), async {
        loop {
            let tip: BlockHash = client.request("getbestblockhash", vec![]).await.unwrap();
            if tip == pool_tip {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the client node did not validate the full-template block");
    let emission = emissions.try_recv().unwrap();
    assert_eq!(emission.header, full_header);
    println!(
        "{}",
        json!({"mode": "full-template", "source": "requested-payout-outputs", "height": full_height, "hash": pool_tip.to_string(), "coinbase_value": full_value, "transaction": full_txid})
    );

    let raw: String = wallet
        .request(
            "createrawtransaction",
            vec![json!([]), json!({identity.clone(): 1})],
        )
        .await
        .unwrap();
    let funded: Value = wallet
        .request("fundrawtransaction", vec![json!(raw)])
        .await
        .unwrap();
    let signed: Value = wallet
        .request("signrawtransactionwithwallet", vec![funded["hex"].clone()])
        .await
        .unwrap();
    assert_eq!(signed["complete"], true);
    let private_transaction: Transaction =
        deserialize(&hex::decode(signed["hex"].as_str().unwrap()).unwrap()).unwrap();
    let private_txid = private_transaction.compute_txid().to_string();
    let private_fee = Amount::from_btc(funded["fee"].as_f64().unwrap())
        .unwrap()
        .to_sat();
    let template: BlockTemplate =
        serde_json::from_str(&pool.get_bch_block_template().await.unwrap()).unwrap();
    assert!(template.transactions.is_empty());
    let private_height = template.height;
    // The client's template pays the private fee the pool never saw.
    let private_value = template.coinbasevalue + private_fee;
    let prepared = test_bch_prepared_for_template(template);
    cache.set_prepared(Arc::clone(&prepared));
    template_sender.send(Some(Arc::clone(&prepared))).unwrap();
    mining.receive(MESSAGE_TYPE_NEW_EXTENDED_MINING_JOB).await;
    mining.receive(MESSAGE_TYPE_MINING_SET_NEW_PREV_HASH).await;
    let private_prefix = Builder::new()
        .push_int(i64::from(private_height))
        .push_slice(b"standard client")
        .into_bytes();
    let mut private_jdc = TestSv2Client::connect(address, authority.public_key).await;
    private_jdc
        .send_message(
            MESSAGE_TYPE_SETUP_CONNECTION,
            test_sv2_setup(Protocol::JobDeclarationProtocol, 0),
        )
        .await;
    private_jdc
        .receive(MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS)
        .await;
    private_jdc.send(extensions).await;
    private_jdc
        .receive_extension(EXTENSIONS_NEGOTIATION, REQUEST_EXTENSIONS_SUCCESS)
        .await;
    private_jdc
        .send_message(
            MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN,
            AllocateMiningJobToken {
                request_id: 5,
                user_identifier: identity.as_str().try_into().unwrap(),
            },
        )
        .await;
    let mut allocated_payload = private_jdc
        .receive(MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN_SUCCESS)
        .await;
    let allocated = AllocateMiningJobTokenSuccess::from_bytes(&mut allocated_payload).unwrap();
    let token = allocated.mining_job_token.as_bytes().to_vec();
    private_jdc
        .send(
            encode_extension_frame(
                PAYOUT_OUTPUTS_EXTENSION,
                REQUEST_PAYOUT_OUTPUTS,
                false,
                &test_payout_outputs_request(
                    6,
                    &token,
                    prepared.template(),
                    private_value,
                    &private_prefix,
                ),
            )
            .unwrap(),
        )
        .await;
    let mut success_payload = private_jdc
        .receive_extension(PAYOUT_OUTPUTS_EXTENSION, REQUEST_PAYOUT_OUTPUTS_SUCCESS)
        .await;
    let success = RequestPayoutOutputsSuccess::from_bytes(&mut success_payload).unwrap();
    let mut request_bytes = test_custom_job_from_payout_outputs(
        &success,
        &token,
        prepared.template(),
        &private_prefix,
        std::slice::from_ref(&private_transaction),
    );
    let request = SetCustomMiningJob::from_bytes(&mut request_bytes).unwrap();
    let private_header = test_bch_solve_custom_job(&request, &extranonce_prefix);
    let private_block = Block {
        header: private_header,
        txdata: vec![
            test_bch_custom_coinbase(&request, &extranonce_prefix),
            private_transaction,
        ],
    };
    client
        .validate_block_proposal(&private_block)
        .await
        .unwrap();
    mining
        .send_message(MESSAGE_TYPE_SET_CUSTOM_MINING_JOB, request.clone())
        .await;
    let mut installed_payload = mining
        .receive(MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_SUCCESS)
        .await;
    let installed = SetCustomMiningJobSuccess::from_bytes(&mut installed_payload).unwrap();
    mining
        .send_message(
            MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED,
            SubmitSharesExtended {
                channel_id,
                sequence_number: 2,
                job_id: installed.job_id,
                nonce: private_header.nonce,
                ntime: private_header.time,
                version: request.version,
                extranonce: B032::new(&[0; 8]).unwrap(),
            },
        )
        .await;
    let mut submitted_payload = mining.receive(MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS).await;
    let submitted = SubmitSharesSuccess::from_bytes(&mut submitted_payload).unwrap();
    assert_eq!(submitted.last_sequence_number, 2);
    let emission = emissions.try_recv().unwrap();
    assert_eq!(emission.header, private_header);
    let result: Value = client
        .request(
            "submitblock",
            vec![json!(hex::encode(serialize(&private_block)))],
        )
        .await
        .unwrap();
    assert!(result.is_null());
    timeout(Duration::from_secs(10), async {
        loop {
            let tip: BlockHash = pool.request("getbestblockhash", vec![]).await.unwrap();
            if tip == private_header.block_hash() {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the pool node did not validate the private block broadcast by the client");
    println!(
        "{}",
        json!({"mode": "coinbase-only", "source": "requested-payout-outputs", "height": private_height, "hash": private_header.block_hash().to_string(), "coinbase_value": private_value, "private_fee": private_fee, "transaction": private_txid})
    );
    drop(mining);
    drop(full_jdc);
    drop(private_jdc);
    shutdown_sender.send(()).unwrap();
    timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    let permits = timeout(
        Duration::from_secs(5),
        connection_limit.acquire_many_owned(3),
    )
    .await
    .unwrap()
    .unwrap();
    drop(permits);
}

/// A Job Declaration client's view of a running node: the template fields it
/// would take from its own Bitcoin Cash node, here read from the running
/// node's extended job so the check needs no RPC login.
struct RunningJob {
    template: BlockTemplate,
    channel_id: u32,
}

/// Open an extended channel on a running node and turn its first job into
/// the template fields a Job Declaration client sends: version, previous
/// block hash, bits, time and height (the BIP34 push of the job's coinbase).
/// No transactions: the client declares an empty block, so its coinbase pays
/// out the block subsidy alone, not the fees in the node's job.
async fn running_node_job(mining: &mut TestSv2Client, identity: &str) -> RunningJob {
    mining
        .send_message(
            MESSAGE_TYPE_SETUP_CONNECTION,
            test_sv2_setup(Protocol::MiningProtocol, 2),
        )
        .await;
    mining.receive(MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS).await;
    mining
        .send_message(
            MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
            OpenExtendedMiningChannel {
                request_id: 1,
                user_identity: identity.try_into().unwrap(),
                nominal_hash_rate: 1.0,
                max_target: U256::new(&[0xff; 32]).unwrap(),
                min_extranonce_size: 8,
            },
        )
        .await;
    let mut open_payload = mining
        .receive(MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS)
        .await;
    let opened = OpenExtendedMiningChannelSuccess::from_bytes(&mut open_payload).unwrap();
    let extranonce_size =
        opened.extranonce_prefix.as_bytes().len() + usize::from(opened.extranonce_size);
    let mut job_payload = mining.receive(MESSAGE_TYPE_NEW_EXTENDED_MINING_JOB).await;
    let job = NewExtendedMiningJob::from_bytes(&mut job_payload).unwrap();
    let mut prev_payload = mining.receive(MESSAGE_TYPE_MINING_SET_NEW_PREV_HASH).await;
    let prev = SetNewPrevHash::from_bytes(&mut prev_payload).unwrap();
    let mut coinbase = job.coinbase_tx_prefix.as_bytes().to_vec();
    coinbase.extend(std::iter::repeat_n(0, extranonce_size));
    coinbase.extend_from_slice(job.coinbase_tx_suffix.as_bytes());
    let coinbase: Transaction = deserialize(&coinbase).unwrap();
    let script_sig = coinbase.input[0].script_sig.as_bytes();
    let height_length = usize::from(script_sig[0]);
    let mut height_bytes = [0u8; 4];
    height_bytes[..height_length].copy_from_slice(&script_sig[1..=height_length]);
    let height = u32::from_le_bytes(height_bytes);
    let subsidy = 5_000_000_000_u64 >> (height / 210_000);
    let prev_hash = BlockHash::from_byte_array(prev.prev_hash.as_ref().try_into().unwrap());
    let template = serde_json::from_value(json!({
        "version": job.version,
        "previousblockhash": prev_hash.to_string(),
        "transactions": [],
        "coinbaseaux": {},
        "coinbasevalue": subsidy,
        "longpollid": "",
        "target": "",
        "mintime": prev.min_ntime,
        "mutable": [],
        "noncerange": "00000000ffffffff",
        "sigoplimit": 0,
        "sizelimit": 0,
        "curtime": prev.min_ntime,
        "bits": format!("{:08x}", prev.nbits),
        "height": height,
    }))
    .unwrap();
    RunningJob {
        template,
        channel_id: opened.channel_id,
    }
}

/// The authority key the node's operator publishes, in the reference
/// implementation's text form: Base58Check of version `[1, 0]` and the
/// 32-byte x-only key.
fn published_authority_key(text: &str) -> [u8; 32] {
    let bytes = bitcoin::base58::decode_check(text).unwrap();
    assert_eq!(bytes[..2], [1, 0], "not a Stratum V2 public key");
    bytes[2..].try_into().unwrap()
}

#[tokio::test]
#[ignore = "talks to a running node's Stratum V2 port; see docs/bch-job-declaration.md"]
async fn a_running_node_serves_requested_payout_outputs() {
    let address: SocketAddr = env::var("P2POOL_SV2_ADDRESS")
        .expect("set P2POOL_SV2_ADDRESS")
        .parse()
        .unwrap();
    let authority = published_authority_key(
        &env::var("P2POOL_SV2_AUTHORITY_KEY").expect("set P2POOL_SV2_AUTHORITY_KEY"),
    );
    // "<payout address>.<worker>,p2p=<share address>", as a miner sends it.
    let identity = env::var("P2POOL_SV2_IDENTITY").expect("set P2POOL_SV2_IDENTITY");

    let mut mining = TestSv2Client::connect(address, authority).await;
    let job = running_node_job(&mut mining, &identity).await;
    let template = job.template;
    let prefix = Builder::new()
        .push_int(i64::from(template.height))
        .push_slice(b"standard client")
        .into_bytes();
    let extensions = encode_extension_frame(
        EXTENSIONS_NEGOTIATION,
        REQUEST_EXTENSIONS,
        false,
        &extensions_payload(1, &[PAYOUT_OUTPUTS_EXTENSION]),
    )
    .unwrap();
    let mut jdc = TestSv2Client::connect(address, authority).await;
    jdc.send_message(
        MESSAGE_TYPE_SETUP_CONNECTION,
        test_sv2_setup(Protocol::JobDeclarationProtocol, 1),
    )
    .await;
    jdc.receive(MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS).await;
    jdc.send(extensions).await;
    assert_eq!(
        jdc.receive_extension(EXTENSIONS_NEGOTIATION, REQUEST_EXTENSIONS_SUCCESS)
            .await,
        extensions_payload(1, &[PAYOUT_OUTPUTS_EXTENSION])
    );
    let token = allocate_token(&mut jdc, 2, &identity).await;

    // A request naming a block the node is not building on is refused.
    let mut stale = template.clone();
    stale.previousblockhash = BlockHash::all_zeros().to_string();
    jdc.send(
        encode_extension_frame(
            PAYOUT_OUTPUTS_EXTENSION,
            REQUEST_PAYOUT_OUTPUTS,
            false,
            &test_payout_outputs_request(3, &token, &stale, template.coinbasevalue, &prefix),
        )
        .unwrap(),
    )
    .await;
    let mut refused_payload = jdc
        .receive_extension(PAYOUT_OUTPUTS_EXTENSION, REQUEST_PAYOUT_OUTPUTS_ERROR)
        .await;
    let refused = RequestPayoutOutputsError::from_bytes(&mut refused_payload).unwrap();
    println!(
        "{}",
        json!({"step": "refused", "prev_hash": stale.previousblockhash, "error_code": String::from_utf8_lossy(refused.error_code.as_bytes())})
    );

    let mut success_payload = request_payout_outputs(&mut jdc, 4, &token, &template, &prefix).await;
    let success = RequestPayoutOutputsSuccess::from_bytes(&mut success_payload).unwrap();
    let outputs: Vec<TxOut> = deserialize(success.coinbase_tx_outputs.as_bytes()).unwrap();
    assert_eq!(success.flags, EXACT_OUTPUTS);
    assert_eq!(success.coinbase_tx_locktime, template.height - 1);
    assert_eq!(
        outputs
            .iter()
            .map(|output| output.value.to_sat())
            .sum::<u64>(),
        template.coinbasevalue
    );
    println!(
        "{}",
        json!({"step": "answered", "height": template.height, "prev_hash": template.previousblockhash, "value": template.coinbasevalue, "outputs": describe_outputs(&outputs)})
    );

    // Declare an empty block on the answered outputs, then install it.
    let mut request_bytes =
        test_custom_job_from_payout_outputs(&success, &token, &template, &prefix, &[]);
    let mut request = SetCustomMiningJob::from_bytes(&mut request_bytes).unwrap();
    request.channel_id = job.channel_id;
    let (coinbase_prefix, coinbase_suffix) = test_declared_coinbase_parts(&request);
    jdc.send_message(
        MESSAGE_TYPE_DECLARE_MINING_JOB,
        DeclareMiningJob {
            request_id: 5,
            mining_job_token: B0255::new(&token).unwrap(),
            version: request.version,
            coinbase_tx_prefix: B064K::new(&coinbase_prefix).unwrap(),
            coinbase_tx_suffix: B064K::new(&coinbase_suffix).unwrap(),
            wtxid_list: Seq064K::new(vec![]).unwrap(),
            excess_data: B064K::new(&[]).unwrap(),
        },
    )
    .await;
    // The node asks for every declared transaction; there are none.
    let mut missing_payload = jdc.receive(MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS).await;
    let missing = ProvideMissingTransactions::from_bytes(&mut missing_payload).unwrap();
    assert_eq!(missing.request_id, 5);
    assert!(missing.unknown_tx_position_list.is_empty());
    jdc.send_message(
        MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS_SUCCESS,
        ProvideMissingTransactionsSuccess {
            request_id: 5,
            transaction_list: Seq064K::new(vec![]).unwrap(),
        },
    )
    .await;
    let mut accepted_payload = jdc.receive(MESSAGE_TYPE_DECLARE_MINING_JOB_SUCCESS).await;
    let accepted = DeclareMiningJobSuccess::from_bytes(&mut accepted_payload).unwrap();
    request.token = B0255::new(accepted.new_mining_job_token.as_bytes()).unwrap();
    mining
        .send_message(MESSAGE_TYPE_SET_CUSTOM_MINING_JOB, request.clone())
        .await;
    let mut installed_payload = mining
        .receive(MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_SUCCESS)
        .await;
    let installed = SetCustomMiningJobSuccess::from_bytes(&mut installed_payload).unwrap();
    println!(
        "{}",
        json!({"step": "installed", "declare_request_id": accepted.request_id, "job_id": installed.job_id})
    );

    // The next share or block makes the answer stale; the node says so, and
    // a fresh token gets the new outputs.
    jdc.receive_extension_within(
        Duration::from_secs(120),
        PAYOUT_OUTPUTS_EXTENSION,
        PAYOUT_OUTPUTS_STALE,
    )
    .await;
    let mut current = TestSv2Client::connect(address, authority).await;
    let current_template = running_node_job(&mut current, &identity).await.template;
    let fresh_prefix = Builder::new()
        .push_int(i64::from(current_template.height))
        .push_slice(b"standard client")
        .into_bytes();
    let fresh_token = allocate_token(&mut jdc, 6, &identity).await;
    let mut fresh_payload =
        request_payout_outputs(&mut jdc, 7, &fresh_token, &current_template, &fresh_prefix).await;
    let fresh = RequestPayoutOutputsSuccess::from_bytes(&mut fresh_payload).unwrap();
    let fresh_outputs: Vec<TxOut> = deserialize(fresh.coinbase_tx_outputs.as_bytes()).unwrap();
    assert_ne!(fresh_outputs, outputs);
    println!(
        "{}",
        json!({"step": "stale-then-fresh", "height": current_template.height, "prev_hash": current_template.previousblockhash, "outputs": describe_outputs(&fresh_outputs)})
    );
}

/// Allocate a mining job token for `identity` on a Job Declaration connection.
async fn allocate_token(jdc: &mut TestSv2Client, request_id: u32, identity: &str) -> Vec<u8> {
    jdc.send_message(
        MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN,
        AllocateMiningJobToken {
            request_id,
            user_identifier: identity.try_into().unwrap(),
        },
    )
    .await;
    let mut payload = jdc
        .receive(MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN_SUCCESS)
        .await;
    AllocateMiningJobTokenSuccess::from_bytes(&mut payload)
        .unwrap()
        .mining_job_token
        .as_bytes()
        .to_vec()
}

/// Ask for the outputs of `token` on `template`, paying out the template's
/// whole coinbase value. Returns the `RequestPayoutOutputs.Success` payload.
async fn request_payout_outputs(
    jdc: &mut TestSv2Client,
    request_id: u32,
    token: &[u8],
    template: &BlockTemplate,
    prefix: &[u8],
) -> Vec<u8> {
    jdc.send(
        encode_extension_frame(
            PAYOUT_OUTPUTS_EXTENSION,
            REQUEST_PAYOUT_OUTPUTS,
            false,
            &test_payout_outputs_request(
                request_id,
                token,
                template,
                template.coinbasevalue,
                prefix,
            ),
        )
        .unwrap(),
    )
    .await;
    jdc.receive_extension(PAYOUT_OUTPUTS_EXTENSION, REQUEST_PAYOUT_OUTPUTS_SUCCESS)
        .await
}

/// Each output's value and script, in order.
fn describe_outputs(outputs: &[TxOut]) -> Value {
    outputs
        .iter()
        .map(|output| {
            json!({"value": output.value.to_sat(), "script": output.script_pubkey.to_hex_string()})
        })
        .collect()
}
