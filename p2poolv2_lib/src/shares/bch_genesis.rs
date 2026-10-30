// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Bitcoin Cash share-chain genesis.
//!
//! Bitcoin genesis files stay as they are. A Bitcoin Cash node anchors its
//! share on the published BCHN genesis block for the selected network and
//! pays the NUMS key with P2PKH. Main and mini differ by their anchor target,
//! so the genesis hash differs and the chains do not reorg into each other.

use crate::config::{BchShareChain, BitcoinCashNetwork, Chain, StratumConfig};
use crate::shares::coinbase_proof::CoinbaseProof;
use crate::shares::genesis;
use crate::shares::share_block::{ShareBlock, ShareHeader, ShareTransaction};
use crate::shares::transactions::coinbase::build_sharechain_coinbase_transaction;
use crate::sim_overrides;
use bitcoin::constants::genesis_block;
use bitcoin::hashes::Hash;
use bitcoin::{Address, Block, CompactTarget, CompressedPublicKey, Network, PubkeyHash};
use p2poolv2_wallet::bch_share::BchShareNetwork;
use p2poolv2_wallet::share_owner::ShareOwner;
use std::error::Error;

/// Anchor target of the main share chain. This is the production value of
/// [`sim_overrides::anchor_target`] and is easier than [`crate::shares::share_block::MAX_POOL_TARGET`],
/// which remains the ceiling for shares after genesis.
pub const MAIN_ANCHOR_TARGET: u32 = 0x1b4188f5;

/// Anchor and ceiling of the mini share chain. The compact exponent is one
/// higher than [`crate::shares::share_block::MAX_POOL_TARGET`], which multiplies
/// the target by 256.
pub const MINI_ANCHOR_TARGET: u32 = 0x1c384bd7;

/// Published BCHN chipnet (and testnet4) genesis hash.
#[cfg(test)]
const CHIPNET_GENESIS_HASH: &str =
    "000000001dd410c49a788668ce26751718cc797474d3152a5fc073dd44fd9f7b";

/// Published BCHN scalenet genesis hash (`CScaleNetParams`).
#[cfg(test)]
const SCALENET_GENESIS_HASH: &str =
    "00000000e6453dc2dfe1ffa19023f86002eb11dbb8e87d0291a4599f0430be52";

/// Build the share genesis for this stratum config.
///
/// Bitcoin keeps [`ShareBlock::build_genesis_for_network`]. Bitcoin Cash uses
/// [`build`] and requires `bitcoin_cash_network`.
pub fn genesis_for_stratum<State>(
    stratum: &StratumConfig<State>,
) -> Result<ShareBlock, Box<dyn Error + Send + Sync>> {
    match stratum.chain {
        Chain::Bitcoin => ShareBlock::build_genesis_for_network(stratum.network),
        Chain::BitcoinCash => {
            let bitcoin_cash_network = stratum
                .bitcoin_cash_network
                .ok_or("bitcoin_cash_network is required")?;
            build(share_network(bitcoin_cash_network), stratum.bch_share_chain)
        }
    }
}

/// Share genesis for a Bitcoin Cash network and main or mini chain.
pub fn build(
    network: BchShareNetwork,
    share_chain: BchShareChain,
) -> Result<ShareBlock, Box<dyn Error + Send + Sync>> {
    let anchor = anchor_block(network);
    let genesis_data = genesis::genesis_data(Network::Regtest)
        .map_err(|error| -> Box<dyn Error + Send + Sync> { error.to_string().into() })?;
    let pubkey_hash = nums_pubkey_hash(genesis_data.public_key)?;
    let payout_network = payout_network(network);
    let miner_bitcoin_address = Address::p2pkh(pubkey_hash, payout_network);
    let miner_address = ShareOwner::CashP2pkh(pubkey_hash.to_byte_array());
    let coinbase =
        build_sharechain_coinbase_transaction(&miner_address, anchor.header.block_hash(), &[]);
    let transactions = vec![ShareTransaction(coinbase)];
    let header = ShareHeader {
        prev_share_blockhash: bitcoin::BlockHash::all_zeros(),
        uncles: vec![],
        miner_bitcoin_address: miner_bitcoin_address.into(),
        miner_address,
        bitcoin_header: anchor.header,
        time: sim_overrides::genesis_timestamp(&genesis_data),
        bits: anchor_bits(share_chain),
        donation_address: None,
        donation: None,
        fee_address: None,
        fee: None,
        bitcoin_height: 0,
        // The anchor block's coinbase predates the share chain and carries no
        // commitment. Genesis is built locally and never verified.
        coinbase_proof: CoinbaseProof::default(),
        bch_coinbase: None,
        pool_merge_mining: None,
        share_version: crate::shares::share_version::ShareVersion::CURRENT,
    };
    let bitcoin_coinbase = anchor
        .txdata
        .first()
        .cloned()
        .ok_or("Bitcoin Cash anchor block has no coinbase")?;
    Ok(ShareBlock {
        header,
        transactions,
        template_merkle_branches: vec![],
        bitcoin_coinbase,
    })
}

