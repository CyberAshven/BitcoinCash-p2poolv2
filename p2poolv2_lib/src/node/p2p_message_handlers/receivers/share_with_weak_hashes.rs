// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A new share relayed with the weak shares it carries listed by hash
//! (`Message::ShareWithWeakHashes`, or `Message::ShareWithoutPayouts` once
//! its coinbase is rebuilt), and the answer to a peer that asks for the ones
//! it lacks (`Message::GetCarriedWeakShares`).
//!
//! The receiver fills each listed hash from its weak pool, then from the
//! weak shares the sender prefilled, asks the sender for the rest, and hands
//! the rebuilt share to `handle_share_block`, which checks it as it checks a
//! share sent whole, the weak root included. The prefilled and fetched weak
//! shares that filled a gap also go to the weak share worker, checked as
//! any relayed weak share, so the gossip does not fetch them again. A
//! share whose weak shares cannot all be filled -- the sender does not
//! answer within `WEAK_FILL_TIMEOUT`, answers without some of them, or lists
//! hashes that do not rebuild the header's weak root -- is not complete yet,
//! never invalid: it is asked for whole through the block fetcher, from
//! another peer that relayed it if one did, as a share announced by its
//! header is fetched.

use crate::node::SwarmSend;
use crate::node::messages::{Message, ShareWithWeakHashes};
use crate::node::p2p_message_handlers::receivers::block_receiver::BlockReceiverHandle;
use crate::node::p2p_message_handlers::receivers::share_blocks::handle_share_block;
use crate::node::request_response_handler::block_fetcher::{BlockFetcherEvent, BlockFetcherHandle};
use crate::node::validation_worker::ValidationSender;
use crate::node::weak_share_fill::{FillStart, WEAK_FILL_TIMEOUT};
use crate::node::weak_share_worker::{WeakShareEvent, WeakShareEventSender};
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::validation::{ShareValidator, check_weak_shares_match_header};
use crate::shares::weak::WeakShare;
use crate::shares::weak_pool::{WeakPool, WeakShareSource};
use bitcoin::BlockHash;
use libp2p::PeerId;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

