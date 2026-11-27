// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

mod cashaddr;
pub mod coinbase_tag;

use bitcoin::base58;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::{Keypair, Secp256k1, SecretKey};
use bitcoin::{Address, Network, PubkeyHash};
use bitcoindrpc::BitcoinRpcConfig;
use ipnet::IpNet;
use p2poolv2_wallet::Address as ShareAddress;
use p2poolv2_wallet::bch_share::{self, BchShareAddress, BchShareNetwork};
pub use p2poolv2_wallet::payout_address::PayoutAddress;
use serde::Deserialize;
use std::marker::PhantomData;
use std::net::IpAddr;
use std::str::FromStr;

/// Default version mask for BIP 323 version rolling.
pub const DEFAULT_VERSION_MASK: i32 = 0x1fffe000;

/// Stratum difficulty cap when `maximum_difficulty` is not configured: 2^32.
///
/// A session at this difficulty that submits a share every 3.33 seconds, the
/// difficulty adjuster's target, hashes at about 5.5 EH/s: more than the whole
/// Bitcoin Cash network, and a large farm behind one proxy connection on
/// Bitcoin. It stays far below either network's block difficulty, so a miner
/// never withholds a block because its share target is harder than the
/// block's. Without a cap a miner could ask for `d=18446744073709551615` and
/// drive the adjuster's sums to their limits.
pub const DEFAULT_MAXIMUM_DIFFICULTY: u64 = 1 << 32;

/// One hundred percent, in basis points.
pub const MAX_BASIS_POINTS: u16 = 10_000;

/// P2Poolv2 Bitcoin Cash donation address, as shown on mainnet.
///
/// A Bitcoin Cash node pays its donation here. The address is fixed: the
/// share chain rejects a Bitcoin Cash share whose donation output pays any
/// other script, so an operator who wants a cut of their own uses
/// `fee_address` and `fee` instead.
pub const BITCOIN_CASH_DONATION_ADDRESS: &str =
    "bitcoincash:qze95tc2dqrnltvxfa5yhunwx5952f34fvw4g6365k";

/// The 20-byte public key hash [`BITCOIN_CASH_DONATION_ADDRESS`] encodes.
///
/// The same key is paid on chipnet (`bchtest:`) and regtest (`bchreg:`), so
/// the donation path is exercised on test networks too.
pub const BITCOIN_CASH_DONATION_PUBKEY_HASH: [u8; 20] = [
    0xb2, 0x5a, 0x2f, 0x0a, 0x68, 0x07, 0x3f, 0xad, 0x86, 0x4f, 0x68, 0x4b, 0xf2, 0x6e, 0x35, 0x0b,
    0x45, 0x26, 0x35, 0x4b,
];

/// Donation a Bitcoin Cash node makes when `donation` is not configured:
/// 100 basis points, 1%. An operator may raise it, lower it, or set 0 to opt
/// out. Each miner may still choose their own at login (`donation=`).
pub const DEFAULT_BITCOIN_CASH_DONATION: u16 = 100;

/// Operator fee a Bitcoin Cash node takes when `fee_address` is set and `fee`
/// is not: 100 basis points, 1%, of what the donation leaves. An operator may
/// set `fee` to change it.
pub const DEFAULT_BITCOIN_CASH_FEE: u16 = 100;

/// Most each of the donation and the operator fee may take of a Bitcoin Cash
/// coinbase before the PPLNS split: 1,000 basis points, 10%, each on its
/// own. There is no combined cap. The fee is taken from what the donation
/// leaves, so at most 10% of the coinbase value too, and the two together at
/// most 19%.
///
/// The share chain enforces this on every Bitcoin Cash share. Without it a
/// node could set a 100% fee to itself and keep the whole reward of every
/// block it finds, while its shares still earn a slice of everyone else's.
pub const MAX_BITCOIN_CASH_CUT: u16 = 1_000;

/// The highest payout level `k` a Bitcoin Cash share may carry: level `k`
/// pays slices below `546 * 2^k` satoshis by a draw
/// (`docs/sharechain/small-miners-design.md`, part 4), so level 7 is 69,888
/// satoshis, the cap that keeps large amounts out of any draw. A share-chain
/// rule: a share header above it does not decode.
pub const MAX_PAYOUT_LEVEL: u8 = 7;

/// The lowest payout level `k` a Bitcoin Cash share may carry. Level 0 paid
/// 546 satoshis, which is below dust once the payout tree's fee is taken, so
/// a level 0 win that landed in the tree went to the tree's largest payee.
/// A header below it does not decode, and a `payout_level` below it is
/// refused.
pub const MIN_PAYOUT_LEVEL: u8 = 1;

/// Why `level` is not a Bitcoin Cash payout level: it is below
/// [`MIN_PAYOUT_LEVEL`]. The message names `level`, the lowest level and
/// that level's amount, `546 * 2^k` satoshis.
pub fn payout_level_below_minimum(level: u8) -> String {
    let amount = 546u64 << MIN_PAYOUT_LEVEL;
    format!(
        "payout level {level} is below the lowest payout level \
         {MIN_PAYOUT_LEVEL} ({amount} satoshis)"
    )
}

/// The payout level a Bitcoin Cash node gives a miner whose login chooses
/// none, when `payout_level` is not set: level 5, 17,472 satoshis, about
/// 0.85% of a payment spent at the default `spend_cost`. Node policy.
pub const DEFAULT_PAYOUT_LEVEL: u8 = 5;

/// What spending one payment costs, in satoshis, when `spend_cost` is not
/// set: a P2PKH input of 148 bytes at 1 sat/byte. A miner's
/// `fee-tolerance=` option is a share of a payment this cost may take.
pub const DEFAULT_SPEND_COST: u64 = 148;

/// [`BITCOIN_CASH_DONATION_ADDRESS`] under a network's CashAddr prefix, for
/// logs and documentation. Each string decodes to
/// [`bitcoin_cash_donation_address`] for that network.
pub fn bitcoin_cash_donation_cashaddr(network: BitcoinCashNetwork) -> &'static str {
    match network {
        BitcoinCashNetwork::Mainnet => BITCOIN_CASH_DONATION_ADDRESS,
        BitcoinCashNetwork::Chipnet | BitcoinCashNetwork::Scalenet => {
            "bchtest:qze95tc2dqrnltvxfa5yhunwx5952f34fv28vandn2"
        }
        BitcoinCashNetwork::Regtest => "bchreg:qze95tc2dqrnltvxfa5yhunwx5952f34fvsm6us7sv",
    }
}

/// Donation output for a Bitcoin Cash network: P2PKH to
/// [`BITCOIN_CASH_DONATION_PUBKEY_HASH`].
pub fn bitcoin_cash_donation_address(network: BitcoinCashNetwork) -> PayoutAddress {
    Address::p2pkh(
        PubkeyHash::from_byte_array(BITCOIN_CASH_DONATION_PUBKEY_HASH),
        network.legacy_network(),
    )
    .into()
}

/// Error type for configuration parsing and validation.
#[derive(Debug, Clone)]
pub struct ConfigError {
    pub message: String,
}

impl std::error::Error for ConfigError {}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

/// Parse a payout address for the chain this node is configured to mine.
///
/// Bitcoin keeps the existing address parser. Bitcoin Cash accepts only a
/// CashAddr whose prefix matches `bitcoin_cash_network`: P2PKH, P2SH20 or
/// P2SH32, token-aware or not. Either way the result is the script paid.
pub fn parse_payout_address(
    address: &str,
    chain: Chain,
    network: Network,
    bitcoin_cash_network: Option<BitcoinCashNetwork>,
) -> Result<PayoutAddress, ConfigError> {
    match chain {
        Chain::Bitcoin => parse_address(address, network).map(PayoutAddress::from),
        Chain::BitcoinCash => {
            let bitcoin_cash_network = bitcoin_cash_network.ok_or_else(|| ConfigError {
                message: "bitcoin_cash_network is required when chain is bitcoin-cash".to_string(),
            })?;
            cashaddr::decode(address, bitcoin_cash_network)
                .map(|decoded| decoded.to_payout_address())
                .map_err(|error| ConfigError {
                    message: format!("Invalid Bitcoin Cash payout address: {error}"),
                })
        }
    }
}

/// Parse and validate a bitcoin address string for a given network.
pub fn parse_address(address: &str, network: Network) -> Result<Address, ConfigError> {
    let parsed_address = Address::from_str(address).map_err(|error| ConfigError {
        message: format!("Invalid address: {error}"),
    })?;

    parsed_address
        .require_network(network)
        .map_err(|_| ConfigError {
            message: format!("Address does not match network: {network}"),
        })
}

/// Parse and validate a share chain address string for a given network.
///
/// Kept separate from [`parse_address`] because the two live on different
/// chains: this one owns share coinbase outputs, that one receives bitcoin.
pub fn parse_share_address(address: &str, network: Network) -> Result<ShareAddress, ConfigError> {
    ShareAddress::from_str(address)
        .map_err(|error| ConfigError {
            message: format!("Invalid share chain address: {error}"),
        })?
        .require_network(network)
        .map_err(|error| ConfigError {
            message: format!("Invalid share chain address: {error}"),
        })
}

/// Pool operating mode.
///
/// P2Poolv2 mode runs the full share chain with ASERT difficulty and
/// P2P networking.  Hydrapool mode runs a standalone PPLNS pool where
/// the share chain ASERT difficulty is not enforced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum PoolMode {
    /// Full P2Poolv2 share chain mode (default)
    #[serde(alias = "P2Poolv2", alias = "p2poolv2")]
    #[default]
    P2poolv2,
    /// Standalone PPLNS pool mode
    #[serde(alias = "Hydrapool", alias = "hydrapool")]
    Hydrapool,
}

/// Block chain that supplies work templates and coinbase payout scripts.
///
/// Existing configurations omit this field and stay on Bitcoin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Chain {
    /// Bitcoin. Template requests include SegWit, and Signet when selected.
    #[default]
    Bitcoin,
    /// Bitcoin Cash. Template requests omit Bitcoin-only rules and are
    /// validated before a job is built. Payout addresses are CashAddr.
    BitcoinCash,
}

/// Bitcoin Cash network selected by the operator.
///
/// Chipnet and scalenet use the `bchtest` CashAddr prefix. BCHN uses that
/// same prefix for its other test networks, so the prefix does not
/// distinguish chipnet from scalenet, testnet3 or testnet4. The RPC URL has
/// to point at the network named here; the node checks at start that it does
/// (`node_chain_names`, `bch_only_block`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BitcoinCashNetwork {
    /// Bitcoin Cash mainnet. CashAddr prefix `bitcoincash`.
    #[serde(alias = "main")]
    Mainnet,
    /// Bitcoin Cash chipnet. CashAddr prefix `bchtest`.
    Chipnet,
    /// Bitcoin Cash regtest. CashAddr prefix `bchreg`.
    Regtest,
    /// Bitcoin Cash scalenet, BCHN's network for testing big blocks: a
    /// 256 MB block size floor, reorged back to height 10,000 now and then
    /// (BCHN `doc/test-networks.md`, `CScaleNetParams`). CashAddr prefix
    /// `bchtest`.
    Scalenet,
}

/// The `[stratum]` key that once chose the Bitcoin Cash main or mini share
/// chain. The two merged into one share chain on 2026-11-09, which keeps the
/// mini chain's anchor, ceiling and genesis, so [`Config::load`] refuses a
/// config that still sets it.
pub const REMOVED_BCH_SHARE_CHAIN_KEY: &str = "bch_share_chain";