/// BCHN genesis block for `network`. Mainnet and regtest are the blocks
/// `bitcoin::constants::genesis_block` already builds. Chipnet and scalenet
/// use the timestamp, nonce, and bits published in BCHN `CChipNetParams` and
/// `CScaleNetParams`.
pub fn anchor_block(network: BchShareNetwork) -> Block {
    match network {
        BchShareNetwork::Mainnet => genesis_block(Network::Bitcoin),
        BchShareNetwork::Regtest => genesis_block(Network::Regtest),
        BchShareNetwork::Chipnet => chipnet_genesis_block(),
        BchShareNetwork::Scalenet => scalenet_genesis_block(),
    }
}

/// Ceiling applied to share targets on the chain anchored at `anchor`.
///
/// Mini keeps its own easier ceiling. Every other anchor, including Bitcoin
/// and the Bitcoin Cash main chain, keeps [`crate::shares::share_block::MAX_POOL_TARGET`].
pub fn target_ceiling(anchor: CompactTarget) -> CompactTarget {
    if anchor.to_consensus() == MINI_ANCHOR_TARGET {
        CompactTarget::from_consensus(MINI_ANCHOR_TARGET)
    } else {
        CompactTarget::from_consensus(crate::shares::share_block::MAX_POOL_TARGET)
    }
}

fn chipnet_genesis_block() -> Block {
    let mut block = genesis_block(Network::Bitcoin);
    block.header.time = 1_597_811_185;
    block.header.bits = CompactTarget::from_consensus(0x1d00ffff);
    block.header.nonce = 114_152_193;
    block
}

/// BCHN `CScaleNetParams` genesis: `CreateGenesisBlock(1598282438,
/// -1567304284, 0x1d00ffff, 1, 50 * COIN)`. The nonce is a `uint32_t`, so
/// -1,567,304,284 is 2,727,663,012.
fn scalenet_genesis_block() -> Block {
    let mut block = genesis_block(Network::Bitcoin);
    block.header.time = 1_598_282_438;
    block.header.bits = CompactTarget::from_consensus(0x1d00ffff);
    block.header.nonce = 2_727_663_012;
    block
}

fn anchor_bits(share_chain: BchShareChain) -> CompactTarget {
    match share_chain {
        BchShareChain::Main => sim_overrides::anchor_target(),
        BchShareChain::Mini => CompactTarget::from_consensus(MINI_ANCHOR_TARGET),
    }
}

fn payout_network(network: BchShareNetwork) -> Network {
    match network {
        BchShareNetwork::Mainnet => Network::Bitcoin,
        BchShareNetwork::Chipnet | BchShareNetwork::Regtest | BchShareNetwork::Scalenet => {
            Network::Regtest
        }
    }
}

fn nums_pubkey_hash(public_key: &str) -> Result<PubkeyHash, Box<dyn Error + Send + Sync>> {
    let key = public_key
        .parse::<CompressedPublicKey>()
        .map_err(|error| -> Box<dyn Error + Send + Sync> { error.to_string().into() })?;
    Ok(PubkeyHash::hash(&key.0.serialize()))
}