/// Handle a new share a peer relayed with its weak shares listed by hash.
///
/// A stored share needs nothing filled, and one that fails the header or
/// coinbase proof check is refused before anything is fetched for it: both
/// go to `handle_share_block` as they came, which then reads only the
/// share's hash and header, as for a share sent whole. A new share must list
/// exactly the weak shares its header counts, each once, and prefill only
/// listed ones, each once; an honest sender lists the body it stored, which
/// its header binds, so another list is refused as from a bad peer. Its weak shares are then
/// filled from the weak pool and the prefilled ones. A share those fill is
/// handed on here; one they do not waits, in a task of its own, for the
/// sender to send the rest.
#[allow(clippy::too_many_arguments)] // handler wiring: each parameter is a distinct collaborator, as for handle_share_block, a params struct would only move the list
pub async fn handle_share_with_weak_hashes<C: Send + Sync + 'static>(
    mut share: ShareWithWeakHashes,
    peer: PeerId,
    chain_store_handle: &ChainStoreHandle,
    validation_tx: ValidationSender,
    block_receiver_handle: &BlockReceiverHandle,
    block_fetcher_handle: &BlockFetcherHandle,
    share_validator: &Arc<dyn ShareValidator + Send + Sync>,
    weak_share_tx: Option<&WeakShareEventSender>,
    swarm_tx: mpsc::Sender<SwarmSend<C>>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let block_hash = share.block_hash();
    let header = &share.share_block().header;
    if chain_store_handle.share_block_exists(&block_hash)
        || share_validator.validate_share_header(header).is_err()
        || share_validator
            .validate_coinbase_proof(header, &share.share_block().template_merkle_branches)
            .is_err()
    {
        return handle_share_block(
            share.into_unfilled(),
            chain_store_handle,
            validation_tx,
            block_receiver_handle,
            block_fetcher_handle,
            share_validator.as_ref(),
        )
        .await;
    }
    check_weak_hashes_listed(&share)?;

    let pool = weak_share_tx.map(WeakShareEventSender::pool);
    let mut slots = match &pool {
        Some(pool) => pool.pooled_weak_shares(share.weak_hashes()),
        None => vec![None; share.weak_hashes().len()],
    };
    let from_pool = slots.iter().flatten().count();
    // A prefilled weak share fills only a slot the pool left empty: the
    // pool's copy passed its checks.
    let prefilled = share.take_prefilled();
    let prefilled_count = prefilled.len();
    let prefilled = fill_slots(share.weak_hashes(), &mut slots, prefilled);
    let missing: Vec<BlockHash> = share
        .weak_hashes()
        .iter()
        .zip(&slots)
        .filter(|(_, slot)| slot.is_none())
        .map(|(hash, _)| *hash)
        .collect();
    // Counted, and the prefilled weak shares checked, only for a fill that
    // goes on, not for a relay of a share that already waits.
    let count_and_check_prefilled = |pool: &WeakPool| {
        pool.count_fill(from_pool, 0);
        pool.count_prefilled(prefilled.len(), prefilled_count - prefilled.len());
        check_as_relayed(weak_share_tx, peer, prefilled.iter().cloned());
    };
    if missing.is_empty() {
        if let Some(pool) = &pool {
            count_and_check_prefilled(pool);
        }
        return complete_share(
            share,
            slots,
            peer,
            chain_store_handle,
            validation_tx,
            block_receiver_handle,
            block_fetcher_handle,
            share_validator.as_ref(),
            pool.as_ref(),
        )
        .await;
    }

    let (Some(weak_share_tx), Some(pool)) = (weak_share_tx, pool) else {
        debug!("No weak pool to fill share {block_hash} from, asking {peer} for it whole");
        fetch_share_whole(block_hash, peer, block_fetcher_handle, None).await;
        return Ok(());
    };
    let fill = weak_share_tx.fill();
    let answer = match fill.start(block_hash, peer) {
        FillStart::Started(answer) => answer,
        FillStart::AlreadyWaiting => {
            debug!("Share {block_hash} from {peer} already waits for its weak shares");
            return Ok(());
        }
        FillStart::Full => {
            debug!("Too many shares wait for weak shares, asking {peer} for {block_hash} whole");
            fetch_share_whole(block_hash, peer, block_fetcher_handle, Some(&pool)).await;
            return Ok(());
        }
    };
    count_and_check_prefilled(&pool);
    debug!(
        "Share {block_hash} from {peer}: {from_pool} weak shares from the pool, {} prefilled, asking for {}",
        prefilled.len(),
        missing.len()
    );
    if let Err(error) = swarm_tx
        .send(SwarmSend::Request(
            peer,
            Message::GetCarriedWeakShares(block_hash, missing),
        ))
        .await
    {
        debug!("Failed to ask {peer} for the weak shares of share {block_hash}: {error}");
        fill.finish(block_hash);
        fetch_share_whole(block_hash, peer, block_fetcher_handle, Some(&pool)).await;
        return Ok(());
    }

    let chain_store_handle = chain_store_handle.clone();
    let block_receiver_handle = block_receiver_handle.clone();
    let block_fetcher_handle = block_fetcher_handle.clone();
    let share_validator = share_validator.clone();
    let weak_share_tx = weak_share_tx.clone();
    tokio::spawn(async move {
        let answered = tokio::time::timeout(WEAK_FILL_TIMEOUT, answer).await;
        let others = fill.finish(block_hash);
        let fetched = match answered {
            Ok(Ok(weak_shares)) => fill_slots(share.weak_hashes(), &mut slots, weak_shares),
            _ => {
                debug!("Peer {peer} did not send the weak shares of share {block_hash} in time");
                Vec::new()
            }
        };
        pool.count_fill(0, fetched.len());
        check_as_relayed(Some(&weak_share_tx), peer, fetched);
        // Another peer that relayed the share is asked for it whole first:
        // this one was slow, or sent what does not complete it.
        let whole_from = others.first().copied().unwrap_or(peer);
        if let Err(error) = complete_share(
            share,
            slots,
            whole_from,
            &chain_store_handle,
            validation_tx,
            &block_receiver_handle,
            &block_fetcher_handle,
            share_validator.as_ref(),
            Some(&pool),
        )
        .await
        {
            // As for a share handed on in the request handler: the peer
            // whose body fails is disconnected.
            warn!("Share {block_hash} relayed by {peer} with weak hashes refused: {error}");
            let _ = swarm_tx.send(SwarmSend::Disconnect(peer)).await;
        }
    });
    Ok(())
}