impl BitcoinCashNetwork {
    /// The network's name as `bitcoin_cash_network` spells it: "mainnet",
    /// "chipnet", "regtest" or "scalenet".
    pub fn name(self) -> &'static str {
        match self {
            BitcoinCashNetwork::Mainnet => "mainnet",
            BitcoinCashNetwork::Chipnet => "chipnet",
            BitcoinCashNetwork::Regtest => "regtest",
            BitcoinCashNetwork::Scalenet => "scalenet",
        }
    }

    /// Human-readable prefix covered by the CashAddr checksum.
    pub fn cashaddr_prefix(self) -> &'static str {
        match self {
            BitcoinCashNetwork::Mainnet => "bitcoincash",
            BitcoinCashNetwork::Chipnet | BitcoinCashNetwork::Scalenet => "bchtest",
            BitcoinCashNetwork::Regtest => "bchreg",
        }
    }

    /// Share address network (`p2pool`, `tp2pool`, `rp2pool`, `sp2pool`) for
    /// this Bitcoin Cash network.
    pub fn share_network(self) -> BchShareNetwork {
        match self {
            BitcoinCashNetwork::Mainnet => BchShareNetwork::Mainnet,
            BitcoinCashNetwork::Chipnet => BchShareNetwork::Chipnet,
            BitcoinCashNetwork::Regtest => BchShareNetwork::Regtest,
            BitcoinCashNetwork::Scalenet => BchShareNetwork::Scalenet,
        }
    }

    /// The Bitcoin Cash network a share address network belongs to.
    ///
    /// Each share address prefix (`p2pool`, `tp2pool`, `rp2pool`, `sp2pool`)
    /// names one network. Payout addresses use BCHN's prefixes, where chipnet
    /// and scalenet share `bchtest`.
    pub fn from_share_network(share_network: BchShareNetwork) -> Self {
        match share_network {
            BchShareNetwork::Mainnet => BitcoinCashNetwork::Mainnet,
            BchShareNetwork::Chipnet => BitcoinCashNetwork::Chipnet,
            BchShareNetwork::Regtest => BitcoinCashNetwork::Regtest,
            BchShareNetwork::Scalenet => BitcoinCashNetwork::Scalenet,
        }
    }

    /// Bitcoin network whose base58 version bytes and proof-of-work limit
    /// match this Bitcoin Cash network: mainnet `0x1d00ffff`, chipnet and
    /// scalenet `0x1d00ffff` (as testnet), regtest `0x207fffff`. It is the
    /// network class a decoded CashAddr payout is held under, and it is the
    /// `stratum.network` a Bitcoin Cash node uses for difficulty and its P2P
    /// protocol id.
    pub fn legacy_network(self) -> Network {
        match self {
            BitcoinCashNetwork::Mainnet => Network::Bitcoin,
            BitcoinCashNetwork::Chipnet | BitcoinCashNetwork::Scalenet => Network::Testnet,
            BitcoinCashNetwork::Regtest => Network::Regtest,
        }
    }

    /// The `chain` names Bitcoin Cash nodes' `getblockchaininfo` reports on
    /// this network: BCHN's first (`CBaseChainParams` in its
    /// `src/chainparamsbase.cpp`), then BCHD's (`Name` in its
    /// `chaincfg/params.go`). BCHD has no scalenet.
    pub fn node_chain_names(self) -> &'static [&'static str] {
        match self {
            BitcoinCashNetwork::Mainnet => &["main", "mainnet"],
            BitcoinCashNetwork::Chipnet => &["chip", "chipnet"],
            BitcoinCashNetwork::Regtest => &["regtest"],
            BitcoinCashNetwork::Scalenet => &["scale"],
        }
    }

    /// A block only Bitcoin Cash on this network has, as (height, hash): a
    /// checkpoint from BCHN's `src/chainparams.cpp`. A node whose block at
    /// that height differs follows another chain. `None` on regtest, whose
    /// blocks are local.
    ///
    /// - Mainnet: block 661,648, the first block after the November 2020
    ///   upgrade, where eCash (then Bitcoin ABC) split off. It is also after
    ///   the November 2018 split of Bitcoin SV, and Bitcoin never had it.
    /// - Chipnet: block 115,252, where chipnet forked from testnet4.
    /// - Scalenet: block 10,000, the height scalenet is reorged back to, so
    ///   it stays in the chain.
    pub fn bch_only_block(self) -> Option<(u64, &'static str)> {
        match self {
            BitcoinCashNetwork::Mainnet => Some((
                661_648,
                "0000000000000000029e471c41818d24b8b74c911071c4ef0b4a0509f9b5a8ce",
            )),
            BitcoinCashNetwork::Chipnet => Some((
                115_252,
                "00000000040ba9641ba98a37b2e5ceead38e4e2930ac8f145c8094f94c708727",
            )),
            BitcoinCashNetwork::Scalenet => Some((
                10_000,
                "00000000b711dc753130e5083888d106f99b920b1b8a492eb5ac41d40e482905",
            )),
            BitcoinCashNetwork::Regtest => None,
        }
    }
}

/// Marker type for raw (unparsed) StratumConfig state
#[derive(Debug, Clone, Default)]
pub struct Raw;

/// Marker type for parsed (validated) StratumConfig state
#[derive(Debug, Clone)]
pub struct Parsed;

#[derive(Debug, Clone, Deserialize)]
#[serde(bound(deserialize = "State: Default"))]
pub struct StratumConfig<State = Raw> {
    /// The hostname for the Stratum server
    pub hostname: String,
    /// The port for the Stratum server
    pub port: u16,
    /// The start difficulty for all miners that connect to the server
    pub start_difficulty: u64,
    /// The minimum difficulty for the pool
    pub minimum_difficulty: u64,
    /// The maximum difficulty for the pool. Parsing fills an unset value with
    /// [`DEFAULT_MAXIMUM_DIFFICULTY`], so a parsed config always has one.
    pub maximum_difficulty: Option<u64>,
    /// The address for solo mining payouts
    pub solo_address: Option<String>,
    /// The ZMQ publisher address for block hashes
    pub zmqpubhashblock: String,
    /// `HOST:PORT` of the node's P2P port. When set, the node's new-block
    /// announcements there wake the template task too, for nodes without ZMQ.
    /// Absent means no P2P connection to the node, as before.
    #[serde(default)]
    pub node_p2p_address: Option<String>,
    /// The bitcoin address to use for first jobs when there are no shares (string in Raw state)
    pub bootstrap_address: String,
    /// The donation address for developers (string in Raw state).
    ///
    /// On Bitcoin it is required whenever `donation` is set. On Bitcoin Cash
    /// it may be omitted; if given it must be [`BITCOIN_CASH_DONATION_ADDRESS`]
    /// under the prefix for `bitcoin_cash_network`.
    pub donation_address: Option<String>,
    /// The donation basis points.
    ///
    /// In the Raw state this is what the operator wrote. After parsing it is
    /// the node's donation: on Bitcoin Cash an unset value becomes
    /// [`DEFAULT_BITCOIN_CASH_DONATION`], and 0 becomes `None` (no donation).
    /// On a Bitcoin Cash P2Pool node it is the default for miners whose login
    /// does not choose their own `donation=`, never an override of one.
    pub donation: Option<u16>,
    /// The fee address (string in Raw state). A pool operator's own cut.
    pub fee_address: Option<String>,
    /// The fee basis points, taken after the donation.
    ///
    /// On Bitcoin Cash an unset value with `fee_address` set becomes
    /// [`DEFAULT_BITCOIN_CASH_FEE`] after parsing. Bitcoin needs both fields
    /// or neither.
    pub fee: Option<u16>,
    /// Share chain address owning every share this pool mines (string in Raw
    /// state). When set, miners need not send `p2p=` in the stratum password,
    /// and one that disagrees with this address is rejected at authorize.
    /// P2Poolv2 mode only; Hydrapool builds no share commitment.
    pub miner_address: Option<String>,
    /// The network can be "main", "testnet4" or "signet".
    /// On Bitcoin this is also the share-address prefix, and [`Config::load`]
    /// requires it. On Bitcoin Cash it is ignored and may be left out:
    /// [`Config::load`] and [`StratumConfig::parse`] replace it with
    /// [`StratumConfig::effective_network`], derived from
    /// `bitcoin_cash_network`.
    #[serde(
        default = "default_unset_network",
        deserialize_with = "deserialize_network"
    )]
    pub network: bitcoin::Network,
    /// Block chain that supplies work templates and payout scripts.
    /// Defaults to Bitcoin so existing configurations keep their behavior.
    #[serde(default)]
    pub chain: Chain,
    /// Required when `chain` is `bitcoin-cash`. Accepted values are
    /// `mainnet` (alias `main`), `chipnet`, and `regtest`.
    #[serde(default)]
    pub bitcoin_cash_network: Option<BitcoinCashNetwork>,
    /// Stratum V2 listen port. Absent means the node does not open an SV2
    /// socket, which is what every existing configuration does.
    #[serde(default)]
    pub sv2_port: Option<u16>,
    /// Stratum V2 authority public key, in the Stratum V2 reference
    /// implementation's text form (Base58Check of version `[1, 0]` and the
    /// 32-byte x-only key). With `sv2_authority_secret_key`, every SV2
    /// connection must complete the Noise NX handshake before any message.
    /// The keys come from an external tool; this node never generates one.
    #[serde(default)]
    pub sv2_authority_public_key: Option<String>,
    /// Stratum V2 authority secret key (Base58Check of the 32 bytes). Prefer
    /// `P2POOL_STRATUM__SV2_AUTHORITY_SECRET_KEY` over the file.
    #[serde(default)]
    pub sv2_authority_secret_key: Option<String>,
    /// Seconds a Noise certificate signed by the authority key stays valid.
    /// Defaults to [`DEFAULT_SV2_CERT_VALIDITY_SECS`].
    #[serde(default)]
    pub sv2_cert_validity_secs: Option<u64>,
    /// Client addresses a Stratum V2 Job Declaration connection may use
    /// Coinbase-Only mode from, as IP addresses or CIDR ranges. In that mode
    /// the node credits shares on a template it never sees, so by default
    /// only this computer may use it (`127.0.0.0/8` and `::1`). Full-Template
    /// declarations, whose templates the node checks, are taken from
    /// anywhere. `[]` refuses Coinbase-Only from every client;
    /// `["0.0.0.0/0", "::/0"]` takes it from every client. A refused client
    /// gets `SetupConnection.Error` `unsupported-feature-flags` naming the
    /// `DECLARE_TX_DATA` flag it must set.
    #[serde(default = "default_sv2_coinbase_only_from")]
    pub sv2_coinbase_only_from: Vec<String>,
    /// Where block templates come from. `rpc`, the default and what every
    /// existing configuration uses, asks the node in `[bitcoinrpc]`.
    /// `rpc_light` asks the same node for light templates, which leave the
    /// transactions in the node, and for full ones when it has none. `sv2`
    /// takes them from a Stratum V2 Template Provider at
    /// `template_provider_address` and sends found blocks back to it.
    #[serde(default)]
    pub template_source: TemplateSource,
    /// `HOST:PORT` of the Stratum V2 Template Provider. Needs
    /// `template_source = "sv2"`.
    #[serde(default)]
    pub template_provider_address: Option<String>,
    /// The Template Provider's authority public key, in the text form of
    /// `sv2_authority_public_key`. With it the connection runs the Noise NX
    /// handshake and accepts only a certificate this key signed. Required
    /// unless the provider listens on this computer (a loopback address).
    #[serde(default)]
    pub template_provider_authority_public_key: Option<String>,
    /// `HOST:PORT` of a SOCKS5 proxy the Template Provider is reached
    /// through: Tor's SOCKS port for a `.onion` address, an I2P router's
    /// SOCKS proxy for a `.i2p` address. The provider's name goes to the
    /// proxy and is never resolved on this computer. Needs
    /// `template_provider_authority_public_key`.
    #[serde(default)]
    pub template_provider_proxy: Option<String>,
    /// The version mask to use for version-rolling
    #[serde(deserialize_with = "deserialize_version_mask")]
    pub version_mask: i32,
    /// Test config to ignore difficulty
    pub ignore_difficulty: Option<bool>,
    /// The operator line of the coinbase tag (`docs/stratum.md`, "The
    /// coinbase tag"), at most [`coinbase_tag::MAX_OPERATOR_LENGTH`]
    /// characters under the coinbase name rules. Unset, empty or
    /// `P2Poolv2`, the old default, leaves the operator line empty.
    pub pool_signature: Option<String>,
    /// Written after a space at the end of the node line of the coinbase tag,
    /// after the node software and version the node reports. At most
    /// [`coinbase_tag::MAX_NODE_LABEL_LENGTH`] characters under the coinbase
    /// name rules.
    #[serde(default)]
    pub node_label: Option<String>,
    /// Maximum concurrent stratum connections. Default: 90% of OS fd limit.
    pub max_connections: Option<u32>,
    /// Wait for the share chain to be current before accepting stratum
    /// connections. Set to false when bootstrapping a new network.
    #[serde(default = "default_wait_for_chain_sync")]
    pub wait_for_chain_sync: bool,
    /// Pool operating mode. Defaults to P2Poolv2 when not specified.
    #[serde(default)]
    pub mode: PoolMode,
    /// A JSON file listing the tokens a Bitcoin Cash P2Pool node
    /// merge-mines for its miners (`docs/bch-auxpow.md`, "Merge-mining
    /// tokens on a public node"). Every miner's jobs then carry a root of
    /// their own token leaves at output 0, with their P2Pool donation and
    /// the node's fee as per-win cuts. The node reads it again whenever it
    /// changes, so the tool that claims a token can move its state. Unset,
    /// the node merge-mines nothing. Bitcoin Cash in P2Pool mode only.
    #[serde(default)]
    pub merge_mining_tokens: Option<String>,
    /// The payout level, [`MIN_PAYOUT_LEVEL`] to [`MAX_PAYOUT_LEVEL`], a
    /// Bitcoin Cash node gives a miner whose login sets neither
    /// `min-payout=` nor `fee-tolerance=`. Parsing fills an unset value with
    /// [`DEFAULT_PAYOUT_LEVEL`] and refuses one below the lowest or above
    /// the maximum.
    #[serde(default)]
    pub payout_level: Option<u8>,
    /// What spending one payment costs, in satoshis, for a miner's
    /// `fee-tolerance=` option and the node's level policy. Parsing fills an
    /// unset value with [`DEFAULT_SPEND_COST`].
    #[serde(default)]
    pub spend_cost: Option<u64>,

    // Parsed addresses - only available when State = Parsed
    #[serde(skip)]
    pub bootstrap_address_parsed: Option<PayoutAddress>,
    #[serde(skip)]
    pub donation_address_parsed: Option<PayoutAddress>,
    #[serde(skip)]
    pub fee_address_parsed: Option<PayoutAddress>,
    #[serde(skip)]
    pub miner_address_parsed: Option<ShareAddress>,
    /// Bitcoin Cash share owner, set only when `chain` is bitcoin-cash.
    #[serde(skip)]
    pub bch_miner_address: Option<BchShareAddress>,
    /// Decoded Stratum V2 authority keys, when both are configured.
    #[serde(skip)]
    pub sv2_authority: Option<Sv2Authority>,
    /// The checked Template Provider, set when `template_source` is `sv2`.
    #[serde(skip)]
    pub template_provider: Option<TemplateProvider>,
    /// `sv2_coinbase_only_from`, parsed.
    #[serde(skip)]
    pub sv2_coinbase_only_networks: Vec<IpNet>,

    #[serde(skip)]
    #[serde(default)]
    _state: PhantomData<State>,
}

