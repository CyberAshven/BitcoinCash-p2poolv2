// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Build the transactions of a Bitcoin Cash share trade offline.
//!
//! A seller sells share units to a buyer for BCH with one hash time locked
//! contract on each chain (`docs/atomic-swap/bch-share-trading.md`). These
//! commands print the contract for agreed terms, decode one a counterparty
//! sent, and build and sign the lock, claim and refund transactions on
//! either chain. They never broadcast: a Bitcoin Cash transaction goes to
//! BCHN (`sendrawtransaction`) or a wallet, and a share-chain transaction to
//! a share-chain node once nodes relay share transactions.
//!
//! Signing needs the owner's key, exported from their wallet as WIF. It is
//! read from a file or a hidden prompt, never from the command line, and is
//! never generated here. The seller's 32-byte secret is read the same way:
//! the seller picks it, for example with `openssl rand -hex 32`.

use bitcoin::consensus::encode::{deserialize_hex, serialize_hex};
use bitcoin::hashes::Hash;
use bitcoin::hex::{DisplayHex, FromHex};
use bitcoin::{
    Amount, NetworkKind, OutPoint, PrivateKey, PubkeyHash, ScriptBuf, Transaction, Txid,
};
use clap::{Args, Subcommand, ValueEnum};
use p2poolv2_lib::address::bch_share::{self, encode_p2sh32_cashaddr};
use p2poolv2_lib::config::{BitcoinCashNetwork, Chain, parse_payout_address};
use p2poolv2_lib::shares::bch_htlc::{HtlcContract, SECRET_LENGTH, secret_hash};
use p2poolv2_lib::shares::bch_script::dust_threshold;
use p2poolv2_lib::shares::bch_trade::{
    Coin, TradeKey, build_claim_transaction, build_lock_transaction, build_refund_transaction,
};
use serde_json::{Value, json};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Fee rate on Bitcoin Cash when none is given: BCHN's minimum relay fee,
/// 1,000 satoshis per kilobyte.
const DEFAULT_BCH_FEE_PER_BYTE: u64 = 1;
/// Fee rate on the share chain when none is given. Share transactions need
/// no fee; a full pool admits the ones burning the most units per byte.
const DEFAULT_SHARE_FEE_PER_BYTE: u64 = 0;

/// The chain a contract lives on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TradeChain {
    /// The share chain: share units, `p2pool:`/`tp2pool:`/`rp2pool:` addresses.
    Share,
    /// Bitcoin Cash: BCH, `bitcoincash:`/`bchtest:`/`bchreg:` addresses.
    Bch,
}

/// Which chain and which Bitcoin Cash network a command works on.
#[derive(Args, Debug, Clone, Copy)]
pub struct ChainArgs {
    /// Chain the contract lives on: share or bch
    #[arg(long, value_enum)]
    pub chain: TradeChain,
    /// Bitcoin Cash network: mainnet, chipnet or regtest
    #[arg(short, long, value_parser = super::bch_share_address::parse_network)]
    pub network: BitcoinCashNetwork,
}