/// Answer a peer's `GetCarriedWeakShares` from the store: the weak shares
/// of the share `share_hash` with the proof-of-work hashes it asked for, in
/// the order the share carries them, or none when this node does not have
/// the share's body. The answer is always sent, so the stream is not left
/// open until timeout.
pub async fn handle_get_carried_weak_shares<C: Send + Sync>(
    share_hash: BlockHash,
    weak_hashes: Vec<BlockHash>,
    chain_store_handle: ChainStoreHandle,
    response_channel: C,
    swarm_tx: mpsc::Sender<SwarmSend<C>>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let asked: HashSet<BlockHash> = weak_hashes.into_iter().collect();
    let weak_shares: Vec<WeakShare> = chain_store_handle
        .get_share(&share_hash)
        .map(|share_block| {
            share_block
                .weak_shares
                .into_iter()
                .filter(|weak_share| asked.contains(&weak_share.pow_hash()))
                .collect()
        })
        .unwrap_or_default();
    debug!(
        "Serving {} of {} weak shares asked for of share {share_hash}",
        weak_shares.len(),
        asked.len()
    );
    if let Err(error) = swarm_tx
        .send(SwarmSend::Response(
            response_channel,
            Message::CarriedWeakShares(share_hash, weak_shares),
        ))
        .await
    {
        debug!("Failed to send carried weak shares response: {error}");
        return Err(format!("Failed to send carried weak shares response: {error}").into());
    }
    Ok(())
}

/// A share must list exactly the weak shares its header counts, each once,
/// and prefill only weak shares it lists, each once.
fn check_weak_hashes_listed(
    share: &ShareWithWeakHashes,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let block_hash = share.block_hash();
    let weak_count = share.share_block().header.weak_count;
    let listed = share.weak_hashes();
    if listed.len() != usize::from(weak_count) {
        return Err(format!(
            "Share {block_hash} lists {} weak shares, its header commits {weak_count}",
            listed.len()
        )
        .into());
    }
    let mut seen = HashSet::with_capacity(listed.len());
    if let Some(repeated) = listed.iter().find(|hash| !seen.insert(**hash)) {
        return Err(
            format!("Share {block_hash} lists weak share {repeated} more than once").into(),
        );
    }
    let mut prefilled = HashSet::with_capacity(share.prefilled().len());
    for pow_hash in share.prefilled().iter().map(WeakShare::pow_hash) {
        if !seen.contains(&pow_hash) {
            return Err(format!(
                "Share {block_hash} prefills weak share {pow_hash} it does not list"
            )
            .into());
        }
        if !prefilled.insert(pow_hash) {
            return Err(format!(
                "Share {block_hash} prefills weak share {pow_hash} more than once"
            )
            .into());
        }
    }
    Ok(())
}

/// Put weak shares the sender sent, prefilled or in answer to a request,
/// into the empty slots their proof-of-work hashes name, and return a copy
/// of each one that filled a slot. A weak share for no slot, or for one
/// already filled, is left out.
fn fill_slots(
    weak_hashes: &[BlockHash],
    slots: &mut [Option<WeakShare>],
    weak_shares: Vec<WeakShare>,
) -> Vec<WeakShare> {
    let mut sent: HashMap<BlockHash, WeakShare> = weak_shares
        .into_iter()
        .map(|weak_share| (weak_share.pow_hash(), weak_share))
        .collect();
    let mut filled = Vec::new();
    for (hash, slot) in weak_hashes.iter().zip(slots.iter_mut()) {
        if slot.is_none()
            && let Some(weak_share) = sent.remove(hash)
        {
            filled.push(weak_share.clone());
            *slot = Some(weak_share);
        }
    }
    filled
}