impl<State> StratumConfig<State> {
    /// The network difficulty, P2P protocol id, and payout parsing use.
    ///
    /// Bitcoin uses `network` as configured. Bitcoin Cash uses the network
    /// implied by `bitcoin_cash_network`, whatever `network` says, so every
    /// node on one Bitcoin Cash network computes the same difficulty and
    /// speaks the same protocol id. Without `bitcoin_cash_network`, `network`
    /// is returned and parsing rejects the configuration.
    pub fn effective_network(&self) -> Network {
        match (self.chain, self.bitcoin_cash_network) {
            (Chain::BitcoinCash, Some(bitcoin_cash_network)) => {
                bitcoin_cash_network.legacy_network()
            }
            (Chain::Bitcoin, _) | (Chain::BitcoinCash, None) => self.network,
        }
    }
}

impl StratumConfig<Raw> {
    /// Parse and validate addresses, converting from Raw to Parsed state.
    ///
    /// On Bitcoin Cash the parsed `network` is [`Self::effective_network`].
    pub fn parse(mut self) -> Result<StratumConfig<Parsed>, ConfigError> {
        self.network = self.effective_network();
        if let Some(pool_signature) = &self.pool_signature {
            coinbase_tag::check_coinbase_name(pool_signature, coinbase_tag::MAX_OPERATOR_LENGTH)
                .map_err(|reason| ConfigError {
                    message: format!("pool_signature {reason}"),
                })?;
        }
        if let Some(node_label) = &self.node_label {
            coinbase_tag::check_coinbase_name(node_label, coinbase_tag::MAX_NODE_LABEL_LENGTH)
                .map_err(|reason| ConfigError {
                    message: format!("node_label {reason}"),
                })?;
        }

        // Bitcoin Cash has a built-in donation address, so `donation` alone is
        // enough there. Bitcoin has none.
        if self.chain == Chain::Bitcoin
            && self.donation_address.is_some() != self.donation.is_some()
        {
            return Err(ConfigError {
                message: "donation_address and donation must both be set or both be unset"
                    .to_string(),
            });
        }

        // A Bitcoin Cash fee_address alone takes the default fee; a fee with
        // nowhere to pay it is an error on both chains.
        let fee_needs_address = self.fee.is_some() && self.fee_address.is_none();
        let address_needs_fee =
            self.chain == Chain::Bitcoin && self.fee_address.is_some() && self.fee.is_none();
        if fee_needs_address || address_needs_fee {
            return Err(ConfigError {
                message: "fee_address and fee must both be set or both be unset".to_string(),
            });
        }

        // Hydrapool never builds a share commitment, so a share chain address
        // could not be used there. Fail at startup rather than let an operator
        // believe their shares are being assigned to it.
        if self.miner_address.is_some() && self.mode == PoolMode::Hydrapool {
            return Err(ConfigError {
                message:
                    "miner_address cannot be set when mode is hydrapool, which has no share chain"
                        .to_string(),
            });
        }

        match (self.chain, self.bitcoin_cash_network) {
            (Chain::Bitcoin, Some(_)) => {
                return Err(ConfigError {
                    message: "bitcoin_cash_network is only valid when chain is bitcoin-cash"
                        .to_string(),
                });
            }
            (Chain::BitcoinCash, None) => {
                return Err(ConfigError {
                    message: "bitcoin_cash_network is required when chain is bitcoin-cash"
                        .to_string(),
                });
            }
            (Chain::Bitcoin, None) | (Chain::BitcoinCash, Some(_)) => {}
        }

        // Tokens are merge-mined through the Bitcoin Cash coinbase P2Pool
        // builds for each miner; Bitcoin and Hydrapool build none.
        if self.merge_mining_tokens.is_some()
            && (self.chain != Chain::BitcoinCash || self.mode != PoolMode::P2poolv2)
        {
            return Err(ConfigError {
                message: "merge_mining_tokens needs chain = bitcoin-cash in p2poolv2 mode"
                    .to_string(),
            });
        }

        if self.chain == Chain::BitcoinCash
            && let Some(solo_address) = &self.solo_address
        {
            parse_payout_address(
                solo_address,
                self.chain,
                self.network,
                self.bitcoin_cash_network,
            )?;
        }

        // Bitcoin share addresses use this network's bech32m prefix. Bitcoin
        // Cash share addresses use bitcoin_cash_network, so that chain does
        // not have to name a Bitcoin share prefix. Genesis still reads network.
        if self.mode == PoolMode::P2poolv2 && self.chain == Chain::Bitcoin {
            p2poolv2_wallet::expected_hrp(self.network).map_err(|error| ConfigError {
                message: format!(
                    "Network {} cannot be used for a share chain: {error}",
                    self.network
                ),
            })?;
        }

        let bootstrap_address_parsed = parse_payout_address(
            &self.bootstrap_address,
            self.chain,
            self.network,
            self.bitcoin_cash_network,
        )?;

        let (miner_address_parsed, bch_miner_address) = match self.chain {
            Chain::Bitcoin => (
                self.miner_address
                    .as_ref()
                    .map(|address| parse_share_address(address, self.network))
                    .transpose()?,
                None,
            ),
            Chain::BitcoinCash => {
                let bitcoin_cash_network =
                    self.bitcoin_cash_network.ok_or_else(|| ConfigError {
                        message: "bitcoin_cash_network is required when chain is bitcoin-cash"
                            .to_string(),
                    })?;
                let parsed = self
                    .miner_address
                    .as_ref()
                    .map(|address| bch_share::parse(address, bitcoin_cash_network.share_network()))
                    .transpose()
                    .map_err(|error| ConfigError {
                        message: format!("Invalid Bitcoin Cash share address: {error}"),
                    })?;
                (None, parsed)
            }
        };

        let (donation_address_parsed, donation) = resolve_donation(
            self.chain,
            self.network,
            self.bitcoin_cash_network,
            self.donation_address.as_deref(),
            self.donation,
        )?;

        let fee_address_parsed = self
            .fee_address
            .as_ref()
            .map(|addr| {
                parse_payout_address(addr, self.chain, self.network, self.bitcoin_cash_network)
            })
            .transpose()?;

        let fee = match (self.chain, &self.fee_address, self.fee) {
            (Chain::BitcoinCash, Some(_), None) => Some(DEFAULT_BITCOIN_CASH_FEE),
            (_, _, fee) => fee,
        };

        validate_cuts(self.chain, donation, fee)?;
        validate_light_templates(self.chain, self.template_source)?;
        let payout_level = self.payout_level.unwrap_or(DEFAULT_PAYOUT_LEVEL);
        if payout_level < MIN_PAYOUT_LEVEL {
            return Err(ConfigError {
                message: payout_level_below_minimum(payout_level),
            });
        }
        if payout_level > MAX_PAYOUT_LEVEL {
            return Err(ConfigError {
                message: format!(
                    "payout_level {payout_level} is above the highest payout level, \
                     {MAX_PAYOUT_LEVEL}"
                ),
            });
        }

        let sv2_authority = parse_sv2_authority(
            self.sv2_port,
            self.sv2_authority_public_key.as_deref(),
            self.sv2_authority_secret_key.as_deref(),
            self.sv2_cert_validity_secs,
        )?;
        let template_provider = parse_template_provider(
            self.chain,
            self.bitcoin_cash_network,
            self.template_source,
            self.template_provider_address.as_deref(),
            self.template_provider_authority_public_key.as_deref(),
            self.template_provider_proxy.as_deref(),
        )?;
        let sv2_coinbase_only_networks = parse_client_networks(&self.sv2_coinbase_only_from)?;

        Ok(StratumConfig {
            hostname: self.hostname,
            port: self.port,
            start_difficulty: self.start_difficulty,
            minimum_difficulty: self.minimum_difficulty,
            maximum_difficulty: Some(
                self.maximum_difficulty
                    .unwrap_or(DEFAULT_MAXIMUM_DIFFICULTY),
            ),
            solo_address: self.solo_address,
            zmqpubhashblock: self.zmqpubhashblock,
            node_p2p_address: self.node_p2p_address,
            bootstrap_address: self.bootstrap_address,
            donation_address: self.donation_address,
            donation,
            fee_address: self.fee_address,
            fee,
            miner_address: self.miner_address,
            network: self.network,
            chain: self.chain,
            bitcoin_cash_network: self.bitcoin_cash_network,
            sv2_port: self.sv2_port,
            sv2_authority_public_key: self.sv2_authority_public_key,
            sv2_authority_secret_key: self.sv2_authority_secret_key,
            sv2_cert_validity_secs: self.sv2_cert_validity_secs,
            sv2_coinbase_only_from: self.sv2_coinbase_only_from,
            template_source: self.template_source,
            template_provider_address: self.template_provider_address,
            template_provider_authority_public_key: self.template_provider_authority_public_key,
            template_provider_proxy: self.template_provider_proxy,
            version_mask: self.version_mask,
            ignore_difficulty: self.ignore_difficulty,
            pool_signature: self.pool_signature,
            node_label: self.node_label,
            max_connections: self.max_connections,
            wait_for_chain_sync: self.wait_for_chain_sync,
            mode: self.mode,
            merge_mining_tokens: self.merge_mining_tokens,
            payout_level: Some(payout_level),
            spend_cost: Some(self.spend_cost.unwrap_or(DEFAULT_SPEND_COST)),
            bootstrap_address_parsed: Some(bootstrap_address_parsed),
            donation_address_parsed,
            fee_address_parsed,
            miner_address_parsed,
            bch_miner_address,
            sv2_authority,
            template_provider,
            sv2_coinbase_only_networks,
            _state: PhantomData,
        })
    }
}