/// The `bch-trade` subcommands.
#[derive(Subcommand, Debug)]
pub enum TradeCommand {
    /// Print the contract for agreed terms
    ///
    /// On the share chain the claim address is the buyer's share address and
    /// the refund address the seller's. On Bitcoin Cash the claim address is
    /// the seller's BCH address and the refund address the buyer's. Prints
    /// the redeem script both sides keep, and on Bitcoin Cash the P2SH32
    /// address a wallet can fund.
    Contract {
        #[command(flatten)]
        chain: ChainArgs,
        /// SHA256 of the seller's secret, 64 hex characters
        #[arg(long)]
        secret_hash: String,
        /// Address that claims with the secret
        #[arg(long)]
        claim_address: String,
        /// Address that takes the coins back after the refund height
        #[arg(long)]
        refund_address: String,
        /// Height (share height on the share chain, block height on Bitcoin
        /// Cash) from which the refund is valid
        #[arg(long)]
        refund_height: u32,
    },
    /// Decode a redeem script a counterparty sent into its terms
    Inspect {
        #[command(flatten)]
        chain: ChainArgs,
        /// Redeem script, hex
        #[arg(long)]
        redeem_script: String,
    },
    /// Print the SHA256 of a secret, the hash both contracts lock to
    SecretHash {
        /// File holding the 32-byte secret as 64 hex characters. Prompts when
        /// omitted.
        #[arg(long)]
        secret_file: Option<PathBuf>,
    },
    /// Lock P2PKH coins into a contract
    Lock {
        #[command(flatten)]
        chain: ChainArgs,
        /// Redeem script of the contract, hex
        #[arg(long)]
        redeem_script: String,
        /// A P2PKH coin of the signing key to spend, as txid:vout:satoshis.
        /// Repeat for more coins.
        #[arg(long = "coin", required = true)]
        coins: Vec<String>,
        /// Satoshis (share units on the share chain) to lock
        #[arg(long)]
        amount: u64,
        /// Address that receives the change
        #[arg(long)]
        change_address: String,
        /// Fee per byte, in satoshis or share units: 1 on Bitcoin Cash and 0
        /// on the share chain by default. Raise it on the share chain when a
        /// node's pool is full: it ranks transactions by units burned per byte
        #[arg(long)]
        fee_rate: Option<u64>,
        /// File holding the signing key as WIF. Prompts when omitted.
        #[arg(long)]
        wif_file: Option<PathBuf>,
    },
    /// Claim a contract with the secret
    Claim {
        #[command(flatten)]
        chain: ChainArgs,
        /// Redeem script of the contract, hex
        #[arg(long)]
        redeem_script: String,
        /// The contract's coin, as txid:vout:satoshis
        #[arg(long)]
        coin: String,
        /// Address that receives the claimed coins
        #[arg(long)]
        to: String,
        /// Fee per byte, in satoshis or share units: 1 on Bitcoin Cash and 0
        /// on the share chain by default. Raise it on the share chain when a
        /// node's pool is full: it ranks transactions by units burned per byte
        #[arg(long)]
        fee_rate: Option<u64>,
        /// File holding the claim key as WIF. Prompts when omitted.
        #[arg(long)]
        wif_file: Option<PathBuf>,
        /// File holding the secret as 64 hex characters. Prompts when omitted.
        #[arg(long)]
        secret_file: Option<PathBuf>,
    },
    /// Take a contract's coins back after its refund height
    Refund {
        #[command(flatten)]
        chain: ChainArgs,
        /// Redeem script of the contract, hex
        #[arg(long)]
        redeem_script: String,
        /// The contract's coin, as txid:vout:satoshis
        #[arg(long)]
        coin: String,
        /// Address that receives the refund
        #[arg(long)]
        to: String,
        /// Fee per byte, in satoshis or share units: 1 on Bitcoin Cash and 0
        /// on the share chain by default. Raise it on the share chain when a
        /// node's pool is full: it ranks transactions by units burned per byte
        #[arg(long)]
        fee_rate: Option<u64>,
        /// File holding the refund key as WIF. Prompts when omitted.
        #[arg(long)]
        wif_file: Option<PathBuf>,
    },
    /// Read the secret from a counterparty's claim transaction
    ExtractSecret {
        /// Redeem script of the claimed contract, hex
        #[arg(long)]
        redeem_script: String,
        /// The claim transaction, hex
        #[arg(long)]
        transaction: String,
    },
}

