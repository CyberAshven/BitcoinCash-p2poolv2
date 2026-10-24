// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::config::{BitcoinCashNetwork, Chain, parse_payout_address};
use bitcoin::Address;
use p2poolv2_wallet::bch_share::encode_payout_address;
use std::borrow::Cow;

/// Max username includes the dot and the worker name
/// btcaddress.workername, with btcaddress max at 62 bytes, we get 66 character worker name
/// npubs are 63 characters, we still get enough room for workername
const MAX_USERNAME_LENGTH: usize = 128;

/// Result of a successful username validation.
#[derive(Debug)]
pub struct ValidatedUsername<'a> {
    /// The key this miner is stored and counted under: the canonical text of
    /// the decoded payout script, so every spelling of one address (with or
    /// without the CashAddr prefix, upper or lower case, token-aware or not)
    /// is one miner. The address text as typed when addresses are not
    /// validated.
    pub user_key: Cow<'a, str>,
    /// Parsed and network-checked bitcoin Address (None when validation is skipped)
    pub parsed_address: Option<Address>,
    /// Optional worker name after the first dot
    pub worker_name: Option<&'a str>,
}

#[derive(Debug, thiserror::Error)]
pub enum UsernameValidationError {
    #[error("Invalid Bitcoin address: {0}")]
    InvalidAddress(String),
    #[error("Worker name too long (max {0} characters)")]
    UserNameTooLong(usize),
    #[error("No username provided. Use <username>.<workername>")]
    UserNameMissing(),
}

/// Validates a stratum username in the format <btcaddress>.<workername>
///
/// # Arguments
///
/// * `username` - The username to validate
/// * `validate_address` - Whether to parse and validate the bitcoin address
/// * `network` - The Bitcoin network to validate the address against
///
/// # Returns
///
/// * `Ok(ValidatedUsername)` - Parsed address string, checked Address, and worker name
/// * `Err(UsernameValidationError)` - Error if validation fails
pub fn validate(
    username: &str,
    validate_address: bool,
    network: bitcoin::Network,
) -> Result<ValidatedUsername<'_>, UsernameValidationError> {
    let (address_part, worker_name) = split_username(username)?;

    let parsed_address = if validate_address {
        let address = address_part.parse::<bitcoin::Address<_>>().map_err(|e| {
            UsernameValidationError::InvalidAddress(format!("Failed to parse address: {e}"))
        })?;

        // Verify the network, return error on failure
        let checked = address.require_network(network).map_err(|_| {
            UsernameValidationError::InvalidAddress(format!(
                "Expected an address for network {network}",
            ))
        })?;
        Some(checked)
    } else {
        None
    };

    // Bitcoin's own spelling of the address: lowercase bech32 or base58.
    let user_key = match &parsed_address {
        Some(address) => Cow::Owned(address.to_string()),
        None => Cow::Borrowed(address_part),
    };
    Ok(ValidatedUsername {
        user_key,
        parsed_address,
        worker_name,
    })
}

/// Validates a Bitcoin Cash stratum username, `<cashaddr>.<workername>`.
///
/// The address is a 20-byte P2PKH or P2SH CashAddr for `cash_network`. Its
/// prefix (`bitcoincash:`, `bchtest:`, `bchreg:`) may be left out, as BCH
/// miners commonly enter it; the network's prefix is then assumed and the
/// checksum still has to match it. `user_key` is the address as a plain
/// (not token-aware) CashAddr with its prefix, lowercase.
pub fn validate_bitcoin_cash(
    username: &str,
    validate_address: bool,
    cash_network: BitcoinCashNetwork,
) -> Result<ValidatedUsername<'_>, UsernameValidationError> {
    let (address_part, worker_name) = split_username(username)?;

    let parsed_address = if validate_address {
        let parsed = if address_part.contains(':') {
            parse_cash_payout(address_part, cash_network)
        } else {
            let prefixed = format!("{}:{address_part}", cash_network.cashaddr_prefix());
            parse_cash_payout(&prefixed, cash_network)
        }?;
        Some(parsed)
    } else {
        None
    };

    let user_key = match &parsed_address {
        Some(address) => encode_payout_address(address, cash_network.share_network())
            .map_or(Cow::Borrowed(address_part), Cow::Owned),
        None => Cow::Borrowed(address_part),
    };
    Ok(ValidatedUsername {
        user_key,
        parsed_address,
        worker_name,
    })
}