impl StratumConfig<Parsed> {
    /// Get the parsed and validated bootstrap address
    pub fn bootstrap_address(&self) -> &PayoutAddress {
        self.bootstrap_address_parsed
            .as_ref()
            .expect("bootstrap_address_parsed should always be Some in Parsed state")
    }

    /// Get the parsed and validated donation address, if any
    pub fn donation_address(&self) -> Option<&PayoutAddress> {
        self.donation_address_parsed.as_ref()
    }

    /// Get the parsed and validated fee address, if any
    pub fn fee_address(&self) -> Option<&PayoutAddress> {
        self.fee_address_parsed.as_ref()
    }

    /// Get the pool wide share chain address, if the operator configured one.
    ///
    /// When present every share this pool mines is owned by this address, and a
    /// miner supplying a different one in `p2p=` is rejected at authorize.
    pub fn miner_address(&self) -> Option<ShareAddress> {
        self.miner_address_parsed
    }

    /// Bitcoin Cash share address configured for this pool, if any.
    pub fn bch_miner_address(&self) -> Option<BchShareAddress> {
        self.bch_miner_address
    }

    /// The operator line of the coinbase tag, from `pool_signature`: empty
    /// when it is unset, empty or the old default `P2Poolv2`.
    pub fn coinbase_operator(&self) -> &str {
        coinbase_tag::operator_line(self.pool_signature.as_deref())
    }

    /// The label at the end of the node line of the coinbase tag, from
    /// `node_label`: empty when it is unset.
    pub fn coinbase_node_label(&self) -> &str {
        self.node_label.as_deref().unwrap_or_default()
    }

    /// The payout level of a miner whose login chooses none
    /// ([`DEFAULT_PAYOUT_LEVEL`] unless `payout_level` is set).
    pub fn payout_level(&self) -> u8 {
        self.payout_level.unwrap_or(DEFAULT_PAYOUT_LEVEL)
    }

    /// What spending one payment costs, in satoshis
    /// ([`DEFAULT_SPEND_COST`] unless `spend_cost` is set).
    pub fn spend_cost(&self) -> u64 {
        self.spend_cost.unwrap_or(DEFAULT_SPEND_COST)
    }

    /// Stratum V2 authority keys, when the operator configured them.
    pub fn sv2_authority(&self) -> Option<Sv2Authority> {
        self.sv2_authority
    }

    /// The Stratum V2 Template Provider templates come from, when
    /// `template_source` is `sv2`. `None` means templates come over RPC.
    pub fn template_provider(&self) -> Option<&TemplateProvider> {
        self.template_provider.as_ref()
    }
}

/// Where the node gets its block templates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TemplateSource {
    /// `getblocktemplate` from the node in `[bitcoinrpc]`.
    #[default]
    Rpc,
    /// `getblocktemplatelight` from the node in `[bitcoinrpc]`, and
    /// `submitblocklight` for a block found on one; `getblocktemplate` for
    /// any template the node cannot give light. Bitcoin Cash only.
    #[serde(rename = "rpc_light")]
    RpcLight,
    /// A Stratum V2 Template Provider, over the Template Distribution
    /// Protocol.
    Sv2,
}

/// A checked Stratum V2 Template Provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateProvider {
    /// `HOST:PORT` to connect to.
    pub address: String,
    /// The provider's 32-byte x-only authority key, pinned for the Noise
    /// handshake. `None` only for a provider on a loopback address, which
    /// is then spoken to without encryption.
    pub authority_public_key: Option<[u8; 32]>,
    /// The Bitcoin Cash network its templates must be for.
    pub network: BitcoinCashNetwork,
    /// `HOST:PORT` of the SOCKS5 proxy every connection to the provider is
    /// opened through, a redirected one included. `None` connects directly.
    pub proxy: Option<String>,
}

impl TemplateProvider {
    /// The provider a `Reconnect` from this one names: `host` and `port`,
    /// or this provider's own when `host` is empty or `port` is 0.
    ///
    /// The pinned key, the network and the proxy carry over, so a provider
    /// spoken to without encryption may only redirect to this computer, and
    /// one reached through a proxy never redirects the node to a direct
    /// connection. Without a proxy, a Tor or I2P name cannot be reached.
    pub fn redirected(&self, host: &str, port: u16) -> Result<TemplateProvider, ConfigError> {
        let (current_host, current_port) = self
            .address
            .rsplit_once(':')
            .and_then(|(host, port)| Some((host, port.parse::<u16>().ok()?)))
            .ok_or_else(|| ConfigError {
                message: format!(
                    "template provider address {} is not HOST:PORT",
                    self.address
                ),
            })?;
        let host = if host.is_empty() { current_host } else { host };
        let port = if port == 0 { current_port } else { port };
        if self.authority_public_key.is_none() && !is_loopback_host(host) {
            return Err(ConfigError {
                message: format!(
                    "an unencrypted Template Provider may not redirect to another computer ({host})"
                ),
            });
        }
        if self.proxy.is_none() && is_overlay_host(host) {
            return Err(ConfigError {
                message: format!(
                    "a Template Provider reached without a proxy may not redirect to {host}"
                ),
            });
        }
        let address = if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        };
        Ok(TemplateProvider {
            address,
            authority_public_key: self.authority_public_key,
            network: self.network,
            proxy: self.proxy.clone(),
        })
    }
}

/// Check that `template_source = "rpc_light"` is on Bitcoin Cash, whose
/// nodes are the ones with light templates.
fn validate_light_templates(chain: Chain, source: TemplateSource) -> Result<(), ConfigError> {
    if source == TemplateSource::RpcLight && chain != Chain::BitcoinCash {
        return Err(ConfigError {
            message:
                "template_source = \"rpc_light\" is supported only with chain = \"bitcoin-cash\""
                    .to_string(),
        });
    }
    Ok(())
}

/// Check the Template Provider settings against `template_source`.
///
/// `rpc` and `rpc_light` take no provider settings, so a provider address
/// left in a configuration cannot be silently ignored. `sv2` needs an
/// address, works only on Bitcoin Cash (a Bitcoin provider's required segwit
/// commitment output is not mapped), and needs the provider's key unless it
/// is on this computer: the specification allows unencrypted frames only
/// between local roles, and an unauthenticated provider could keep every
/// found block.
///
/// `proxy` is a SOCKS5 proxy the provider is reached through. It always
/// needs the provider's key: the proxy, and a Tor exit behind it, carry the
/// connection. A `.onion` or `.i2p` address needs a proxy, since nothing
/// else can reach it and the name must not go to this computer's resolver.
fn parse_template_provider(
    chain: Chain,
    bitcoin_cash_network: Option<BitcoinCashNetwork>,
    source: TemplateSource,
    address: Option<&str>,
    authority_public_key: Option<&str>,
    proxy: Option<&str>,
) -> Result<Option<TemplateProvider>, ConfigError> {
    if source != TemplateSource::Sv2 {
        if address.is_some() || authority_public_key.is_some() || proxy.is_some() {
            return Err(ConfigError {
                message: "template_provider_address, template_provider_authority_public_key and template_provider_proxy need template_source = \"sv2\"".to_string(),
            });
        }
        return Ok(None);
    }
    let network = match (chain, bitcoin_cash_network) {
        (Chain::BitcoinCash, Some(network)) => network,
        _ => {
            return Err(ConfigError {
                message:
                    "template_source = \"sv2\" is supported only with chain = \"bitcoin-cash\""
                        .to_string(),
            });
        }
    };
    let address = address.ok_or_else(|| ConfigError {
        message: "template_source = \"sv2\" needs template_provider_address".to_string(),
    })?;
    let host = address
        .rsplit_once(':')
        .filter(|(host, port)| !host.is_empty() && port.parse::<u16>().is_ok())
        .map(|(host, _)| host)
        .ok_or_else(|| ConfigError {
            message: format!("template_provider_address must be HOST:PORT, got {address}"),
        })?;
    let authority_public_key = authority_public_key
        .map(|text| decode_sv2_public_key(text, "template_provider_authority_public_key"))
        .transpose()?;
    if authority_public_key.is_none() && !is_loopback_host(host) {
        return Err(ConfigError {
            message: format!(
                "template_provider_authority_public_key is required for a Template Provider on another computer ({address})"
            ),
        });
    }
    match proxy {
        Some(proxy) => {
            if !is_host_port(proxy) {
                return Err(ConfigError {
                    message: format!("template_provider_proxy must be HOST:PORT, got {proxy}"),
                });
            }
            if authority_public_key.is_none() {
                return Err(ConfigError {
                    message: format!(
                        "template_provider_authority_public_key is required for a Template Provider reached through a proxy ({proxy})"
                    ),
                });
            }
        }
        None if is_overlay_host(host) => {
            return Err(ConfigError {
                message: format!(
                    "template_provider_proxy is required for a Tor or I2P Template Provider ({address})"
                ),
            });
        }
        None => (),
    }
    Ok(Some(TemplateProvider {
        address: address.to_string(),
        authority_public_key,
        network,
        proxy: proxy.map(str::to_string),
    }))
}

/// Whether `address` is `HOST:PORT` with a host and a port number.
fn is_host_port(address: &str) -> bool {
    address
        .rsplit_once(':')
        .is_some_and(|(host, port)| !host.is_empty() && port.parse::<u16>().is_ok())
}

/// Whether `host` is a Tor or I2P name, which only a proxy of that network
/// can reach.
fn is_overlay_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host.ends_with(".onion") || host.ends_with(".i2p")
}

