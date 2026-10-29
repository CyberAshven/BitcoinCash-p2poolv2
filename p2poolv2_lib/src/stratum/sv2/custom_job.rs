// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Validate the miner's BCH job before installing it in a mining channel.

use super::session::DeclaredJob;
use crate::shares::auxpow::Disclosure;
use crate::shares::bch_coinbase::BchCoinbase;
use crate::shares::coinbase_proof::MAX_COINBASE_MERKLE_BRANCH_LENGTH;
use crate::shares::merge_mining::MergeMiningCommitment;
use crate::shares::share_commitment::ShareCommitment;
use crate::stratum::work::block_template::TemplateTransaction;
use crate::stratum::work::coinbase::{
    EXTRANONCE_SEPARATOR, build_declared_coinbase, check_bch_coinbase_limit,
};
use crate::stratum::work::gbt::{
    build_merkle_branches_for_template, compute_merkle_root_from_branches,
};
use crate::stratum::work::prepared_notify::token_split;
use crate::stratum::work::tracker::JobDetails;
use crate::utils::time_provider::{SystemTimeProvider, TimeProvider};
use bitcoin::block::{Header, Version};
use bitcoin::consensus::{deserialize, serialize};
use bitcoin::hashes::{Hash, sha256d};
use bitcoin::{Address, Amount, Block, BlockHash, CompactTarget, TxMerkleNode, TxOut};
use mining_sv2::SetCustomMiningJob;
use p2poolv2_wallet::share_owner::ShareOwner;
use std::str::FromStr;
use std::sync::Arc;

/// The leaves a Full-Template declaration discloses for its coinbase, read
/// from `DeclareMiningJob.excess_data` (`CTMD` v2, as Pickaxe's Job
/// Declaration client sends them): exactly one canonical disclosure whose
/// leaves rebuild output 0's root at its height and salt.
///
/// Empty `excess_data` means no root: a coinbase whose output 0 is a token
/// root and declares no leaves is refused, and so are leaves beside the
/// empty commitment. `coinbase_suffix` is the declared coinbase after the
/// extranonce: the sequence, the outputs and the locktime.
pub(crate) fn declared_leaves(
    coinbase_suffix: &[u8],
    excess_data: &[u8],
) -> Result<Disclosure, String> {
    let outputs: Vec<TxOut> = coinbase_suffix
        .len()
        .checked_sub(4)
        .and_then(|end| coinbase_suffix.get(4..end))
        .and_then(|outputs| deserialize(outputs).ok())
        .ok_or("the declared coinbase's outputs do not read")?;
    let merge_mining = outputs
        .first()
        .ok_or("the declared coinbase has no output 0")
        .and_then(MergeMiningCommitment::from_output)?;
    if merge_mining.is_empty() {
        return if excess_data.is_empty() {
            Ok(Disclosure::default())
        } else {
            Err("token leaves were declared without a token commitment".into())
        };
    }
    if excess_data.is_empty() {
        return Err("a token commitment needs its leaves in excess_data".into());
    }
    let disclosure = Disclosure::decode(excess_data).map_err(|error| error.to_string())?;
    disclosure
        .check_root(&merge_mining)
        .map_err(|error| error.to_string())?;
    Ok(disclosure)
}

