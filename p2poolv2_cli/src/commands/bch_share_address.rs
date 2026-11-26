// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Encode a Bitcoin Cash P2PKH key hash as a share chain address.
//!
//! The input is a P2PKH CashAddr from the miner's own wallet, naming the key
//! that will own their shares. The share coinbase pays that same P2PKH
//! script, so the wallet that holds the key can spend it. Only the prefix
//! changes (`bitcoincash:` to `p2pool:`, `bchtest:` to `tp2pool:`, `bchreg:`
//! to `rp2pool:`), which keeps the two strings from being mistaken for each
//! other.
//!
//! The node never does this on its own: the share address and the payout
//! address are separate settings, and the miner chooses which key owns
//! shares. A fresh wallet address, rather than the payout address, keeps the
//! two roles apart.
//!
//! Deliberately takes an address and never a private key: no P2Poolv2 crate
//! generates private keys, and only `bch-trade` reads one, to sign a trade.

use p2poolv2_lib::config::{BitcoinCashNetwork, payout_to_share_address};
use std::error::Error;
use std::io::Read;

/// Parse a Bitcoin Cash network name as the config spells it.
///
/// Used as a clap value parser, so the error names the accepted values.
pub fn parse_network(value: &str) -> Result<BitcoinCashNetwork, String> {
    match value {
        "mainnet" | "main" => Ok(BitcoinCashNetwork::Mainnet),
        "chipnet" => Ok(BitcoinCashNetwork::Chipnet),
        "regtest" => Ok(BitcoinCashNetwork::Regtest),
        "scalenet" => Ok(BitcoinCashNetwork::Scalenet),
        _ => Err(format!(
            "unknown Bitcoin Cash network '{value}', expected mainnet, chipnet, scalenet or regtest"
        )),
    }
}

/// Build the share chain address for a P2PKH CashAddr on `network`.
///
/// The rule itself lives in [`payout_to_share_address`], shared with the
/// dashboard's help page; this wrapper keeps the CLI's error type. A P2SH
/// CashAddr is rejected: a share is owned by a P2PKH script.
fn encode(cash_address: &str, network: BitcoinCashNetwork) -> Result<String, Box<dyn Error>> {
    Ok(payout_to_share_address(cash_address, network)?)
}

/// Read the CashAddr from the argument, or from stdin when none was given.
///
/// Trims before the empty check so a blank line or an empty pipe gets the
/// friendly message rather than a parse error; [`payout_to_share_address`]
/// trims as well, so trimming here never changes what converts.
fn read_cash_address(cash_address: Option<String>) -> Result<String, Box<dyn Error>> {
    let raw = match cash_address {
        Some(value) => value,
        None => {
            let mut buffer = String::new();
            std::io::stdin().read_to_string(&mut buffer)?;
            buffer
        }
    };

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("no CashAddr given. Pass one as an argument or on stdin.".into());
    }
    Ok(trimmed.to_string())
}

/// Execute the command, printing the share chain address alone so it can be
/// piped or copied into `[stratum] miner_address` or a miner's `p2p=`.
pub fn execute(
    cash_address: Option<String>,
    network: BitcoinCashNetwork,
) -> Result<(), Box<dyn Error>> {
    let cash_address = read_cash_address(cash_address)?;
    println!("{}", encode(&cash_address, network)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use p2poolv2_lib::address::bch_share;

    #[test]
    fn mainnet_p2pkh_keeps_its_hash_under_the_share_prefix() {
        let encoded = encode(
            "bitcoincash:qr6m7j9njldwwzlg9v7v53unlr4jkmx6eylep8ekg2",
            BitcoinCashNetwork::Mainnet,
        )
        .unwrap();
        assert!(encoded.starts_with("p2pool:q"));
        let decoded =
            bch_share::parse(&encoded, BitcoinCashNetwork::Mainnet.share_network()).unwrap();
        assert_eq!(
            decoded.hash,
            [
                0xf5, 0xbf, 0x48, 0xb3, 0x97, 0xda, 0xe7, 0x0b, 0xe8, 0x2b, 0x3c, 0xca, 0x47, 0x93,
                0xf8, 0xeb, 0x2b, 0x6c, 0xda, 0xc9,
            ]
        );
    }

    #[test]
    fn regtest_p2pkh_encodes_under_rp2pool() {
        let encoded = encode(
            "bchreg:qrr4pe4ret3zwmj0428rtksq83ql80w6myqjlk828q",
            BitcoinCashNetwork::Regtest,
        )
        .unwrap();
        assert_eq!(
            encoded,
            "rp2pool:qrr4pe4ret3zwmj0428rtksq83ql80w6my7w2wp75h"
        );
    }

    /// A token-aware CashAddr names the same key as its plain form, so both
    /// give the same share address (CashTokens test vectors).
    #[test]
    fn a_token_aware_p2pkh_encodes_the_same_share_address_as_its_plain_form() {
        let token_aware = encode(
            "bchtest:zr7fzmep8g7h7ymfxy74lgc0v950j3r295x8qj2hgs",
            BitcoinCashNetwork::Chipnet,
        )
        .unwrap();
        let plain = encode(
            "bchtest:qr7fzmep8g7h7ymfxy74lgc0v950j3r295pdnvy3hr",
            BitcoinCashNetwork::Chipnet,
        )
        .unwrap();
        assert_eq!(token_aware, plain);
        assert!(token_aware.starts_with("tp2pool:q"), "{token_aware}");
    }

    /// Piped input arrives with its trailing newline, so the stdin path's
    /// trim is what converts it. Pinning it keeps the CLI honest about the
    /// widget's whitespace behavior, which the endpoint tests pin on that
    /// side.
    #[test]
    fn a_cash_addr_read_from_stdin_is_trimmed_before_conversion() {
        let raw = "bchtest:qr7fzmep8g7h7ymfxy74lgc0v950j3r295pdnvy3hr\n";
        let read = read_cash_address(Some(raw.to_string())).unwrap();
        let address = encode(&read, BitcoinCashNetwork::Chipnet).unwrap();
        assert!(address.starts_with("tp2pool:q"), "{address}");
    }

    /// An empty or blank read is refused with the friendly message rather
    /// than a parse error about an empty address.
    #[test]
    fn a_blank_read_is_refused_before_the_parser_sees_it() {
        let error = read_cash_address(Some("   \n".to_string())).unwrap_err();
        assert!(error.to_string().contains("no CashAddr given"), "{error}");
    }

    #[test]
    fn p2sh_cashaddr_is_rejected() {
        let error = encode(
            "bchtest:pr6m7j9njldwwzlg9v7v53unlr4jkmx6eyvwc0uz5t",
            BitcoinCashNetwork::Chipnet,
        )
        .unwrap_err();
        assert!(error.to_string().contains("P2PKH"), "{error}");
    }

    #[test]
    fn cashaddr_for_another_network_is_rejected() {
        assert!(
            encode(
                "bitcoincash:qr6m7j9njldwwzlg9v7v53unlr4jkmx6eylep8ekg2",
                BitcoinCashNetwork::Chipnet,
            )
            .is_err()
        );
    }

    #[test]
    fn network_names_match_the_config() {
        assert_eq!(parse_network("mainnet"), Ok(BitcoinCashNetwork::Mainnet));
        assert_eq!(parse_network("chipnet"), Ok(BitcoinCashNetwork::Chipnet));
        assert_eq!(parse_network("regtest"), Ok(BitcoinCashNetwork::Regtest));
        assert_eq!(parse_network("scalenet"), Ok(BitcoinCashNetwork::Scalenet));
        assert!(parse_network("signet").is_err());
    }
}
