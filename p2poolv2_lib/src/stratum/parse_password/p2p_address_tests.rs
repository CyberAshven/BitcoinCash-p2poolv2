use super::*;
use bitcoin::Network;

/// BIP086 tweak of PUBKEY_G, the same value the fixtures use.
const SHARE_ADDRESS: &str = "tp2pool1pmfr3p9j00pfxjh0zmgp99y8zftmd3s5pmedqhyptwy6lm87hf5sss3v29v";

#[test]
fn absent_key_yields_none() {
    assert!(parse_password("d=1000").miner_address.is_none());
}

#[test]
fn empty_password_yields_none() {
    assert!(parse_password("").miner_address.is_none());
}

#[test]
fn address_alone_is_parsed() {
    let parsed = parse_password(&format!("p2p={SHARE_ADDRESS}"));
    let address = parsed.miner_address.unwrap().unwrap();
    assert_eq!(address.to_string(), SHARE_ADDRESS);
    assert_eq!(address.network(), Network::Testnet4);
}

#[test]
fn address_after_difficulty_is_parsed() {
    let parsed = parse_password(&format!("d=1000,p2p={SHARE_ADDRESS}"));
    assert_eq!(parsed.difficulty, Some(1000));
    assert_eq!(
        parsed.miner_address.unwrap().unwrap().to_string(),
        SHARE_ADDRESS
    );
}

#[test]
fn address_before_difficulty_is_parsed() {
    let parsed = parse_password(&format!("p2p={SHARE_ADDRESS},d=1000"));
    assert_eq!(parsed.difficulty, Some(1000));
    assert_eq!(
        parsed.miner_address.unwrap().unwrap().to_string(),
        SHARE_ADDRESS
    );
}

#[test]
fn address_is_parsed_alongside_terahash() {
    let parsed = parse_password(&format!("th=100 p2p={SHARE_ADDRESS}"));
    assert!(parsed.difficulty.is_some());
    assert_eq!(
        parsed.miner_address.unwrap().unwrap().to_string(),
        SHARE_ADDRESS
    );
}

#[test]
fn semicolon_separates_the_address_from_a_later_option() {
    let parsed = parse_password(&format!("p2p={SHARE_ADDRESS};d=1000"));
    assert_eq!(parsed.difficulty, Some(1000));
    assert_eq!(
        parsed.miner_address.unwrap().unwrap().to_string(),
        SHARE_ADDRESS
    );
}

/// The key is matched case insensitively, like `d=` and `th=`.
#[test]
fn key_is_matched_case_insensitively() {
    let parsed = parse_password(&format!("P2P={SHARE_ADDRESS}"));
    assert_eq!(
        parsed.miner_address.unwrap().unwrap().to_string(),
        SHARE_ADDRESS
    );
}

/// A malformed address must be distinguishable from an absent one, so that
/// authorize can tell a miner who typoed from one who supplied nothing.
#[test]
fn malformed_address_is_some_err_not_none() {
    let parsed = parse_password("p2p=not-an-address");
    assert!(parsed.miner_address.unwrap().is_err());
}

#[test]
fn bitcoin_address_in_p2p_is_some_err() {
    let parsed = parse_password("p2p=tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx");
    assert!(parsed.miner_address.unwrap().is_err());
}

/// Mixed case must survive extraction unrepaired so the address parser can
/// reject it. Lowercasing the password first would silently accept it and
/// defeat the bech32 checksum guarantee.
#[test]
fn mixed_case_address_reaches_the_parser_and_is_rejected() {
    let mixed_case = format!("p2p=Tp2pool{}", &SHARE_ADDRESS[7..]);
    let parsed = parse_password(&mixed_case);
    assert!(parsed.miner_address.unwrap().is_err());
}

/// An uppercase address is valid bech32 and must round trip to the same
/// address, which only works because extraction preserves the original case.
#[test]
fn uppercase_address_is_accepted() {
    let parsed = parse_password(&format!("p2p={}", SHARE_ADDRESS.to_uppercase()));
    assert_eq!(
        parsed.miner_address.unwrap().unwrap().to_string(),
        SHARE_ADDRESS
    );
}

/// `p2p=` must not match inside another token, matching the boundary rule
/// that `extract_value` already applies to `d=` and `th=`.
#[test]
fn key_embedded_in_another_token_is_ignored() {
    let parsed = parse_password(&format!("xp2p={SHARE_ADDRESS}"));
    assert!(parsed.miner_address.is_none());
}

#[test]
fn empty_value_yields_none() {
    assert!(parse_password("p2p=").miner_address.is_none());
}

/// `p2p=<address>` is 71 characters, which is long for a stratum password
/// field and some miner firmware imposes its own input limit. The bech32
/// checksum is what makes that safe: a truncated address fails loudly
/// instead of silently resolving to a different valid one. This pins that,
/// because silent truncation would pay shares to the wrong owner.
#[test]
fn truncated_address_is_rejected_rather_than_silently_accepted() {
    let truncated = &SHARE_ADDRESS[..SHARE_ADDRESS.len() - 1];
    let parsed = parse_password(&format!("p2p={truncated}"));
    assert!(parsed.miner_address.unwrap().is_err());
}

#[test]
fn heavily_truncated_address_is_rejected() {
    let truncated = &SHARE_ADDRESS[..SHARE_ADDRESS.len() - 8];
    let parsed = parse_password(&format!("p2p={truncated}"));
    assert!(parsed.miner_address.unwrap().is_err());
}

#[test]
fn empty_value_followed_by_another_option_yields_none() {
    let parsed = parse_password("p2p=,d=1000");
    assert!(parsed.miner_address.is_none());
    assert_eq!(parsed.difficulty, Some(1000));
}
