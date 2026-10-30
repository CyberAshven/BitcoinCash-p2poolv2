// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::block_template::BlockTemplate;
use super::error::WorkError;
use crate::accounting::CoinbasePayouts;
use crate::accounting::payout::coinbase_budget::PayoutSplit;
use crate::accounting::payout::payout_distribution::{PayoutDistribution, build_pplns_outputs};
use crate::accounting::payout::sharechain_pplns::pplns_window::{
    PayingBlock, bitcoin_block_difficulty,
};
use crate::config::StratumConfig;
use crate::pool_difficulty;
use crate::shares::bch_asert::BchAsert;
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::share_version::{ShareVersion, next_share_version};
use crate::shares::transaction_pool::ShareTransactionPool;
use crate::shares::validation::tally_share_version_votes;
use crate::stratum::sv2::PoolTransactionCache;
use crate::stratum::work::merge_tokens::{TokenFile, TokenSet};
use crate::stratum::work::prepared_notify::{PreparedNotifyParams, PreparedNotifyParamsBuilder};
use p2poolv2_wallet::payout_address::PayoutAddress;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, warn};

/// Session-level context for building notify messages.
///
/// Groups parameters that remain constant across the notifier loop.
pub(crate) struct NotifyContext {
    pub chain_store_handle: ChainStoreHandle,
    pub config: StratumConfig<crate::config::Parsed>,
    pub pool_signature: Vec<u8>,
    pub pool_difficulty: pool_difficulty::PoolDifficulty,
    pub payout: Box<dyn PayoutDistribution + Send>,
    /// Latest prepared template, for Stratum V2 Job Declaration tokens.
    pub pool_transactions: PoolTransactionCache,
    /// Bitcoin Cash: the share transactions waiting for a share. `None` on
    /// Bitcoin, whose shares carry their coinbase alone.
    pub share_transaction_pool: Option<ShareTransactionPool>,
    /// The tokens a Bitcoin Cash P2Pool node merge-mines for its miners
    /// (`[stratum] merge_mining_tokens`), read again when the file changes.
    pub merge_tokens: Option<TokenFile>,
}

/// Build the output distribution for the coinbase transaction using
/// payout accounting in NotifyContext.
///
/// The template's bitcoin difficulty and previous block are passed to the
/// payout. Hydrapool uses the difficulty as its window target. The share
/// chain uses both on Bitcoin Cash, to size the window and pick the work it
/// pays, which is why they must come from the template: the share's header
/// carries the template's bits and previous block, so a validator reading
/// them from the header resolves the same window. On Bitcoin the share chain
/// window is `PPLNS_WINDOW_SHARES` deep and ignores them.
///
/// Returns `Err` if the payout distribution cannot be resolved.
fn build_output_distribution(
    template: &BlockTemplate,
    anchor: bitcoin::BlockHash,
    context: &mut NotifyContext,
) -> Result<CoinbasePayouts, WorkError> {
    let total_amount = bitcoin::Amount::from_sat(template.coinbasevalue);

    let compact_target = bitcoin::pow::CompactTarget::from_unprefixed_hex(&template.bits).unwrap();
    let bitcoin_difficulty = bitcoin_block_difficulty(compact_target, context.config.network);
    let bch_prev = template
        .previousblockhash
        .parse::<bitcoin::BlockHash>()
        .map_err(|error| WorkError {
            message: format!(
                "Template previous block {} is not a block hash: {error}",
                template.previousblockhash
            ),
        })?;

    context
        .payout
        .get_output_distribution(
            &context.chain_store_handle,
            anchor,
            PayingBlock::new(bitcoin_difficulty, bch_prev),
            total_amount,
            &context.config,
        )
        .map_err(|error| WorkError {
            message: format!("Payout distribution failed: {error}"),
        })
}