/// Whether `host` names this computer: `localhost` or a loopback IP,
/// bracketed or not.
fn is_loopback_host(host: &str) -> bool {
    host == "localhost"
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Decode a Stratum V2 public key in the reference implementation's text
/// form: Base58Check of version `[1, 0]` and the 32-byte x-only key.
/// `field` names the setting in the error.
fn decode_sv2_public_key(text: &str, field: &str) -> Result<[u8; 32], ConfigError> {
    let bytes = base58::decode_check(text).map_err(|error| ConfigError {
        message: format!("Invalid {field}: {error}"),
    })?;
    match bytes.split_first_chunk::<2>() {
        Some((version, key)) if *version == SV2_PUBLIC_KEY_VERSION => {
            key.try_into().map_err(|_| ConfigError {
                message: format!("Invalid {field}: expected version 1 and a 32-byte key"),
            })
        }
        _ => Err(ConfigError {
            message: format!("Invalid {field}: expected version 1 and a 32-byte key"),
        }),
    }
}

/// Seconds a Noise certificate signed by the Stratum V2 authority key stays
/// valid when `sv2_cert_validity_secs` is not set.
pub const DEFAULT_SV2_CERT_VALIDITY_SECS: u64 = 3600;

/// Version prefix of a Stratum V2 authority public key in the reference
/// implementation's text form: little-endian `1`.
const SV2_PUBLIC_KEY_VERSION: [u8; 2] = [1, 0];

/// Stratum V2 authority keys that sign the Noise certificate.
///
/// Debug output never shows the secret.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Sv2Authority {
    /// 32-byte x-only public key miners pin.
    pub public_key: [u8; 32],
    /// 32-byte secret key.
    pub secret_key: [u8; 32],
    /// Seconds each Noise certificate stays valid.
    pub cert_validity_secs: u64,
}

impl std::fmt::Debug for Sv2Authority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Sv2Authority")
            .field("public_key", &self.public_key)
            .field("secret_key", &"<redacted>")
            .field("cert_validity_secs", &self.cert_validity_secs)
            .finish()
    }
}

/// Decode the configured Stratum V2 authority keys, if any.
///
/// Both keys or neither; they need `sv2_port`; and the public key must be
/// the one the secret key derives, so a typo cannot make every miner's
/// handshake fail against a key the operator did not mean to publish.
fn parse_sv2_authority(
    sv2_port: Option<u16>,
    public_key: Option<&str>,
    secret_key: Option<&str>,
    cert_validity_secs: Option<u64>,
) -> Result<Option<Sv2Authority>, ConfigError> {
    let (public_text, secret_text) = match (public_key, secret_key) {
        (None, None) => return Ok(None),
        (Some(public_text), Some(secret_text)) => (public_text, secret_text),
        _ => {
            return Err(ConfigError {
                message: "sv2_authority_public_key and sv2_authority_secret_key must both be set or both be unset".to_string(),
            });
        }
    };
    if sv2_port.is_none() {
        return Err(ConfigError {
            message: "sv2_authority keys need sv2_port".to_string(),
        });
    }
    let public_key = decode_sv2_public_key(public_text, "sv2_authority_public_key")?;
    let secret_bytes = base58::decode_check(secret_text).map_err(|_| ConfigError {
        message: "Invalid sv2_authority_secret_key: not Base58Check".to_string(),
    })?;
    let secret = SecretKey::from_slice(&secret_bytes).map_err(|_| ConfigError {
        message: "Invalid sv2_authority_secret_key: not a 32-byte secp256k1 secret key".to_string(),
    })?;
    let derived = Keypair::from_secret_key(&Secp256k1::signing_only(), &secret)
        .x_only_public_key()
        .0
        .serialize();
    if derived != public_key {
        return Err(ConfigError {
            message: "sv2_authority_public_key is not the key sv2_authority_secret_key derives"
                .to_string(),
        });
    }
    // The certificate stores its validity window in 32-bit seconds; a longer
    // value would wrap, and 0 would expire every certificate as it is issued.
    let cert_validity_secs = cert_validity_secs.unwrap_or(DEFAULT_SV2_CERT_VALIDITY_SECS);
    if cert_validity_secs == 0 || cert_validity_secs > u64::from(u32::MAX) {
        return Err(ConfigError {
            message: format!(
                "sv2_cert_validity_secs must be between 1 and {}, got {cert_validity_secs}",
                u32::MAX
            ),
        });
    }
    Ok(Some(Sv2Authority {
        public_key: derived,
        secret_key: secret.secret_bytes(),
        cert_validity_secs,
    }))
}

/// Donation output and basis points this node puts in the shares it mines.
///
/// Bitcoin keeps the upstream rule: both fields or neither, and any address.
/// Bitcoin Cash pays [`bitcoin_cash_donation_address`]: `donation` defaults
/// to [`DEFAULT_BITCOIN_CASH_DONATION`], 0 opts out, and a configured
/// `donation_address` must name that same key.
fn resolve_donation(
    chain: Chain,
    network: Network,
    bitcoin_cash_network: Option<BitcoinCashNetwork>,
    donation_address: Option<&str>,
    donation: Option<u16>,
) -> Result<(Option<PayoutAddress>, Option<u16>), ConfigError> {
    match chain {
        Chain::Bitcoin => {
            let parsed = donation_address
                .map(|address| parse_payout_address(address, chain, network, None))
                .transpose()?;
            Ok((parsed, donation))
        }
        Chain::BitcoinCash => {
            let bitcoin_cash_network = bitcoin_cash_network.ok_or_else(|| ConfigError {
                message: "bitcoin_cash_network is required when chain is bitcoin-cash".to_string(),
            })?;
            let built_in = bitcoin_cash_donation_address(bitcoin_cash_network);
            if let Some(address) = donation_address {
                let configured =
                    parse_payout_address(address, chain, network, Some(bitcoin_cash_network))?;
                if configured.script_pubkey() != built_in.script_pubkey() {
                    return Err(ConfigError {
                        message: format!(
                            "donation_address on bitcoin-cash is fixed to {BITCOIN_CASH_DONATION_ADDRESS} \
                             (this network's prefix). Remove donation_address, set donation = 0 to \
                             opt out, or use fee_address and fee for your own pool fee"
                        ),
                    });
                }
            }
            match donation.unwrap_or(DEFAULT_BITCOIN_CASH_DONATION) {
                0 => Ok((None, None)),
                basis_points => Ok((Some(built_in), Some(basis_points))),
            }
        }
    }
}

/// Reject donation and fee basis points the payout cannot honor.
///
/// Each cut is a share of what is left, so each must be at most 100%. On a
/// Bitcoin Cash node each must also stay within [`MAX_BITCOIN_CASH_CUT`]
/// on its own, which its peers enforce on every share it mines.
fn validate_cuts(chain: Chain, donation: Option<u16>, fee: Option<u16>) -> Result<(), ConfigError> {
    let donation = donation.unwrap_or(0);
    let fee = fee.unwrap_or(0);
    if donation > MAX_BASIS_POINTS || fee > MAX_BASIS_POINTS {
        return Err(ConfigError {
            message: format!(
                "donation and fee are basis points and must each be at most {MAX_BASIS_POINTS}"
            ),
        });
    }
    if chain == Chain::BitcoinCash
        && (donation > MAX_BITCOIN_CASH_CUT || fee > MAX_BITCOIN_CASH_CUT)
    {
        return Err(ConfigError {
            message: format!(
                "donation ({donation}) and fee ({fee}) on bitcoin-cash must each be at most \
                 {MAX_BITCOIN_CASH_CUT} basis points; peers reject shares above it"
            ),
        });
    }
    Ok(())
}

#[cfg(any(test, feature = "test-utils"))]
impl StratumConfig<Raw> {
    /// Helper for tests to create a basic StratumConfig
    /// Uses regtest network by default with a valid regtest address
    pub fn new_for_test_default() -> Self {
        StratumConfig {
            hostname: "127.0.0.1".to_string(),
            port: 3333,
            start_difficulty: 1,
            minimum_difficulty: 1,
            maximum_difficulty: Some(1000),
            solo_address: None,
            zmqpubhashblock: "tcp://127.0.0.1:28332".to_string(),
            node_p2p_address: None,
            bootstrap_address: "tb1qyazxde6558qj6z3d9np5e6msmrspwpf6k0qggk".to_string(),
            donation_address: None,
            donation: None,
            fee_address: None,
            fee: None,
            miner_address: None,
            network: bitcoin::Network::Signet,
            chain: Chain::Bitcoin,
            bitcoin_cash_network: None,
            sv2_port: None,
            sv2_authority_public_key: None,
            sv2_authority_secret_key: None,
            sv2_cert_validity_secs: None,
            sv2_coinbase_only_from: default_sv2_coinbase_only_from(),
            template_source: TemplateSource::Rpc,
            template_provider_address: None,
            template_provider_authority_public_key: None,
            template_provider_proxy: None,
            version_mask: DEFAULT_VERSION_MASK,
            ignore_difficulty: None,
            pool_signature: None,
            node_label: None,
            max_connections: None,
            wait_for_chain_sync: true,
            mode: PoolMode::default(),
            merge_mining_tokens: None,
            payout_level: None,
            spend_cost: None,
            bootstrap_address_parsed: None,
            donation_address_parsed: None,
            fee_address_parsed: None,
            miner_address_parsed: None,
            bch_miner_address: None,
            sv2_authority: None,
            template_provider: None,
            sv2_coinbase_only_networks: Vec::new(),
            _state: PhantomData,
        }
    }
}

/// The `network` of a config that leaves it out, as a Bitcoin Cash config
/// may: Bitcoin Cash derives it from `bitcoin_cash_network`, and
/// [`Config::load`] refuses a Bitcoin config without one, so this value is
/// never used to mine.
fn default_unset_network() -> bitcoin::Network {
    bitcoin::Network::Bitcoin
}

/// Helper function to deserialize the network from the config file.
/// Possible values are: main, test, testnet4, signet, regtest
fn deserialize_network<'de, D>(deserializer: D) -> Result<bitcoin::Network, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let network_string: String = serde::Deserialize::deserialize(deserializer)?;
    bitcoin::Network::from_core_arg(&network_string).map_err(serde::de::Error::custom)
}

fn deserialize_version_mask<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let hex_string: String = serde::Deserialize::deserialize(deserializer)?;
    i32::from_str_radix(&hex_string, 16).map_err(serde::de::Error::custom)
}