/// Hand weak shares a relayed share brought, which the weak pool lacked, to
/// the weak share worker as weak shares `peer` relayed: checked as any
/// relayed weak share, and pooled if they pass, so the weak share gossip
/// does not fetch them again. A full worker queue drops them; the share
/// that carries them is checked either way.
fn check_as_relayed(
    weak_share_tx: Option<&WeakShareEventSender>,
    peer: PeerId,
    weak_shares: impl IntoIterator<Item = WeakShare>,
) {
    let Some(weak_share_tx) = weak_share_tx else {
        return;
    };
    for weak_share in weak_shares {
        let pow_hash = weak_share.pow_hash();
        if weak_share_tx
            .try_send(WeakShareEvent {
                weak_share,
                source: WeakShareSource::Peer(peer),
            })
            .is_err()
        {
            debug!("Weak share queue full, not checking weak share {pow_hash} from {peer}");
        }
    }
}

/// Hand a share on once its weak shares are filled: rebuilt, it goes to
/// `handle_share_block` as a share sent whole. One still missing a weak
/// share, or whose weak shares do not rebuild its header's weak root, is
/// not complete: it is asked for whole from `whole_from` instead, and never
/// checked, so never marked invalid.
#[allow(clippy::too_many_arguments)] // handler wiring: each parameter is a distinct collaborator, as for handle_share_block, a params struct would only move the list
async fn complete_share(
    share: ShareWithWeakHashes,
    slots: Vec<Option<WeakShare>>,
    whole_from: PeerId,
    chain_store_handle: &ChainStoreHandle,
    validation_tx: ValidationSender,
    block_receiver_handle: &BlockReceiverHandle,
    block_fetcher_handle: &BlockFetcherHandle,
    share_validator: &(dyn ShareValidator + Send + Sync),
    pool: Option<&WeakPool>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let block_hash = share.block_hash();
    let Some(weak_shares) = slots.into_iter().collect::<Option<Vec<WeakShare>>>() else {
        debug!("Share {block_hash} still lacks weak shares, asking {whole_from} for it whole");
        fetch_share_whole(block_hash, whole_from, block_fetcher_handle, pool).await;
        return Ok(());
    };
    let share_block = share.with_weak_shares(weak_shares);
    if let Err(error) = check_weak_shares_match_header(&share_block) {
        debug!(
            "Weak shares listed for share {block_hash} do not rebuild its body ({error}), asking {whole_from} for it whole"
        );
        fetch_share_whole(block_hash, whole_from, block_fetcher_handle, pool).await;
        return Ok(());
    }
    handle_share_block(
        share_block,
        chain_store_handle,
        validation_tx,
        block_receiver_handle,
        block_fetcher_handle,
        share_validator,
    )
    .await
}

/// Ask the block fetcher for the share `block_hash` whole, from `peer`
/// first and by round robin if it does not answer in time, as a share a
/// peer announced by its header is fetched. With `pool`, it counts as a
/// share whose weak shares could not be filled.
pub(crate) async fn fetch_share_whole(
    block_hash: BlockHash,
    peer: PeerId,
    block_fetcher_handle: &BlockFetcherHandle,
    pool: Option<&WeakPool>,
) {
    if let Some(pool) = pool {
        pool.count_fill_fetched_whole();
    }
    if let Err(error) = block_fetcher_handle
        .send(BlockFetcherEvent::FetchBlocks {
            blockhashes: vec![block_hash],
            peer_id: peer,
            use_peer: true,
        })
        .await
    {
        error!("Failed to ask the block fetcher for share {block_hash}: {error}");
    }
}

#[cfg(test)]
mod tests;