/// Build a PreparedNotifyParams from a template using the notify context.
///
/// Queries chain state (tip, uncles, difficulty) and prepares the
/// pre-serialized notify template for per-miner customization.
fn build_prepared_notify(
    template: &Arc<BlockTemplate>,
    clean_jobs: bool,
    context: &mut NotifyContext,
) -> Result<PreparedNotifyParams, WorkError> {
    let (tip, uncles) = context
        .chain_store_handle
        .get_mining_base_and_uncles()
        .map_err(|error| WorkError {
            message: format!("Failed to get mining base: {error}"),
        })?;
    let parent = context
        .chain_store_handle
        .get_asert_parent(&tip)
        .map_err(|error| WorkError {
            message: format!("Failed to read the mining base for ASERT: {error}"),
        })?;
    let target = context.pool_difficulty.calculate_target_clamped(parent);
    // Bitcoin Cash shares follow the mining base's rule version, or the next
    // one once 95% of the vote span's work desires it (`next_share_version`).
    let share_version = if context.config.chain == crate::config::Chain::BitcoinCash {
        let mining_base_version = context
            .chain_store_handle
            .get_share_header(&tip)
            .map_err(|error| WorkError {
                message: format!("Failed to read the mining base's share version: {error}"),
            })?
            .share_version
            .version;
        next_share_version(mining_base_version, parent.height, || {
            tally_share_version_votes(tip, &context.chain_store_handle)
        })
        .map_err(|error| WorkError {
            message: format!("Failed to tally the share version vote: {error}"),
        })?
    } else {
        ShareVersion::CURRENT
    };

    //* Anchor the payout on the same `tip` we commit as prev_share_blockhash
    //* below, so a confirmed-chain advance between reads cannot make the
    //* coinbase pay a window inconsistent with its declared prev.
    let output_distribution = build_output_distribution(template, tip, context)?;
    let weights = context.payout.pplns_weights();
    let merge_tokens = context.merge_tokens.as_mut().and_then(TokenFile::current);
    let (output_distribution, merge_tokens) = merge_token_payouts(
        template,
        output_distribution,
        weights.as_ref(),
        merge_tokens,
        context,
    )?;
    let share_transactions = context
        .share_transaction_pool
        .as_ref()
        .map(|pool| pool.select_for_share(tip))
        .unwrap_or_default();

    PreparedNotifyParamsBuilder::new(
        Arc::clone(template),
        output_distribution,
        &context.pool_signature,
        clean_jobs,
    )
    .pplns_weights(weights)
    .payout_split(PayoutSplit::for_chain(
        context.config.chain,
        merge_tokens.is_some(),
    ))
    .merge_tokens(merge_tokens)
    .prev_share_blockhash(tip)
    .uncles(uncles)
    .bits(target)
    .donation_address(context.config.donation_address().cloned())
    .donation(context.config.donation)
    .fee_address(context.config.fee_address().cloned())
    .fee(context.config.fee)
    .include_merge_mining(context.config.chain == crate::config::Chain::BitcoinCash)
    .miner_donations(miner_donation_address(&context.config))
    .share_version(share_version)
    .share_transactions(share_transactions)
    .build()
}

/// The payouts of jobs that merge-mine `merge_tokens` for every miner, and
/// the tokens they carry.
///
/// Every such job's share is a token share, so its coinbase follows the
/// token share's limit (`PayoutSplit::for_chain`): the same weights, cuts
/// and amounts, split again at that limit, as each validator rebuilds them.
/// Without the window's weights (a node that has not built one yet) the
/// split cannot be redone, and the template is mined without tokens.
fn merge_token_payouts(
    template: &BlockTemplate,
    output_distribution: CoinbasePayouts,
    weights: Option<&std::collections::HashMap<PayoutAddress, u128>>,
    merge_tokens: Option<Arc<TokenSet>>,
    context: &NotifyContext,
) -> Result<(CoinbasePayouts, Option<Arc<TokenSet>>), WorkError> {
    let Some(merge_tokens) = merge_tokens else {
        return Ok((output_distribution, None));
    };
    if let Err(reason) = template_meets_token_asert(template, &context.config) {
        warn!(
            "Template at height {} fails the ASERT band token shares must meet ({reason}); it is mined without merge-mined tokens",
            template.height
        );
        return Ok((output_distribution, None));
    }
    let merge_tokens = match merge_tokens.at_height(template.height) {
        Ok(merge_tokens) => merge_tokens,
        Err(reason) => {
            warn!(
                "Template at height {} is mined without merge-mined tokens: {reason}",
                template.height
            );
            return Ok((output_distribution, None));
        }
    };
    let Some(weights) = weights else {
        warn!("No PPLNS weights for this template yet; it is mined without merge-mined tokens");
        return Ok((output_distribution, None));
    };
    let payouts = build_pplns_outputs(
        template.coinbasevalue,
        weights,
        &context.config.donation_address().cloned(),
        context.config.donation,
        &context.config.fee_address().cloned(),
        context.config.fee,
        PayoutSplit::for_chain(context.config.chain, true),
    )
    .map_err(|error| WorkError {
        message: format!("Token share payouts failed: {error}"),
    })?;
    Ok((payouts, Some(merge_tokens)))
}

/// Whether a token share mined on `template` would pass the ASERT check
/// peers run on token shares (`validate_token_share_asert`): its nBits at
/// its height, with its `curtime` standing in for the header's time. A
/// network without an ASERT anchor (regtest, scalenet) always passes.
fn template_meets_token_asert(
    template: &BlockTemplate,
    config: &StratumConfig<crate::config::Parsed>,
) -> Result<(), String> {
    let Some(asert) = config.bitcoin_cash_network.and_then(BchAsert::for_network) else {
        return Ok(());
    };
    let bits = bitcoin::pow::CompactTarget::from_unprefixed_hex(&template.bits)
        .map_err(|error| format!("template bits {}: {error}", template.bits))?;
    asert.check_header_bits(bits, template.curtime, u64::from(template.height))
}