fn share_network(network: BitcoinCashNetwork) -> BchShareNetwork {
    match network {
        BitcoinCashNetwork::Mainnet => BchShareNetwork::Mainnet,
        BitcoinCashNetwork::Chipnet => BchShareNetwork::Chipnet,
        BitcoinCashNetwork::Regtest => BchShareNetwork::Regtest,
        BitcoinCashNetwork::Scalenet => BchShareNetwork::Scalenet,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use p2poolv2_wallet::share_owner::ShareOwner;

    #[test]
    fn mainnet_anchor_is_the_bch_genesis_block() {
        let block = anchor_block(BchShareNetwork::Mainnet);
        assert_eq!(
            block.block_hash().to_string(),
            "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f"
        );
    }

    #[test]
    fn regtest_anchor_is_the_bchn_regtest_genesis_block() {
        let block = anchor_block(BchShareNetwork::Regtest);
        assert_eq!(
            block.block_hash().to_string(),
            "0f9188f13cb7b2c71f2a335e3a4fc328bf5beb436012afca590b1a11466e2206"
        );
    }

    #[test]
    fn chipnet_anchor_is_the_published_bchn_genesis_block() {
        let block = anchor_block(BchShareNetwork::Chipnet);
        assert_eq!(block.block_hash().to_string(), CHIPNET_GENESIS_HASH);
    }

    #[test]
    fn scalenet_anchor_is_the_published_bchn_genesis_block() {
        let block = anchor_block(BchShareNetwork::Scalenet);
        assert_eq!(block.block_hash().to_string(), SCALENET_GENESIS_HASH);
    }

    /// Scalenet's share chain has its own genesis, so its protocol string
    /// differs from chipnet's and the two never peer.
    #[test]
    fn scalenet_genesis_differs_from_chipnet_genesis() {
        let scalenet = build(BchShareNetwork::Scalenet, BchShareChain::Main).unwrap();
        let chipnet = build(BchShareNetwork::Chipnet, BchShareChain::Main).unwrap();
        assert_ne!(scalenet.block_hash(), chipnet.block_hash());
    }

    #[test]
    fn bitcoin_cash_genesis_pays_p2pkh_and_mini_is_a_different_chain() {
        let main = build(BchShareNetwork::Regtest, BchShareChain::Main).unwrap();
        let mini = build(BchShareNetwork::Regtest, BchShareChain::Mini).unwrap();
        assert!(matches!(
            main.header.miner_address,
            ShareOwner::CashP2pkh(_)
        ));
        assert!(main.header.miner_bitcoin_address.script_pubkey().is_p2pkh());
        assert_eq!(main.transactions[0].output.len(), 1);
        assert!(main.transactions[0].input[0].witness.is_empty());
        assert_ne!(main.block_hash(), mini.block_hash());
        assert_eq!(mini.header.bits.to_consensus(), MINI_ANCHOR_TARGET);
        assert_eq!(main.header.bits.to_consensus(), MAIN_ANCHOR_TARGET);
    }

    // The Bitcoin Cash genesis shares, pinned. Their hashes name the share
    // chain in every protocol string, so a change to anything they are built
    // from (the regtest genesis entry's key and timestamp, the anchor blocks,
    // the share coinbase or header encoding) would split the network. These
    // values were computed before and after the upstream sync through
    // `61810c56`, which moved the Bitcoin genesis blocks to `.dat` files.

    #[test]
    fn mainnet_main_genesis_hash_is_pinned() {
        let genesis = build(BchShareNetwork::Mainnet, BchShareChain::Main).unwrap();
        assert_eq!(
            genesis.block_hash().to_string(),
            "fb3cb844455afa62b8971c94af8927f23eb934c37709bdefb3cc2881001c98d5"
        );
    }

    #[test]
    fn mainnet_mini_genesis_hash_is_pinned() {
        let genesis = build(BchShareNetwork::Mainnet, BchShareChain::Mini).unwrap();
        assert_eq!(
            genesis.block_hash().to_string(),
            "ab4967c507cc16bb42e6041756690c173ad9227840f8037d1129824e4f533c78"
        );
    }

    #[test]
    fn chipnet_main_genesis_hash_is_pinned() {
        let genesis = build(BchShareNetwork::Chipnet, BchShareChain::Main).unwrap();
        assert_eq!(
            genesis.block_hash().to_string(),
            "2f2e457b8ef23012e9593db190914b87fda47817220e5e5f1fb7bc031fc663ed"
        );
    }

    #[test]
    fn chipnet_mini_genesis_hash_is_pinned() {
        let genesis = build(BchShareNetwork::Chipnet, BchShareChain::Mini).unwrap();
        assert_eq!(
            genesis.block_hash().to_string(),
            "6d6c2b7c38ee5caf040199adb831e31973d2e941578c0fe2833b3c26f3fe7cd0"
        );
    }

    #[test]
    fn regtest_main_genesis_hash_is_pinned() {
        let genesis = build(BchShareNetwork::Regtest, BchShareChain::Main).unwrap();
        assert_eq!(
            genesis.block_hash().to_string(),
            "ae37095edfbb29bfca6bff03565945ddb0188ba03761f454088ead6f841fc732"
        );
    }

    #[test]
    fn regtest_mini_genesis_hash_is_pinned() {
        let genesis = build(BchShareNetwork::Regtest, BchShareChain::Mini).unwrap();
        assert_eq!(
            genesis.block_hash().to_string(),
            "70249f9448079ca55b4842f6efa5dd888db2fcc7b12718ae73a9425d999bcc19"
        );
    }

    #[test]
    fn bitcoin_stratum_keeps_the_taproot_genesis() {
        let config = crate::config::StratumConfig::new_for_test_default();
        let genesis = genesis_for_stratum(&config).unwrap();
        assert!(matches!(
            genesis.header.miner_address,
            ShareOwner::Taproot(_)
        ));
    }
}
