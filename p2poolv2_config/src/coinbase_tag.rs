// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The name rules of the coinbase tag (`docs/stratum.md`, "The coinbase
//! tag").
//!
//! A node writes `P2Poolv2 0x0F <operator> 0x0F <node> 0x0F <chain>` in the
//! signature push of each coinbase it builds, the way DATUM writes
//! `primary 0x0F secondary`. The operator line comes from
//! `[stratum] pool_signature`, the node line from the node software and
//! `[stratum] node_label`, and the chain line is computed from the share
//! chain (`p2poolv2_lib::stratum::work::coinbase_tag`). Every name that
//! goes on chain passes the same rules, so no address, email or URL is
//! written into a block: only
//! letters, digits, spaces, `.`, `_` and `-`, which keeps out `@`, `:` and
//! `/`, and no long word that reads as an address pasted without its prefix.
//! A name that breaks a rule is refused, never cut short.

/// The first line of every coinbase tag, written by the software and never
/// configured.
pub const COINBASE_TAG_MARKER: &str = "P2Poolv2";

/// The byte between the lines of a coinbase tag, as DATUM separates its
/// primary and secondary tags.
pub const COINBASE_TAG_SEPARATOR: u8 = 0x0F;

/// The most characters of the operator line, `pool_signature`.
pub const MAX_OPERATOR_LENGTH: usize = 24;

/// The most characters of `node_label`. The node line has room for the
/// node software beside it; see `MAX_NODE_LINE_LENGTH` in
/// `p2poolv2_lib::stratum::work::coinbase_tag`.
pub const MAX_NODE_LABEL_LENGTH: usize = 8;

/// A word this long or longer made only of base58, hex or CashAddr
/// characters is refused: it may be an address pasted without its prefix.
const ADDRESS_LIKE_WORD_LENGTH: usize = 20;

/// The base58 alphabet of legacy addresses and keys: no `0`, `O`, `I` or
/// `l`.
const BASE58_CHARACTERS: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

/// The bech32 alphabet of CashAddr payloads, in lower case, five bits a
/// character. CashAddr is written in either case, so a word is compared in
/// lower case. The chain line of the coinbase tag is written in it too.
pub const CASHADDR_CHARACTERS: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";

/// Check a name that goes into the coinbase tag against the name rules, at
/// most `max_length` characters.
///
/// The error says what broke a rule, for an operator to fix; the caller
/// names the setting.
pub fn check_coinbase_name(name: &str, max_length: usize) -> Result<(), String> {
    if let Some(character) = name
        .chars()
        .find(|character| !is_name_character(*character))
    {
        return Err(format!(
            "{name:?} has {character:?}: a coinbase name holds only letters, digits, spaces, '.', '_' and '-'"
        ));
    }
    if let Some(word) = name.split(' ').find(|word| is_address_like(word)) {
        return Err(format!(
            "{name:?} has the word {word:?}: {} characters of base58, hex or CashAddr alone may be an address, and no address goes on chain",
            word.len()
        ));
    }
    if name.len() > max_length {
        return Err(format!(
            "{name:?} is {} characters, more than the {max_length} allowed",
            name.len()
        ));
    }
    Ok(())
}

/// The operator line `pool_signature` gives: empty when it is unset, empty
/// or `P2Poolv2`, the old default, so the node line never stands in the
/// position of the operator line.
pub fn operator_line(pool_signature: Option<&str>) -> &str {
    match pool_signature {
        Some(name) if name != COINBASE_TAG_MARKER => name,
        _ => "",
    }
}

fn is_name_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, ' ' | '.' | '_' | '-')
}