/// Validate `username` for the chain this node mines.
///
/// A Bitcoin Cash node with no Bitcoin Cash network cannot parse a payout
/// address, so its usernames are rejected rather than read as Bitcoin.
pub fn validate_for_chain(
    username: &str,
    validate_address: bool,
    network: bitcoin::Network,
    chain: Chain,
    cash_network: Option<BitcoinCashNetwork>,
) -> Result<ValidatedUsername<'_>, UsernameValidationError> {
    match (chain, cash_network) {
        (Chain::Bitcoin, _) => validate(username, validate_address, network),
        (Chain::BitcoinCash, Some(cash_network)) => {
            validate_bitcoin_cash(username, validate_address, cash_network)
        }
        (Chain::BitcoinCash, None) => Err(UsernameValidationError::InvalidAddress(
            "Bitcoin Cash usernames need bitcoin_cash_network".to_string(),
        )),
    }
}

fn parse_cash_payout(
    address: &str,
    cash_network: BitcoinCashNetwork,
) -> Result<Address, UsernameValidationError> {
    parse_payout_address(
        address,
        Chain::BitcoinCash,
        cash_network.legacy_network(),
        Some(cash_network),
    )
    .map_err(|error| UsernameValidationError::InvalidAddress(error.message))
}

/// Split `<address>.<worker>` at the first dot, enforcing the length limit.
fn split_username(username: &str) -> Result<(&str, Option<&str>), UsernameValidationError> {
    if username.is_empty() {
        return Err(UsernameValidationError::UserNameMissing());
    }

    if username.len() > MAX_USERNAME_LENGTH {
        return Err(UsernameValidationError::UserNameTooLong(
            MAX_USERNAME_LENGTH,
        ));
    }

    match username.split_once('.') {
        Some((address_part, worker_name)) => Ok((address_part, Some(worker_name))),
        None => Ok((username, None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::Network;

    #[test]
    fn test_valid_address_no_worker() {
        let testnet_address = "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx";
        let result = validate(testnet_address, true, Network::Testnet);
        assert!(result.is_ok());
        let validated = result.unwrap();
        assert_eq!(validated.user_key, testnet_address);
        assert!(validated.parsed_address.is_some());
        assert_eq!(validated.worker_name, None);
    }

    #[test]
    fn test_valid_address_with_worker() {
        let testnet_address = "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx.worker1";
        let result = validate(testnet_address, true, Network::Testnet);
        assert!(result.is_ok());
        let validated = result.unwrap();
        assert_eq!(
            validated.user_key,
            "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx"
        );
        assert!(validated.parsed_address.is_some());
        assert_eq!(validated.worker_name, Some("worker1"));
    }

    #[test]
    fn test_invalid_address() {
        let invalid_address = "not_a_bitcoin_address";
        let result = validate(invalid_address, true, Network::Bitcoin);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            UsernameValidationError::InvalidAddress(_)
        ));
    }

    #[test]
    fn test_wrong_network() {
        // Using a testnet address on mainnet
        let testnet_address = "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx";
        let result = validate(testnet_address, true, Network::Bitcoin);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            UsernameValidationError::InvalidAddress(_)
        ));
    }

    #[test]
    fn test_worker_name_too_long() {
        let mainnet_address = "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa";
        let long_worker = format!("{}.{}", mainnet_address, "a".repeat(100));
        let result = validate(&long_worker, true, Network::Bitcoin);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            UsernameValidationError::UserNameTooLong(MAX_USERNAME_LENGTH)
        ));
    }

    #[test]
    fn test_worker_name_max_length() {
        let mainnet_address = "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa";
        let max_worker = format!("{}.{}", mainnet_address, "a".repeat(29));
        let result = validate(&max_worker, true, Network::Bitcoin);
        assert!(result.is_ok());
    }

    #[test]
    fn test_multiple_dots_in_username() {
        let mainnet_address = "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa";
        let multiple_dots = format!("{mainnet_address}.worker.with.dots");
        let result = validate(&multiple_dots, true, Network::Bitcoin);
        assert!(result.is_ok());
        let validated = result.unwrap();
        assert_eq!(validated.user_key, mainnet_address);
        assert!(validated.parsed_address.is_some());
        assert_eq!(validated.worker_name, Some("worker.with.dots"));
    }

    #[test]
    fn bitcoin_cash_accepts_a_prefixed_cashaddr_with_a_worker() {
        let result = validate_bitcoin_cash(
            "bchreg:qzh5k63rhy4jg6jtyssmzddju5kkng4jzu8ktcypfm.rig1",
            true,
            BitcoinCashNetwork::Regtest,
        )
        .unwrap();
        assert_eq!(
            result.user_key,
            "bchreg:qzh5k63rhy4jg6jtyssmzddju5kkng4jzu8ktcypfm"
        );
        assert_eq!(result.worker_name, Some("rig1"));
        assert!(result.parsed_address.unwrap().script_pubkey().is_p2pkh());
    }

    #[test]
    fn bitcoin_cash_accepts_a_cashaddr_without_its_prefix() {
        let result = validate_bitcoin_cash(
            "qr6m7j9njldwwzlg9v7v53unlr4jkmx6eylep8ekg2",
            true,
            BitcoinCashNetwork::Mainnet,
        )
        .unwrap();
        assert_eq!(
            result.user_key,
            "bitcoincash:qr6m7j9njldwwzlg9v7v53unlr4jkmx6eylep8ekg2"
        );
        assert!(result.parsed_address.unwrap().script_pubkey().is_p2pkh());
    }

    /// Statistics are keyed by the decoded script, so one address typed
    /// without its prefix, in upper case or token-aware is one miner.
    #[test]
    fn every_spelling_of_one_cashaddr_has_the_same_user_key() {
        let prefixed = validate_bitcoin_cash(
            "bchtest:qr7fzmep8g7h7ymfxy74lgc0v950j3r295pdnvy3hr.a",
            true,
            BitcoinCashNetwork::Chipnet,
        )
        .unwrap();
        let bare = validate_bitcoin_cash(
            "qr7fzmep8g7h7ymfxy74lgc0v950j3r295pdnvy3hr.b",
            true,
            BitcoinCashNetwork::Chipnet,
        )
        .unwrap();
        let upper = validate_bitcoin_cash(
            "BCHTEST:QR7FZMEP8G7H7YMFXY74LGC0V950J3R295PDNVY3HR.c",
            true,
            BitcoinCashNetwork::Chipnet,
        )
        .unwrap();
        let token_aware = validate_bitcoin_cash(
            "bchtest:zr7fzmep8g7h7ymfxy74lgc0v950j3r295x8qj2hgs.d",
            true,
            BitcoinCashNetwork::Chipnet,
        )
        .unwrap();

        let key = "bchtest:qr7fzmep8g7h7ymfxy74lgc0v950j3r295pdnvy3hr";
        assert_eq!(prefixed.user_key, key);
        assert_eq!(bare.user_key, key);
        assert_eq!(upper.user_key, key);
        assert_eq!(token_aware.user_key, key);
    }

    /// Bitcoin bech32 may be typed in upper case; the key is the lowercase
    /// spelling.
    #[test]
    fn an_uppercase_bech32_address_has_the_lowercase_user_key() {
        let result = validate(
            "TB1QW508D6QEJXTDG4Y5R3ZARVARY0C5XW7KXPJZSX.rig",
            true,
            Network::Testnet,
        )
        .unwrap();
        assert_eq!(
            result.user_key,
            "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx"
        );
    }

    /// Without address validation (a node donating everything) nothing is
    /// decoded, so the key is the text as typed.
    #[test]
    fn an_unvalidated_address_is_keyed_as_typed() {
        let result = validate_bitcoin_cash(
            "qr6m7j9njldwwzlg9v7v53unlr4jkmx6eylep8ekg2.rig",
            false,
            BitcoinCashNetwork::Mainnet,
        )
        .unwrap();
        assert_eq!(
            result.user_key,
            "qr6m7j9njldwwzlg9v7v53unlr4jkmx6eylep8ekg2"
        );
    }

    /// A token-aware CashAddr (type 2, from the CashTokens test vectors) is
    /// paid with the same P2PKH script as its plain form.
    #[test]
    fn bitcoin_cash_accepts_a_token_aware_cashaddr_as_its_plain_payout() {
        let token_aware = validate_bitcoin_cash(
            "zr7fzmep8g7h7ymfxy74lgc0v950j3r295x8qj2hgs.rig1",
            true,
            BitcoinCashNetwork::Chipnet,
        )
        .unwrap();
        let plain = validate_bitcoin_cash(
            "bchtest:qr7fzmep8g7h7ymfxy74lgc0v950j3r295pdnvy3hr.rig1",
            true,
            BitcoinCashNetwork::Chipnet,
        )
        .unwrap();
        assert_eq!(token_aware.worker_name, Some("rig1"));
        assert_eq!(
            token_aware.parsed_address.unwrap().script_pubkey(),
            plain.parsed_address.unwrap().script_pubkey()
        );
    }

    #[test]
    fn bitcoin_cash_rejects_a_cashaddr_for_another_network() {
        let result = validate_bitcoin_cash(
            "bitcoincash:qr6m7j9njldwwzlg9v7v53unlr4jkmx6eylep8ekg2",
            true,
            BitcoinCashNetwork::Regtest,
        );
        assert!(matches!(
            result.unwrap_err(),
            UsernameValidationError::InvalidAddress(_)
        ));
    }

    #[test]
    fn bitcoin_cash_rejects_a_prefixless_cashaddr_from_another_network() {
        // The checksum covers the prefix, so a mainnet string read as regtest fails.
        let result = validate_bitcoin_cash(
            "qr6m7j9njldwwzlg9v7v53unlr4jkmx6eylep8ekg2",
            true,
            BitcoinCashNetwork::Regtest,
        );
        assert!(matches!(
            result.unwrap_err(),
            UsernameValidationError::InvalidAddress(_)
        ));
    }

    #[test]
    fn bitcoin_cash_rejects_a_bitcoin_address() {
        let result = validate_bitcoin_cash(
            "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx",
            true,
            BitcoinCashNetwork::Chipnet,
        );
        assert!(matches!(
            result.unwrap_err(),
            UsernameValidationError::InvalidAddress(_)
        ));
    }

    #[test]
    fn bitcoin_cash_chain_without_a_network_rejects_usernames() {
        let result = validate_for_chain(
            "bchreg:qzh5k63rhy4jg6jtyssmzddju5kkng4jzu8ktcypfm",
            true,
            Network::Regtest,
            Chain::BitcoinCash,
            None,
        );
        assert!(matches!(
            result.unwrap_err(),
            UsernameValidationError::InvalidAddress(_)
        ));
    }

    #[test]
    fn bitcoin_chain_keeps_bitcoin_username_parsing() {
        let result = validate_for_chain(
            "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx.worker1",
            true,
            Network::Testnet,
            Chain::Bitcoin,
            None,
        )
        .unwrap();
        assert_eq!(result.worker_name, Some("worker1"));
        assert!(result.parsed_address.is_some());
    }

    #[test]
    fn test_empty_username() {
        let result = validate("", true, Network::Bitcoin);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            UsernameValidationError::UserNameMissing()
        ));
    }

    #[test]
    fn test_empty_username_without_address_validation() {
        let result = validate("", false, Network::Bitcoin);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            UsernameValidationError::UserNameMissing()
        ));
    }
}
