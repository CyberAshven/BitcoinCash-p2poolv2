// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::share_block::{ShareBlock, ShareHeader, ShareTransaction};
use crate::shares::transactions::coinbase::build_sharechain_coinbase_transaction;
use crate::stratum::emission::Emission;
use std::error::Error;
use tracing::debug;

/// Build a ShareBlock from a stratum share emission and store it via
/// ChainStoreHandle.
///
/// The share carries its coinbase and then the job's share transactions,
/// the set the commitment's `share_witness_root` covers (on Bitcoin Cash,
/// drawn from the share transaction pool when the job was built).
///
/// The share block is persisted and its header organised onto the candidate
/// chain in one atomic write, matching the block receiver path. Returns the
/// stored share block for the validation worker.
pub async fn handle_stratum_share(
    emission: Emission,
    chain_store_handle: &ChainStoreHandle,
) -> Result<ShareBlock, Box<dyn Error + Send + Sync>> {
    let Emission {
        header,
        blocktemplate,
        share_commitment,
        template_merkle_branches,
        bitcoin_coinbase,
        coinbase_proof,
        share_transactions: other_share_transactions,
    } = emission;

    // The weak block hash is what makes this coinbase unique to this
    // share. On Bitcoin the coinbase's BIP141 witness commitment covers the
    // other transactions' wtxids.
    let share_coinbase = build_sharechain_coinbase_transaction(
        &share_commitment.miner_address,
        header.block_hash(),
        &other_share_transactions,
    );

    let mut share_transactions = Vec::with_capacity(1 + other_share_transactions.len());
    share_transactions.push(ShareTransaction(share_coinbase));
    share_transactions.extend(other_share_transactions.iter().cloned());

    let share_header = ShareHeader::from_commitment_and_header(
        share_commitment,
        header,
        blocktemplate.height as u64,
        coinbase_proof,
    );

    let share_block = ShareBlock {
        header: share_header,
        transactions: share_transactions,
        template_merkle_branches,
        bitcoin_coinbase,
    };

    debug!(
        "Built share to add with {} txs",
        share_block.transactions.len()
    );

    chain_store_handle
        .add_share_block_and_organise_header(share_block.clone())
        .await
        .map_err(|e| format!("Failed to add share to chain: {e}"))?;

    Ok(share_block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounting::payout::payout_distribution::build_pplns_outputs;
    use crate::address::share_owner::ShareOwner;
    use crate::config::Chain;
    use crate::pool_difficulty::MockPoolDifficulty;
    use crate::shares::coinbase_proof::CoinbaseProof;
    use crate::shares::transactions::coinbase::compute_witness_root;
    use crate::shares::validation::{DefaultShareValidator, ShareValidator};
    use crate::store::writer::StoreError;
    use crate::stratum::work::block_template::BlockTemplate;
    use crate::stratum::work::difficulty::validate::build_coinbase_from_components;
    use crate::stratum::work::gbt::compute_merkle_root_from_branches;
    use crate::stratum::work::prepared_notify::{PreparedNotifyParamsBuilder, insert_miner_job};
    use crate::stratum::work::tracker::start_tracker_actor;
    use crate::test_utils::{create_test_commitment, test_coinbase_transaction};
    use bitcoin::block::Header;
    use bitcoin::hashes::Hash;
    use bitcoin::{Address, Amount, OutPoint, ScriptBuf, Sequence, TxIn, TxOut, Txid, Witness};
    use bitcoin::{BlockHash, CompactTarget};
    use bitcoin::{Transaction, absolute::LockTime, transaction::Version};
    use std::collections::HashMap;
    use std::slice;
    use std::str::FromStr;
    use std::sync::Arc;

    /// Helper to create a minimal BlockTemplate for testing
    fn create_test_blocktemplate() -> BlockTemplate {
        BlockTemplate {
            version: 0x20000000,
            rules: vec![],
            vbavailable: HashMap::new(),
            vbrequired: 0,
            previousblockhash: "0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
            transactions: vec![],
            coinbaseaux: HashMap::new(),
            coinbasevalue: 5000000000,
            longpollid: "".to_string(),
            target: "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            mintime: 0,
            mutable: vec![],
            noncerange: "00000000ffffffff".to_string(),
            sigoplimit: 80000,
            sizelimit: 4000000,
            weightlimit: 4000000,
            curtime: 1700000000,
            bits: "207fffff".to_string(),
            height: 1,
            default_witness_commitment: None,
            provided: None,
        }
    }

    /// Helper to create a test Emission with share commitment (p2p mining mode)
    fn create_test_emission_with_commitment() -> Emission {
        let bitcoin_header = Header {
            version: bitcoin::block::Version::TWO,
            prev_blockhash: BlockHash::all_zeros(),
            merkle_root: bitcoin::TxMerkleNode::all_zeros(),
            time: 1700000000,
            bits: CompactTarget::from_consensus(0x1b4188f5),
            nonce: 12345,
        };

        let commitment = create_test_commitment();

        Emission {
            header: bitcoin_header,
            blocktemplate: Arc::new(create_test_blocktemplate()),
            share_commitment: commitment,
            template_merkle_branches: vec![],
            bitcoin_coinbase: test_coinbase_transaction(1),
            coinbase_proof: CoinbaseProof::default(),
            share_transactions: Arc::from([]),
        }
    }

    #[tokio::test]
    async fn test_handle_stratum_share_with_commitment_returns_share_block() {
        let mut mock_chain_store = ChainStoreHandle::default();

        // Mock add_share to succeed
        mock_chain_store
            .expect_add_share_block_and_organise_header()
            .returning(|_| Ok(None));

        let emission = create_test_emission_with_commitment();

        let result = handle_stratum_share(emission, &mock_chain_store).await;

        assert!(result.is_ok());
        let share_block = result.unwrap();
        // Verify the share block has the expected structure
        assert_eq!(share_block.transactions.len(), 1); // One share coinbase
    }

    /// Regression test: a locally mined share must be persisted and have
    /// its header organised onto the candidate chain atomically, exactly
    /// like the block receiver path.
    ///
    /// Organising the header writes the block metadata up front. If the
    /// share were persisted without it (the body-only `add_share_block`
    /// path), a chain-context validation rejection in the organise worker
    /// would leave the share in the DAG with a parent->child edge but no
    /// metadata. A later share building on it would drive the confirm walk
    /// into `get_block_metadata` -> NotFound and wedge the confirmed chain
    /// permanently. No expectation is set for the body-only path, so
    /// reverting to it would panic this test.
    #[tokio::test]
    async fn test_handle_stratum_share_organises_header_atomically() {
        let mut mock_chain_store = ChainStoreHandle::default();

        mock_chain_store
            .expect_add_share_block_and_organise_header()
            .times(1)
            .returning(|_| Ok(None));

        let emission = create_test_emission_with_commitment();

        let result = handle_stratum_share(emission, &mock_chain_store).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_stratum_share_channel_closed_returns_error() {
        let mut mock_chain_store = ChainStoreHandle::default();

        // Mock the share chain store to return an error (simulating channel closed)
        mock_chain_store
            .expect_add_share_block_and_organise_header()
            .returning(|_| Err(StoreError::ChannelClosed));

        let emission = create_test_emission_with_commitment();

        let result = handle_stratum_share(emission, &mock_chain_store).await;

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Failed to add share to chain")
        );
    }

    #[tokio::test]
    async fn test_handle_stratum_share_builds_correct_share_header() {
        let mut mock_chain_store = ChainStoreHandle::default();

        // Mock add_share to succeed
        mock_chain_store
            .expect_add_share_block_and_organise_header()
            .returning(|_| Ok(None));

        let emission = create_test_emission_with_commitment();
        let expected_commitment = emission.share_commitment.clone();
        let expected_bitcoin_header = emission.header;

        let result = handle_stratum_share(emission, &mock_chain_store).await;

        assert!(result.is_ok());
        let share_block = result.unwrap();

        // Verify share header fields from commitment
        assert_eq!(
            share_block.header.prev_share_blockhash,
            expected_commitment.prev_share_blockhash
        );
        assert_eq!(share_block.header.uncles, expected_commitment.uncles);
        assert_eq!(
            share_block.header.miner_bitcoin_address,
            expected_commitment.miner_bitcoin_address
        );
        assert_eq!(share_block.header.bits, expected_commitment.bits);
        assert_eq!(share_block.header.time, expected_commitment.time);

        // Verify bitcoin header is preserved
        assert_eq!(share_block.header.bitcoin_header, expected_bitcoin_header);
    }

    #[tokio::test]
    async fn test_handle_stratum_share_with_bitcoin_transactions() {
        use crate::stratum::work::block_template::TemplateTransaction;
        use bitcoin::consensus::Encodable;

        let mut mock_chain_store = ChainStoreHandle::default();

        // Mock add_share to succeed
        mock_chain_store
            .expect_add_share_block_and_organise_header()
            .returning(|_| Ok(None));

        // Create emission with some bitcoin transactions
        let bitcoin_header = Header {
            version: bitcoin::block::Version::TWO,
            prev_blockhash: BlockHash::all_zeros(),
            merkle_root: bitcoin::TxMerkleNode::all_zeros(),
            time: 1700000000,
            bits: CompactTarget::from_consensus(0x1b4188f5),
            nonce: 12345,
        };

        // Create a valid template transaction by serializing a real Transaction
        let template_source_tx = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![],
            output: vec![],
        };
        let mut tx_bytes = Vec::new();
        template_source_tx.consensus_encode(&mut tx_bytes).unwrap();
        let tx_hex = hex::encode(&tx_bytes);

        let template_tx = TemplateTransaction {
            data: tx_hex.clone(),
            txid: template_source_tx.compute_txid().to_string(),
            hash: template_source_tx.compute_wtxid().to_string(),
            depends: vec![],
            fee: 0,
            sigops: 0,
            weight: 100,
        };

        let mut blocktemplate = create_test_blocktemplate();
        blocktemplate.transactions = vec![template_tx.clone(), template_tx];

        let commitment = create_test_commitment();

        let emission = Emission {
            header: bitcoin_header,
            blocktemplate: Arc::new(blocktemplate),
            share_commitment: commitment,
            template_merkle_branches: vec![
                bitcoin::TxMerkleNode::all_zeros(),
                bitcoin::TxMerkleNode::all_zeros(),
            ],
            bitcoin_coinbase: test_coinbase_transaction(1),
            coinbase_proof: CoinbaseProof::default(),
            share_transactions: Arc::from([]),
        };

        let result = handle_stratum_share(emission, &mock_chain_store).await;

        assert!(result.is_ok());
        let share_block = result.unwrap();

        // Verify merkle branches are passed through from Emission to ShareBlock
        assert_eq!(share_block.template_merkle_branches.len(), 2);
    }

    /// A job built with pool transactions commits to them, and the share a
    /// miner finds on it carries them after its coinbase: the header's proof
    /// ties the commitment to the bitcoin merkle root, and the body matches
    /// the header's share witness root.
    #[tokio::test]
    async fn test_a_share_found_on_a_job_with_pool_transactions_carries_them() {
        let template: BlockTemplate = serde_json::from_str(include_str!(
            "../../../bitcoindrpc/test_data/bchn-regtest-template.json"
        ))
        .unwrap();
        let payout = Address::p2pkh(
            bitcoin::PubkeyHash::from_byte_array([1; 20]),
            bitcoin::Network::Regtest,
        );
        let outputs = build_pplns_outputs(
            template.coinbasevalue,
            &HashMap::from([(payout.clone(), 1)]),
            &None,
            None,
            &None,
            None,
        )
        .unwrap();
        let pool_transaction = ShareTransaction(Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([7; 32]), 0),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(1_000),
                script_pubkey: ScriptBuf::new(),
            }],
        });
        let prepared =
            PreparedNotifyParamsBuilder::new(Arc::new(template), outputs, b"P2Poolv2", true)
                .prev_share_blockhash(BlockHash::from_byte_array([0x33; 32]))
                .bits(CompactTarget::from_consensus(0x2100ffff))
                .include_merge_mining(true)
                .share_transactions(vec![pool_transaction.clone()])
                .build()
                .unwrap();
        assert_eq!(
            prepared.share_transactions(),
            slice::from_ref(&pool_transaction)
        );
        let tracker = start_tracker_actor();
        let owner = ShareOwner::CashP2pkh([9; 20]);
        let (job_id, _) = insert_miner_job(&prepared, Some(&payout), Some(owner), &tracker);
        let job = tracker.get_job(job_id).unwrap();
        let commitment = job.share_commitment.clone().unwrap();
        assert_eq!(
            commitment.share_witness_root,
            compute_witness_root(slice::from_ref(&pool_transaction))
        );
        assert_eq!(
            job.share_transactions.as_ref(),
            slice::from_ref(&pool_transaction)
        );

        let coinbase = build_coinbase_from_components(
            &job.coinbase1,
            "00000000",
            "0000000000000000",
            &job.coinbase2,
        )
        .unwrap();
        let header = Header {
            version: bitcoin::block::Version::TWO,
            prev_blockhash: BlockHash::from_str(&job.blocktemplate.previousblockhash).unwrap(),
            merkle_root: compute_merkle_root_from_branches(
                coinbase.compute_txid(),
                &job.template_merkle_branches,
            ),
            time: job.blocktemplate.curtime,
            bits: CompactTarget::from_unprefixed_hex(&job.blocktemplate.bits).unwrap(),
            nonce: 1,
        };
        let emission = Emission {
            header,
            blocktemplate: Arc::clone(&job.blocktemplate),
            share_commitment: commitment.clone(),
            template_merkle_branches: job.template_merkle_branches.clone(),
            bitcoin_coinbase: coinbase.clone(),
            coinbase_proof: CoinbaseProof::from_coinbase(&coinbase, commitment.share_witness_root)
                .unwrap(),
            share_transactions: Arc::clone(&job.share_transactions),
        };
        let mut mock_chain_store = ChainStoreHandle::default();
        mock_chain_store
            .expect_add_share_block_and_organise_header()
            .returning(|_| Ok(None));

        let share_block = handle_stratum_share(emission, &mock_chain_store)
            .await
            .unwrap();

        assert_eq!(share_block.transactions.len(), 2);
        assert!(share_block.transactions[0].is_coinbase());
        assert_eq!(share_block.transactions[1], pool_transaction);
        let validator =
            DefaultShareValidator::new(MockPoolDifficulty::default()).for_chain(Chain::BitcoinCash);
        assert!(
            validator
                .validate_coinbase_proof(&share_block.header, &share_block.template_merkle_branches)
                .is_ok()
        );
        assert!(validator.validate_body_matches_header(&share_block).is_ok());
        assert!(validator.validate_block_size(&share_block).is_ok());
    }
}
