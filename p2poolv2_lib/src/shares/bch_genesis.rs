// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Bitcoin Cash share-chain genesis.
//!
//! Bitcoin genesis files stay as they are. A Bitcoin Cash node anchors its
//! share on the published BCHN genesis block for the selected network and
//! pays the NUMS key with P2PKH. Bitcoin Cash has one share chain per
//! network: the main and mini chains merged on 2026-11-09 into one that keeps
//! the mini chain's anchor, ceiling and genesis (`MINI_ANCHOR_TARGET`).

use crate::config::{BitcoinCashNetwork, Chain, StratumConfig};
use crate::shares::band::GENESIS_BAND_CAP;
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

/// Anchor and ceiling of the Bitcoin Cash share chain, the former mini
/// chain's. The compact exponent is one higher than
/// [`crate::shares::share_block::MAX_POOL_TARGET`], Bitcoin's ceiling, which
/// multiplies the target by 256: difficulty about 4.5, so a small pool still
/// makes a share every 10 seconds.
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
            build(share_network(bitcoin_cash_network))
        }
    }
}

/// Share genesis for a Bitcoin Cash network.
pub fn build(network: BchShareNetwork) -> Result<ShareBlock, Box<dyn Error + Send + Sync>> {
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
    let anchor_bits = sim_overrides::bch_anchor_target();
    let header = ShareHeader {
        prev_share_blockhash: bitcoin::BlockHash::all_zeros(),
        uncles: vec![],
        miner_bitcoin_address: miner_bitcoin_address.into(),
        miner_address,
        bitcoin_header: anchor.header,
        time: sim_overrides::genesis_timestamp(&genesis_data),
        bits: anchor_bits,
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
        asert_bits: anchor_bits,
        wanted_bits: anchor_bits,
        band_cap: GENESIS_BAND_CAP,
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
/// The Bitcoin Cash chain, anchored at [`MINI_ANCHOR_TARGET`], keeps that
/// easier ceiling. Every other anchor keeps
/// [`crate::shares::share_block::MAX_POOL_TARGET`]: Bitcoin's, and the sim's
/// Bitcoin Cash anchor at its network hashrate's steady difficulty
/// ([`sim_overrides::bch_anchor_target`]).
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
        let scalenet = build(BchShareNetwork::Scalenet).unwrap();
        let chipnet = build(BchShareNetwork::Chipnet).unwrap();
        assert_ne!(scalenet.block_hash(), chipnet.block_hash());
    }

    /// The one Bitcoin Cash share chain starts at the mini chain's anchor.
    #[test]
    fn bitcoin_cash_genesis_pays_p2pkh_at_the_mini_anchor() {
        let genesis = build(BchShareNetwork::Regtest).unwrap();
        assert!(matches!(
            genesis.header.miner_address,
            ShareOwner::CashP2pkh(_)
        ));
        assert!(
            genesis
                .header
                .miner_bitcoin_address
                .script_pubkey()
                .is_p2pkh()
        );
        assert_eq!(genesis.transactions[0].output.len(), 1);
        assert!(genesis.transactions[0].input[0].witness.is_empty());
        assert_eq!(genesis.header.bits.to_consensus(), MINI_ANCHOR_TARGET);
        assert_eq!(
            target_ceiling(genesis.header.bits).to_consensus(),
            MINI_ANCHOR_TARGET
        );
    }

    // The Bitcoin Cash genesis shares, pinned. Their hashes name the share
    // chain in every protocol string, so a change to anything they are built
    // from (the regtest genesis entry's key and timestamp, the anchor blocks,
    // the share coinbase or header encoding) would split the network. These
    // values were computed before and after the upstream sync through
    // `61810c56`, which moved the Bitcoin genesis blocks to `.dat` files.

    /// Pinned again on 2026-11-09: the header gained its band (`shares::band`),
    /// which changed every Bitcoin Cash genesis hash in a flag day.
    #[test]
    fn mainnet_mini_genesis_hash_is_pinned() {
        let genesis = build(BchShareNetwork::Mainnet).unwrap();
        assert_eq!(
            genesis.block_hash().to_string(),
            "1b5a447b4685d855498907e637e00d8528fd6a5716ae35c1bfcfa3ad4530d972"
        );
    }

    #[test]
    fn chipnet_mini_genesis_hash_is_pinned() {
        let genesis = build(BchShareNetwork::Chipnet).unwrap();
        assert_eq!(
            genesis.block_hash().to_string(),
            "175f4f3cf68a69323552d41874511160d3d4d001a5475a12e2510022e44bb61d"
        );
    }

    #[test]
    fn regtest_mini_genesis_hash_is_pinned() {
        let genesis = build(BchShareNetwork::Regtest).unwrap();
        assert_eq!(
            genesis.block_hash().to_string(),
            "493b5b78a3e75b2c17f5aedcf4a49dc79eb79550f7447dc41c526ef948616f07"
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