fn is_address_like(word: &str) -> bool {
    word.len() >= ADDRESS_LIKE_WORD_LENGTH
        && (word.bytes().all(|byte| BASE58_CHARACTERS.contains(&byte))
            || word.bytes().all(|byte| byte.is_ascii_hexdigit())
            || word
                .bytes()
                .all(|byte| CASHADDR_CHARACTERS.contains(&byte.to_ascii_lowercase())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_of_letters_digits_spaces_and_punctuation_passes() {
        assert_eq!(
            check_coinbase_name("My Pool_2.0-eu", MAX_OPERATOR_LENGTH),
            Ok(())
        );
    }

    #[test]
    fn an_empty_name_passes() {
        assert_eq!(check_coinbase_name("", MAX_OPERATOR_LENGTH), Ok(()));
    }

    #[test]
    fn a_name_of_the_longest_length_passes() {
        assert_eq!(
            check_coinbase_name("Longest-Operator-Name-24", MAX_OPERATOR_LENGTH),
            Ok(())
        );
    }

    #[test]
    fn an_email_is_refused() {
        let error = check_coinbase_name("pool@example.com", MAX_OPERATOR_LENGTH).unwrap_err();
        assert!(error.contains("'@'"), "{error}");
    }

    #[test]
    fn a_cashaddr_with_its_prefix_is_refused() {
        let error = check_coinbase_name(
            "bitcoincash:qpm2qsznhks23z7629mms6s4cwef74vcwvy22gdx6a",
            MAX_OPERATOR_LENGTH,
        )
        .unwrap_err();
        assert!(error.contains("':'"), "{error}");
    }

    #[test]
    fn a_testnet_cashaddr_with_its_prefix_is_refused() {
        let error =
            check_coinbase_name("bchtest:qpm2qsznhks23z76", MAX_OPERATOR_LENGTH).unwrap_err();
        assert!(error.contains("':'"), "{error}");
    }

    #[test]
    fn a_cashaddr_without_its_prefix_is_refused() {
        let error = check_coinbase_name(
            "qpm2qsznhks23z7629mms6s4cwef74vcwvy22gdx6a",
            MAX_OPERATOR_LENGTH,
        )
        .unwrap_err();
        assert!(error.contains("may be an address"), "{error}");
    }

    #[test]
    fn a_cashaddr_word_in_a_short_name_is_refused() {
        let error =
            check_coinbase_name("x qpzry9x8gf2tvdw0s3jn5", MAX_OPERATOR_LENGTH).unwrap_err();
        assert!(error.contains("may be an address"), "{error}");
    }

    #[test]
    fn an_upper_case_cashaddr_word_is_refused() {
        let error = check_coinbase_name("QPZRY9X8GF2TVDW0S3JN54", MAX_OPERATOR_LENGTH).unwrap_err();
        assert!(error.contains("may be an address"), "{error}");
    }

    #[test]
    fn a_base58_word_is_refused() {
        let error = check_coinbase_name("1A2b3C4d5E6f7G8h9J1k2M", MAX_OPERATOR_LENGTH).unwrap_err();
        assert!(error.contains("may be an address"), "{error}");
    }

    #[test]
    fn a_hex_word_is_refused() {
        let error = check_coinbase_name("deadbeef00112233445566", MAX_OPERATOR_LENGTH).unwrap_err();
        assert!(error.contains("may be an address"), "{error}");
    }

    #[test]
    fn a_base58_word_one_character_short_of_the_limit_passes() {
        assert_eq!(
            check_coinbase_name("1A2b3C4d5E6f7G8h9J1", MAX_OPERATOR_LENGTH),
            Ok(())
        );
    }

    #[test]
    fn a_name_over_the_length_is_refused() {
        let error =
            check_coinbase_name("This Pool Name Is Too Long", MAX_OPERATOR_LENGTH).unwrap_err();
        assert!(error.contains("more than the 24 allowed"), "{error}");
    }

    #[test]
    fn a_label_over_its_length_is_refused() {
        let error = check_coinbase_name("node-label", MAX_NODE_LABEL_LENGTH).unwrap_err();
        assert!(error.contains("more than the 8 allowed"), "{error}");
    }

    #[test]
    fn a_slash_is_refused() {
        let error = check_coinbase_name("MyPool/1.0", MAX_OPERATOR_LENGTH).unwrap_err();
        assert!(error.contains("'/'"), "{error}");
    }

    #[test]
    fn the_tag_separator_is_refused() {
        let error = check_coinbase_name("Pool\u{f}Other", MAX_OPERATOR_LENGTH).unwrap_err();
        assert!(error.contains("'\\u{f}'"), "{error}");
    }

    #[test]
    fn a_non_ascii_letter_is_refused() {
        assert!(check_coinbase_name("Pool\u{e9}", MAX_OPERATOR_LENGTH).is_err());
    }

    #[test]
    fn an_unset_pool_signature_gives_an_empty_operator_line() {
        assert_eq!(operator_line(None), "");
    }

    #[test]
    fn an_empty_pool_signature_gives_an_empty_operator_line() {
        assert_eq!(operator_line(Some("")), "");
    }

    #[test]
    fn the_old_default_pool_signature_gives_an_empty_operator_line() {
        assert_eq!(operator_line(Some("P2Poolv2")), "");
    }

    #[test]
    fn a_pool_signature_is_the_operator_line() {
        assert_eq!(operator_line(Some("CyberAshven")), "CyberAshven");
    }
}