#[derive(Debug, Deserialize, Clone)]
pub struct NetworkConfig {
    pub listen_address: String,
    /// Peers to dial at startup and reconnect to with backoff. Optional;
    /// when absent the node finds peers through DNS seeds alone.
    #[serde(default)]
    pub dial_peers: Vec<String>,
    pub max_pending_incoming: u32,
    pub max_pending_outgoing: u32,
    pub max_established_incoming: u32,
    pub max_established_outgoing: u32,
    pub max_established_per_peer: u32,
    pub max_workbase_per_second: u32,
    pub max_userworkbase_per_second: u32,
    pub max_miningshare_per_second: u32,
    pub max_inventory_per_second: u32,
    pub max_transaction_per_second: u32,
    pub max_requests_per_second: u64,
    pub dial_timeout_secs: u64,
    /// IP addresses to block from connecting. Connections from these IPs
    /// are immediately disconnected.
    #[serde(default)]
    pub blocked_ips: Vec<String>,
    /// Optional external address override for nodes behind NAT.
    /// If set, this address is advertised to peers via the identify protocol.
    /// Format: Multiaddr string, e.g. "/ip4/203.0.113.1/tcp/6884", or a Tor
    /// onion service's "/onion3/<address>:6884".
    #[serde(default)]
    pub external_address: Option<String>,
    // Fall back to the built-in DNS seeds for this network when no peer is
    // connected. Seeds are queried at startup without dial_peers, or 10s
    // after startup when dial_peers are set but none connected. Set false
    // to rely only on dial_peers.
    //
    // A Bitcoin Cash node takes the seeds of its `bitcoin_cash_network`,
    // never Bitcoin's.
    #[serde(default = "default_dns_seed")]
    pub dns_seed: bool,
    /// A Tor SOCKS5 proxy, `host:port` (Tor's default is `127.0.0.1:9050`),
    /// through which the node dials `/onion3` peers, as Bitcoin Cash Node's
    /// `-onion` does. Unset, the node uses `127.0.0.1:9050` once Tor's
    /// control port has made its onion service (`listen_onion`), as Bitcoin
    /// Cash Node does, and until then neither dials nor learns onion
    /// addresses. Other addresses are dialed directly, unless
    /// `proxy_clearnet` is on.
    #[serde(default)]
    pub tor_proxy: Option<String>,
    /// Announce this node only as a Tor onion service, as Bitcoin Cash
    /// Node's `-discover=0` with an onion `-externalip`: peers learn only an
    /// `/onion3` `external_address`, or the service `listen_onion` makes.
    /// The node takes no address from what peers observe, never makes a
    /// listen address its external one, and sends no listen addresses
    /// through Identify. A non-onion `external_address` stops it at start.
    /// It announces on Nostr only when it reaches the relays through Tor,
    /// as a node that opens no clearnet connection of its own does
    /// (`proxy_clearnet`, or an onion-only `onlynet`): a relay it reached
    /// directly would see its IP beside its onion address. An `onlynet`
    /// without `"ipv4"` and `"ipv6"` turns this on too
    /// ([`NetworkConfig::announces_only_onion`]). It governs what the node
    /// announces, not what it dials. Default false.
    #[serde(default)]
    pub onion_only: bool,
    /// Make this node a Tor onion service through Tor's control port
    /// (`tor_control`), as Bitcoin Cash Node's `-listenonion` does, so
    /// peers with Tor reach it at `/onion3/<address>:6884`. The service
    /// forwards to the node's `listen_address`, and its key is kept in
    /// `onion_v3_private_key` beside the store, so the address survives
    /// restarts. Off when the node does not listen, its `external_address`
    /// is an onion address already, or `onlynet` leaves onion out. While no
    /// Tor answers the node tries again now and then, logging at debug
    /// only. Default true.
    #[serde(default = "default_listen_onion")]
    pub listen_onion: bool,
    /// Tor's control port, `host:port`, as Bitcoin Cash Node's
    /// `-torcontrol`. Default [`DEFAULT_TOR_CONTROL`].
    #[serde(default = "default_tor_control")]
    pub tor_control: String,
    /// The password for Tor's control port (`HASHEDPASSWORD`), as Bitcoin
    /// Cash Node's `-torpassword`. Unset, the node authenticates with no
    /// password or with the cookie file of Tor (`COOKIE`), which sends the
    /// cookie itself to whatever answers on `tor_control`; a password is the
    /// stronger choice.
    #[serde(default)]
    pub tor_password: Option<String>,
    /// Dial IPv4, IPv6 and host-name peers through `tor_proxy` too, names
    /// unresolved, as Bitcoin Cash Node's `-proxy` does. The node's own TCP
    /// then only listens, and it resolves no host name itself: DNS seeds are
    /// skipped and Nostr relays are reached through `tor_proxy`. The peers
    /// it dials see a Tor exit, so it takes no address from what they
    /// observe, as `-proxy` sets `-discover=0`. Needs `tor_proxy`.
    #[serde(default)]
    pub proxy_clearnet: bool,
    /// The networks the node dials peers on, as Bitcoin Cash Node's
    /// `-onlynet`: any of `"ipv4"`, `"ipv6"` and `"onion"`. Unset, it dials
    /// IPv4, IPv6 and, through a Tor proxy, onion peers. A host name is
    /// clearnet: `/dns4` is IPv4, `/dns6` IPv6, `/dns` either. With
    /// `["onion"]` the node opens no clearnet connection: its own TCP only
    /// listens, DNS seeds are skipped and Nostr relays are reached through
    /// Tor. It then announces only onion addresses, as with `onion_only`,
    /// since Bitcoin Cash Node advertises no address on a network it does
    /// not reach. `"onion"` needs `tor_proxy`, or `listen_onion`, whose
    /// service brings the default Tor SOCKS port, as `-listenonion` does.
    #[serde(default)]
    pub onlynet: Option<Vec<PeerNetwork>>,
    /// DNS seed hostnames that replace the built-in ones, so a new seed can
    /// be used before a release names it. Their A/AAAA records list
    /// long-running nodes on the network's seed port (6884 on Bitcoin Cash),
    /// or on the port an entry names as `host:port`.
    #[serde(default)]
    pub dns_seed_hosts: Vec<String>,
    /// File holding this node's libp2p identity, whose public key is its
    /// peer ID. Created on the first start and read on every start after,
    /// so peers, saved peer lists and announcements keep finding the node
    /// under one peer ID. Default: `p2p_node_key` in the directory holding
    /// the store (`NetworkConfig::node_key_file`).
    #[serde(default)]
    pub node_key_path: Option<String>,
    /// Give the node a new peer ID on every start instead, as it had before
    /// identities were kept. Nothing then links a node's peer ID across
    /// restarts, but peers and announcements forget it each time.
    #[serde(default)]
    pub ephemeral_node_key: bool,
    /// Nostr relays the node announces itself on and, while it has fewer
    /// than 8 peers, asks for other nodes' announcements. Default
    /// [`DEFAULT_NOSTR_RELAYS`]; an empty list keeps the node off Nostr.
    #[serde(default = "default_nostr_relays")]
    pub nostr_relays: Vec<String>,
    /// Publish `external_address` on the relays so nodes short of peers can
    /// dial this one. Only a node with an `external_address`, which its
    /// operator says is reachable, or an onion service made through the Tor
    /// control port announces, and an `onion_only` one only through Tor.
    /// Default true.
    #[serde(default = "default_nostr_announce")]
    pub nostr_announce: bool,
    /// Outgoing peers the node keeps. While it has fewer, it dials a peer it
    /// learned of through Kademlia or Identify at a public IP address, one a
    /// second, never past `max_established_outgoing` and never in an IPv4
    /// /16 or IPv6 /32 where it already has an outgoing peer (`dial_peers`
    /// do not count). Connections Kademlia opens for a lookup close once no
    /// lookup needs them. 0 turns both off.
    #[serde(default = "default_target_outgoing_peers")]
    pub target_outgoing_peers: u32,
}

/// A network the node can dial peers on, named as Bitcoin Cash Node names
/// them for `-onlynet`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PeerNetwork {
    /// `/ip4` addresses and `/dns4` host names.
    Ipv4,
    /// `/ip6` addresses and `/dns6` host names.
    Ipv6,
    /// `/onion3` addresses, dialed through `tor_proxy`, or through the
    /// default Tor SOCKS port once the Tor control port has made the onion
    /// service of the node.
    Onion,
}

fn default_dns_seed() -> bool {
    true
}

fn default_listen_onion() -> bool {
    true
}

/// Tor's control port where Tor runs with its defaults, as Bitcoin Cash
/// Node's `-torcontrol` default.
pub const DEFAULT_TOR_CONTROL: &str = "127.0.0.1:9051";

fn default_tor_control() -> String {
    DEFAULT_TOR_CONTROL.to_string()
}

/// Public Nostr relays a node uses by default, run by different operators.
/// Each answered an announcement lookup when they were picked (2026-11-17).
pub const DEFAULT_NOSTR_RELAYS: [&str; 6] = [
    "wss://nos.lol",
    "wss://relay.primal.net",
    "wss://nostr.mom",
    "wss://offchain.pub",
    "wss://nostr.oxtr.dev",
    "wss://relay.nostr.net",
];

fn default_nostr_relays() -> Vec<String> {
    DEFAULT_NOSTR_RELAYS
        .iter()
        .map(|relay| relay.to_string())
        .collect()
}

fn default_nostr_announce() -> bool {
    true
}

/// File name of the node identity when `node_key_path` is not set.
pub const DEFAULT_NODE_KEY_FILE: &str = "p2p_node_key";

/// File name of the saved book of known peers, beside the store.
pub const DEFAULT_PEER_BOOK_FILE: &str = "p2p_peers.json";

/// File name of the onion service's key, beside the store, as Bitcoin Cash
/// Node names it.
pub const DEFAULT_ONION_KEY_FILE: &str = "onion_v3_private_key";

impl NetworkConfig {
    /// Where this node keeps its libp2p identity: `node_key_path`, else
    /// [`DEFAULT_NODE_KEY_FILE`] in the directory holding `store_path`.
    /// `None` with `ephemeral_node_key`.
    pub fn node_key_file(&self, store_path: &str) -> Option<std::path::PathBuf> {
        if self.ephemeral_node_key {
            return None;
        }
        Some(match &self.node_key_path {
            Some(path) => std::path::PathBuf::from(path),
            None => std::path::Path::new(store_path)
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join(DEFAULT_NODE_KEY_FILE),
        })
    }

    /// Where this node saves its book of known peers:
    /// [`DEFAULT_PEER_BOOK_FILE`] in the directory holding `store_path`.
    /// `None` with `ephemeral_node_key`, which keeps nothing that links a
    /// node's restarts.
    pub fn peer_book_file(&self, store_path: &str) -> Option<std::path::PathBuf> {
        if self.ephemeral_node_key {
            return None;
        }
        Some(
            std::path::Path::new(store_path)
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join(DEFAULT_PEER_BOOK_FILE),
        )
    }

    /// Where this node keeps its onion service's key (`listen_onion`):
    /// [`DEFAULT_ONION_KEY_FILE`] in the directory holding `store_path`.
    /// `None` with `ephemeral_node_key`, whose onion address is new on every
    /// start as its peer ID is.
    pub fn onion_key_file(&self, store_path: &str) -> Option<std::path::PathBuf> {
        if self.ephemeral_node_key {
            return None;
        }
        Some(
            std::path::Path::new(store_path)
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join(DEFAULT_ONION_KEY_FILE),
        )
    }

    /// Whether `onlynet` lets the node dial peers on `network`: it lists
    /// `network`, or there is no `onlynet`.
    pub fn onlynet_allows(&self, network: PeerNetwork) -> bool {
        self.onlynet
            .as_ref()
            .is_none_or(|networks| networks.contains(&network))
    }

    /// Whether the node dials peers on `network` from the start: one
    /// `onlynet` allows, and onion only through a `tor_proxy`. Without one,
    /// onion peers become dialable once the Tor control port has made the
    /// onion service of the node, through the default Tor SOCKS port, if
    /// `onlynet` allows onion.
    pub fn dials(&self, network: PeerNetwork) -> bool {
        self.onlynet_allows(network) && (network != PeerNetwork::Onion || self.tor_proxy.is_some())
    }

    /// Whether the node opens clearnet connections of its own: it dials
    /// IPv4 or IPv6 peers, and not through `tor_proxy` (`proxy_clearnet`).
    /// Otherwise it resolves no host name either: it skips DNS seeds and
    /// reaches Nostr relays through Tor.
    pub fn dials_clearnet_directly(&self) -> bool {
        !self.proxy_clearnet && (self.dials(PeerNetwork::Ipv4) || self.dials(PeerNetwork::Ipv6))
    }

    /// Whether the node announces only onion addresses: `onion_only`, or an
    /// `onlynet` that allows neither IPv4 nor IPv6, as Bitcoin Cash Node
    /// advertises no address on a network it does not reach.
    pub fn announces_only_onion(&self) -> bool {
        let clearnet =
            self.onlynet_allows(PeerNetwork::Ipv4) || self.onlynet_allows(PeerNetwork::Ipv6);
        self.onion_only || !clearnet
    }
}