/// Run a `bch-trade` subcommand and print its JSON result.
pub fn execute(command: &TradeCommand) -> Result<(), Box<dyn Error>> {
    let output = match command {
        TradeCommand::Contract {
            chain,
            secret_hash,
            claim_address,
            refund_address,
            refund_height,
        } => contract(
            *chain,
            secret_hash,
            claim_address,
            refund_address,
            *refund_height,
        )?,
        TradeCommand::Inspect {
            chain,
            redeem_script,
        } => inspect(*chain, redeem_script)?,
        TradeCommand::SecretHash { secret_file } => {
            let secret = read_secret(secret_file.as_deref())?;
            json!({ "secret_hash": secret_hash(&secret).to_lower_hex_string() })
        }
        TradeCommand::Lock {
            chain,
            redeem_script,
            coins,
            amount,
            change_address,
            fee_rate,
            wif_file,
        } => {
            let key = read_key(wif_file.as_deref(), chain.network)?;
            lock(
                *chain,
                redeem_script,
                coins,
                *amount,
                change_address,
                *fee_rate,
                &key,
            )?
        }
        TradeCommand::Claim {
            chain,
            redeem_script,
            coin,
            to,
            fee_rate,
            wif_file,
            secret_file,
        } => {
            let key = read_key(wif_file.as_deref(), chain.network)?;
            let secret = read_secret(secret_file.as_deref())?;
            claim(*chain, redeem_script, coin, to, *fee_rate, &key, &secret)?
        }
        TradeCommand::Refund {
            chain,
            redeem_script,
            coin,
            to,
            fee_rate,
            wif_file,
        } => {
            let key = read_key(wif_file.as_deref(), chain.network)?;
            refund(*chain, redeem_script, coin, to, *fee_rate, &key)?
        }
        TradeCommand::ExtractSecret {
            redeem_script,
            transaction,
        } => extract_secret(redeem_script, transaction)?,
    };
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

/// The contract for agreed terms, as JSON.
fn contract(
    chain: ChainArgs,
    secret_hash_hex: &str,
    claim_address: &str,
    refund_address: &str,
    refund_height: u32,
) -> Result<Value, Box<dyn Error>> {
    let secret_hash: [u8; 32] = <[u8; 32]>::from_hex(secret_hash_hex)
        .map_err(|error| format!("--secret-hash is not 32 bytes of hex: {error}"))?;
    let contract = HtlcContract::new(
        secret_hash,
        pubkey_hash(chain, claim_address)?,
        pubkey_hash(chain, refund_address)?,
        refund_height,
    )?;
    Ok(describe(chain, &contract))
}

/// The terms of a redeem script, as JSON.
fn inspect(chain: ChainArgs, redeem_script_hex: &str) -> Result<Value, Box<dyn Error>> {
    Ok(describe(chain, &parse_contract(redeem_script_hex)?))
}

/// Everything a party needs to check or fund a contract.
fn describe(chain: ChainArgs, contract: &HtlcContract) -> Value {
    let locking_script = contract.locking_script();
    let dust = dust_threshold(&bitcoin::TxOut {
        value: Amount::ZERO,
        script_pubkey: locking_script.clone(),
    });
    let address = match chain.chain {
        TradeChain::Bch => Value::String(encode_p2sh32_cashaddr(
            chain.network.cashaddr_prefix(),
            &contract.script_hash(),
        )),
        TradeChain::Share => Value::Null,
    };
    json!({
        "chain": chain_name(chain.chain),
        "secret_hash": contract.secret_hash.to_lower_hex_string(),
        "claim_address": address_text(chain, &contract.claim_pubkey_hash),
        "refund_address": address_text(chain, &contract.refund_pubkey_hash),
        "refund_height": contract.refund_height,
        "redeem_script": contract.redeem_script().to_lower_hex_string(),
        "locking_script": locking_script.as_bytes().to_lower_hex_string(),
        "address": address,
        "minimum_amount": dust,
    })
}

/// Sign a lock of `key`'s P2PKH coins into the contract.
fn lock(
    chain: ChainArgs,
    redeem_script_hex: &str,
    coins: &[String],
    amount: u64,
    change_address: &str,
    fee_rate: Option<u64>,
    key: &TradeKey,
) -> Result<Value, Box<dyn Error>> {
    let contract = parse_contract(redeem_script_hex)?;
    let coins = coins
        .iter()
        .map(|coin| parse_coin(coin))
        .collect::<Result<Vec<_>, _>>()?;
    let change_script = destination_script(chain, change_address)?;
    let transaction = build_lock_transaction(
        &coins,
        key,
        &contract,
        Amount::from_sat(amount),
        &change_script,
        fee_rate.unwrap_or(default_fee_rate(chain.chain)),
    )?;
    let spent: u64 = coins.iter().map(|coin| coin.value.to_sat()).sum();
    let txid = transaction.compute_txid();
    Ok(json!({
        "txid": txid.to_string(),
        "hex": serialize_hex(&transaction),
        "contract_coin": format!("{txid}:0:{amount}"),
        "fee": spent - total_output(&transaction),
    }))
}

/// Sign a claim of the contract's coin with the secret.
fn claim(
    chain: ChainArgs,
    redeem_script_hex: &str,
    coin: &str,
    to: &str,
    fee_rate: Option<u64>,
    key: &TradeKey,
    secret: &[u8; SECRET_LENGTH],
) -> Result<Value, Box<dyn Error>> {
    let contract = parse_contract(redeem_script_hex)?;
    let coin = parse_coin(coin)?;
    let transaction = build_claim_transaction(
        &coin,
        &contract,
        key,
        secret,
        &destination_script(chain, to)?,
        fee_rate.unwrap_or(default_fee_rate(chain.chain)),
    )?;
    Ok(spend_summary(&transaction, coin))
}

/// Sign a refund of the contract's coin.
fn refund(
    chain: ChainArgs,
    redeem_script_hex: &str,
    coin: &str,
    to: &str,
    fee_rate: Option<u64>,
    key: &TradeKey,
) -> Result<Value, Box<dyn Error>> {
    let contract = parse_contract(redeem_script_hex)?;
    let coin = parse_coin(coin)?;
    let transaction = build_refund_transaction(
        &coin,
        &contract,
        key,
        &destination_script(chain, to)?,
        fee_rate.unwrap_or(default_fee_rate(chain.chain)),
    )?;
    let mut summary = spend_summary(&transaction, coin);
    summary["valid_from_height"] = json!(contract.refund_height + 1);
    Ok(summary)
}

/// The secret a claim of the contract revealed, as JSON.
fn extract_secret(redeem_script_hex: &str, transaction_hex: &str) -> Result<Value, Box<dyn Error>> {
    let contract = parse_contract(redeem_script_hex)?;
    let transaction: Transaction = deserialize_hex(transaction_hex.trim())
        .map_err(|error| format!("--transaction is not a transaction in hex: {error}"))?;
    let secret = contract
        .find_secret(&transaction)
        .ok_or("the transaction does not claim this contract with its secret")?;
    Ok(json!({ "secret": secret.to_lower_hex_string() }))
}

fn spend_summary(transaction: &Transaction, coin: Coin) -> Value {
    json!({
        "txid": transaction.compute_txid().to_string(),
        "hex": serialize_hex(transaction),
        "value": total_output(transaction),
        "fee": coin.value.to_sat() - total_output(transaction),
    })
}

fn total_output(transaction: &Transaction) -> u64 {
    transaction
        .output
        .iter()
        .map(|output| output.value.to_sat())
        .sum()
}

fn default_fee_rate(chain: TradeChain) -> u64 {
    match chain {
        TradeChain::Share => DEFAULT_SHARE_FEE_PER_BYTE,
        TradeChain::Bch => DEFAULT_BCH_FEE_PER_BYTE,
    }
}

fn chain_name(chain: TradeChain) -> &'static str {
    match chain {
        TradeChain::Share => "share",
        TradeChain::Bch => "bch",
    }
}