/// The token rules of a declared job, so the node's fee and the miner's
/// P2Pool donation apply to its tokens as to its BCH.
///
/// A Coinbase-only job sends no declaration, so the node could never see
/// its leaves: its output 0 must be the empty commitment. A Full-Template
/// job's disclosed leaves (`declared_leaves`, checked when it was declared)
/// must make its root, and each must pay the channel's miner, with the
/// split of its job's terms: the donation the token carries, then the
/// node's fee, each to the script the coinbase pays it to.
fn check_declared_tokens(
    declared: &DeclaredJob,
    merge_mining: &MergeMiningCommitment,
    commitment: &ShareCommitment,
) -> Result<(), String> {
    let Some(declaration) = &declared.declaration else {
        return if merge_mining.is_empty() {
            Ok(())
        } else {
            Err("a Coinbase-only custom job cannot carry a token commitment".into())
        };
    };
    declaration
        .disclosure
        .check_root(merge_mining)
        .map_err(|error| error.to_string())?;
    let payout_hash =
        sha256d::Hash::hash(commitment.miner_bitcoin_address.script_pubkey().as_bytes())
            .to_byte_array();
    let split = token_split(
        &commitment.donation_address,
        commitment.donation,
        &commitment.fee_address,
        commitment.fee,
    )
    .map_err(|error| error.message)?;
    for disclosed in &declaration.disclosure.leaves {
        let category = hex::encode(disclosed.leaf.category);
        if disclosed.leaf.payout_hash != payout_hash {
            return Err(format!("token {category}'s leaf does not pay the miner"));
        }
        if disclosed.split != split {
            return Err(format!(
                "token {category}'s split is not the job's donation and the node's fee"
            ));
        }
    }
    Ok(())
}