fn default_target_outgoing_peers() -> u32 {
    16
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            listen_address: "".to_string(),
            dial_peers: vec![],
            max_pending_incoming: 10,
            max_pending_outgoing: 10,
            // Each outgoing peer is another node's inbound one: with 16
            // outgoing a node takes 16 on average, and 128 is room for that
            // while one node in four accepts inbound connections.
            max_established_incoming: 128,
            max_established_outgoing: 50,
            // Two, not one: when two nodes dial each other at the same moment
            // each can keep its own outbound connection and refuse the other's
            // inbound one, so with a limit of one both connections close and
            // they retry. A second slot lets both survive. More only duplicates
            // traffic, as libp2p multiplexes every protocol over one connection.
            max_established_per_peer: 2,
            max_workbase_per_second: 10,
            max_userworkbase_per_second: 10,
            max_miningshare_per_second: 100,
            max_inventory_per_second: 100,
            max_transaction_per_second: 100,
            max_requests_per_second: 100,
            dial_timeout_secs: 30,
            blocked_ips: vec![],
            external_address: None,
            dns_seed: true,
            tor_proxy: None,
            onion_only: false,
            listen_onion: true,
            tor_control: default_tor_control(),
            tor_password: None,
            proxy_clearnet: false,
            onlynet: None,
            dns_seed_hosts: vec![],
            node_key_path: None,
            ephemeral_node_key: false,
            nostr_relays: default_nostr_relays(),
            nostr_announce: true,
            target_outgoing_peers: default_target_outgoing_peers(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct StoreConfig {
    pub path: String,
    /// Time-to-live for PPLNS shares (in days)
    #[serde(default = "default_pplns_ttl_days")]
    pub pplns_ttl_days: u64,
    /// Keep the whole weak shares a Bitcoin Cash share carries until its body
    /// is pruned, two payout depths below the tip, as an explorer or an
    /// archive wants. By default (false) they are pruned one payout depth
    /// below the tip, and their leaf root and summary stay with the body;
    /// either way the node serves every body a syncing peer asks for.
    #[serde(default)]
    pub keep_whole_weak_shares: bool,
}

fn default_wait_for_chain_sync() -> bool {
    true
}

/// This computer: IPv4 and IPv6 loopback.
fn default_sv2_coinbase_only_from() -> Vec<String> {
    vec!["127.0.0.0/8".to_string(), "::1/128".to_string()]
}

/// Parse client address ranges: each an IP address or a CIDR range.
fn parse_client_networks(entries: &[String]) -> Result<Vec<IpNet>, ConfigError> {
    entries
        .iter()
        .map(|entry| {
            entry
                .parse::<IpNet>()
                .or_else(|_| entry.parse::<IpAddr>().map(IpNet::from))
                .map_err(|_| ConfigError {
                    message: format!(
                        "sv2_coinbase_only_from: {entry:?} is not an IP address or CIDR range"
                    ),
                })
        })
        .collect()
}

fn default_pplns_ttl_days() -> u64 {
    7
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct LoggingConfig {
    /// Log to file if specified
    pub file: Option<String>,
    /// Log level (defaults to "info")
    #[serde(default = "default_log_level")]
    pub level: String,
    /// Directory for stats
    #[serde(default = "default_stats_dir")]
    pub stats_dir: String,
    /// Log to stdout if `true`
    #[serde(default = "default_log_console")]
    pub console: Option<bool>,
}

fn default_log_console() -> Option<bool> {
    Some(true)
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_stats_dir() -> String {
    "./logs/stats".to_string()
}

#[derive(Deserialize, Clone)]
pub struct ApiConfig {
    /// The hostname for the API server
    pub hostname: String,
    /// The port for the API server
    pub port: u16,
    /// Optional authentication username
    pub auth_user: Option<String>,
    /// Optional authentication token (salt$hmac format, used by the server)
    pub auth_token: Option<String>,
    /// Optional raw password for CLI client authentication (not used by server)
    pub auth_password: Option<String>,
    /// Enable permissive CORS for the API server
    #[serde(default)]
    pub cors_allowed: bool,
    /// Serve the dashboard, a `web_root` frontend and the read-only share
    /// chain routes (including `GET /peers` and a tokenless `/ws`
    /// connection) they read without the API's authentication, for a
    /// public node. The node's own routes (`/metrics`, `/blocked_ips`,
    /// `/jd_prepare`) keep requiring `auth_user`.
    #[serde(default)]
    pub public_dashboard: bool,
    /// A folder holding a self-hosted frontend's build, served for every
    /// path no API route takes. Any frontend that builds to static files
    /// works, such as TypeScript or Rust compiled to WebAssembly.
    pub web_root: Option<String>,
}

/// Custom Debug to redact password
impl std::fmt::Debug for ApiConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ApiConfig")
            .field("hostname", &self.hostname)
            .field("port", &self.port)
            .field("auth_user", &self.auth_user)
            .field("auth_token", &"[redacted]")
            .field("auth_password", &"[redacted]")
            .field("cors_allowed", &self.cors_allowed)
            .field("public_dashboard", &self.public_dashboard)
            .field("web_root", &self.web_root)
            .finish()
    }
}

/// Configuration for the Electrum wallet server.
///
/// The server speaks Electrum JSON-RPC over plain TCP and is meant for the
/// operator's own wallet only, so it binds localhost by default and has no
/// TLS or client authentication. It is off unless the `[electrum]` section
/// enables it.
#[derive(Debug, Deserialize, Clone)]
pub struct ElectrumConfig {
    /// Start the Electrum server with the node
    #[serde(default)]
    pub enabled: bool,
    /// The hostname the Electrum server binds to
    #[serde(default = "default_electrum_hostname")]
    pub hostname: String,
    /// The port the Electrum server binds to
    #[serde(default = "default_electrum_port")]
    pub port: u16,
    /// Maximum number of concurrent wallet connections
    #[serde(default = "default_electrum_max_connections")]
    pub max_connections: u32,
}

fn default_electrum_hostname() -> String {
    "127.0.0.1".to_string()
}

/// Conventional Electrum TCP port.
fn default_electrum_port() -> u16 {
    50001
}

/// Local wallets only, so a small cap is plenty.
fn default_electrum_max_connections() -> u32 {
    64
}

impl Default for ElectrumConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            hostname: default_electrum_hostname(),
            port: default_electrum_port(),
            max_connections: default_electrum_max_connections(),
        }
    }
}

/// Configuration for the no-PoW load-test simulation.
///
/// Only acted upon when a binary is built with the `sim` cargo feature; the
/// field is always present in `Config` so non-sim builds parse the same config
/// files without error.
/// See docs/simulation/load-test-plan.md.
#[derive(Debug, Deserialize, Clone)]
pub struct SimConfig {
    /// Master switch for this node's sim emitter / statistical block-find.
    #[serde(default)]
    pub enabled: bool,
    /// This node's payout identity (bitcoin address); should be distinct per node.
    pub miner_address: String,
    /// This node's share chain identity. Cannot be derived from `miner_address`
    /// -- different chain, different key -- so it is supplied separately.
    /// Falls back to `[stratum] miner_address` when omitted.
    #[serde(default)]
    pub share_address: Option<String>,
    /// Modeled hashrate Hᵢ in hashes/sec; sets the emission rate.
    pub hashrate: f64,
    /// Expected number of shares per bitcoin block (global; same on all nodes).
    pub block_to_share_ratio: u64,
    /// Per-node RNG seed for reproducible emission/block-find. Should differ per
    /// node so timelines decorrelate; omit for a nondeterministic seed. A cheap
    /// RNG is fine — this is a simulation, not entropy that guards funds.
    #[serde(default)]
    pub seed: Option<u64>,
    /// Artificial delay (milliseconds) applied to this node's outbound share
    /// announcements, modeling network latency. Over loopback, propagation is
    /// otherwise near-instant and the chain stays linear; a non-zero delay
    /// widens the window for concurrent emission, so uncles appear and uncle
    /// rate tracks this value. Default 0 (no delay).
    #[serde(default)]
    pub propagation_delay_ms: Option<u64>,
    /// Unix time (seconds) to use as the ASERT difficulty anchor instead of the
    /// genesis timestamp. The fixed regtest genesis is dated in the past, so the
    /// share chain is permanently "behind schedule" and ASERT stays floored at
    /// the easy clamp (the chain races, never reaching a steady rate). Setting
    /// this to ~launch time lets ASERT regulate around the 10s target. MUST be
    /// identical across all nodes (the harness writes one shared value) or their
    /// ASERT targets diverge and shares are rejected.
    #[serde(default)]
    pub asert_anchor_time: Option<u64>,
    /// Total network hashrate (hashes/sec) = sum over all nodes. Used to anchor
    /// the genesis difficulty at the steady-state value (`hashrate · 10s / 2^32`)
    /// so the chain starts regulated instead of climbing from the easy clamp for
    /// ~15-20 min. MUST be identical across nodes (it sets the genesis target,
    /// hence the genesis hash). Omit to keep the fixed genesis target.
    #[serde(default)]
    pub network_hashrate: Option<u64>,
    /// Sim-only override of the ASERT ideal block time (seconds). Smaller =
    /// more blocks per minute (time-compressed runs for faster data). MUST be
    /// identical across nodes (ASERT consensus). Omit for the default 10s.
    #[serde(default)]
    pub ideal_block_time_secs: Option<u32>,
    /// Bitcoin Cash: this node's miner wants shares this many times the
    /// minimum's work, as a big miner's node gives it: it mines at that
    /// multiple within the share's band cap and records the rest as its
    /// wanted target. Omit to mine at the minimum.
    #[serde(default)]
    pub share_multiple: Option<u32>,
    /// With `share_multiple`, mine at that multiple even past the band cap,
    /// so peers can be seen refusing the shares.
    #[serde(default)]
    pub share_multiple_ignores_cap: bool,
    /// Bitcoin Cash: on a job at the minimum, pace proofs by the job's weak
    /// target and send those short of the share target to the node as weak
    /// shares, as a real miner capped at the weak difficulty does. Off by
    /// default, so other sim runs keep pacing by the share target.
    #[serde(default)]
    pub weak_shares: bool,
    /// Bitcoin Cash: with `weak_shares`, every job this node's miner mines at
    /// the minimum wants a target twice as easy as its weak target, never
    /// easier than the chain's ceiling, so its weak shares ask for an easier
    /// one whatever the node's default would ask. For the end-to-end check of
    /// the weak shares that ask; off by default, so the miner wants its bits.
    #[serde(default)]
    pub weak_asks: bool,
    /// Bitcoin Cash: of the share blocks this node broadcasts that carry weak
    /// shares, every this many goes out with its first weak share's coinbase
    /// value one satoshi higher, as a peer serving an altered copy of an
    /// honest share would. Such a copy goes whole, not with its weak shares
    /// listed by hash, which would name the honest ones. The node's store
    /// and its answers to block requests stay honest. For the end-to-end
    /// check of the weak-share commitment; omit to broadcast every share as
    /// stored.
    #[serde(default)]
    pub alter_weak_copy_every: Option<u64>,
    /// Bitcoin Cash: carry only this node's own weak shares in the shares it
    /// builds, as a node in a coalition refusing to carry others' weak shares
    /// would. It still checks, stores and relays them. For the end-to-end
    /// check of `WEAK_DEPTH` against a refusing coalition.
    #[serde(default)]
    pub carry_only_own_weak_shares: bool,
    /// Bitcoin Cash: model this many miners on the node instead of one. Every
    /// proof is found by one of them, picked at random, each with its own
    /// payout address and share owner, so a small swarm fills the window with
    /// hundreds of payees: for end-to-end runs of the level draw, the hybrid
    /// split and the payout tree. Their keys do not exist, so what they are
    /// paid on regtest cannot be spent. Omit for the node's one miner.
    #[serde(default)]
    pub virtual_miners: Option<u32>,
}