fn parse_contract(redeem_script_hex: &str) -> Result<HtlcContract, Box<dyn Error>> {
    let bytes = Vec::<u8>::from_hex(redeem_script_hex.trim())
        .map_err(|error| format!("--redeem-script is not hex: {error}"))?;
    Ok(HtlcContract::from_redeem_script(&bytes)
        .ok_or("--redeem-script is not a share-trading contract")?)
}

/// Parse `txid:vout:satoshis`.
fn parse_coin(text: &str) -> Result<Coin, Box<dyn Error>> {
    let mut parts = text.trim().split(':');
    let (Some(txid), Some(vout), Some(value), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(format!("coin '{text}' is not txid:vout:satoshis").into());
    };
    Ok(Coin {
        outpoint: OutPoint::new(Txid::from_str(txid)?, vout.parse()?),
        value: Amount::from_sat(value.parse()?),
    })
}

/// The 20-byte key hash an address on `chain` names. The share chain takes a
/// share address and Bitcoin Cash a P2PKH CashAddr, so an address meant for
/// one chain cannot be used on the other by mistake.
fn pubkey_hash(chain: ChainArgs, address: &str) -> Result<[u8; 20], Box<dyn Error>> {
    match chain.chain {
        TradeChain::Share => Ok(bch_share::parse(address, chain.network.share_network())?.hash),
        TradeChain::Bch => {
            let decoded = parse_payout_address(
                address,
                Chain::BitcoinCash,
                chain.network.legacy_network(),
                Some(chain.network),
            )?;
            let script = decoded.script_pubkey();
            if !script.is_p2pkh() {
                return Err(format!("{address} is not a P2PKH CashAddr").into());
            }
            Ok(script.as_bytes()[3..23]
                .try_into()
                .expect("a P2PKH script holds a 20-byte hash"))
        }
    }
}