/// The address a miner's own `donation=` choice pays: the network's built-in
/// donation address on a Bitcoin Cash node in P2Pool mode, which builds every
/// miner's jobs and shares itself. `None` elsewhere: a Bitcoin node or
/// Hydrapool mode mines with the node's donation and refuses the option.
fn miner_donation_address(config: &StratumConfig<crate::config::Parsed>) -> Option<PayoutAddress> {
    match (config.chain, config.mode, config.bitcoin_cash_network) {
        (crate::config::Chain::BitcoinCash, crate::config::PoolMode::P2poolv2, Some(network)) => {
            Some(crate::config::bitcoin_cash_donation_address(network))
        }
        _ => None,
    }
}

/// NotifyCmd is used to send a new block template to the notifier.
pub enum NotifyCmd {
    /// Send a new block template to all connected miners.
    SendToAll {
        /// The block template to notify clients about.
        template: Arc<BlockTemplate>,
    },
    /// Rebuild and resend work using the latest template.
    ///
    /// Sent by the organise worker when the confirmed chain catches
    /// up to the candidate tip, meaning the payout distribution has
    /// changed and miners need updated work, and also we avoid uncles
    /// while miners build on previous blockhash
    NewNotify,
}

/// Sender half of the notify command channel.
pub type NotifySender = mpsc::Sender<NotifyCmd>;
pub type NotifyReceiver = mpsc::Receiver<NotifyCmd>;

/// Build a PreparedNotifyParams and publish it via the watch channel.
///
/// A preparation failure withdraws the previous job from both mining and Job
/// Declaration, and returns `Ok(false)` so a later template can recover. No
/// template transactions are removed: their CTOR order is not dependency order.
/// Returns `Ok(true)` after publishing, or `Err` if all receivers were dropped.
fn publish_prepared_notify(
    template: &Arc<BlockTemplate>,
    clean_jobs: bool,
    context: &mut NotifyContext,
    template_tx: &watch::Sender<Option<Arc<PreparedNotifyParams>>>,
) -> Result<bool, WorkError> {
    let prepared = match build_prepared_notify(template, clean_jobs, context) {
        Ok(prepared) => {
            let prepared = Arc::new(prepared);
            context
                .pool_transactions
                .set_prepared(Arc::clone(&prepared));
            Some(prepared)
        }
        Err(reason) => {
            warn!(
                "Withdrawing mining work at height {}: {reason}; waiting for a fitting template",
                template.height
            );
            context.pool_transactions.clear_prepared();
            None
        }
    };
    let published = prepared.is_some();
    template_tx.send(prepared).map_err(|_| WorkError {
        message: "All template receivers dropped".to_string(),
    })?;
    Ok(published)
}