/// Config for p2poolv2 nodes
///
/// The network config switches to defaults if not provided. This is
/// the case for standalone PPLNS pools like Hydrapool.
#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
pub struct Config {
    #[serde(default)]
    pub network: NetworkConfig,
    pub store: StoreConfig,
    pub stratum: StratumConfig,
    pub bitcoinrpc: BitcoinRpcConfig,
    /// Extra BCHN nodes every found block also goes to, beside the node in
    /// `[bitcoinrpc]`, each as a `[[submit_nodes]]` table with `url`,
    /// `username` and `password`. Optional and empty by default. A block
    /// then does not depend on one node or one connection.
    #[serde(default)]
    pub submit_nodes: Vec<BitcoinRpcConfig>,
    pub logging: LoggingConfig,
    pub api: ApiConfig,
    #[serde(default)]
    pub electrum: ElectrumConfig,
}

#[allow(dead_code)]
impl Config {
    /// Load the configuration file at `path`, with environment overrides
    /// spelled `P2POOL_<SECTION>__<KEY>`, for example `P2POOL_API__PORT`.
    ///
    /// On Bitcoin Cash, `stratum.network` is replaced by the network implied
    /// by `bitcoin_cash_network` (see [`BitcoinCashNetwork::legacy_network`]).
    /// The P2P protocol id and every difficulty calculation read that field,
    /// so two Bitcoin Cash nodes on the same network must not disagree on it
    /// because one left a Bitcoin value such as `signet` in its file.
    ///
    /// `[[submit_nodes]]` with light templates is refused (see
    /// [`Config::validate_submit_nodes`]), and so are Tor settings with no
    /// Tor proxy to dial through (see [`Config::validate_tor_proxy`]).
    ///
    /// A config that still sets `bch_share_chain`, in the file or as
    /// `P2POOL_STRATUM__BCH_SHARE_CHAIN`, is refused: the main and mini
    /// Bitcoin Cash share chains merged into one (see
    /// [`REMOVED_BCH_SHARE_CHAIN_KEY`]), and neither value means what it
    /// meant before.
    pub fn load(path: &str) -> Result<Self, config::ConfigError> {
        let built = config::Config::builder()
            .add_source(config::File::with_name(path))
            // Nesting uses `__` so that a single `_` stays part of a field name.
            // With a single-underscore separator, P2POOL_STORE_PPLNS_TTL_DAYS
            // addresses store.pplns.ttl.days, which does not exist, and the
            // override is silently discarded. See the env_override tests.
            .add_source(
                config::Environment::with_prefix("P2POOL")
                    .prefix_separator("_")
                    .separator("__"),
            )
            .build()?;
        let network_given = built.get_string("stratum.network").is_ok();
        if built
            .get::<config::Value>(&format!("stratum.{REMOVED_BCH_SHARE_CHAIN_KEY}"))
            .is_ok()
        {
            return Err(config::ConfigError::Message(format!(
                "[stratum] {REMOVED_BCH_SHARE_CHAIN_KEY} is no longer a setting: the main and mini \
                 Bitcoin Cash share chains merged into one share chain, which keeps the mini \
                 chain's anchor, ceiling and genesis. Remove {REMOVED_BCH_SHARE_CHAIN_KEY} from \
                 the config (and P2POOL_STRATUM__BCH_SHARE_CHAIN from the environment), and \
                 start on a fresh store."
            )));
        }
        let loaded: Self = built.try_deserialize()?;
        if loaded.stratum.chain == Chain::Bitcoin && !network_given {
            return Err(config::ConfigError::Message(
                "[stratum] network is required with chain = \"bitcoin\": \"main\", \
                 \"testnet4\" or \"signet\". A Bitcoin Cash node sets chain = \
                 \"bitcoin-cash\" and bitcoin_cash_network instead."
                    .to_string(),
            ));
        }
        loaded.validate_submit_nodes()?;
        loaded.validate_tor_proxy()?;
        Ok(loaded.with_bitcoin_cash_network_derived())
    }

    /// Refuse `[[submit_nodes]]` with `template_source = "rpc_light"`. A
    /// block found on a light template is sent as its header and coinbase,
    /// which only the node in `[bitcoinrpc]` can complete: it kept the
    /// template's transactions. That node relays the block to its peers.
    pub fn validate_submit_nodes(&self) -> Result<(), config::ConfigError> {
        if self.stratum.template_source == TemplateSource::RpcLight && !self.submit_nodes.is_empty()
        {
            return Err(config::ConfigError::Message(
                "[[submit_nodes]] cannot be used with template_source = \"rpc_light\": a block \
                 found on a light template can only be completed by the node in [bitcoinrpc]"
                    .to_string(),
            ));
        }
        Ok(())
    }

    /// Refuse `[network] proxy_clearnet` without a `tor_proxy` to dial
    /// through, `"onion"` in `onlynet` with neither a `tor_proxy` nor
    /// `listen_onion`, whose onion service brings the default Tor SOCKS port
    /// (as `-listenonion` does for `-onlynet=onion`), and an `onlynet` that
    /// lists no network: the node could not dial what each of them names.
    pub fn validate_tor_proxy(&self) -> Result<(), config::ConfigError> {
        let network = &self.network;
        if network.proxy_clearnet && network.tor_proxy.is_none() {
            return Err(config::ConfigError::Message(
                "[network] proxy_clearnet needs tor_proxy, the Tor SOCKS5 proxy every dial goes \
                 through, for example tor_proxy = \"127.0.0.1:9050\""
                    .to_string(),
            ));
        }
        if network.tor_proxy.is_none()
            && !network.listen_onion
            && network
                .onlynet
                .as_ref()
                .is_some_and(|networks| networks.contains(&PeerNetwork::Onion))
        {
            return Err(config::ConfigError::Message(
                "[network] onlynet lists \"onion\", which needs tor_proxy, the Tor SOCKS5 proxy \
                 onion peers are dialed through, for example tor_proxy = \"127.0.0.1:9050\", or \
                 listen_onion, which dials them through the default Tor SOCKS port once the \
                 Tor control port has made this node an onion service"
                    .to_string(),
            ));
        }
        if network.onlynet.as_ref().is_some_and(Vec::is_empty) {
            return Err(config::ConfigError::Message(
                "[network] onlynet lists no network: list \"ipv4\", \"ipv6\" or \"onion\", or \
                 leave it out to dial every network the node can reach"
                    .to_string(),
            ));
        }
        Ok(())
    }

    /// Set `stratum.network` from `bitcoin_cash_network` on a Bitcoin Cash
    /// node. Bitcoin configurations are returned unchanged.
    pub fn with_bitcoin_cash_network_derived(mut self) -> Self {
        self.stratum.network = self.stratum.effective_network();
        self
    }

    pub fn with_listen_address(mut self, listen_address: String) -> Self {
        self.network.listen_address = listen_address;
        self
    }

    pub fn with_dial_peers(mut self, dial_peers: Vec<String>) -> Self {
        self.network.dial_peers = dial_peers;
        self
    }

    pub fn with_max_pending_incoming(mut self, max_pending_incoming: u32) -> Self {
        self.network.max_pending_incoming = max_pending_incoming;
        self
    }

    pub fn with_max_pending_outgoing(mut self, max_pending_outgoing: u32) -> Self {
        self.network.max_pending_outgoing = max_pending_outgoing;
        self
    }

    pub fn with_max_established_incoming(mut self, max_established_incoming: u32) -> Self {
        self.network.max_established_incoming = max_established_incoming;
        self
    }

    pub fn with_max_established_outgoing(mut self, max_established_outgoing: u32) -> Self {
        self.network.max_established_outgoing = max_established_outgoing;
        self
    }

    pub fn with_max_established_per_peer(mut self, max_established_per_peer: u32) -> Self {
        self.network.max_established_per_peer = max_established_per_peer;
        self
    }

    pub fn with_store_path(mut self, store_path: String) -> Self {
        self.store.path = store_path;
        self
    }

    pub fn with_pplns_ttl_days(mut self, days: u64) -> Self {
        self.store.pplns_ttl_days = days;
        self
    }

    pub fn with_stratum_hostname(mut self, stratum_hostname: String) -> Self {
        self.stratum.hostname = stratum_hostname;
        self
    }

    pub fn with_stratum_port(mut self, stratum_port: u16) -> Self {
        self.stratum.port = stratum_port;
        self
    }

    pub fn with_stratum_solo_address(mut self, solo_address: String) -> Self {
        self.stratum.solo_address = Some(solo_address);
        self
    }

    pub fn with_stratum_zmqpubhashblock(mut self, zmqpubhashblock: String) -> Self {
        self.stratum.zmqpubhashblock = zmqpubhashblock;
        self
    }

    pub fn with_stratum_node_p2p_address(mut self, node_p2p_address: Option<String>) -> Self {
        self.stratum.node_p2p_address = node_p2p_address;
        self
    }

    pub fn with_stratum_bootstrap_address(mut self, bootstrap_address: String) -> Self {
        self.stratum.bootstrap_address = bootstrap_address;
        self
    }

    pub fn with_start_difficulty(mut self, start_difficulty: u64) -> Self {
        self.stratum.start_difficulty = start_difficulty;
        self
    }

    pub fn with_minimum_difficulty(mut self, minimum_difficulty: u64) -> Self {
        self.stratum.minimum_difficulty = minimum_difficulty;
        self
    }

    pub fn with_maximum_difficulty(mut self, maximum_difficulty: Option<u64>) -> Self {
        self.stratum.maximum_difficulty = maximum_difficulty;
        self
    }

    pub fn with_ignore_difficulty(mut self, ignore_difficulty: Option<bool>) -> Self {
        self.stratum.ignore_difficulty = ignore_difficulty;
        self
    }

    pub fn with_pool_signature(mut self, pool_signature: Option<String>) -> Self {
        self.stratum.pool_signature = pool_signature;
        self
    }

    pub fn with_bitcoinrpc_url(mut self, bitcoin_url: String) -> Self {
        self.bitcoinrpc.url = bitcoin_url;
        self
    }

    pub fn with_bitcoinrpc_username(mut self, bitcoin_username: String) -> Self {
        self.bitcoinrpc.username = bitcoin_username;
        self
    }

    pub fn with_bitcoinrpc_password(mut self, bitcoin_password: String) -> Self {
        self.bitcoinrpc.password = bitcoin_password;
        self
    }

    pub fn with_bitcoin_network(mut self, network: bitcoin::Network) -> Self {
        self.stratum.network = network;
        self
    }

    pub fn with_stats_dir(mut self, stats_dir: String) -> Self {
        self.logging.stats_dir = stats_dir;
        self
    }

    pub fn with_api_hostname(mut self, hostname: String) -> Self {
        self.api.hostname = hostname;
        self
    }

    pub fn with_api_port(mut self, port: u16) -> Self {
        self.api.port = port;
        self
    }

    pub fn with_api_auth_user(mut self, auth_user: Option<String>) -> Self {
        self.api.auth_user = auth_user;
        self
    }

    pub fn with_api_auth_token(mut self, auth_token: Option<String>) -> Self {
        self.api.auth_token = auth_token;
        self
    }

    pub fn with_electrum_enabled(mut self, enabled: bool) -> Self {
        self.electrum.enabled = enabled;
        self
    }

    pub fn with_electrum_hostname(mut self, hostname: String) -> Self {
        self.electrum.hostname = hostname;
        self
    }

    pub fn with_electrum_port(mut self, port: u16) -> Self {
        self.electrum.port = port;
        self
    }

    pub fn with_max_requests_per_second(mut self, max_requests_per_second: u64) -> Self {
        self.network.max_requests_per_second = max_requests_per_second;
        self
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod bitcoin_cash_donation_tests;

#[cfg(test)]
mod sv2_authority_tests;

#[cfg(test)]
mod template_provider_tests;

#[cfg(test)]
mod miner_address_tests;