fn destination_script(chain: ChainArgs, address: &str) -> Result<ScriptBuf, Box<dyn Error>> {
    Ok(ScriptBuf::new_p2pkh(&PubkeyHash::from_byte_array(
        pubkey_hash(chain, address)?,
    )))
}

fn address_text(chain: ChainArgs, hash: &[u8; 20]) -> String {
    match chain.chain {
        TradeChain::Share => {
            bch_share::BchShareAddress::new(chain.network.share_network(), *hash).encode()
        }
        TradeChain::Bch => bch_share::encode_cashaddr(
            chain.network.cashaddr_prefix(),
            bch_share::CashAddrType::PayToPublicKeyHash,
            hash,
        ),
    }
}

/// Read a WIF key from `file`, or from a hidden prompt.
fn read_key(file: Option<&Path>, network: BitcoinCashNetwork) -> Result<TradeKey, Box<dyn Error>> {
    let text = match file {
        Some(path) => std::fs::read_to_string(path)?,
        None => rpassword::prompt_password("WIF private key: ")?,
    };
    parse_key(&text, network)
}

/// Parse a WIF key and require its network kind to match `network`.
fn parse_key(text: &str, network: BitcoinCashNetwork) -> Result<TradeKey, Box<dyn Error>> {
    let private_key =
        PrivateKey::from_wif(text.trim()).map_err(|_| "the key is not a WIF private key")?;
    let expected = match network {
        BitcoinCashNetwork::Mainnet => NetworkKind::Main,
        BitcoinCashNetwork::Chipnet | BitcoinCashNetwork::Regtest => NetworkKind::Test,
    };
    if private_key.network != expected {
        return Err("the WIF key is for another network".into());
    }
    Ok(TradeKey::new(private_key))
}

/// Read a 32-byte secret in hex from `file`, or from a hidden prompt.
fn read_secret(file: Option<&Path>) -> Result<[u8; SECRET_LENGTH], Box<dyn Error>> {
    let text = match file {
        Some(path) => std::fs::read_to_string(path)?,
        None => rpassword::prompt_password("Secret (64 hex characters): ")?,
    };
    parse_secret(&text)
}