/// Start the notifier task that broadcasts prepared templates via a watch channel.
///
/// Listens for new block templates on notifier_rx, builds a
/// PreparedNotifyParams once per template, and publishes it via the
/// watch channel. Each connection handler receives the prepared
/// template and builds per-miner notifies independently.
///
/// A template that cannot be prepared withdraws the previous work. Mining
/// connections close on that withdrawal, rather than leaving miners hashing a
/// stale previousblockhash. The worker stays available for the next template or
/// a rebuild of the latest template, and recovery always clears previous jobs.
/// Returns when the command channel or all template receivers have closed.
///
/// On Bitcoin Cash `share_transaction_pool` supplies the share transactions
/// each notify's shares carry; Bitcoin passes `None`.
///
/// A token file (`merge_mining_tokens`) that cannot be read or is not valid
/// returns at once too, so a node never starts mining without the tokens it
/// was told to merge-mine.
#[allow(clippy::too_many_arguments)] // wiring constructor: each parameter is a distinct collaborator, a params struct would only move the list
pub async fn start_notify(
    mut notifier_rx: mpsc::Receiver<NotifyCmd>,
    template_tx: watch::Sender<Option<Arc<PreparedNotifyParams>>>,
    chain_store_handle: ChainStoreHandle,
    config: &StratumConfig<crate::config::Parsed>,
    payout: Box<dyn PayoutDistribution + Send>,
    pool_difficulty: pool_difficulty::PoolDifficulty,
    pool_transactions: PoolTransactionCache,
    share_transaction_pool: Option<ShareTransactionPool>,
) {
    let pool_signature = match config.pool_signature {
        Some(ref sig) => sig.as_bytes().to_vec(),
        None => Vec::new(),
    };
    let merge_tokens = match config
        .merge_mining_tokens
        .as_deref()
        .map(TokenFile::open)
        .transpose()
    {
        Ok(merge_tokens) => merge_tokens,
        Err(reason) => {
            error!("Cannot merge-mine the tokens in merge_mining_tokens, shutting down: {reason}");
            return;
        }
    };

    let mut notify_context = NotifyContext {
        chain_store_handle,
        config: config.clone(),
        pool_signature,
        pool_difficulty,
        payout,
        pool_transactions,
        share_transaction_pool,
        merge_tokens,
    };

    let mut latest_template: Option<Arc<BlockTemplate>> = None;
    let mut work_available = false;

    while let Some(cmd) = notifier_rx.recv().await {
        match cmd {
            NotifyCmd::SendToAll { template } => {
                let clean_jobs = !work_available
                    || latest_template.is_none()
                    || latest_template.as_ref().unwrap().previousblockhash
                        != template.previousblockhash;
                latest_template = Some(Arc::clone(&template));

                match publish_prepared_notify(
                    &template,
                    clean_jobs,
                    &mut notify_context,
                    &template_tx,
                ) {
                    Ok(published) => work_available = published,
                    Err(error) => {
                        error!("Failed to publish notify, shutting down: {error}");
                        return;
                    }
                }
            }
            NotifyCmd::NewNotify => {
                if let Some(ref template) = latest_template {
                    debug!("NewNotify: sending notify with latest template");
                    match publish_prepared_notify(template, true, &mut notify_context, &template_tx)
                    {
                        Ok(published) => work_available = published,
                        Err(error) => {
                            error!("Failed to publish new notify, shutting down: {error}");
                            return;
                        }
                    }
                } else {
                    debug!("NewNotify received but no template available yet");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounting::OutputPair;
    use crate::accounting::payout::payout_distribution::MockPayoutDistribution;
    use crate::config::BitcoinCashNetwork;
    use crate::pool_difficulty::AsertParent;
    use crate::shares::bch_asert::{U256, get_compact};
    use crate::stratum::messages::Notify;
    use crate::stratum::work::block_template::{BlockTemplate, TemplateTransaction};
    use crate::stratum::work::coinbase::extract_outputs_from_coinbase2;
    use crate::stratum::work::prepared_notify::{
        PreparedNotifyParamsBuilder, build_notify_from_prepared,
    };
    use crate::stratum::work::tracker::{JobId, start_tracker_actor};
    use crate::test_utils::genesis_for_tests;
    use crate::test_utils::make_test_share_address;
    use bitcoin::CompressedPublicKey;
    use bitcoin::hashes::Hash;
    use bitcoin::{Address, Amount, Network, ScriptBuf, TxOut};
    use std::collections::HashMap;
    use std::str::FromStr;
    use tokio::sync::mpsc;

    /// Create a mock payout that returns a fixed output distribution.
    fn mock_payout_with_distribution(distribution: Vec<OutputPair>) -> MockPayoutDistribution {
        let mut mock_payout = MockPayoutDistribution::default();
        mock_payout.expect_pplns_weights().returning(|| None);
        mock_payout
            .expect_get_output_distribution()
            .return_once(move |_, _, _, _, _| Ok(CoinbasePayouts::direct(distribution)));
        mock_payout
    }

    #[tokio::test]
    async fn test_build_notify_from_gbt_and_compare_to_expected() {
        let data = include_str!(
            "../../../../p2poolv2_tests/test_data/gbt/regtest/ckpool/one-txn/gbt.json"
        );
        let template: BlockTemplate =
            serde_json::from_str(data).expect("Failed to parse BlockTemplate");

        let test_distribution = vec![OutputPair {
            address: "bcrt1qe2qaq0e8qlp425pxytrakala7725dynwhknufr"
                .parse::<bitcoin::Address<_>>()
                .unwrap()
                .assume_checked()
                .into(),
            amount: Amount::from_sat(template.coinbasevalue),
        }];

        let prepared =
            PreparedNotifyParamsBuilder::new(Arc::new(template), test_distribution, &[], false)
                .bits(bitcoin::CompactTarget::from_consensus(0x1d00ffff))
                .build()
                .expect("Failed to build prepared notify");

        let address: PayoutAddress = "bcrt1qe2qaq0e8qlp425pxytrakala7725dynwhknufr"
            .parse()
            .unwrap();
        let tracker_handle = start_tracker_actor();
        let notify_json = build_notify_from_prepared(
            &prepared,
            Some(&address),
            Some(p2poolv2_wallet::share_owner::ShareOwner::Taproot(
                make_test_share_address(1, bitcoin::Network::Signet).witness_program(),
            )),
            &tracker_handle,
        )
        .expect("Failed to build notify");

        let notify: Notify = serde_json::from_str(&notify_json).expect("Invalid notify JSON");

        // Compare all fields except job_id (random) and coinbase which has current time component
        assert_eq!(notify.params.version, "20000000");
        assert_eq!(notify.params.nbits, "207fffff");
        assert_eq!(notify.params.ntime, "68300262"); // we use curtime from gbt
        assert!(!notify.params.clean_jobs);
        assert_eq!(
            notify.params.prevhash,
            "aadbdeb0c770ef1cc9115a42aa0a34e91732c422c0cd7ddbe71b3d9145f85fa6"
        );

        assert_eq!(
            notify.params.merkle_branches,
            vec!["fecdf8cf1147587b0b3a262b16a955849053c6dfe0239718559f6a3d3ed20523".to_string()]
        );
    }

    /// A failed payout resolution withdraws work without ending the notifier.
    #[tokio::test]
    async fn test_start_notify_withdraws_when_a_template_cannot_be_prepared() {
        let (template_tx, mut template_rx) =
            watch::channel::<Option<Arc<PreparedNotifyParams>>>(None);
        let (notify_tx, notify_rx) = mpsc::channel::<NotifyCmd>(10);

        let mut chain_store_handle = ChainStoreHandle::default();
        let genesis = genesis_for_tests();
        let genesis_hash = genesis.block_hash();
        let genesis_header = genesis.header.clone();
        chain_store_handle
            .expect_get_mining_base_and_uncles()
            .returning(move || Ok((genesis_hash, vec![])));
        chain_store_handle.expect_get_asert_parent().returning(|_| {
            let genesis_header = genesis_for_tests().header;
            Ok(AsertParent::from_header(
                &genesis_header,
                0,
                genesis_header.time,
            ))
        });

        let pool_difficulty =
            pool_difficulty::PoolDifficulty::new(genesis_header.bits, genesis_header.time, 0);
        let stratum_config = StratumConfig::new_for_test_default().parse().unwrap();

        // The payout anchor cannot be resolved, which is persistent: the mining
        // base does not move while confirmation is stalled.
        let mut mock_payout = MockPayoutDistribution::default();
        mock_payout
            .expect_get_output_distribution()
            .returning(|_, _, _, _, _| Err("no payout distribution for anchor".into()));

        let task_handle = tokio::spawn(async move {
            start_notify(
                notify_rx,
                template_tx,
                chain_store_handle,
                &stratum_config,
                Box::new(mock_payout),
                pool_difficulty,
                PoolTransactionCache::new(),
                None,
            )
            .await;
        });

        let data = include_str!(
            "../../../../p2poolv2_tests/test_data/gbt/regtest/ckpool/one-txn/gbt.json"
        );
        let gbt_json: serde_json::Value = serde_json::from_str(data).expect("Invalid JSON");
        let template: BlockTemplate =
            serde_json::from_value(gbt_json).expect("Failed to parse BlockTemplate");
        notify_tx
            .send(NotifyCmd::SendToAll {
                template: Arc::new(template),
            })
            .await
            .expect("Failed to send template");

        tokio::time::timeout(std::time::Duration::from_secs(5), template_rx.changed())
            .await
            .expect("notifier must withdraw work when preparation fails")
            .expect("notifier must keep its template sender open");

        assert!(
            template_rx.borrow_and_update().is_none(),
            "no job may be published from a template that failed to prepare"
        );
        assert!(!task_handle.is_finished());
        drop(notify_tx);
        task_handle.await.expect("notifier task panicked");
    }

    /// An oversized BCH job withdraws the old work, and the next fitting
    /// template is published with clean_jobs even at the same BCH chain tip.
    #[tokio::test]
    async fn test_start_notify_recovers_after_an_oversized_bch_template() {
        let (template_tx, mut template_rx) = watch::channel(None);
        let (notify_tx, notify_rx) = mpsc::channel(10);
        let mut chain_store_handle = ChainStoreHandle::default();
        let genesis = genesis_for_tests();
        let genesis_hash = genesis.block_hash();
        let genesis_header = genesis.header.clone();
        chain_store_handle
            .expect_get_mining_base_and_uncles()
            .returning(move || Ok((genesis_hash, vec![])));
        chain_store_handle.expect_get_asert_parent().returning(|_| {
            let header = genesis_for_tests().header;
            Ok(AsertParent::from_header(&header, 0, header.time))
        });
        chain_store_handle
            .expect_get_share_header()
            .returning(|_| Ok(genesis_for_tests().header));
        let pool_difficulty =
            pool_difficulty::PoolDifficulty::new(genesis_header.bits, genesis_header.time, 0);
        let mut config = StratumConfig::new_for_test_default().parse().unwrap();
        config.chain = crate::config::Chain::BitcoinCash;
        config.bitcoin_cash_network = Some(BitcoinCashNetwork::Regtest);
        let address = PayoutAddress::from(Address::p2pkh(
            bitcoin::PubkeyHash::from_byte_array([0x22; 20]),
            Network::Regtest,
        ));
        let mut payout = MockPayoutDistribution::default();
        payout.expect_pplns_weights().returning(|| None);
        payout
            .expect_get_output_distribution()
            .returning(move |_, _, _, amount, _| {
                Ok(CoinbasePayouts::direct(vec![OutputPair {
                    address: address.clone(),
                    amount,
                }]))
            });
        let task_handle = tokio::spawn(async move {
            start_notify(
                notify_rx,
                template_tx,
                chain_store_handle,
                &config,
                Box::new(payout),
                pool_difficulty,
                PoolTransactionCache::new(),
                None,
            )
            .await;
        });
        let template: BlockTemplate = serde_json::from_str(include_str!(
            "../../../../bitcoindrpc/test_data/bchn-regtest-template.json"
        ))
        .unwrap();
        notify_tx
            .send(NotifyCmd::SendToAll {
                template: Arc::new(template.clone()),
            })
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), template_rx.changed())
            .await
            .unwrap()
            .unwrap();
        assert!(template_rx.borrow_and_update().is_some());

        let mut oversized = template.clone();
        oversized.sizelimit = 81;
        oversized.validate_bch().unwrap();
        notify_tx
            .send(NotifyCmd::SendToAll {
                template: Arc::new(oversized),
            })
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), template_rx.changed())
            .await
            .unwrap()
            .unwrap();
        assert!(template_rx.borrow_and_update().is_none());
        assert!(!task_handle.is_finished());

        notify_tx
            .send(NotifyCmd::SendToAll {
                template: Arc::new(template.clone()),
            })
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), template_rx.changed())
            .await
            .unwrap()
            .unwrap();
        let prepared = template_rx.borrow_and_update().clone().unwrap();
        assert_eq!(
            prepared.template().previousblockhash,
            template.previousblockhash
        );
        let tracker = start_tracker_actor();
        let notify: Notify = serde_json::from_str(
            &build_notify_from_prepared(&prepared, None, None, &tracker).unwrap(),
        )
        .unwrap();
        assert!(notify.params.clean_jobs);
        drop(notify_tx);
        task_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_start_notify() {
        // Create watch channel for prepared templates
        let (template_tx, mut template_rx) =
            watch::channel::<Option<Arc<PreparedNotifyParams>>>(None);

        // Create a channel for block template notifications
        let (notify_tx, notify_rx) = mpsc::channel::<NotifyCmd>(10);

        let mut chain_store_handle = ChainStoreHandle::default();

        let genesis = genesis_for_tests();
        let genesis_hash = genesis.block_hash();
        let genesis_header = genesis.header.clone();
        chain_store_handle
            .expect_get_mining_base_and_uncles()
            .returning(move || Ok((genesis_hash, vec![])));

        let pool_difficulty =
            pool_difficulty::PoolDifficulty::new(genesis_header.bits, genesis_header.time, 0);

        chain_store_handle.expect_get_asert_parent().returning(|_| {
            let genesis_header = genesis_for_tests().header;
            Ok(AsertParent::from_header(
                &genesis_header,
                0,
                genesis_header.time,
            ))
        });

        let stratum_config = StratumConfig::new_for_test_default().parse().unwrap();

        let test_distribution = vec![OutputPair {
            address: "bcrt1qe2qaq0e8qlp425pxytrakala7725dynwhknufr"
                .parse::<bitcoin::Address<_>>()
                .unwrap()
                .assume_checked()
                .into(),
            amount: Amount::from_sat(5000000000),
        }];
        let mut mock_payout = MockPayoutDistribution::default();
        mock_payout.expect_pplns_weights().returning(|| None);
        mock_payout
            .expect_get_output_distribution()
            .returning(move |_, _, _, _, _| Ok(CoinbasePayouts::direct(test_distribution.clone())));

        let task_handle = tokio::spawn(async move {
            start_notify(
                notify_rx,
                template_tx,
                chain_store_handle,
                &stratum_config,
                Box::new(mock_payout),
                pool_difficulty,
                PoolTransactionCache::new(),
                None,
            )
            .await;
        });

        // Load a sample block template
        let data = include_str!(
            "../../../../p2poolv2_tests/test_data/gbt/regtest/ckpool/one-txn/gbt.json"
        );
        let gbt_json: serde_json::Value = serde_json::from_str(data).expect("Invalid JSON");
        let template: BlockTemplate =
            serde_json::from_value(gbt_json.clone()).expect("Failed to parse BlockTemplate");

        // Send the template through the channel
        notify_tx
            .send(NotifyCmd::SendToAll {
                template: Arc::new(template),
            })
            .await
            .expect("Failed to send template");

        // Wait for the watch channel to receive the prepared template
        template_rx.changed().await.expect("Watch channel closed");
        let prepared = template_rx.borrow_and_update().clone();
        assert!(
            prepared.is_some(),
            "Watch channel should contain PreparedNotifyParams"
        );

        // Verify we can build a per-miner notify from the prepared template
        let miner_pubkey: CompressedPublicKey =
            "020202020202020202020202020202020202020202020202020202020202020202"
                .parse()
                .unwrap();
        let miner_address = PayoutAddress::from(Address::p2wpkh(&miner_pubkey, Network::Signet));
        let tracker_handle = start_tracker_actor();
        let result = build_notify_from_prepared(
            prepared.as_ref().unwrap(),
            Some(&miner_address),
            Some(p2poolv2_wallet::share_owner::ShareOwner::Taproot(
                make_test_share_address(1, bitcoin::Network::Signet).witness_program(),
            )),
            &tracker_handle,
        );
        assert!(result.is_ok(), "build_notify_from_prepared should succeed");

        // Cleanup
        drop(notify_tx);
        task_handle.await.expect("Task failed");
    }

    #[tokio::test]
    async fn test_build_prepared_notify() {
        // Load a sample block template
        let data = include_str!(
            "../../../../p2poolv2_tests/test_data/gbt/regtest/ckpool/one-txn/gbt.json"
        );
        let gbt_json: serde_json::Value = serde_json::from_str(data).expect("Invalid JSON");
        let template: BlockTemplate =
            serde_json::from_value(gbt_json.clone()).expect("Failed to parse BlockTemplate");

        let mut chain_store_handle = ChainStoreHandle::default();

        let genesis = genesis_for_tests();
        let genesis_hash = genesis.block_hash();
        chain_store_handle
            .expect_get_mining_base_and_uncles()
            .returning(move || Ok((genesis_hash, vec![])));

        chain_store_handle.expect_get_asert_parent().returning(|_| {
            let genesis_header = genesis_for_tests().header;
            Ok(AsertParent::from_header(
                &genesis_header,
                0,
                genesis_header.time,
            ))
        });

        // Setup config and tracker
        let stratum_config = StratumConfig::new_for_test_default().parse().unwrap();
        let tracker_handle = start_tracker_actor();
        let miner_pubkey: CompressedPublicKey =
            "020202020202020202020202020202020202020202020202020202020202020202"
                .parse()
                .unwrap();
        let btcaddress = PayoutAddress::from(Address::p2wpkh(&miner_pubkey, Network::Signet));

        let pool_difficulty =
            pool_difficulty::PoolDifficulty::new(genesis.header.bits, genesis.header.time, 0);

        let test_distribution = vec![OutputPair {
            address: "bcrt1qe2qaq0e8qlp425pxytrakala7725dynwhknufr"
                .parse::<bitcoin::Address<_>>()
                .unwrap()
                .assume_checked()
                .into(),
            amount: Amount::from_sat(template.coinbasevalue),
        }];
        let mock_payout = mock_payout_with_distribution(test_distribution);

        let mut context = NotifyContext {
            chain_store_handle,
            config: stratum_config,
            pool_signature: b"test_pool".to_vec(),
            pool_difficulty,
            payout: Box::new(mock_payout),
            pool_transactions: PoolTransactionCache::new(),
            share_transaction_pool: None,
            merge_tokens: None,
        };

        // Build prepared notify
        let prepared = build_prepared_notify(&Arc::new(template.clone()), false, &mut context);
        assert!(prepared.is_ok());
        let prepared = prepared.unwrap();

        // Build per-miner notify from prepared
        let notify_str = build_notify_from_prepared(
            &prepared,
            Some(&btcaddress),
            Some(p2poolv2_wallet::share_owner::ShareOwner::Taproot(
                make_test_share_address(1, bitcoin::Network::Signet).witness_program(),
            )),
            &tracker_handle,
        )
        .expect("build_notify_from_prepared should succeed");

        // Verify notify string is valid JSON
        let notify: Notify = serde_json::from_str(&notify_str).expect("Invalid notify JSON");
        assert_eq!(notify.params.version, "20000000");
        assert_eq!(notify.params.nbits, "207fffff");
        assert!(!notify.params.clean_jobs);

        // Verify the job was inserted in the tracker
        let job_id = JobId(u64::from_str_radix(&notify.params.job_id, 16).unwrap());
        let job_details = tracker_handle.get_job(job_id);
        assert!(job_details.is_some());
        let details = job_details.unwrap();

        // Verify share_commitment is properly set in tracker
        assert!(details.share_commitment.is_some());
        let stored_commitment = details.share_commitment.unwrap();
        assert_eq!(stored_commitment.miner_bitcoin_address, btcaddress);
        assert_eq!(stored_commitment.prev_share_blockhash, genesis.block_hash());
    }

    /// Build notify via prepared path, parse coinbase2, and verify extracted outputs match.
    #[tokio::test]
    async fn test_build_notify_and_extract_outputs_integration() {
        let template = BlockTemplate {
            default_witness_commitment: Some(
                "6a24aa21a9ed0100000000000000000000000000000000000000000000000000000000000000"
                    .to_string(),
            ),
            height: 100,
            version: 0x20000000,
            previousblockhash: "0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
            bits: "1d00ffff".to_string(),
            curtime: 1234567890,
            transactions: vec![TemplateTransaction {
                data: "".to_string(),
                txid: "0000000000000000000000000000000000000000000000000000000000000001"
                    .to_string(),
                hash: "".to_string(),
                depends: vec![],
                fee: 0,
                sigops: 0,
                weight: 0,
            }],
            coinbasevalue: 100000,
            coinbaseaux: HashMap::new(),
            rules: vec![],
            vbavailable: HashMap::new(),
            vbrequired: 0,
            longpollid: "".to_string(),
            target: "".to_string(),
            mintime: 0,
            mutable: vec![],
            noncerange: "".to_string(),
            sigoplimit: 0,
            sizelimit: 0,
            weightlimit: 0,
            provided: None,
        };

        let original_output_pairs = vec![
            OutputPair {
                address: bitcoin::Address::from_str("bcrt1qe2qaq0e8qlp425pxytrakala7725dynwhknufr")
                    .unwrap()
                    .assume_checked()
                    .into(),
                amount: Amount::from_sat(50000),
            },
            OutputPair {
                address: bitcoin::Address::from_str("bcrt1qlk935ze2fsu86zjp395uvtegztrkaezawxx0wf")
                    .unwrap()
                    .assume_checked()
                    .into(),
                amount: Amount::from_sat(12345),
            },
        ];

        let pool_signature = b"test_sig";
        let witness_commitment = template.default_witness_commitment.clone();

        let prepared = PreparedNotifyParamsBuilder::new(
            Arc::new(template),
            original_output_pairs.clone(),
            pool_signature,
            true,
        )
        .bits(bitcoin::CompactTarget::from_consensus(0x1d00ffff))
        .build()
        .expect("Failed to build prepared notify");

        let address = original_output_pairs[0].address.clone();
        let tracker_handle = start_tracker_actor();
        let notify_json = build_notify_from_prepared(
            &prepared,
            Some(&address),
            Some(p2poolv2_wallet::share_owner::ShareOwner::Taproot(
                make_test_share_address(1, bitcoin::Network::Signet).witness_program(),
            )),
            &tracker_handle,
        )
        .expect("Failed to build notify");

        let notify: Notify = serde_json::from_str(&notify_json).expect("Invalid notify JSON");
        let coinbase2_hex = &notify.params.coinbase2;

        // Extract outputs from coinbase2 and verify they match the original
        let extracted_txouts =
            extract_outputs_from_coinbase2(coinbase2_hex, pool_signature.len()).unwrap();

        let expected_txout_1 = TxOut {
            value: original_output_pairs[0].amount,
            script_pubkey: original_output_pairs[0].address.script_pubkey(),
        };
        let expected_txout_2 = TxOut {
            value: original_output_pairs[1].amount,
            script_pubkey: original_output_pairs[1].address.script_pubkey(),
        };

        let witness_script = ScriptBuf::from(hex::decode(witness_commitment.unwrap()).unwrap());
        let expected_txout_3 = TxOut {
            value: Amount::ZERO,
            script_pubkey: witness_script,
        };

        // 2 payments + witness commitment + padding + share commitment
        assert_eq!(extracted_txouts.len(), 5);
        assert_eq!(extracted_txouts[0], expected_txout_1);
        assert_eq!(extracted_txouts[1], expected_txout_2);
        assert_eq!(extracted_txouts[2], expected_txout_3);
        assert_eq!(extracted_txouts[3].value, Amount::ZERO);
        assert_eq!(extracted_txouts[4].value, Amount::ZERO);
    }

    /// A template whose nBits fail the token ASERT band gets no tokens: its
    /// token shares would be refused by every peer. Regtest has no anchor.
    #[test]
    fn token_jobs_need_a_template_inside_the_asert_band() {
        let data = include_str!(
            "../../../../p2poolv2_tests/test_data/gbt/regtest/ckpool/one-txn/gbt.json"
        );
        let mut template: BlockTemplate = serde_json::from_str(data).unwrap();
        let chipnet = BchAsert::CHIPNET;
        template.height = chipnet.anchor_height + 1 + 120;
        template.curtime = (chipnet.anchor_parent_time + 600) as u32;
        let computed = chipnet
            .next_target(template.height - 1, template.curtime)
            .unwrap();
        let mut config = StratumConfig::new_for_test_default().parse().unwrap();
        config.bitcoin_cash_network = Some(BitcoinCashNetwork::Chipnet);

        template.bits = format!("{:08x}", get_compact(computed).to_consensus());
        template_meets_token_asert(&template, &config).unwrap();
        template.bits = format!(
            "{:08x}",
            get_compact(computed / U256::from(16)).to_consensus()
        );
        assert!(
            template_meets_token_asert(&template, &config)
                .unwrap_err()
                .contains("harder")
        );
        config.bitcoin_cash_network = Some(BitcoinCashNetwork::Regtest);
        template_meets_token_asert(&template, &config).unwrap();
    }
}