/// Coinbase-only jobs deliberately do not reveal their transaction list.
/// Their declarator must validate CTOR/consensus and broadcast found blocks.
pub(crate) fn validate_custom_job(
    request: &SetCustomMiningJob,
    declared: DeclaredJob,
    payout: Address,
    owner: ShareOwner,
    version_mask: i32,
) -> Result<(JobDetails, Option<Block>), String> {
    let original = declared.prepared.template();
    let prev_hash =
        BlockHash::from_str(&original.previousblockhash).map_err(|error| error.to_string())?;
    let bits =
        CompactTarget::from_unprefixed_hex(&original.bits).map_err(|error| error.to_string())?;
    let now = SystemTimeProvider.seconds_since_epoch();
    if request.prev_hash.as_bytes() != prev_hash.as_byte_array()
        || request.nbits != bits.to_consensus()
        || (request.version as i32 ^ original.version) & !version_mask != 0
        || request.min_ntime < original.mintime
        || u64::from(request.min_ntime) > now + 7200
    {
        return Err("invalid-header".into());
    }
    if request.coinbase_tx_version != 2
        || request.coinbase_tx_input_n_sequence != 0xffff_fffe
        || request.coinbase_tx_locktime != original.height.checked_sub(1).ok_or("invalid-height")?
    {
        return Err("invalid-coinbase-layout".into());
    }
    if !matches!(owner, ShareOwner::CashP2pkh(_)) {
        return Err("invalid-share-owner".into());
    }
    let outputs: Vec<TxOut> =
        deserialize(request.coinbase_tx_outputs.as_bytes()).map_err(|error| error.to_string())?;
    // Merge-mining format v2 puts the commitment at output 0; the rebuilt
    // coinbase below must then match every output byte for byte.
    let merge_mining =
        MergeMiningCommitment::from_output(outputs.first().ok_or("missing-commitment-outputs")?)?;
    let value = outputs
        .iter()
        .try_fold(0u64, |total, output| {
            total.checked_add(output.value.to_sat())
        })
        .filter(|value| *value <= Amount::MAX_MONEY.to_sat())
        .ok_or("invalid-coinbase-value")?;
    let descriptor = BchCoinbase {
        prefix: request.coinbase_prefix.as_bytes().to_vec(),
        merge_mining,
    };
    descriptor.validate(u64::from(original.height))?;
    let mut commitment =
        ShareCommitment::from_mining_job_token(request.token.as_bytes(), payout, owner)
            .map_err(|error| error.to_string())?;
    commitment.bch_coinbase = Some(descriptor.clone());
    check_declared_tokens(&declared, &merge_mining, &commitment)?;
    let distribution = declared
        .prepared
        .declared_outputs(value, &merge_mining)
        .map_err(|error| error.to_string())?;
    let coinbase = build_declared_coinbase(
        &distribution,
        u64::from(original.height),
        commitment.hash(),
        &EXTRANONCE_SEPARATOR,
        &descriptor,
    )
    .map_err(|error| error.to_string())?;
    if coinbase.output != outputs {
        return Err("invalid-pplns-outputs".into());
    }
    check_bch_coinbase_limit(&coinbase, declared.prepared.declared_split(&merge_mining))
        .map_err(|_| "coinbase-too-large")?;
    let serialized = serialize(&coinbase);
    // The bounded scriptSig uses a one-byte length after version, vin count and outpoint.
    let nonce_offset = 4 + 1 + 36 + 1 + descriptor.prefix.len();
    let prefix = &serialized[..nonce_offset];
    let suffix = &serialized[nonce_offset + EXTRANONCE_SEPARATOR.len()..];
    if prefix.len() > 65535 || suffix.len() > 65535 {
        return Err("coinbase-too-large".into());
    }
    if let Some(declaration) = &declared.declaration
        && (declaration.version != request.version
            || declaration.prefix != prefix
            || declaration.suffix != suffix)
    {
        return Err("declaration-mismatch".into());
    }
    if request.merkle_path.len() > MAX_COINBASE_MERKLE_BRANCH_LENGTH {
        return Err("merkle-path-too-long".into());
    }
    let branches = request
        .merkle_path
        .iter()
        .map(|branch| {
            let bytes: [u8; 32] = branch.as_bytes().try_into().expect("SV2 U256 is 32 bytes");
            TxMerkleNode::from_byte_array(bytes)
        })
        .collect::<Vec<_>>();
    let mut template = original.as_ref().clone();
    template.version = request.version as i32;
    template.curtime = request.min_ntime;
    template.coinbasevalue = value;
    template.coinbaseaux.clear();
    template.default_witness_commitment = None;
    // A declared block has the declarator's transactions, not a provider
    // template's, so it never goes back to a Template Provider.
    template.provided = None;
    template.transactions = declared
        .transactions
        .as_ref()
        .map(|transactions| {
            transactions
                .iter()
                .map(|transaction| TemplateTransaction {
                    data: hex::encode(serialize(transaction)),
                    txid: transaction.compute_txid().to_string(),
                    hash: transaction.compute_wtxid().to_string(),
                    depends: Vec::new(),
                    fee: 0,
                    sigops: 0,
                    weight: 0,
                })
                .collect()
        })
        .unwrap_or_default();
    template.validate_bch().map_err(|error| error.to_string())?;
    template
        .validate_bch_block_size(&coinbase)
        .map_err(|error| error.to_string())?;
    let header = Header {
        version: Version::from_consensus(request.version as i32),
        prev_blockhash: prev_hash,
        merkle_root: compute_merkle_root_from_branches(coinbase.compute_txid(), &branches),
        time: request.min_ntime,
        bits,
        nonce: 0,
    };
    let block_available = declared.transactions.is_some();
    let proposal = if block_available {
        let expected = build_merkle_branches_for_template(&template)
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(TxMerkleNode::from_raw_hash)
            .collect::<Vec<_>>();
        if expected != branches {
            return Err("invalid-merkle-path".into());
        }
        let mut txdata = Vec::with_capacity(template.transactions.len() + 1);
        txdata.push(coinbase);
        txdata.extend(
            declared
                .transactions
                .as_ref()
                .expect("full declaration")
                .iter()
                .cloned(),
        );
        Some(Block { header, txdata })
    } else {
        None
    };
    Ok((
        JobDetails {
            block_available,
            blocktemplate: Arc::new(template),
            coinbase1: hex::encode(prefix),
            coinbase2: hex::encode(suffix),
            generation_timestamp: now,
            share_commitment: Some(commitment),
            coinbase_nsecs: 0,
            template_merkle_branches: branches,
            // A declared job's terms travel in a 255-byte token that has no
            // room for share transactions, so its share carries none.
            share_transactions: Arc::from([]),
            merge_tokens: None,
            payout_tree: distribution.tree.map(Arc::new),
        },
        proposal,
    ))
}

#[cfg(test)]
mod tests;