fn parse_secret(text: &str) -> Result<[u8; SECRET_LENGTH], Box<dyn Error>> {
    Ok(<[u8; SECRET_LENGTH]>::from_hex(text.trim())
        .map_err(|_| "the secret is not 32 bytes of hex")?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use p2poolv2_lib::shares::bch_script::{verify_outputs, verify_transaction};

    /// Fixed test keys, spelled out so no test generates one.
    const SELLER_SECRET_KEY: [u8; 32] = [0x5e; 32];
    const BUYER_SECRET_KEY: [u8; 32] = [0xb7; 32];
    const SECRET: [u8; 32] = [0x42; 32];

    #[test]
    fn a_coin_is_txid_vout_and_satoshis() {
        let coin = parse_coin(
            "1111111111111111111111111111111111111111111111111111111111111111:3:40000000",
        )
        .unwrap();
        assert_eq!(coin.outpoint.vout, 3);
        assert_eq!(coin.value, Amount::from_sat(40_000_000));
        assert!(parse_coin("1111:3").is_err());
        assert!(
            parse_coin(
                "1111111111111111111111111111111111111111111111111111111111111111:3:40000000:1"
            )
            .is_err()
        );
    }

    #[test]
    fn a_bch_contract_prints_its_p2sh32_address_and_reads_back() {
        let chain = ChainArgs {
            chain: TradeChain::Bch,
            network: BitcoinCashNetwork::Chipnet,
        };
        let seller =
            TradeKey::new(PrivateKey::from_slice(&SELLER_SECRET_KEY, NetworkKind::Test).unwrap());
        let buyer =
            TradeKey::new(PrivateKey::from_slice(&BUYER_SECRET_KEY, NetworkKind::Test).unwrap());
        let seller_address = address_text(chain, &seller.pubkey_hash());
        let buyer_address = address_text(chain, &buyer.pubkey_hash());
        assert!(seller_address.starts_with("bchtest:q"));

        let described = contract(
            chain,
            &secret_hash(&SECRET).to_lower_hex_string(),
            &seller_address,
            &buyer_address,
            300_000,
        )
        .unwrap();
        assert!(
            described["address"]
                .as_str()
                .unwrap()
                .starts_with("bchtest:p")
        );
        assert_eq!(described["claim_address"], seller_address);
        assert_eq!(described["refund_address"], buyer_address);
        assert_eq!(described["minimum_amount"], 576);
        let inspected = inspect(chain, described["redeem_script"].as_str().unwrap()).unwrap();
        assert_eq!(inspected, described);
    }

    #[test]
    fn a_share_contract_needs_share_addresses() {
        let chain = ChainArgs {
            chain: TradeChain::Share,
            network: BitcoinCashNetwork::Chipnet,
        };
        let bch_chain = ChainArgs {
            chain: TradeChain::Bch,
            network: BitcoinCashNetwork::Chipnet,
        };
        let buyer =
            TradeKey::new(PrivateKey::from_slice(&BUYER_SECRET_KEY, NetworkKind::Test).unwrap());
        let seller =
            TradeKey::new(PrivateKey::from_slice(&SELLER_SECRET_KEY, NetworkKind::Test).unwrap());
        let buyer_bch_address = address_text(bch_chain, &buyer.pubkey_hash());
        let seller_share_address = address_text(chain, &seller.pubkey_hash());
        assert!(seller_share_address.starts_with("tp2pool:q"));

        let error = contract(
            chain,
            &secret_hash(&SECRET).to_lower_hex_string(),
            &buyer_bch_address,
            &seller_share_address,
            9_000,
        )
        .unwrap_err();
        assert!(error.to_string().contains("not a share address"), "{error}");
    }

    #[test]
    fn a_wif_key_for_another_network_is_refused() {
        let wif = PrivateKey::from_slice(&SELLER_SECRET_KEY, NetworkKind::Test)
            .unwrap()
            .to_wif();
        assert!(parse_key(&wif, BitcoinCashNetwork::Chipnet).is_ok());
        let error = parse_key(&wif, BitcoinCashNetwork::Mainnet).err().unwrap();
        assert!(error.to_string().contains("another network"), "{error}");
    }

    #[test]
    fn a_secret_is_exactly_32_bytes_of_hex() {
        assert_eq!(
            parse_secret(&format!("{}\n", SECRET.to_lower_hex_string())).unwrap(),
            SECRET
        );
        assert!(parse_secret("42").is_err());
    }

    /// The commands a seller and a buyer run for one trade, end to end, each
    /// result checked by the share-chain interpreter.
    #[test]
    fn the_commands_build_a_whole_trade() {
        let share = ChainArgs {
            chain: TradeChain::Share,
            network: BitcoinCashNetwork::Chipnet,
        };
        let bch = ChainArgs {
            chain: TradeChain::Bch,
            network: BitcoinCashNetwork::Chipnet,
        };
        let seller =
            TradeKey::new(PrivateKey::from_slice(&SELLER_SECRET_KEY, NetworkKind::Test).unwrap());
        let buyer =
            TradeKey::new(PrivateKey::from_slice(&BUYER_SECRET_KEY, NetworkKind::Test).unwrap());
        let hash = secret_hash(&SECRET).to_lower_hex_string();

        // Share chain: the seller locks 0.4 share for the buyer.
        let share_contract = contract(
            share,
            &hash,
            &address_text(share, &buyer.pubkey_hash()),
            &address_text(share, &seller.pubkey_hash()),
            20_000,
        )
        .unwrap();
        let share_redeem = share_contract["redeem_script"].as_str().unwrap();
        let share_coin =
            "2222222222222222222222222222222222222222222222222222222222222222:0:100000000";
        let share_lock = lock(
            share,
            share_redeem,
            &[share_coin.to_string()],
            40_000_000,
            &address_text(share, &seller.pubkey_hash()),
            None,
            &seller,
        )
        .unwrap();
        assert_eq!(share_lock["fee"], 0);
        let share_lock_transaction: Transaction =
            deserialize_hex(share_lock["hex"].as_str().unwrap()).unwrap();
        assert_eq!(verify_outputs(&share_lock_transaction), Ok(()));

        // Bitcoin Cash: the buyer funds the BCH contract (here from a
        // P2PKH coin), and the seller claims it with the secret.
        let bch_contract = contract(
            bch,
            &hash,
            &address_text(bch, &seller.pubkey_hash()),
            &address_text(bch, &buyer.pubkey_hash()),
            300_000,
        )
        .unwrap();
        let bch_redeem = bch_contract["redeem_script"].as_str().unwrap();
        let bch_lock = lock(
            bch,
            bch_redeem,
            &[
                "3333333333333333333333333333333333333333333333333333333333333333:1:80000"
                    .to_string(),
            ],
            50_000,
            &address_text(bch, &buyer.pubkey_hash()),
            None,
            &buyer,
        )
        .unwrap();
        let bch_claim = claim(
            bch,
            bch_redeem,
            bch_lock["contract_coin"].as_str().unwrap(),
            &address_text(bch, &seller.pubkey_hash()),
            None,
            &seller,
            &SECRET,
        )
        .unwrap();
        let bch_claim_transaction: Transaction =
            deserialize_hex(bch_claim["hex"].as_str().unwrap()).unwrap();
        assert_eq!(
            verify_transaction(
                &bch_claim_transaction,
                &[(
                    0,
                    bitcoin::TxOut {
                        value: Amount::from_sat(50_000),
                        script_pubkey: ScriptBuf::from_hex(
                            bch_contract["locking_script"].as_str().unwrap()
                        )
                        .unwrap(),
                    },
                )],
            ),
            Ok(())
        );

        // The buyer reads the secret from the BCH claim and claims the shares.
        let revealed = extract_secret(bch_redeem, bch_claim["hex"].as_str().unwrap()).unwrap();
        let revealed_secret = parse_secret(revealed["secret"].as_str().unwrap()).unwrap();
        let share_claim = claim(
            share,
            share_redeem,
            share_lock["contract_coin"].as_str().unwrap(),
            &address_text(share, &buyer.pubkey_hash()),
            None,
            &buyer,
            &revealed_secret,
        )
        .unwrap();
        assert_eq!(share_claim["value"], 40_000_000);
        let share_claim_transaction: Transaction =
            deserialize_hex(share_claim["hex"].as_str().unwrap()).unwrap();
        assert_eq!(
            verify_transaction(
                &share_claim_transaction,
                &[(
                    0,
                    bitcoin::TxOut {
                        value: Amount::from_sat(40_000_000),
                        script_pubkey: ScriptBuf::from_hex(
                            share_contract["locking_script"].as_str().unwrap()
                        )
                        .unwrap(),
                    },
                )],
            ),
            Ok(())
        );
    }

    #[test]
    fn a_refund_reports_the_first_height_it_is_valid_at() {
        let share = ChainArgs {
            chain: TradeChain::Share,
            network: BitcoinCashNetwork::Regtest,
        };
        let seller =
            TradeKey::new(PrivateKey::from_slice(&SELLER_SECRET_KEY, NetworkKind::Test).unwrap());
        let buyer =
            TradeKey::new(PrivateKey::from_slice(&BUYER_SECRET_KEY, NetworkKind::Test).unwrap());
        let share_contract = contract(
            share,
            &secret_hash(&SECRET).to_lower_hex_string(),
            &address_text(share, &buyer.pubkey_hash()),
            &address_text(share, &seller.pubkey_hash()),
            20_000,
        )
        .unwrap();
        let refunded = refund(
            share,
            share_contract["redeem_script"].as_str().unwrap(),
            "4444444444444444444444444444444444444444444444444444444444444444:0:40000000",
            &address_text(share, &seller.pubkey_hash()),
            None,
            &seller,
        )
        .unwrap();
        assert_eq!(refunded["valid_from_height"], 20_001);
        assert_eq!(refunded["value"], 40_000_000);
    }
}
